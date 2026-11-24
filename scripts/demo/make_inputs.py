"""Generate the synthetic inputs used by the FileCraft demo recording.

Creates three files shaped like a real application package from a phone:

* IMG_4821.HEIC         - a 4032 x 3024 iPhone-style HEIC: 512 px HEVC tiles,
                          Display P3 color and EXIF with a made-up GPS position
* signed-forms-scan.pdf - four 300 dpi grayscale pages stored as JPEG, the way
                          office scanners write them
* IMG_4822.MOV          - a 20 s portrait iPhone-style clip: 10-bit HEVC with
                          HLG HDR tags, a rotation flag, AAC audio and a
                          timecode track

Every pixel, sample and glyph is generated locally, so the files can be shared
with the recording. Requires Python 3.10+, Pillow, pillow-heif, NumPy and an
FFmpeg with libx265 on PATH.

    python scripts/demo/make_inputs.py <output-directory>
"""

from __future__ import annotations

import io
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import pillow_heif
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


def landscape(rng: np.random.Generator, width: int, height: int) -> Image.Image:
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
    return Image.fromarray(np.clip(image + grain, 0, 255).astype(np.uint8), "RGB")


def gps(degrees: float) -> tuple[float, float, float]:
    whole = int(degrees)
    minutes = (degrees - whole) * 60
    return (float(whole), float(int(minutes)), round((minutes - int(minutes)) * 60, 2))


def heic(path: Path, picture: Image.Image) -> None:
    exif = Image.Exif()
    exif[0x010F] = "FileCraft"
    exif[0x0110] = "Synthetic phone camera"
    gps_ifd = {1: "N", 2: gps(41.2867), 3: "E", 4: gps(36.33)}
    exif[0x8825] = gps_ifd
    pillow_heif.options.GRID_TILE_SIZE = 512
    pillow_heif.from_pillow(picture).save(
        path,
        quality=88,
        exif=exif.tobytes(),
        color_primaries=12,
        transfer_characteristic=13,
        matrix_coefficients=6,
        full_range_flag=1,
    )


def scan_page(number: int, rng: np.random.Generator) -> Image.Image:
    width, height = 2550, 3300
    page = Image.new("L", (width, height), 247)
    draw = ImageDraw.Draw(page)
    title, label, body = font(84), font(48), font(42)

    draw.text((220, 220), "Application form", font=title, fill=30)
    draw.text((220, 340), f"Synthetic sample for FileCraft demo  ·  page {number} of 4", font=body, fill=90)
    draw.line((220, 430, width - 220, 430), fill=60, width=6)

    top = 540
    for index in range(9):
        y = top + index * 224
        draw.text((220, y), f"Field {number}.{index + 1}", font=label, fill=50)
        draw.rectangle((220, y + 72, width - 220, y + 172), outline=80, width=4)
        words = rng.integers(4, 9)
        text = " ".join("".join(chr(rng.integers(97, 123)) for _ in range(rng.integers(3, 9))) for _ in range(words))
        draw.text((252, y + 94), text.capitalize(), font=body, fill=35)

    points = [(1520 + i * 18, 2740 + int(56 * np.sin(i / 3.2) + rng.integers(-12, 13))) for i in range(48)]
    draw.line(points, fill=25, width=8, joint="curve")
    draw.line((1480, 2860, width - 220, 2860), fill=70, width=4)
    draw.text((1480, 2884), "Signature", font=body, fill=80)

    pixels = np.asarray(page, dtype=np.int16)
    noise = rng.normal(0.0, 3.0, pixels.shape)
    speckles = rng.random(pixels.shape) < 0.002
    pixels = np.where(speckles, pixels - rng.integers(30, 90, pixels.shape), pixels + noise)
    return Image.fromarray(np.clip(pixels, 0, 255).astype(np.uint8), "L")


def pdf(path: Path, rng: np.random.Generator) -> None:
    objects: list[bytes] = []

    def add(body: bytes) -> int:
        objects.append(body)
        return len(objects)

    def stream(header: str, data: bytes) -> bytes:
        return f"<< {header} /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream"

    catalog = add(b"")
    tree = add(b"")
    kids = []
    for number in range(1, 5):
        page = scan_page(number, rng)
        encoded = io.BytesIO()
        page.save(encoded, "JPEG", quality=92)
        image = add(stream(
            f"/Type /XObject /Subtype /Image /Width {page.width} /Height {page.height} "
            "/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
            encoded.getvalue(),
        ))
        content = add(stream("", b"q 612 0 0 792 0 0 cm /Scan Do Q"))
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


def mov(path: Path, picture: Image.Image) -> None:
    """Pan across the landscape like a phone held sideways, then flag it as portrait."""
    seconds = 20
    with tempfile.TemporaryDirectory() as work:
        still = Path(work) / "landscape.png"
        flat = Path(work) / "flat.mov"
        picture.resize((3000, 2250)).save(still)
        tone = (
            f"aevalsrc='0.10*sin(2*PI*220*t)+0.07*sin(2*PI*277.2*t)+0.05*sin(2*PI*329.6*t)'"
            f":s=44100:d={seconds},aformat=channel_layouts=stereo"
        )
        subprocess.run([
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
            "-loop", "1", "-framerate", "30", "-t", str(seconds), "-i", str(still),
            "-f", "lavfi", "-i", tone,
            "-map", "0:v", "-map", "1:a", "-map_metadata", "-1",
            "-vf", "crop=1920:1080:'min(1080,54*t)':600,noise=alls=2:allf=t,format=yuv420p10le",
            "-c:v", "libx265", "-preset", "fast", "-b:v", "18M", "-tag:v", "hvc1",
            "-x265-params", "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc",
            "-color_primaries", "bt2020", "-color_trc", "arib-std-b67", "-colorspace", "bt2020nc",
            "-c:a", "aac", "-b:a", "192k", "-timecode", "14:02:10:00", "-shortest", "-f", "mov",
            str(flat),
        ], check=True)
        subprocess.run([
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-display_rotation", "90",
            "-i", str(flat), "-map", "0", "-c", "copy", "-f", "mov", str(path),
        ], check=True)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__.strip().splitlines()[-1].strip(), file=sys.stderr)
        return 2
    target = Path(sys.argv[1])
    target.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(SEED)
    picture = landscape(rng, 4032, 3024)
    heic(target / "IMG_4821.HEIC", picture)
    pdf(target / "signed-forms-scan.pdf", rng)
    mov(target / "IMG_4822.MOV", picture)
    for item in sorted(target.iterdir()):
        print(f"{item.name}\t{item.stat().st_size:,} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
