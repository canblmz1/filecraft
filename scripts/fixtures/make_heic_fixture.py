"""Generate the synthetic HEIC test fixture.

The file mimics an iPhone photo: HEVC tiles in a 256 px grid, a Display P3
color profile (nclx), an EXIF block and a 90 degree display rotation. Every
pixel is generated here.

    pip install pillow pillow-heif numpy
    python scripts/fixtures/make_heic_fixture.py crates/filecraft-core/tests/fixtures/rotated-p3-grid.heic
"""

import sys
from pathlib import Path

import numpy as np
import pillow_heif
from PIL import Image

WIDTH, HEIGHT = 640, 480
# A Display P3 color inside the sRGB gamut, painted near the top-left corner.
PATCH = (180, 90, 90)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__.strip().splitlines()[-1].strip(), file=sys.stderr)
        return 2
    y, x = np.mgrid[0:HEIGHT, 0:WIDTH]
    pixels = np.stack(
        [x * 255 // WIDTH, y * 255 // HEIGHT, (x + y) * 255 // (WIDTH + HEIGHT)], axis=-1
    ).astype(np.uint8)
    pixels[40:140, 40:200] = PATCH
    image = Image.fromarray(pixels, "RGB")
    exif = image.getexif()
    exif[0x0112] = 6  # display rotated 90 degrees clockwise
    exif[0x010F] = "FileCraft synthetic fixture"
    pillow_heif.options.GRID_TILE_SIZE = 256
    heif = pillow_heif.from_pillow(image)
    target = Path(sys.argv[1])
    target.parent.mkdir(parents=True, exist_ok=True)
    heif.save(
        target,
        quality=80,
        exif=exif.tobytes(),
        color_primaries=12,
        transfer_characteristic=13,
        matrix_coefficients=6,
        full_range_flag=1,
    )
    print(f"{target} {target.stat().st_size} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
