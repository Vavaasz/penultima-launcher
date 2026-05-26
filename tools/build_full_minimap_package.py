#!/usr/bin/env python3
"""Build the launcher full minimap package from client and server map assets.

The client needs only ``minimap/Minimap_Color_*`` for the normal automap
reveal. Those files are the same cache surface the client populates as players
walk around.

The Cyclopedia ``map-*.dat`` protobuf already contains exact top-left
coordinates for each ``minimap-32`` asset. This script decodes those assets and
splits them into the 256x256 normal automap PNG tiles consumed by the client.

Do not generate ``Minimap_WaypointCost_*`` files. The client uses those files
for mouse pathing/click movement and writes valid values as the player explores.
Fake waypoint costs can make mouse movement impossible even when keyboard
movement still works.

Do not publish generated server Cyclopedia assets through this package. The
15.23 client treats those assets as startup-critical, and a mismatched catalog,
map, staticdata, or staticmapdata file can prevent the client from opening.
"""

from __future__ import annotations

import argparse
import binascii
import hashlib
import json
import lzma
import re
import struct
import sys
import zipfile
import zlib
from pathlib import Path


ASSET_PREFIXES = (
    "subarea-",
    "minimap-",
    "satellite-",
    "map-",
    "staticdata-",
    "staticmapdata-",
)
WORLD_PACKAGE_ASSET_PREFIXES = (
    "minimap-",
    "satellite-",
)
CLIENT_MINIMAP_RE = re.compile(r"^Minimap_Color_\d+_\d+_\d+\.png$", re.I)
CIP_LZMA_SIGNATURE_PREFIX = bytes((0x70, 0x0A, 0xFA, 0x80))
CIP_LZMA_MARKER_SIZE = 5
CIP_HEADER_SIZE = 32
NORMAL_TILE_SIZE = 256
MINIMAP_ASSET_TYPE = 2
FULL_RES_MINIMAP_SCALE = 1.0 / 32.0
PNG_COMPRESSION_LEVEL = 6


class BuildError(RuntimeError):
    pass


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
            raise BuildError("protobuf varint is too large")
    raise BuildError("truncated protobuf varint")


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
    raise BuildError(f"unsupported protobuf wire type {wire_type}")


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


def parse_map_asset(data: bytes) -> dict[int, object]:
    offset = 0
    asset: dict[int, object] = {}
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
            asset[field] = data[offset : offset + length].decode("utf-8", "replace")
            offset += length
        elif field == 7 and wire_type == 1:
            asset[field] = struct.unpack_from("<d", data, offset)[0]
            offset += 8
        else:
            offset = skip_protobuf_field(data, offset, wire_type)
    return asset


def parse_map_assets(data: bytes) -> list[dict[int, object]]:
    offset = 0
    assets: list[dict[int, object]] = []
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


def decode_cip_lzma_asset(path: Path) -> bytes:
    data = path.read_bytes()
    if len(data) <= CIP_HEADER_SIZE:
        raise BuildError(f"{path} is too small to be a CIP LZMA asset")

    signature_offset = data[:CIP_HEADER_SIZE].find(CIP_LZMA_SIGNATURE_PREFIX)
    if signature_offset < 0:
        raise BuildError(f"{path} does not contain the CIP LZMA signature")

    payload_size, _ = read_varint(data, signature_offset + CIP_LZMA_MARKER_SIZE)
    payload = data[CIP_HEADER_SIZE : CIP_HEADER_SIZE + payload_size]
    if len(payload) != payload_size:
        raise BuildError(f"{path} has a truncated CIP LZMA payload")
    if len(payload) < 13:
        raise BuildError(f"{path} has an invalid LZMA payload")

    properties = payload[0]
    dict_size = struct.unpack_from("<I", payload, 1)[0]
    lc = properties % 9
    rest = properties // 9
    lp = rest % 5
    pb = rest // 5
    filters = [
        {
            "id": lzma.FILTER_LZMA1,
            "dict_size": dict_size,
            "lc": lc,
            "lp": lp,
            "pb": pb,
        }
    ]
    return lzma.decompress(payload[13:], format=lzma.FORMAT_RAW, filters=filters)


