"""Validation helpers for the pinned S-Code Harbor runtime bundle.

The bundle is intentionally small and rigid.  It is a gzip-compressed, flat
tar archive containing ``manifest.json`` and exactly the four runtime files in
``REQUIRED_RUNTIME_FILES``.  Callers must supply the archive's SHA-256 digest;
the outer digest is never embedded in the archive itself.

This module uses only the Python standard library and supports Python 3.9+.
Bundle validation does not extract or execute any archive member.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import hmac
import json
import os
import platform
import re
import stat
import tarfile
import tempfile
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from types import MappingProxyType
from typing import BinaryIO, Dict, Iterator, Mapping, Sequence, Tuple, Union


BUNDLE_FORMAT = "s-code-harbor-bundle"
BUNDLE_SCHEMA_VERSION = 1
SUPPORTED_TARGET = "x86_64-unknown-linux-gnu"
MANIFEST_NAME = "manifest.json"
REQUIRED_RUNTIME_FILES: Tuple[str, ...] = (
    "s-code",
    "s-code-cli",
    "s-code-daemon",
    "s-code-pdf-worker",
)
# The Harbor container environment mode exists only in daemons built with this
# feature; build-bundle.sh is the one build that enables it.
HARBOR_CONTAINER_ENV_FEATURE = "s-code-daemon/harbor-container-env"

_EXPECTED_ARCHIVE_MEMBERS = (MANIFEST_NAME,) + REQUIRED_RUNTIME_FILES
_EXPECTED_MEMBER_MODES = MappingProxyType(
    {MANIFEST_NAME: 0o644, **{name: 0o755 for name in REQUIRED_RUNTIME_FILES}}
)
_SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")
_GIT_OBJECT_RE = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
_GLIBC_RE = re.compile(r"glibc [0-9]+(?:\.[0-9]+)+\Z")
_CARGO_FEATURE_RE = re.compile(r"[A-Za-z0-9_-]+/[A-Za-z0-9_][A-Za-z0-9_+.-]*\Z")
_TOOL_KEYS = ("cargo", "node", "npm", "python", "rustc", "zlib")
_LOCK_KEYS = ("Cargo.lock", "web/package-lock.json")
_MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
_MAX_MANIFEST_BYTES = 64 * 1024
_MAX_MEMBER_BYTES = 512 * 1024 * 1024
_MAX_UNPACKED_BYTES = 1024 * 1024 * 1024
_MAX_TAR_STREAM_BYTES = _MAX_UNPACKED_BYTES + 10 * 1024 * 1024
_COPY_CHUNK_BYTES = 1024 * 1024
_NORMALIZED_GZIP_HEADER = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff"


class BundleValidationError(ValueError):
    """Raised when a Harbor runtime bundle fails closed validation."""


@dataclass(frozen=True)
class BundleFile:
    """Validated metadata for one runtime payload file."""

    sha256: str
    size: int
    mode: int


@dataclass(frozen=True)
class BundleManifest:
    """Validated bundle metadata plus its caller-supplied outer digest."""

    schema_version: int
    bundle_format: str
    source_commit: str
    source_tree: str
    source_dirty: bool
    target: str
    glibc_version: str
    lock_sha256s: Mapping[str, str]
    tool_versions: Mapping[str, str]
    cargo_features: Tuple[str, ...]
    files: Mapping[str, BundleFile]
    archive_sha256: str


PathLike = Union[str, os.PathLike]


def validated_sha256(value: str, field: str = "sha256") -> str:
    """Return a canonical lowercase SHA-256 string or raise."""

    if not isinstance(value, str) or _SHA256_RE.fullmatch(value) is None:
        raise BundleValidationError(
            f"{field} must be exactly 64 lowercase hexadecimal characters"
        )
    return value


def validate_sha256(value: str, field: str = "sha256") -> str:
    """Compatibility spelling for :func:`validated_sha256`."""

    return validated_sha256(value, field)


def _open_regular_file(path: PathLike) -> BinaryIO:
    try:
        flags = os.O_RDONLY
        if hasattr(os, "O_CLOEXEC"):
            flags |= os.O_CLOEXEC
        if hasattr(os, "O_NONBLOCK"):
            flags |= os.O_NONBLOCK
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(os.fspath(path), flags)
    except (OSError, TypeError, ValueError) as error:
        raise BundleValidationError(f"cannot open bundle as a regular file: {error}") from error

    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise BundleValidationError("bundle path must name a regular file")
        return os.fdopen(descriptor, "rb")
    except Exception:
        os.close(descriptor)
        raise


def sha256_file(path: PathLike) -> str:
    """Hash a regular, non-symlink file without loading it into memory."""

    digest = hashlib.sha256()
    with _open_regular_file(path) as source:
        for block in iter(lambda: source.read(_COPY_CHUNK_BYTES), b""):
            digest.update(block)
    return digest.hexdigest()


@contextmanager
def _verified_snapshot(
    path: PathLike, expected_sha256: str
) -> Iterator[Tuple[BinaryIO, str]]:
    """Copy and authenticate one stable archive snapshot.

    Validation reads the snapshot rather than reopening the caller's path, so
    a concurrent path replacement cannot change bytes between hashing and tar
    inspection.
    """

    expected = validated_sha256(expected_sha256, "expected_sha256")
    digest = hashlib.sha256()
    total = 0
    snapshot = tempfile.SpooledTemporaryFile(max_size=32 * 1024 * 1024, mode="w+b")
    try:
        with _open_regular_file(path) as source:
            while True:
                block = source.read(_COPY_CHUNK_BYTES)
                if not block:
                    break
                total += len(block)
                if total > _MAX_ARCHIVE_BYTES:
                    raise BundleValidationError(
                        f"bundle exceeds {_MAX_ARCHIVE_BYTES} compressed bytes"
                    )
                digest.update(block)
                snapshot.write(block)
        actual = digest.hexdigest()
        if not hmac.compare_digest(actual, expected):
            raise BundleValidationError(
                f"bundle SHA-256 mismatch: expected {expected}, got {actual}"
            )
        snapshot.seek(0)
        yield snapshot, actual
    finally:
        snapshot.close()


def _require_exact_keys(value: object, expected: Sequence[str], field: str) -> Dict[str, object]:
    if not isinstance(value, dict):
        raise BundleValidationError(f"{field} must be a JSON object")
    actual = set(value)
    wanted = set(expected)
    if actual != wanted:
        missing = sorted(wanted - actual)
        unexpected = sorted(actual - wanted)
        details = []
        if missing:
            details.append(f"missing {missing}")
        if unexpected:
            details.append(f"unexpected {unexpected}")
        raise BundleValidationError(f"{field} has invalid keys ({'; '.join(details)})")
    return value


def _reject_duplicate_keys(pairs: Sequence[Tuple[str, object]]) -> Dict[str, object]:
    result: Dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise BundleValidationError(f"manifest contains duplicate JSON key {key!r}")
        result[key] = value
    return result


def _reject_json_constant(value: str) -> object:
    raise BundleValidationError(f"manifest contains invalid JSON constant {value!r}")


def _manifest_text(value: object, field: str) -> str:
    if not isinstance(value, str) or not value or len(value) > 512:
        raise BundleValidationError(f"{field} must be a non-empty string of at most 512 characters")
    if any(ord(character) < 0x20 or ord(character) > 0x7E for character in value):
        raise BundleValidationError(f"{field} must contain printable ASCII without newlines")
    return value


def _parse_manifest(
    manifest_bytes: bytes, archive_sha256: str, expected_target: str
) -> BundleManifest:
    try:
        manifest_text = manifest_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise BundleValidationError("manifest.json must be UTF-8") from error
    try:
        value = json.loads(
            manifest_text,
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=_reject_json_constant,
        )
    except BundleValidationError:
        raise
    except (json.JSONDecodeError, TypeError, ValueError) as error:
        raise BundleValidationError(f"manifest.json is not strict JSON: {error}") from error

    root = _require_exact_keys(
        value,
        ("schema_version", "format", "target", "source", "build", "files"),
        "manifest",
    )
    if type(root["schema_version"]) is not int or root["schema_version"] != BUNDLE_SCHEMA_VERSION:
        raise BundleValidationError(
            f"manifest.schema_version must be {BUNDLE_SCHEMA_VERSION}"
        )
    if root["format"] != BUNDLE_FORMAT:
        raise BundleValidationError(f"manifest.format must be {BUNDLE_FORMAT!r}")
    if root["target"] != expected_target:
        raise BundleValidationError(
            f"manifest.target must be {expected_target!r}, got {root['target']!r}"
        )

    source = _require_exact_keys(
        root["source"], ("commit", "tree", "dirty", "locks"), "manifest.source"
    )
    commit = source["commit"]
    tree = source["tree"]
    if not isinstance(commit, str) or _GIT_OBJECT_RE.fullmatch(commit) is None:
        raise BundleValidationError("manifest.source.commit must be a lowercase Git object ID")
    if not isinstance(tree, str) or _GIT_OBJECT_RE.fullmatch(tree) is None:
        raise BundleValidationError("manifest.source.tree must be a lowercase Git object ID")
    if len(commit) != len(tree):
        raise BundleValidationError("manifest source commit and tree use different hash formats")
    if source["dirty"] is not False:
        raise BundleValidationError("manifest.source.dirty must be false")
    locks = _require_exact_keys(source["locks"], _LOCK_KEYS, "manifest.source.locks")
    lock_sha256s = {
        name: validated_sha256(locks[name], f"manifest.source.locks.{name}")
        for name in _LOCK_KEYS
    }

    raw_build = root["build"]
    if isinstance(raw_build, dict) and "cargo_features" not in raw_build:
        # Bundles built before the field existed have no features, so the
        # adapter refuses them with a rebuild message rather than a key error.
        raw_build = {**raw_build, "cargo_features": []}
    build = _require_exact_keys(
        raw_build, _TOOL_KEYS + ("cargo_features", "glibc"), "manifest.build"
    )
    tool_versions = {
        name: _manifest_text(build[name], f"manifest.build.{name}") for name in _TOOL_KEYS
    }
    glibc_version = _manifest_text(build["glibc"], "manifest.build.glibc")
    if _GLIBC_RE.fullmatch(glibc_version) is None:
        raise BundleValidationError(
            "manifest.build.glibc must have the canonical form 'glibc <version>'"
        )
    cargo_features = build["cargo_features"]
    if (
        not isinstance(cargo_features, list)
        or any(
            not isinstance(feature, str) or _CARGO_FEATURE_RE.fullmatch(feature) is None
            for feature in cargo_features
        )
        or cargo_features != sorted(set(cargo_features))
    ):
        raise BundleValidationError(
            "manifest.build.cargo_features must be a sorted array of unique "
            "'package/feature' names"
        )

    raw_files = _require_exact_keys(root["files"], REQUIRED_RUNTIME_FILES, "manifest.files")
    files: Dict[str, BundleFile] = {}
    for name in REQUIRED_RUNTIME_FILES:
        raw_metadata = _require_exact_keys(
            raw_files[name], ("sha256", "size", "mode"), f"manifest.files.{name}"
        )
        digest = validated_sha256(raw_metadata["sha256"], f"manifest.files.{name}.sha256")
        size = raw_metadata["size"]
        if type(size) is not int or size <= 0 or size > _MAX_MEMBER_BYTES:
            raise BundleValidationError(
                f"manifest.files.{name}.size must be an integer from 1 to {_MAX_MEMBER_BYTES}"
            )
        if raw_metadata["mode"] != "0755":
            raise BundleValidationError(f"manifest.files.{name}.mode must be '0755'")
        files[name] = BundleFile(sha256=digest, size=size, mode=0o755)

    return BundleManifest(
        schema_version=BUNDLE_SCHEMA_VERSION,
        bundle_format=BUNDLE_FORMAT,
        source_commit=commit,
        source_tree=tree,
        source_dirty=False,
        target=expected_target,
        glibc_version=glibc_version,
        lock_sha256s=MappingProxyType(lock_sha256s),
        tool_versions=MappingProxyType(tool_versions),
        cargo_features=tuple(cargo_features),
        files=MappingProxyType(files),
        archive_sha256=archive_sha256,
    )


def _validate_member_header(member: tarfile.TarInfo) -> None:
    name = member.name
    path = PurePosixPath(name)
    if (
        not name
        or "\\" in name
        or path.is_absolute()
        or len(path.parts) != 1
        or path.name in ("", ".", "..")
    ):
        raise BundleValidationError(f"unsafe archive member path {name!r}")
    if name not in _EXPECTED_ARCHIVE_MEMBERS:
        raise BundleValidationError(f"unexpected archive member {name!r}")
    if not member.isfile() or member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE):
        raise BundleValidationError(f"archive member {name!r} must be a regular file")
    if member.linkname:
        raise BundleValidationError(f"archive member {name!r} must not have a link target")
    if member.pax_headers:
        raise BundleValidationError(f"archive member {name!r} must not use PAX headers")
    if getattr(member, "sparse", None):
        raise BundleValidationError(f"archive member {name!r} must not be sparse")
    if member.uid != 0 or member.gid != 0 or member.uname or member.gname:
        raise BundleValidationError(f"archive member {name!r} has non-normalized ownership")
    if member.mtime != 0:
        raise BundleValidationError(f"archive member {name!r} has non-normalized mtime")
    expected_mode = _EXPECTED_MEMBER_MODES[name]
    if member.mode != expected_mode:
        raise BundleValidationError(
            f"archive member {name!r} mode must be {expected_mode:04o}, got {member.mode:04o}"
        )
    maximum = _MAX_MANIFEST_BYTES if name == MANIFEST_NAME else _MAX_MEMBER_BYTES
    if member.size <= 0 or member.size > maximum:
        raise BundleValidationError(
            f"archive member {name!r} size must be from 1 to {maximum} bytes"
        )


def _read_member(archive: tarfile.TarFile, member: tarfile.TarInfo, maximum: int) -> bytes:
    stream = archive.extractfile(member)
    if stream is None:
        raise BundleValidationError(f"cannot read archive member {member.name!r}")
    value = stream.read(maximum + 1)
    if len(value) != member.size:
        raise BundleValidationError(f"archive member {member.name!r} is truncated or oversized")
    return value


def _hash_member(archive: tarfile.TarFile, member: tarfile.TarInfo) -> Tuple[str, int]:
    stream = archive.extractfile(member)
    if stream is None:
        raise BundleValidationError(f"cannot read archive member {member.name!r}")
    digest = hashlib.sha256()
    size = 0
    while True:
        block = stream.read(_COPY_CHUNK_BYTES)
        if not block:
            break
        size += len(block)
        if size > member.size:
            raise BundleValidationError(f"archive member {member.name!r} exceeds its header size")
        digest.update(block)
    if size != member.size:
        raise BundleValidationError(f"archive member {member.name!r} is truncated")
    return digest.hexdigest(), size


class _LimitedReader:
    """Bound decompressed bytes consumed by tarfile's streaming reader."""

    def __init__(self, source: BinaryIO, limit: int) -> None:
        self._source = source
        self._limit = limit
        self._count = 0

    def read(self, size: int = -1) -> bytes:
        remaining = self._limit - self._count
        if remaining < 0:
            raise BundleValidationError("decompressed tar stream exceeds its size limit")
        if size < 0 or size > remaining + 1:
            size = remaining + 1
        value = self._source.read(size)
        self._count += len(value)
        if self._count > self._limit:
            raise BundleValidationError("decompressed tar stream exceeds its size limit")
        return value

    @property
    def count(self) -> int:
        return self._count


