"""Reject release trees that retain repository-local absolute paths."""

from __future__ import annotations

import argparse
import os
import stat
import sys
from collections.abc import Iterator, Sequence
from dataclasses import dataclass
from pathlib import Path

COPY_BUFFER_BYTES = 1024 * 1024


class PortabilityVerificationError(Exception):
    """Raised when a release tree is not independent from its checkout."""


@dataclass(frozen=True)
class ScanResult:
    files: int
    bytes: int


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-root", type=Path, required=True)
    parser.add_argument(
        "--forbid-path",
        type=Path,
        action="append",
        required=True,
        help="Absolute checkout path that must not occur in any release file",
    )
    return parser.parse_args(arguments)


def is_link_or_reparse(metadata: os.stat_result) -> bool:
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    file_attributes = getattr(metadata, "st_file_attributes", 0)
    return stat.S_ISLNK(metadata.st_mode) or bool(file_attributes & reparse_flag)


def validate_release_root(path: Path) -> Path:
    metadata = path.lstat()
    if is_link_or_reparse(metadata) or not stat.S_ISDIR(metadata.st_mode):
        raise PortabilityVerificationError(
            f"release root must be a physical directory: {path}"
        )
    return path.resolve(strict=True)


def validate_forbidden_paths(paths: Sequence[Path]) -> list[Path]:
    forbidden: list[Path] = []
    for path in paths:
        if not path.is_absolute():
            raise PortabilityVerificationError(
                f"forbidden repository path must be absolute: {path}"
            )
        absolute = path.absolute()
        resolved = path.resolve(strict=True)
        for candidate in (absolute, resolved):
            if candidate not in forbidden:
                forbidden.append(candidate)
    return forbidden


def forbidden_needles(paths: Sequence[Path]) -> tuple[bytes, ...]:
    values: set[str] = set()
    for path in paths:
        raw = str(path)
        variants = {raw, raw.replace("\\", "/"), raw.replace("/", "\\")}
        if os.name == "nt":
            variants.update(value.lower() for value in tuple(variants))
        values.update(variants)

    needles: set[bytes] = set()
    for value in values:
        if len(value) < 4:
            raise PortabilityVerificationError(
                "forbidden repository path is too short for a safe binary scan"
            )
        needles.add(value.encode("utf-8"))
        needles.add(value.encode("utf-16-le"))
    return tuple(sorted(needles, key=lambda value: (len(value), value)))


def regular_files(root: Path) -> Iterator[Path]:
    pending = [root]
    while pending:
        directory = pending.pop()
        with os.scandir(directory) as iterator:
            entries = sorted(iterator, key=lambda entry: entry.name)
        for entry in entries:
            path = Path(entry.path)
            metadata = entry.stat(follow_symlinks=False)
            if is_link_or_reparse(metadata):
                raise PortabilityVerificationError(
                    f"release tree contains a link or reparse point: {path.relative_to(root)}"
                )
            if stat.S_ISDIR(metadata.st_mode):
                pending.append(path)
            elif stat.S_ISREG(metadata.st_mode):
                yield path
            else:
                raise PortabilityVerificationError(
                    f"release tree contains a special file: {path.relative_to(root)}"
                )


def file_contains(path: Path, needles: tuple[bytes, ...]) -> bool:
    overlap_bytes = max(len(needle) for needle in needles) - 1
    tail = b""
    with path.open("rb") as stream:
        while chunk := stream.read(COPY_BUFFER_BYTES):
            value = tail + chunk
            comparable = value.lower() if os.name == "nt" else value
            if any(needle in comparable for needle in needles):
                return True
            tail = value[-overlap_bytes:] if overlap_bytes else b""
    return False


def verify_release_tree(root: Path, forbidden: Sequence[Path]) -> ScanResult:
    needles = forbidden_needles(forbidden)
    files = 0
    total_bytes = 0
    for path in regular_files(root):
        metadata = path.lstat()
        if file_contains(path, needles):
            raise PortabilityVerificationError(
                "release file contains a repository-local path: "
                f"{path.relative_to(root)}"
            )
        files += 1
        total_bytes += metadata.st_size
    if files == 0:
        raise PortabilityVerificationError("release tree is empty")
    return ScanResult(files=files, bytes=total_bytes)


def main(arguments: Sequence[str]) -> int:
    try:
        parsed = parse_arguments(arguments)
        root = validate_release_root(parsed.release_root)
        forbidden = validate_forbidden_paths(parsed.forbid_path)
        result = verify_release_tree(root, forbidden)
        print(
            "release portability verified: "
            f"{result.files} regular files, {result.bytes} bytes"
        )
        return 0
    except (OSError, PortabilityVerificationError) as error:
        print(f"release portability verification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
