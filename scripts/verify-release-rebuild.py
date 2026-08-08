"""Compare independently rebuilt binaries with the binaries in a release archive."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import tarfile
import tempfile
import zipfile
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import BinaryIO

COPY_BUFFER_BYTES = 1024 * 1024
SOURCE_REVISION_PATTERN = re.compile(r"[0-9a-f]{40}")
PLATFORM_PATTERN = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}")


class RebuildVerificationError(Exception):
    """Raised when independent rebuild evidence cannot be trusted."""


@dataclass(frozen=True)
class BinarySubject:
    archive_path: str
    rebuilt_path: Path


@dataclass(frozen=True)
class Digest:
    sha256: str
    size: int


def is_link_or_reparse(metadata: os.stat_result) -> bool:
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    file_attributes = getattr(metadata, "st_file_attributes", 0)
    return stat.S_ISLNK(metadata.st_mode) or bool(file_attributes & reparse_flag)


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--binary",
        action="append",
        nargs=2,
        metavar=("ARCHIVE_PATH", "REBUILT_PATH"),
        required=True,
    )
    return parser.parse_args(arguments)


def validate_regular_file(path: Path, description: str) -> os.stat_result:
    metadata = path.lstat()
    if is_link_or_reparse(metadata) or not stat.S_ISREG(metadata.st_mode):
        raise RebuildVerificationError(f"{description} must be a regular file: {path}")
    return metadata


def validate_archive_path(value: str) -> str:
    if "\\" in value:
        raise RebuildVerificationError(
            f"archive binary path contains a backslash: {value}"
        )
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or not path.parts
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        raise RebuildVerificationError(f"archive binary path is unsafe: {value}")
    normalized = path.as_posix()
    if normalized != value:
        raise RebuildVerificationError(f"archive binary path is not canonical: {value}")
    return normalized


def parse_subjects(values: list[list[str]]) -> list[BinarySubject]:
    subjects: list[BinarySubject] = []
    seen: set[str] = set()
    for archive_value, rebuilt_value in values:
        archive_path = validate_archive_path(archive_value)
        if archive_path in seen:
            raise RebuildVerificationError(
                f"archive binary path was specified more than once: {archive_path}"
            )
        seen.add(archive_path)
        rebuilt_path = Path(rebuilt_value)
        validate_regular_file(rebuilt_path, "rebuilt binary")
        subjects.append(BinarySubject(archive_path, rebuilt_path))
    subjects.sort(key=lambda subject: subject.archive_path)
    return subjects


def digest_stream(stream: BinaryIO) -> Digest:
    digest = hashlib.sha256()
    size = 0
    while chunk := stream.read(COPY_BUFFER_BYTES):
        digest.update(chunk)
        size += len(chunk)
    return Digest(digest.hexdigest(), size)


def stable_file_digest(path: Path, description: str) -> Digest:
    before = validate_regular_file(path, description)
    with path.open("rb") as stream:
        digest = digest_stream(stream)
    after = validate_regular_file(path, description)
    identity_before = (
        before.st_dev,
        before.st_ino,
        before.st_size,
        before.st_mtime_ns,
    )
    identity_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
    if identity_before != identity_after or digest.size != after.st_size:
        raise RebuildVerificationError(
            f"{description} changed while it was read: {path}"
        )
    return digest


def tar_subject_digests(
    archive: Path, subjects: list[BinarySubject]
) -> dict[str, Digest]:
    with tarfile.open(archive, mode="r:gz") as value:
        members = value.getmembers()
        results: dict[str, Digest] = {}
        for subject in subjects:
            matches = [
                member for member in members if member.name == subject.archive_path
            ]
            if len(matches) != 1:
                raise RebuildVerificationError(
                    f"release archive must contain exactly one {subject.archive_path}"
                )
            member = matches[0]
            if not member.isfile() or member.issym() or member.islnk():
                raise RebuildVerificationError(
                    f"release archive binary is not a regular file: {subject.archive_path}"
                )
            stream = value.extractfile(member)
            if stream is None:
                raise RebuildVerificationError(
                    f"release archive binary cannot be read: {subject.archive_path}"
                )
            with stream:
                digest = digest_stream(stream)
            if digest.size != member.size:
                raise RebuildVerificationError(
                    f"release archive binary size changed: {subject.archive_path}"
                )
            results[subject.archive_path] = digest
        return results


def zip_subject_digests(
    archive: Path, subjects: list[BinarySubject]
) -> dict[str, Digest]:
    with zipfile.ZipFile(archive, mode="r") as value:
        entries = value.infolist()
        results: dict[str, Digest] = {}
        for subject in subjects:
            matches = [
                entry for entry in entries if entry.filename == subject.archive_path
            ]
            if len(matches) != 1:
                raise RebuildVerificationError(
                    f"release archive must contain exactly one {subject.archive_path}"
                )
            entry = matches[0]
            file_type = stat.S_IFMT((entry.external_attr >> 16) & 0xFFFF)
            if entry.is_dir() or file_type not in {0, stat.S_IFREG}:
                raise RebuildVerificationError(
                    f"release archive binary is not a regular file: {subject.archive_path}"
                )
            with value.open(entry, mode="r") as stream:
                digest = digest_stream(stream)
            if digest.size != entry.file_size:
                raise RebuildVerificationError(
                    f"release archive binary size changed: {subject.archive_path}"
                )
            results[subject.archive_path] = digest
        return results


def archive_subject_digests(
    archive: Path, subjects: list[BinarySubject]
) -> dict[str, Digest]:
    name = archive.name.lower()
    if name.endswith(".tar.gz"):
        return tar_subject_digests(archive, subjects)
    if name.endswith(".zip"):
        return zip_subject_digests(archive, subjects)
    raise RebuildVerificationError(
        "--archive must be a .tar.gz or .zip release archive"
    )


def evidence_document(
    archive: Path,
    platform: str,
    source_revision: str,
    subjects: list[BinarySubject],
) -> dict[str, object]:
    archive_before = validate_regular_file(archive, "release archive")
    primary_digests = archive_subject_digests(archive, subjects)
    results: list[dict[str, object]] = []
    for subject in subjects:
        primary = primary_digests[subject.archive_path]
        rebuilt = stable_file_digest(subject.rebuilt_path, "rebuilt binary")
        if rebuilt != primary:
            raise RebuildVerificationError(
                f"independent rebuild does not match {subject.archive_path}: "
                f"release sha256:{primary.sha256}, rebuild sha256:{rebuilt.sha256}"
            )
        results.append(
            {
                "archivePath": subject.archive_path,
                "matchesIndependentRebuild": True,
                "sha256": primary.sha256,
                "size": primary.size,
            }
        )
    archive_digest = stable_file_digest(archive, "release archive")
    archive_after = validate_regular_file(archive, "release archive")
    if (
        archive_before.st_dev,
        archive_before.st_ino,
        archive_before.st_size,
        archive_before.st_mtime_ns,
    ) != (
        archive_after.st_dev,
        archive_after.st_ino,
        archive_after.st_size,
        archive_after.st_mtime_ns,
    ):
        raise RebuildVerificationError(
            f"release archive changed while it was read: {archive}"
        )
    return {
        "archive": {
            "name": archive.name,
            "sha256": archive_digest.sha256,
            "size": archive_digest.size,
        },
        "platform": platform,
        "schema": "a3s.use.release-rebuild.v1",
        "sourceRevision": source_revision,
        "subjects": results,
    }


def write_evidence(
    output: Path, document: dict[str, object], protected_inputs: list[Path]
) -> None:
    output_path = Path(os.path.abspath(output))
    try:
        metadata = output_path.lstat()
    except FileNotFoundError:
        metadata = None
    if metadata is not None and is_link_or_reparse(metadata):
        raise RebuildVerificationError(
            "--output cannot replace a link or reparse point"
        )
    parent = output_path.parent.resolve(strict=True)
    if not parent.is_dir():
        raise RebuildVerificationError("--output parent must be a directory")
    output_path = parent / output_path.name
    for input_path in protected_inputs:
        if output_path == input_path.resolve(strict=True):
            raise RebuildVerificationError("--output cannot replace an input file")
    payload = (json.dumps(document, indent=2, sort_keys=True) + "\n").encode()
    descriptor, temporary_value = tempfile.mkstemp(
        prefix=f".{output_path.name}.", suffix=".tmp", dir=parent
    )
    temporary = Path(temporary_value)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.chmod(0o644)
        os.replace(temporary, output_path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main(arguments: Sequence[str]) -> int:
    options = parse_arguments(arguments)
    try:
        if not PLATFORM_PATTERN.fullmatch(options.platform):
            raise RebuildVerificationError("--platform is malformed")
        if not SOURCE_REVISION_PATTERN.fullmatch(options.source_revision):
            raise RebuildVerificationError(
                "--source-revision must be a lowercase commit SHA"
            )
        subjects = parse_subjects(options.binary)
        document = evidence_document(
            options.archive,
            options.platform,
            options.source_revision,
            subjects,
        )
        write_evidence(
            options.output,
            document,
            [options.archive, *(subject.rebuilt_path for subject in subjects)],
        )
    except (
        OSError,
        RebuildVerificationError,
        RuntimeError,
        tarfile.TarError,
        ValueError,
        zipfile.BadZipFile,
    ) as error:
        print(f"release rebuild verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