def decode_bmp_rgb(data: bytes) -> tuple[int, int, bytes]:
    if len(data) < 54 or data[:2] != b"BM":
        raise BuildError("decoded asset is not a BMP file")

    pixel_offset = struct.unpack_from("<I", data, 10)[0]
    width = struct.unpack_from("<i", data, 18)[0]
    height_raw = struct.unpack_from("<i", data, 22)[0]
    bits_per_pixel = struct.unpack_from("<H", data, 28)[0]
    compression = struct.unpack_from("<I", data, 30)[0]
    if width <= 0 or height_raw == 0:
        raise BuildError("BMP has invalid dimensions")
    if bits_per_pixel not in (24, 32):
        raise BuildError(f"unsupported BMP depth {bits_per_pixel}")
    if compression not in (0, 3):
        raise BuildError(f"unsupported BMP compression {compression}")

    height = abs(height_raw)
    top_down = height_raw < 0
    row_stride = ((width * bits_per_pixel + 31) // 32) * 4
    bytes_per_pixel = bits_per_pixel // 8
    rgb = bytearray(width * height * 3)

    for y in range(height):
        source_y = y if top_down else height - 1 - y
        source_offset = pixel_offset + source_y * row_stride
        target_offset = y * width * 3
        for x in range(width):
            pixel_offset_in_row = source_offset + x * bytes_per_pixel
            blue = data[pixel_offset_in_row]
            green = data[pixel_offset_in_row + 1]
            red = data[pixel_offset_in_row + 2]
            target = target_offset + x * 3
            rgb[target : target + 3] = bytes((red, green, blue))

    return width, height, bytes(rgb)


def png_chunk(chunk_type: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + chunk_type
        + payload
        + struct.pack(">I", binascii.crc32(chunk_type + payload) & 0xFFFFFFFF)
    )


def encode_png_rgb(width: int, height: int, rgb: bytes) -> bytes:
    if len(rgb) != width * height * 3:
        raise BuildError("RGB payload size does not match PNG dimensions")

    rows = bytearray()
    row_length = width * 3
    for y in range(height):
        rows.append(0)
        start = y * row_length
        rows.extend(rgb[start : start + row_length])

    return b"".join(
        (
            b"\x89PNG\r\n\x1a\n",
            png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)),
            png_chunk(b"IDAT", zlib.compress(bytes(rows), PNG_COMPRESSION_LEVEL)),
            png_chunk(b"IEND", b""),
        )
    )


