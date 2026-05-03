#!/usr/bin/env python3
"""Generate a simple W icon as ICO file using only stdlib."""
import struct, io, zlib

SIZES = [256, 48, 32, 16]

def make_w_image(size):
    """Create RGBA pixels for a black background with white W."""
    pixels = bytearray(size * size * 4)
    # Fill black with full alpha
    for i in range(size * size):
        pixels[i*4+3] = 255  # alpha

    # Draw W character
    margin = max(1, size // 8)
    top = margin
    bottom = size - margin
    h = bottom - top

    # W has 5 key x-positions
    x0 = margin
    x1 = size * 3 // 10
    x2 = size // 2
    x3 = size * 7 // 10
    x4 = size - margin - 1

    thickness = max(1, size // 10)

    def draw_line(x0, y0, x1, y1):
        steps = max(abs(x1 - x0), abs(y1 - y0), 1)
        for s in range(steps + 1):
            t = s / steps
            x = int(x0 + (x1 - x0) * t)
            y = int(y0 + (y1 - y0) * t)
            for dx in range(-thickness//2, thickness//2 + 1):
                px = x + dx
                if 0 <= px < size and 0 <= y < size:
                    idx = (y * size + px) * 4
                    pixels[idx] = 255    # R
                    pixels[idx+1] = 255  # G
                    pixels[idx+2] = 255  # B
                    pixels[idx+3] = 255  # A

    # Draw W: 4 strokes
    mid_y = top + h * 2 // 3
    draw_line(x0, top, x1, bottom)   # left down
    draw_line(x1, bottom, x2, mid_y) # up to middle
    draw_line(x2, mid_y, x3, bottom) # down again
    draw_line(x3, bottom, x4, top)   # right up

    return bytes(pixels)

def make_png(size, rgba_data):
    """Create a minimal PNG from RGBA data."""
    def chunk(ctype, data):
        c = ctype + data
        crc = struct.pack('>I', zlib.crc32(c) & 0xffffffff)
        return struct.pack('>I', len(data)) + c + crc

    header = b'\x89PNG\r\n\x1a\n'
    ihdr = struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)
    raw = b''
    for y in range(size):
        raw += b'\x00'  # filter none
        raw += rgba_data[y*size*4:(y+1)*size*4]
    compressed = zlib.compress(raw)
    return header + chunk(b'IHDR', ihdr) + chunk(b'IDAT', compressed) + chunk(b'IEND', b'')

def make_ico(entries):
    """Create ICO file from list of (size, png_data)."""
    count = len(entries)
    header = struct.pack('<HHH', 0, 1, count)
    dir_size = 6 + count * 16
    offset = dir_size
    directory = b''
    data = b''
    for size, png in entries:
        w = 0 if size == 256 else size
        h = 0 if size == 256 else size
        directory += struct.pack('<BBBBHHII', w, h, 0, 0, 1, 32, len(png), offset)
        data += png
        offset += len(png)
    return header + directory + data

entries = []
for size in SIZES:
    rgba = make_w_image(size)
    png = make_png(size, rgba)
    entries.append((size, png))

import os
out_dir = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'assets')
os.makedirs(out_dir, exist_ok=True)
out_path = os.path.join(out_dir, 'icon.ico')
with open(out_path, 'wb') as f:
    f.write(make_ico(entries))
print(f'Generated {out_path} ({os.path.getsize(out_path)} bytes)')
