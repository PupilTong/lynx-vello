# `crates/image` decode fixtures

Four tiny files, 1.4 KB total. They exist because this crate has **no JPEG or
WebP encoder** — it decodes three formats and encodes none — so unlike the PNG
cases, which `tests/decode.rs` builds in-process with the `png` crate, these
cannot be synthesised at test time.

That is a feature rather than a workaround. A round-trip through our own encoder
would only prove the decoder agrees with a sibling we wrote; these are frozen
third-party ground truth, and the real assertion is that **every backend agrees
on them** — which is exactly where the platform decoders diverge.

| file | content | what it pins |
|---|---|---|
| `checker-16.jpg` | 16×16, four opaque quadrants (red / green / blue / white) | cross-backend decode agreement; JPEG is lossy, so comparisons carry a per-channel tolerance |
| `checker-16.webp` | the same image, lossless, with the fourth quadrant fully transparent | cross-backend agreement *and* alpha handling, where the backends genuinely disagree (straight vs. premultiplied) |
| `exif-rot90.jpg` | 16×8 tagged EXIF orientation 6 | orientation normalisation: every backend must report an **8×16** natural size and matching pixels, even though `image`, ImageIO and `AImageDecoder` orient differently by default |
| `truncated.png` | a valid PNG cut immediately after the `IDAT` chunk header | truncation rejection. ImageIO decodes this to a full-size, entirely transparent image and reports the source *complete*, where the software decoder errors — `format::is_complete` is what stops that divergence reaching a backend, and this file is its regression test |

## Regenerating

Requires Python with Pillow (`pip install pillow`). Run from the repo root; then
**look at the files** before committing them.

```python
from PIL import Image
import os

out = 'crates/image/tests/fixtures'

def checker(size, alpha_quadrant):
    img = Image.new('RGBA', (size, size))
    half = size // 2
    quads = [(0, 0, (255, 0, 0, 255)), (half, 0, (0, 255, 0, 255)),
             (0, half, (0, 0, 255, 255)),
             (half, half, (0, 0, 0, 0) if alpha_quadrant else (255, 255, 255, 255))]
    for ox, oy, color in quads:
        for y in range(oy, oy + half):
            for x in range(ox, ox + half):
                img.putpixel((x, y), color)
    return img

checker(16, False).convert('RGB').save(
    f'{out}/checker-16.jpg', 'JPEG', quality=100, subsampling=0)
checker(16, True).save(f'{out}/checker-16.webp', 'WEBP', lossless=True, exact=True)

wide = Image.new('RGB', (16, 8), (200, 30, 30))
for x in range(8):
    for y in range(8):
        wide.putpixel((x, y), (30, 30, 200))
exif = wide.getexif()
exif[0x0112] = 6
wide.save(f'{out}/exif-rot90.jpg', 'JPEG', quality=100, subsampling=0, exif=exif)

full = Image.new('RGBA', (16, 16), (10, 200, 90, 255))
full.save(f'{out}/full-tmp.png', 'PNG')
data = open(f'{out}/full-tmp.png', 'rb').read()
idat = data.find(b'IDAT')
open(f'{out}/truncated.png', 'wb').write(data[:idat + 8])
os.remove(f'{out}/full-tmp.png')
```

`quality=100, subsampling=0` is deliberate: chroma subsampling would smear the
quadrant boundaries badly enough that a cross-backend tolerance could no longer
distinguish "these decoders agree" from "both are equally blurry".