def _validate_snapshot(
    snapshot: BinaryIO, archive_sha256: str, expected_target: str
) -> BundleManifest:
    try:
        snapshot.seek(0)
        if snapshot.read(len(_NORMALIZED_GZIP_HEADER)) != _NORMALIZED_GZIP_HEADER:
            raise BundleValidationError("bundle does not have a normalized gzip header")
        snapshot.seek(0)
        with gzip.GzipFile(fileobj=snapshot, mode="rb") as compressed:
            limited = _LimitedReader(compressed, _MAX_TAR_STREAM_BYTES)
            with tarfile.open(
                fileobj=limited, mode="r|", format=tarfile.USTAR_FORMAT, ignore_zeros=True
            ) as archive:
                by_name: Dict[str, tarfile.TarInfo] = {}
                observed: Dict[str, Tuple[str, int]] = {}
                manifest_bytes = None
                unpacked_size = 0
                next_header_offset = 0
                for member in archive:
                    if (
                        member.offset != next_header_offset
                        or member.offset_data != member.offset + tarfile.BLOCKSIZE
                    ):
                        raise BundleValidationError(
                            "archive contains hidden or non-normalized tar headers"
                        )
                    if member.name in by_name:
                        raise BundleValidationError(f"duplicate archive member {member.name!r}")
                    _validate_member_header(member)
                    by_name[member.name] = member
                    unpacked_size += member.size
                    if unpacked_size > _MAX_UNPACKED_BYTES:
                        raise BundleValidationError(
                            f"archive exceeds {_MAX_UNPACKED_BYTES} unpacked bytes"
                        )
                    if member.name == MANIFEST_NAME:
                        manifest_bytes = _read_member(
                            archive, member, _MAX_MANIFEST_BYTES
                        )
                    else:
                        observed[member.name] = _hash_member(archive, member)
                    padded_size = (
                        (member.size + tarfile.BLOCKSIZE - 1)
                        // tarfile.BLOCKSIZE
                        * tarfile.BLOCKSIZE
                    )
                    next_header_offset = member.offset_data + padded_size

                minimum_end = next_header_offset + 2 * tarfile.BLOCKSIZE
                normalized_stream_size = (
                    (minimum_end + tarfile.RECORDSIZE - 1)
                    // tarfile.RECORDSIZE
                    * tarfile.RECORDSIZE
                )
                if limited.count != normalized_stream_size:
                    raise BundleValidationError(
                        "archive has non-normalized tar padding or trailing data"
                    )

                if set(by_name) != set(_EXPECTED_ARCHIVE_MEMBERS):
                    missing = set(_EXPECTED_ARCHIVE_MEMBERS) - set(by_name)
                    if missing:
                        raise BundleValidationError(
                            f"archive is missing members {sorted(missing)}"
                        )
                    raise BundleValidationError(
                        "archive must contain exactly manifest.json and the four runtime files"
                    )
                if manifest_bytes is None:
                    raise BundleValidationError("archive is missing manifest.json")
                manifest = _parse_manifest(
                    manifest_bytes, archive_sha256, expected_target
                )

                for name in REQUIRED_RUNTIME_FILES:
                    member = by_name[name]
                    metadata = manifest.files[name]
                    digest, size = observed[name]
                    if member.size != metadata.size or size != metadata.size:
                        raise BundleValidationError(
                            f"archive member {name!r} size does not match manifest"
                        )
                    if member.mode != metadata.mode:
                        raise BundleValidationError(
                            f"archive member {name!r} mode does not match manifest"
                        )
                    if not hmac.compare_digest(digest, metadata.sha256):
                        raise BundleValidationError(
                            f"archive member {name!r} content does not match manifest"
                        )
                return manifest
    except BundleValidationError:
        raise
    except (tarfile.TarError, EOFError, OSError, ValueError) as error:
        raise BundleValidationError(f"invalid gzip tar bundle: {error}") from error


