"""Generate crates/fx-app/assets/fotox.ico from the Fotox logo mark.

The logo is the one `ui/js/main.js` draws (`logoMark`): a #2f6df6 rounded
square (32 x 32 viewBox, rx 8, inset 1) with a white "F" polygon. Pure Python
(no Pillow): 4 x 4 supersampling, PNG-compressed ICO entries at
16/24/32/48/64/128/256 px.

    python crates/fx-app/assets/make_icon.py
"""

import struct
import zlib
from pathlib import Path

BLUE = (0x2F, 0x6D, 0xF6)
WHITE = (0xFF, 0xFF, 0xFF)
# The "F": M9 23V9h13v3.2h-9.3v3.1h8.2v3.2h-8.2V23z
F = [(9, 23), (9, 9), (22, 9), (22, 12.2), (12.7, 12.2), (12.7, 15.3), (20.9, 15.3), (20.9, 18.5), (12.7, 18.5), (12.7, 23)]
SIZES = [16, 24, 32, 48, 64, 128, 256]
SS = 4  # supersampling per axis


def in_rounded_square(x, y):
    x0, y0, x1, y1, r = 1.0, 1.0, 31.0, 31.0, 8.0
    if not (x0 <= x <= x1 and y0 <= y <= y1):
        return False
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def in_polygon(x, y, poly):
    inside = False
    j = len(poly) - 1
    for i in range(len(poly)):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def render(size):
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            r = g = b = a = 0.0
            for sy in range(SS):
                for sx in range(SS):
                    x = (px + (sx + 0.5) / SS) * 32.0 / size
                    y = (py + (sy + 0.5) / SS) * 32.0 / size
                    if in_rounded_square(x, y):
                        color = WHITE if in_polygon(x, y, F) else BLUE
                        r += color[0]
                        g += color[1]
                        b += color[2]
                        a += 255
            n = SS * SS
            alpha = a / n
            if alpha > 0:
                # average colour of the covered samples (straight alpha)
                covered = a / 255
                row += bytes((round(r / covered), round(g / covered), round(b / covered), round(alpha)))
            else:
                row += bytes(4)
        rows.append(bytes(row))
    return rows


def png(size, rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + row for row in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(images):
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    data = b""
    for size, blob in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(blob), offset + len(data))
        data += blob
    return header + entries + data


if __name__ == "__main__":
    images = [(size, png(size, render(size))) for size in SIZES]
    out = Path(__file__).with_name("fotox.ico")
    out.write_bytes(ico(images))
    print(f"wrote {out} ({out.stat().st_size} bytes)")
