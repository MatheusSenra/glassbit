#!/usr/bin/env python3
"""Derives the web page's images from assets/logo.png.

Writes crates/web/www/logo.png (the wordmark, cropped and halved) and
crates/web/www/favicon.png (the letter G). Uses only the standard library,
so it needs no image packages. Run it again whenever the logo changes:

    python scripts/make-web-images.py
"""

import pathlib
import struct
import zlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets" / "logo.png"
WWW = ROOT / "crates" / "web" / "www"

# (left, top, width, height) in the source image, and the shrink factor.
WORDMARK = (204, 246, 1000, 304, 2)
LETTER_G = (238, 274, 132, 132, 2)


def read_png(path):
    """Returns (width, height, rows) for an 8-bit RGB or RGBA PNG."""
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    position, compressed = 8, b""
    while position < len(data):
        (length,) = struct.unpack(">I", data[position : position + 4])
        kind = data[position + 4 : position + 8]
        body = data[position + 8 : position + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert depth == 8 and color in (2, 6) and interlace == 0, "unsupported PNG"
            channels = 3 if color == 2 else 4
        elif kind == b"IDAT":
            compressed += body
        position += 12 + length

    raw = zlib.decompress(compressed)
    stride = width * channels
    rows, previous = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        filter_type = raw[start]
        line = bytearray(raw[start + 1 : start + 1 + stride])
        for i in range(stride):
            left = line[i - channels] if i >= channels else 0
            up = previous[i]
            up_left = previous[i - channels] if i >= channels else 0
            if filter_type == 1:
                line[i] = (line[i] + left) & 0xFF
            elif filter_type == 2:
                line[i] = (line[i] + up) & 0xFF
            elif filter_type == 3:
                line[i] = (line[i] + (left + up) // 2) & 0xFF
            elif filter_type == 4:
                estimate = left + up - up_left
                distances = (abs(estimate - left), abs(estimate - up), abs(estimate - up_left))
                nearest = (left, up, up_left)[distances.index(min(distances))]
                line[i] = (line[i] + nearest) & 0xFF
        # Keep RGB only.
        rows.append(bytes(line) if channels == 3 else bytes(
            line[i] for i in range(stride) if i % 4 != 3
        ))
        previous = line
    return width, height, rows


def crop_and_shrink(rows, left, top, width, height, factor):
    """Crops a region and averages each factor x factor block of pixels."""
    out = []
    for y in range(0, height, factor):
        line = bytearray()
        for x in range(0, width, factor):
            for channel in range(3):
                total = 0
                for dy in range(factor):
                    row = rows[top + y + dy]
                    for dx in range(factor):
                        total += row[(left + x + dx) * 3 + channel]
                line.append(total // (factor * factor))
        out.append(bytes(line))
    return width // factor, height // factor, out


def write_png(path, width, height, rows):
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    raw = b"".join(b"\x00" + row for row in rows)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    print(f"wrote {path.relative_to(ROOT)} ({width}x{height}, {path.stat().st_size} bytes)")


def main():
    _, _, rows = read_png(SOURCE)
    write_png(WWW / "logo.png", *crop_and_shrink(rows, *WORDMARK))
    write_png(WWW / "favicon.png", *crop_and_shrink(rows, *LETTER_G))


if __name__ == "__main__":
    main()