def validate_bundle(
    bundle_path: PathLike,
    expected_sha256: str,
    expected_target: str = SUPPORTED_TARGET,
) -> BundleManifest:
    """Authenticate and strictly validate a bundle without extracting it.

    ``expected_sha256`` must come from a trusted adapter configuration, not
    from the bundle or its adjacent sidecar.  The only supported payload target
    is x86_64 Linux with glibc; callers cannot weaken that check by supplying a
    different ``expected_target``.
    """

    if expected_target != SUPPORTED_TARGET:
        raise BundleValidationError(
            f"unsupported expected target {expected_target!r}; only {SUPPORTED_TARGET!r} is allowed"
        )
    with _verified_snapshot(bundle_path, expected_sha256) as (snapshot, archive_sha256):
        return _validate_snapshot(snapshot, archive_sha256, expected_target)


def assert_supported_runtime() -> str:
    """Fail closed unless the current machine is x86_64 Linux with glibc.

    Controllers may validate a bundle on another OS, so this check is explicit
    rather than part of :func:`validate_bundle`.  Invoke it in the task
    container before installing or executing the runtime payload.
    """

    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise BundleValidationError("S-Code Harbor bundles require x86_64 Linux")
    try:
        glibc_version = os.confstr("CS_GNU_LIBC_VERSION")
    except (AttributeError, OSError, ValueError):
        glibc_version = None
    if not glibc_version or _GLIBC_RE.fullmatch(glibc_version) is None:
        raise BundleValidationError("S-Code Harbor bundles require detectable glibc")
    return glibc_version


def _main(arguments: Sequence[str]) -> int:
    parser = argparse.ArgumentParser(description="Validate a pinned S-Code Harbor bundle")
    parser.add_argument("bundle", type=Path)
    parser.add_argument("sha256", help="trusted lowercase SHA-256 of the complete archive")
    options = parser.parse_args(arguments)
    manifest = validate_bundle(options.bundle, options.sha256)
    print(
        f"validated {options.bundle}: {manifest.source_commit} "
        f"{manifest.target} {manifest.archive_sha256}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(_main(os.sys.argv[1:]))
