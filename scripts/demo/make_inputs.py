"""Generate the synthetic inputs used by the FileCraft demo recording.

Creates three files that resemble a typical application package:

* portfolio-photo.png  - a 3000 x 2000 procedural landscape with film grain
* signed-forms-scan.pdf - four 150 dpi grayscale "scans" stored without stream compression
* site-visit.mp4        - a 1080 x 1920 H.264/AAC phone-style clip at a high bitrate

Every pixel, sample and glyph is generated locally, so the files can be shared
with the recording. Requires Python 3.10+, Pillow, NumPy and FFmpeg on PATH.

    python scripts/demo/make_inputs.py <output-directory>
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

SEED = 20261124


def font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    for name in ("arial.ttf", "DejaVuSans.ttf", "Helvetica.ttc"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default(size=size)


def ridge(rng: np.random.Generator, width: int, base: float, roughness: float) -> np.ndarray:
    x = np.linspace(0.0, 1.0, width)
    height = np.full(width, base)
    amplitude = roughness
    for octave in range(1, 7):
        phase = rng.uniform(0, 2 * np.pi)
        height += amplitude * np.sin(2 * np.pi * x * (1.7 ** octave) + phase)
        amplitude *= 0.52
    return height


def photo(path: Path, rng: np.random.Generator) -> None:
    width, height = 3000, 2000
    y = np.linspace(0.0, 1.0, height)[:, None]
    x = np.linspace(0.0, 1.0, width)[None, :]
    top, horizon = np.array([28, 54, 112]), np.array([246, 170, 110])
    blend = np.clip(y / 0.62, 0, 1)[..., None]
    image = top * (1 - blend) + horizon * blend
    image = np.broadcast_to(image, (height, width, 3)).copy()

    distance = np.sqrt(((x - 0.68) * 1.5) ** 2 + (y - 0.46) ** 2)
    glow = np.exp(-(distance / 0.16) ** 2)[..., None]
    image = image * (1 - glow * 0.55) + np.array([255, 226, 170]) * glow * 0.55
    image[distance < 0.035] = [255, 244, 214]

    layers = [
        (0.50, 0.050, [122, 104, 134]),
        (0.58, 0.060, [86, 74, 108]),
        (0.68, 0.070, [52, 50, 78]),
        (0.80, 0.045, [28, 34, 52]),
    ]
    rows = np.arange(height)[:, None]
    for base, roughness, color in layers:
        crest = (ridge(rng, width, base, roughness) * height)[None, :]
        mask = (rows >= crest)[..., None]
        shade = np.clip((rows - crest) / (height * 0.5), 0, 1)[..., None]
        fill = np.array(color) * (1 - shade * 0.35)
        image = np.where(mask, fill, image)

    grain = rng.normal(0.0, 2.6, size=(height, width, 1))
    image = np.clip(image + grain, 0, 255).astype(np.uint8)
    Image.fromarray(image, "RGB").save(path, optimize=False)


def scan_page(number: int, rng: np.random.Generator) -> Image.Image:
    width, height = 1275, 1650
    page = Image.new("L", (width, height), 247)
    draw = ImageDraw.Draw(page)
    title, label, body = font(42), font(24), font(21)

    draw.text((110, 110), "Application form", font=title, fill=30)
    draw.text((110, 170), f"Synthetic sample for FileCraft demo  ·  page {number} of 4", font=body, fill=90)
    draw.line((110, 215, width - 110, 215), fill=60, width=3)

    top = 270
    for index in range(9):
        y = top + index * 112
        draw.text((110, y), f"Field {number}.{index + 1}", font=label, fill=50)
        draw.rectangle((110, y + 36, width - 110, y + 86), outline=80, width=2)
        words = rng.integers(4, 9)
        text = " ".join("".join(chr(rng.integers(97, 123)) for _ in range(rng.integers(3, 9))) for _ in range(words))
        draw.text((126, y + 47), text.capitalize(), font=body, fill=35)

    points = [(760 + i * 9, 1370 + int(28 * np.sin(i / 3.2) + rng.integers(-6, 7))) for i in range(48)]
    draw.line(points, fill=25, width=4, joint="curve")
    draw.line((740, 1430, width - 110, 1430), fill=70, width=2)
    draw.text((740, 1442), "Signature", font=body, fill=80)

    pixels = np.asarray(page, dtype=np.int16)
    speckles = rng.random(pixels.shape) < 0.004
    pixels = np.where(speckles, pixels - rng.integers(30, 90, pixels.shape), pixels)
    return Image.fromarray(np.clip(pixels, 0, 255).astype(np.uint8), "L")


def pdf(path: Path, rng: np.random.Generator) -> None:
    pages = [scan_page(number, rng) for number in range(1, 5)]
    objects: list[bytes] = []

    def add(body: bytes) -> int:
        objects.append(body)
        return len(objects)

    def stream(header: str, data: bytes) -> bytes:
        return f"<< {header} /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream"

    catalog = add(b"")
    tree = add(b"")
    kids = []
    for page in pages:
        image = add(stream(
            f"/Type /XObject /Subtype /Image /Width {page.width} /Height {page.height} "
            "/ColorSpace /DeviceGray /BitsPerComponent 8",
            page.tobytes(),
        ))
        content = add(stream("", f"q 612 0 0 792 0 0 cm /Scan Do Q".encode()))
        kids.append(add(
            f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
            f"/Resources << /XObject << /Scan {image} 0 R >> >> /Contents {content} 0 R >>".encode()
        ))
    objects[catalog - 1] = f"<< /Type /Catalog /Pages {tree} 0 R >>".encode()
    objects[tree - 1] = (
        f"<< /Type /Pages /Count {len(kids)} /Kids [{' '.join(f'{kid} 0 R' for kid in kids)}] >>".encode()
    )

    output = bytearray(b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(output))
        output += f"{number} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(output)
    output += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    output += b"".join(f"{offset:010d} 00000 n \n".encode() for offset in offsets)
    output += f"trailer\n<< /Size {len(objects) + 1} /Root {catalog} 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    path.write_bytes(output)


def video(path: Path, landscape: Path) -> None:
    """Pan a portrait window across the landscape with sensor-like grain.

    Phones often record at a fixed high bitrate, so the clip is encoded as
    constant-bitrate H.264 at 14 Mbit/s with stereo AAC audio.
    """
    seconds = 24
    picture = "crop=1080:1920:'min(1920,80*t)':40,noise=alls=2:allf=t,format=yuv420p"
    tone = (
        f"aevalsrc='0.10*sin(2*PI*220*t)+0.07*sin(2*PI*277.2*t)+0.05*sin(2*PI*329.6*t)'"
        f":s=48000:d={seconds},aformat=channel_layouts=stereo"
    )
    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
        "-loop", "1", "-framerate", "30", "-t", str(seconds), "-i", str(landscape),
        "-f", "lavfi", "-i", tone,
        "-map", "0:v", "-map", "1:a", "-map_metadata", "-1", "-vf", picture,
        "-c:v", "libx264", "-preset", "medium",
        "-b:v", "14M", "-minrate", "14M", "-maxrate", "14M", "-bufsize", "14M", "-x264-params", "nal-hrd=cbr",
        "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart",
        str(path),
    ], check=True)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__.strip().splitlines()[-1].strip(), file=sys.stderr)
        return 2
    target = Path(sys.argv[1])
    target.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(SEED)
    photo(target / "portfolio-photo.png", rng)
    pdf(target / "signed-forms-scan.pdf", rng)
    video(target / "site-visit.mp4", target / "portfolio-photo.png")
    for item in sorted(target.iterdir()):
        print(f"{item.name}\t{item.stat().st_size:,} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
