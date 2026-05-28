#!/usr/bin/env python3
"""Create a clean Tibia 15.23 client package from the real asset closure.

The asset closure is based on:
- assets/catalog-content.json file entries
- map-*.dat protobuf map asset records
- sounds/catalog-sound.json and protobuf sound-bank string fields

The script never deletes the source package. It writes a new ZIP and a report,
then can re-audit the generated ZIP to prove missingFiles=0 and extraFiles=0.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Iterable


CATALOG_CONTENT = "assets/catalog-content.json"
CATALOG_SOUND = "sounds/catalog-sound.json"

ROOT_METADATA_NAMES = {
    "assets.json",
    "assets.json.sha256",
    "login-build-metadata.json",
    "package.json",
    "package.json.version",
    "version.txt",
}

CORE_ROOT_DIRS = {
    "3rdpartylicences",
    "bin",
}

CORE_CONF_FILES = {
    "conf/config.ini",
}

VOLATILE_ROOT_DIRS = {
    ".git",
    ".idea",
    ".vs",
    "cache",
    "characterdata",
    "crashdump",
    "log",
    "minimap",
    "screenshots",
    "storeimages",
}


class CleanError(RuntimeError):
    pass


@dataclass(frozen=True)
class ReferencedFile:
    path: str
    reason: str


class Source:
    def names(self) -> list[str]:
        raise NotImplementedError

    def read_bytes(self, relative_path: str) -> bytes:
        raise NotImplementedError

    def exists(self, relative_path: str) -> bool:
        return self.resolve_name(relative_path) is not None

    def resolve_name(self, relative_path: str) -> str | None:
        raise NotImplementedError


class FolderSource(Source):
    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self._names: list[str] | None = None
        self._index: dict[str, str] | None = None

    def names(self) -> list[str]:
        if self._names is None:
            names: list[str] = []
            for path in self.root.rglob("*"):
                if path.is_file():
                    names.append(path.relative_to(self.root).as_posix())
            names.sort(key=str.lower)
            self._names = names
            self._index = {name.lower(): name for name in names}
        return list(self._names)

    def resolve_name(self, relative_path: str) -> str | None:
        self.names()
        assert self._index is not None
        return self._index.get(normalize_rel(relative_path).lower())

    def read_bytes(self, relative_path: str) -> bytes:
        resolved = self.resolve_name(relative_path)
        if resolved is None:
            raise CleanError(f"missing file: {relative_path}")
        return (self.root / Path(resolved)).read_bytes()


class ZipSource(Source):
    def __init__(self, zip_path: Path) -> None:
        self.zip_path = zip_path.resolve()
        self._zip = zipfile.ZipFile(self.zip_path)
        names = [
            normalize_rel(info.filename)
            for info in self._zip.infolist()
            if not info.is_dir()
        ]
        names.sort(key=str.lower)
        self._names = names
        self._index = {name.lower(): name for name in names}

    def names(self) -> list[str]:
        return list(self._names)

    def resolve_name(self, relative_path: str) -> str | None:
        return self._index.get(normalize_rel(relative_path).lower())

    def read_bytes(self, relative_path: str) -> bytes:
        resolved = self.resolve_name(relative_path)
        if resolved is None:
            raise CleanError(f"missing file: {relative_path}")
        return self._zip.read(resolved)

    def close(self) -> None:
        self._zip.close()


def normalize_rel(path: str) -> str:
    path = path.replace("\\", "/").strip("/")
    parts: list[str] = []
    for part in PurePosixPath(path).parts:
        if part in ("", "."):
            continue
        if part == "..":
            raise CleanError(f"unsafe relative path: {path}")
        parts.append(part)
    return "/".join(parts)


def asset_rel(file_name: str) -> str:
    normalized = normalize_rel(file_name)
    if "/" not in normalized:
        return f"assets/{normalized}"
    return normalized


def sound_rel(file_name: str) -> str:
    normalized = normalize_rel(file_name)
    if "/" not in normalized:
        return f"sounds/{normalized}"
    return normalized


def read_json(source: Source, relative_path: str) -> Any:
    try:
        return json.loads(source.read_bytes(relative_path).decode("utf-8-sig"))
    except json.JSONDecodeError as error:
        raise CleanError(f"invalid JSON in {relative_path}: {error}") from error


def iter_file_values(value: Any) -> Iterable[str]:
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "file" and isinstance(child, str):
                yield child
            else:
                yield from iter_file_values(child)
    elif isinstance(value, list):
        for child in value:
            yield from iter_file_values(child)


def read_varint(data: bytes, offset: int) -> tuple[int, int]:
    value = 0
    shift = 0
    while offset < len(data):
        current = data[offset]
        offset += 1
        value |= (current & 0x7F) << shift
        if current < 0x80:
            return value, offset
        shift += 7
        if shift > 63:
            raise CleanError("protobuf varint is too large")
    raise CleanError("truncated protobuf varint")


def skip_protobuf_field(data: bytes, offset: int, wire_type: int) -> int:
    if wire_type == 0:
        _, offset = read_varint(data, offset)
        return offset
    if wire_type == 1:
        return offset + 8
    if wire_type == 2:
        length, offset = read_varint(data, offset)
        return offset + length
    if wire_type == 5:
        return offset + 4
    raise CleanError(f"unsupported protobuf wire type {wire_type}")


def parse_position(data: bytes) -> tuple[int, int, int]:
    offset = 0
    position = {1: 0, 2: 0, 3: 0}
    while offset < len(data):
        key, offset = read_varint(data, offset)
        field = key >> 3
        wire_type = key & 7
        if field in position and wire_type == 0:
            position[field], offset = read_varint(data, offset)
        else:
            offset = skip_protobuf_field(data, offset, wire_type)
    return position[1], position[2], position[3]


def parse_map_asset(data: bytes) -> dict[int, Any]:
    offset = 0
    asset: dict[int, Any] = {}
    while offset < len(data):
        key, offset = read_varint(data, offset)
        field = key >> 3
        wire_type = key & 7
        if field in (1, 4, 5, 6) and wire_type == 0:
            asset[field], offset = read_varint(data, offset)
        elif field == 2 and wire_type == 2:
            length, offset = read_varint(data, offset)
            asset[field] = parse_position(data[offset : offset + length])
            offset += length
        elif field == 3 and wire_type == 2:
            length, offset = read_varint(data, offset)
            asset[field] = data[offset : offset + length].decode("utf-8", "strict")
            offset += length
        elif field == 7 and wire_type == 1:
            offset += 8
        else:
            offset = skip_protobuf_field(data, offset, wire_type)
    return asset


def parse_map_assets(data: bytes) -> list[dict[int, Any]]:
    offset = 0
    assets: list[dict[int, Any]] = []
    while offset < len(data):
        key, offset = read_varint(data, offset)
        field = key >> 3
        wire_type = key & 7
        if field == 3 and wire_type == 2:
            length, offset = read_varint(data, offset)
            assets.append(parse_map_asset(data[offset : offset + length]))
            offset += length
        else:
            offset = skip_protobuf_field(data, offset, wire_type)
    return assets


def iter_protobuf_strings(data: bytes, max_depth: int = 8) -> Iterable[str]:
    def walk(buffer: bytes, depth: int) -> Iterable[str]:
        offset = 0
        while offset < len(buffer):
            try:
                key, offset = read_varint(buffer, offset)
            except CleanError:
                return
            field = key >> 3
            wire_type = key & 7
            if field == 0:
                return
            if wire_type == 2:
                try:
                    length, offset = read_varint(buffer, offset)
                except CleanError:
                    return
                if length < 0 or offset + length > len(buffer):
                    return
                payload = buffer[offset : offset + length]
                offset += length
                try:
                    text = payload.decode("utf-8", "strict")
                except UnicodeDecodeError:
                    text = ""
                if text and all(
                    (character >= " " and character != "\x7f")
                    or character in "\r\n\t"
                    for character in text
                ):
                    yield text
                if depth < max_depth and len(payload) > 1:
                    yield from walk(payload, depth + 1)
            else:
                try:
                    offset = skip_protobuf_field(buffer, offset, wire_type)
                except CleanError:
                    return

    yield from walk(data, 0)


def add_reason(reasons: dict[str, set[str]], relative_path: str, reason: str) -> None:
    reasons.setdefault(normalize_rel(relative_path), set()).add(reason)


def collect_expected_files(source: Source) -> tuple[set[str], dict[str, set[str]], list[ReferencedFile]]:
    all_names = source.names()
    all_lower = {name.lower(): name for name in all_names}
    expected: set[str] = set()
    reasons: dict[str, set[str]] = {}
    missing: list[ReferencedFile] = []

    def keep(path: str, reason: str) -> None:
        normalized = normalize_rel(path)
        resolved = all_lower.get(normalized.lower())
        if resolved is None:
            missing.append(ReferencedFile(normalized, reason))
            return
        expected.add(resolved)
        add_reason(reasons, resolved, reason)

    keep(CATALOG_CONTENT, "catalog-content")
    catalog = read_json(source, CATALOG_CONTENT)
    for file_name in iter_file_values(catalog):
        keep(asset_rel(file_name), "catalog file")

    if isinstance(catalog, list):
        map_files = [
            item.get("file")
            for item in catalog
            if isinstance(item, dict)
            and item.get("type") == "map"
            and isinstance(item.get("file"), str)
        ]
    else:
        map_files = [
            file_name
            for file_name in iter_file_values(catalog)
            if Path(file_name).name.lower().startswith("map-")
        ]

    for map_file in map_files:
        map_path = asset_rel(map_file)
        resolved_map = source.resolve_name(map_path)
        if resolved_map is None:
            missing.append(ReferencedFile(map_path, "map protobuf"))
            continue
        for asset in parse_map_assets(source.read_bytes(resolved_map)):
            file_name = asset.get(3)
            if isinstance(file_name, str) and file_name:
                keep(asset_rel(file_name), "map protobuf resource_files.file_name")

    if source.exists(CATALOG_SOUND):
        keep(CATALOG_SOUND, "catalog-sound")
        sound_catalog = read_json(source, CATALOG_SOUND)
        sound_bank_files = list(iter_file_values(sound_catalog))
        for sound_bank in sound_bank_files:
            bank_path = sound_rel(sound_bank)
            keep(bank_path, "sound catalog file")
            resolved_bank = source.resolve_name(bank_path)
            if resolved_bank is None:
                continue
            for sound_name in iter_protobuf_strings(source.read_bytes(resolved_bank)):
                if sound_name.lower().endswith(".ogg"):
                    keep(sound_rel(sound_name), "sound bank protobuf ogg")

    for name in all_names:
        lower = name.lower()
        root = lower.split("/", 1)[0]
        file_name = lower.rsplit("/", 1)[-1]
        if root in CORE_ROOT_DIRS:
            expected.add(name)
            add_reason(reasons, name, f"core {root}")
        elif lower in CORE_CONF_FILES:
            expected.add(name)
            add_reason(reasons, name, "core conf")
        elif "/" not in lower and (
            file_name in ROOT_METADATA_NAMES
            or file_name == "manifest"
            or file_name == "manifest.json"
            or file_name.endswith(".manifest")
        ):
            expected.add(name)
            add_reason(reasons, name, "metadata")

    return expected, reasons, missing


def is_volatile_or_untracked(relative_path: str) -> bool:
    root = relative_path.lower().split("/", 1)[0]
    return root in VOLATILE_ROOT_DIRS


def audit_source(source: Source, source_label: str) -> dict[str, Any]:
    names = source.names()
    expected, reasons, missing = collect_expected_files(source)
    extras = sorted(
        [name for name in names if name not in expected],
        key=str.lower,
    )
    removed_examples = [
        name for name in extras if not is_volatile_or_untracked(name)
    ][:50]
    if len(removed_examples) < 50:
        for name in extras:
            if name not in removed_examples:
                removed_examples.append(name)
            if len(removed_examples) >= 50:
                break

    reason_counts: dict[str, int] = {}
    for reason_set in reasons.values():
        for reason in reason_set:
            reason_counts[reason] = reason_counts.get(reason, 0) + 1

    return {
        "source": source_label,
        "totalFilesOriginal": len(names),
        "totalKept": len(expected),
        "totalRemoved": len(extras),
        "extraFiles": len(extras),
        "missingFiles": len(missing),
        "removedExamples": removed_examples,
        "missingRefs": [
            {"path": item.path, "reason": item.reason} for item in missing
        ],
        "keptByReason": dict(sorted(reason_counts.items())),
        "keptFiles": sorted(expected, key=str.lower),
        "extraFileList": extras,
    }


def write_clean_zip(source: Source, output_zip: Path, expected_files: Iterable[str]) -> None:
    output_zip.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output_zip, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name in sorted(expected_files, key=str.lower):
            archive.writestr(name, source.read_bytes(name))


def write_report(report_path: Path, report: dict[str, Any]) -> None:
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def default_output_path(source_path: Path) -> Path:
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    return source_path.resolve().parent / f"{source_path.stem}-clean-{stamp}.zip"


def open_source(path: Path) -> Source:
    if path.is_dir():
        return FolderSource(path)
    if path.is_file() and path.suffix.lower() == ".zip":
        return ZipSource(path)
    raise CleanError(f"source must be a folder or .zip: {path}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Build a clean client ZIP from catalog protobuf closure.")
    parser.add_argument("--source", required=True, help="Official client folder or ZIP.")
    parser.add_argument("--output", default="", help="Output clean ZIP path.")
    parser.add_argument("--report", default="", help="Output report JSON path.")
    parser.add_argument("--audit-only", action="store_true", help="Only audit; do not write a ZIP.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    source_path = Path(args.source)
    output_zip = Path(args.output) if args.output else default_output_path(source_path)
    report_path = (
        Path(args.report)
        if args.report
        else output_zip.with_suffix(output_zip.suffix + ".report.json")
    )

    source = open_source(source_path)
    try:
        report = audit_source(source, str(source_path))
        if report["missingFiles"]:
            write_report(report_path, report)
            raise CleanError(
                f"missing refs found; report written to {report_path}. ZIP was not generated."
            )

        if not args.audit_only:
            write_clean_zip(source, output_zip, report["keptFiles"])
            zip_source = ZipSource(output_zip)
            try:
                validation = audit_source(zip_source, str(output_zip))
            finally:
                zip_source.close()
            report["outputZip"] = str(output_zip)
            report["validation"] = {
                "extraFiles": validation["extraFiles"],
                "missingFiles": validation["missingFiles"],
                "totalFilesOriginal": validation["totalFilesOriginal"],
                "totalKept": validation["totalKept"],
            }
            if validation["extraFiles"] or validation["missingFiles"]:
                report["validation"]["missingRefs"] = validation["missingRefs"]
                report["validation"]["extraFileList"] = validation["extraFileList"]
                write_report(report_path, report)
                raise CleanError(f"validation failed for generated ZIP: {output_zip}")

        write_report(report_path, report)
        print(json.dumps({
            "report": str(report_path),
            "outputZip": str(output_zip) if not args.audit_only else None,
            "totalFilesOriginal": report["totalFilesOriginal"],
            "totalKept": report["totalKept"],
            "totalRemoved": report["totalRemoved"],
            "extraFiles": report.get("validation", {}).get("extraFiles"),
            "missingFiles": report.get("validation", {}).get("missingFiles"),
        }, indent=2))
        return 0
    finally:
        if isinstance(source, ZipSource):
            source.close()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CleanError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1)
