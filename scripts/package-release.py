"""Create release archives with normalized, deterministic metadata."""

from __future__ import annotations

import argparse
import gzip
import os
import shutil
import stat
import sys
import tarfile
import tempfile
import time
import zipfile
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

ZIP_MIN_EPOCH = 315532800  # 1980-01-01T00:00:00Z
ZIP_MAX_EPOCH = 4354819198  # 2107-12-31T23:59:58Z
COPY_BUFFER_BYTES = 1024 * 1024


class PackagingError(Exception):
    """Raised when release input cannot be represented safely."""


@dataclass(frozen=True)
class Entry:
    source: Path
    relative: PurePosixPath
    is_directory: bool
    mode: int
    size: int


def is_link_or_reparse(metadata: os.stat_result) -> bool:
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    file_attributes = getattr(metadata, "st_file_attributes", 0)
    return stat.S_ISLNK(metadata.st_mode) or bool(file_attributes & reparse_flag)


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("tar.gz", "zip"), required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--epoch", type=int, required=True)
    return parser.parse_args(arguments)


def normalized_mode(path: Path, file_mode: int, is_directory: bool) -> int:
    if is_directory:
        return 0o755
    if file_mode & 0o111 or path.suffix.lower() in {".bat", ".cmd", ".exe"}:
        return 0o755
    return 0o644


def collect_entries(root: Path) -> list[Entry]:
    entries: list[Entry] = []
    for current_value, directory_names, file_names in os.walk(
        root, topdown=True, followlinks=False
    ):
        current = Path(current_value)
        directory_names.sort()
        file_names.sort()
        for name, is_directory in [
            *((name, True) for name in directory_names),
            *((name, False) for name in file_names),
        ]:
            source = current / name
            metadata = source.lstat()
            if is_link_or_reparse(metadata):
                raise PackagingError(
                    f"release input contains a link or reparse point: {source}"
                )
            if is_directory and not stat.S_ISDIR(metadata.st_mode):
                raise PackagingError(
                    f"release input contains an unsupported entry: {source}"
                )
            if not is_directory and not stat.S_ISREG(metadata.st_mode):
                raise PackagingError(f"release input contains a special file: {source}")
            relative = PurePosixPath(source.relative_to(root).as_posix())
            entries.append(
                Entry(
                    source=source,
                    relative=relative,
                    is_directory=is_directory,
                    mode=normalized_mode(source, metadata.st_mode, is_directory),
                    size=0 if is_directory else metadata.st_size,
                )
            )
    entries.sort(key=lambda entry: entry.relative.as_posix())
    return entries


def tar_info(entry: Entry, epoch: int) -> tarfile.TarInfo:
    name = entry.relative.as_posix() + ("/" if entry.is_directory else "")
    info = tarfile.TarInfo(name)
    info.type = tarfile.DIRTYPE if entry.is_directory else tarfile.REGTYPE
    info.mode = entry.mode
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.size = entry.size
    info.mtime = epoch
    info.pax_headers = {}
    return info


def write_tar_gz(path: Path, entries: Iterable[Entry], epoch: int) -> None:
    with path.open("wb") as raw_output:
        with (
            gzip.GzipFile(
                filename="",
                mode="wb",
                compresslevel=9,
                fileobj=raw_output,
                mtime=epoch,
            ) as compressed_output,
            tarfile.open(
                fileobj=compressed_output,
                mode="w",
                format=tarfile.PAX_FORMAT,
                pax_headers={},
            ) as archive,
        ):
            for entry in entries:
                info = tar_info(entry, epoch)
                if entry.is_directory:
                    archive.addfile(info)
                else:
                    with entry.source.open("rb") as source:
                        archive.addfile(info, source)
        raw_output.flush()
        os.fsync(raw_output.fileno())


def zip_timestamp(epoch: int) -> tuple[int, int, int, int, int, int]:
    fields = list(time.gmtime(epoch)[:6])
    fields[5] -= fields[5] % 2
    return tuple(fields)  # type: ignore[return-value]


def zip_info(entry: Entry, epoch: int) -> zipfile.ZipInfo:
    name = entry.relative.as_posix() + ("/" if entry.is_directory else "")
    info = zipfile.ZipInfo(name, zip_timestamp(epoch))
    info.create_system = 3
    info.compress_type = (
        zipfile.ZIP_STORED if entry.is_directory else zipfile.ZIP_DEFLATED
    )
    file_type = stat.S_IFDIR if entry.is_directory else stat.S_IFREG
    info.external_attr = ((file_type | entry.mode) & 0xFFFF) << 16
    if entry.is_directory:
        info.external_attr |= 0x10
    info.extra = b""
    info.comment = b""
    info.file_size = entry.size
    return info


def write_zip(path: Path, entries: Iterable[Entry], epoch: int) -> None:
    with zipfile.ZipFile(
        path,
        mode="w",
        compression=zipfile.ZIP_DEFLATED,
        compresslevel=9,
        allowZip64=True,
        strict_timestamps=True,
    ) as archive:
        archive.comment = b""
        for entry in entries:
            info = zip_info(entry, epoch)
            if entry.is_directory:
                archive.writestr(info, b"")
                continue
            with (
                entry.source.open("rb") as source,
                archive.open(
                    info,
                    mode="w",
                    force_zip64=entry.size >= (1 << 31),
                ) as destination,
            ):
                shutil.copyfileobj(source, destination, COPY_BUFFER_BYTES)
    with path.open("r+b") as archive_input:
        os.fsync(archive_input.fileno())


def package_release(format_name: str, source: Path, output: Path, epoch: int) -> None:
    if epoch < ZIP_MIN_EPOCH or epoch > ZIP_MAX_EPOCH:
        raise PackagingError("--epoch must fit the portable ZIP timestamp range")
    if is_link_or_reparse(source.lstat()):
        raise PackagingError("--source cannot be a link or reparse point")
    root = source.resolve(strict=True)
    if not root.is_dir():
        raise PackagingError("--source must identify a directory")

    output_path = Path(os.path.abspath(output))
    try:
        output_metadata = output_path.lstat()
    except FileNotFoundError:
        output_metadata = None
    if output_metadata is not None and is_link_or_reparse(output_metadata):
        raise PackagingError("--output cannot replace a link or reparse point")
    output_parent = output_path.parent.resolve(strict=True)
    output_path = output_parent / output_path.name
    if output_path == root or root in output_path.parents:
        raise PackagingError("--output cannot be inside --source")
    if not output_path.parent.is_dir():
        raise PackagingError("--output parent must already exist")

    entries = collect_entries(root)
    descriptor, temporary_value = tempfile.mkstemp(
        prefix=f".{output_path.name}.", suffix=".tmp", dir=output_path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_value)
    try:
        if format_name == "tar.gz":
            write_tar_gz(temporary, entries, epoch)
        else:
            write_zip(temporary, entries, epoch)
        os.replace(temporary, output_path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main(arguments: Sequence[str]) -> int:
    options = parse_arguments(arguments)
    try:
        package_release(options.format, options.source, options.output, options.epoch)
    except (OSError, PackagingError, ValueError) as error:
        print(f"release packaging failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