def automap_palette() -> bytes:
    palette = bytearray()
    for index in range(256):
        if index <= 0 or index >= 216:
            palette.extend(b"\x00\x00\x00")
            continue
        palette.extend(
            (
                (index // 36) % 6 * 51,
                (index // 6) % 6 * 51,
                index % 6 * 51,
            )
        )
    return bytes(palette)


AUTOMAP_PALETTE = automap_palette()


def rgb_to_automap_index(red: int, green: int, blue: int) -> int:
    red_index = max(0, min(5, (red + 25) // 51))
    green_index = max(0, min(5, (green + 25) // 51))
    blue_index = max(0, min(5, (blue + 25) // 51))
    index = red_index * 36 + green_index * 6 + blue_index
    if index <= 0 or index >= 216:
        return 0
    return index


def rgb_to_automap_indexes(width: int, height: int, rgb: bytes) -> bytes:
    if len(rgb) != width * height * 3:
        raise BuildError("RGB payload size does not match minimap dimensions")

    color_cache: dict[bytes, int] = {}
    indexes = bytearray(width * height)
    for pixel_offset in range(0, len(rgb), 3):
        color = rgb[pixel_offset : pixel_offset + 3]
        index = color_cache.get(color)
        if index is None:
            index = rgb_to_automap_index(color[0], color[1], color[2])
            color_cache[color] = index
        indexes[pixel_offset // 3] = index
    return bytes(indexes)


def encode_png_indexed(width: int, height: int, indexes: bytes) -> bytes:
    if len(indexes) != width * height:
        raise BuildError("indexed minimap payload size does not match PNG dimensions")

    rows = bytearray()
    for y in range(height):
        rows.append(0)
        start = y * width
        rows.extend(indexes[start : start + width])

    return b"".join(
        (
            b"\x89PNG\r\n\x1a\n",
            png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 3, 0, 0, 0)),
            png_chunk(b"PLTE", AUTOMAP_PALETTE),
            png_chunk(b"IDAT", zlib.compress(bytes(rows), PNG_COMPRESSION_LEVEL)),
            png_chunk(b"IEND", b""),
        )
    )


def is_asset_file(path: Path) -> bool:
    name = path.name.lower()
    return any(name.startswith(prefix) for prefix in ASSET_PREFIXES)


def collect_asset_files(source_roots: list[Path]) -> dict[str, Path]:
    assets: dict[str, Path] = {}
    for root in source_roots:
        if not root.exists():
            continue
        for path in root.iterdir():
            if path.is_file() and is_asset_file(path):
                assets.setdefault(path.name, path)
    return assets


def add_existing_client_minimap(client_root: Path, entries: dict[str, bytes | Path]) -> None:
    minimap_root = client_root / "minimap"
    if not minimap_root.exists():
        return
    for path in minimap_root.rglob("*"):
        if path.is_file() and CLIENT_MINIMAP_RE.match(path.name):
            entries[f"minimap/{path.name}"] = path


def add_client_map_assets(client_assets_root: Path, entries: dict[str, bytes | Path]) -> None:
    if not client_assets_root.exists():
        return
    for path in sorted(client_assets_root.iterdir()):
        if path.is_file() and (is_asset_file(path) or path.name.lower() == "catalog-content.json"):
            entries[f"assets/{path.name}"] = path


def find_world_map_data_path(world_root: Path) -> Path | None:
    if not world_root.exists():
        return None
    map_paths = sorted(world_root.glob("map-*.dat"), key=lambda path: path.stat().st_mtime, reverse=True)
    return map_paths[0] if map_paths else None


def canonical_map_data_filename(data: bytes) -> str:
    return f"map-{hashlib.sha256(data).hexdigest()}.dat"


def patch_catalog_map_entry(client_assets_root: Path, map_filename: str, entries: dict[str, bytes | Path]) -> None:
    catalog_path = client_assets_root / "catalog-content.json"
    if not catalog_path.exists():
        return

    catalog = json.loads(catalog_path.read_text(encoding="utf-8-sig"))
    patched = False
    if isinstance(catalog, list):
        for item in catalog:
            if isinstance(item, dict) and item.get("type") == "map":
                item["file"] = map_filename
                patched = True
    if not patched:
        catalog.append({"type": "map", "file": map_filename})

    entries["assets/catalog-content.json"] = (
        json.dumps(catalog, indent=2, ensure_ascii=False) + "\n"
    ).encode("utf-8")


def add_world_map_assets(world_root: Path, client_assets_root: Path, entries: dict[str, bytes | Path]) -> None:
    world_map_path = find_world_map_data_path(world_root)
    if world_map_path:
        world_map_data = world_map_path.read_bytes()
        world_map_filename = canonical_map_data_filename(world_map_data)
        entries[f"assets/{world_map_filename}"] = world_map_data
        patch_catalog_map_entry(client_assets_root, world_map_filename, entries)

    if not world_root.exists():
        return
    for path in sorted(world_root.iterdir()):
        lower_name = path.name.lower()
        if not path.is_file() or not lower_name.startswith(WORLD_PACKAGE_ASSET_PREFIXES):
            continue
        entries.setdefault(f"assets/{path.name}", path)


def tile_key(world_x: int, world_y: int, floor: int) -> tuple[int, int, int]:
    return (
        (world_x // NORMAL_TILE_SIZE) * NORMAL_TILE_SIZE,
        (world_y // NORMAL_TILE_SIZE) * NORMAL_TILE_SIZE,
        floor,
    )


def decode_minimap_asset(path: Path) -> tuple[int, int, bytes]:
    width, height, rgb = decode_bmp_rgb(decode_cip_lzma_asset(path))
    return width, height, rgb_to_automap_indexes(width, height, rgb)


def merge_minimap_asset_into_tiles(
    asset: dict[int, object],
    asset_path: Path,
    tiles: dict[tuple[int, int, int], bytearray],
) -> None:
    top_left = asset.get(2)
    filename = asset.get(3)
    width_square = int(asset.get(4, 0) or 0)
    height_square = int(asset.get(5, 0) or 0)
    scale = float(asset.get(7, 0.0) or 0.0)
    if (
        not isinstance(top_left, tuple)
        or not isinstance(filename, str)
        or int(asset.get(1, -1) or -1) != MINIMAP_ASSET_TYPE
        or abs(scale - FULL_RES_MINIMAP_SCALE) > 0.000001
    ):
        return

    image_width, image_height, indexes = decode_minimap_asset(asset_path)
    usable_width = min(image_width, width_square if width_square > 0 else image_width)
    usable_height = min(image_height, height_square if height_square > 0 else image_height)
    if usable_width <= 0 or usable_height <= 0:
        return

    start_x, start_y, floor = (int(top_left[0]), int(top_left[1]), int(top_left[2]))
    for source_y in range(usable_height):
        world_y = start_y + source_y
        source_x = 0
        while source_x < usable_width:
            world_x = start_x + source_x
            key = tile_key(world_x, world_y, floor)
            tile = tiles.setdefault(key, bytearray(NORMAL_TILE_SIZE * NORMAL_TILE_SIZE))
            target_x = world_x - key[0]
            target_y = world_y - key[1]
            span = min(usable_width - source_x, NORMAL_TILE_SIZE - target_x)
            source_offset = source_y * image_width + source_x
            target_offset = target_y * NORMAL_TILE_SIZE + target_x
            tile[target_offset : target_offset + span] = indexes[
                source_offset : source_offset + span
            ]
            source_x += span


def generate_normal_minimap_entries(
    source_roots: list[Path],
    asset_files: dict[str, Path],
    entries: dict[str, bytes | Path],
) -> int:
    tiles: dict[tuple[int, int, int], bytearray] = {}
    processed_assets = 0

    for root in source_roots:
        if not root.exists():
            continue
        for map_data_path in sorted(root.glob("map-*.dat")):
            for asset in parse_map_assets(map_data_path.read_bytes()):
                filename = asset.get(3)
                if not isinstance(filename, str):
                    continue
                asset_path = asset_files.get(filename)
                if not asset_path:
                    continue
                before_count = len(tiles)
                merge_minimap_asset_into_tiles(asset, asset_path, tiles)
                if len(tiles) != before_count or int(asset.get(1, -1) or -1) == MINIMAP_ASSET_TYPE:
                    processed_assets += 1

    generated_tiles = 0
    for (base_x, base_y, floor), indexes in sorted(tiles.items()):
        if not any(indexes):
            continue
        color_png = encode_png_indexed(NORMAL_TILE_SIZE, NORMAL_TILE_SIZE, bytes(indexes))
        color_name = f"minimap/Minimap_Color_{base_x}_{base_y}_{floor}.png"
        entries[color_name] = color_png
        generated_tiles += 1

    if generated_tiles == 0:
        raise BuildError("no normal minimap tiles were generated from map assets")

    print(
        f"Generated {generated_tiles} normal minimap color tiles "
        f"from {processed_assets} Cyclopedia minimap assets."
    )
    return generated_tiles


def write_zip(output_path: Path, entries: dict[str, bytes | Path]) -> None:
    output_path.parent.mkdir(parents=True, exist_ok=True)
    if output_path.exists():
        output_path.unlink()

    with zipfile.ZipFile(output_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for archive_name in sorted(entries):
            source = entries[archive_name]
            if isinstance(source, Path):
                archive.write(source, archive_name)
            else:
                archive.writestr(archive_name, source)


def build_package(client_root: Path, world_root: Path, output_path: Path) -> None:
    client_assets_root = client_root / "assets"
    source_roots = [client_assets_root, world_root]
    asset_files = collect_asset_files(source_roots)

    entries: dict[str, bytes | Path] = {}
    add_existing_client_minimap(client_root, entries)
    generated_minimap_files = generate_normal_minimap_entries(source_roots, asset_files, entries)
    write_zip(output_path, entries)

    minimap_count = sum(1 for name in entries if name.startswith("minimap/"))
    asset_count = sum(1 for name in entries if name.startswith("assets/"))
    print(
        f"Full minimap package written: {output_path} "
        f"({minimap_count} minimap files, {asset_count} asset files, "
        f"{generated_minimap_files} generated minimap files)."
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Build Penultima full minimap ZIP")
    parser.add_argument("--client-root", required=True, type=Path)
    parser.add_argument("--world-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        build_package(args.client_root, args.world_root, args.output)
        return 0
    except Exception as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
