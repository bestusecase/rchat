#!/usr/bin/env python3
"""Generate the app and tray icons plus the notification chime."""
import math
import os
import struct
import wave

from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICONS = os.path.join(ROOT, "icons")
ASSETS = os.path.join(ROOT, "assets")
os.makedirs(ICONS, exist_ok=True)
os.makedirs(ASSETS, exist_ok=True)

BG = (20, 22, 27, 255)
PLATE = (124, 140, 255, 255)
PLATE_DARK = (99, 113, 224, 255)
TEAL = (94, 234, 212, 255)


def rounded(size, radius, fill):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=fill)
    return img


def app_icon(size):
    img = rounded(size, int(size * 0.22), BG)
    d = ImageDraw.Draw(img)
    # speech bubble
    pad = int(size * 0.20)
    box = (pad, int(size * 0.18), size - pad, int(size * 0.70))
    d.rounded_rectangle(box, radius=int(size * 0.16), fill=PLATE)
    # tail
    tail = [
        (int(size * 0.34), int(size * 0.66)),
        (int(size * 0.30), int(size * 0.82)),
        (int(size * 0.50), int(size * 0.68)),
    ]
    d.polygon(tail, fill=PLATE)
    # three chat dots
    cy = int(size * 0.44)
    r = max(2, int(size * 0.045))
    for i, cx in enumerate((0.38, 0.50, 0.62)):
        d.ellipse(
            (int(size * cx) - r, cy - r, int(size * cx) + r, cy + r),
            fill=TEAL if i == 1 else BG,
        )
    return img


def tray_icon(size, unread=False):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    s = size
    box = (int(s * 0.10), int(s * 0.14), int(s * 0.88), int(s * 0.68))
    d.rounded_rectangle(box, radius=int(s * 0.16), fill=(235, 235, 240, 255))
    tail = [
        (int(s * 0.26), int(s * 0.64)),
        (int(s * 0.20), int(s * 0.86)),
        (int(s * 0.44), int(s * 0.66)),
    ]
    d.polygon(tail, fill=(235, 235, 240, 255))
    for cx in (0.32, 0.49, 0.66):
        r = max(1, int(s * 0.055))
        x, y = int(s * cx), int(s * 0.41)
        d.ellipse((x - r, y - r, x + r, y + r), fill=(28, 30, 38, 255))
    if unread:
        r = int(s * 0.30)
        cx, cy = s - r - 1, r + 1
        d.ellipse((cx - r - 1, cy - r - 1, cx + r + 1, cy + r + 1), fill=(255, 255, 255, 255))
        d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=(239, 68, 68, 255))
    return img


app_icon(512).save(os.path.join(ICONS, "icon.png"))
app_icon(32).save(os.path.join(ICONS, "32x32.png"))
app_icon(128).save(os.path.join(ICONS, "128x128.png"))
app_icon(256).save(os.path.join(ICONS, "128x128@2x.png"))
tray_icon(32).save(os.path.join(ICONS, "tray.png"))
tray_icon(32, True).save(os.path.join(ICONS, "tray-unread.png"))


def chime(path):
    rate = 44100
    frames = bytearray()
    notes = [(660.0, 0.10), (880.0, 0.13)]
    for freq, dur in notes:
        n = int(rate * dur)
        for i in range(n):
            env = min(1.0, i / (rate * 0.012)) * min(1.0, (n - i) / (rate * 0.03))
            sample = math.sin(2 * math.pi * freq * i / rate) * 0.22 * env
            frames += struct.pack("<h", int(sample * 32767))
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(bytes(frames))


chime(os.path.join(ASSETS, "chime.wav"))
print("icons and chime written")
