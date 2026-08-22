# `bobcat_core::image` decode fixtures

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
| `exif-rot90.jpg` | 16×8 tagged EXIF orientation 6 | orientation normalisation: every backend must report an **8×16** natural size and matching pixels, even though neither the software codecs nor ImageIO orient by default |
| `apng-fallback.png` | 4×4 APNG: `acTL`, then a **transparent** default image with no preceding `fcTL`, then an opaque red animation frame 0 | frame-0 selection. Because no `fcTL` precedes `IDAT`, the default image is a fallback for non-APNG decoders and is *not* part of the animation — `Reader::next_frame` hands it back first, so a decoder that takes it returns the transparent placeholder instead of the red frame |
| `truncated.png` | a valid PNG cut immediately after the `IDAT` chunk header | truncation rejection. ImageIO decodes this to a full-size, entirely transparent image and reports the source *complete*, where the software decoder errors — `format::is_complete` is what stops that divergence reaching a backend, and this file is its regression test |

## Regenerating

Requires Python with Pillow (`pip install pillow`). Run from the repo root; then
**look at the files** before committing them.

```python
from PIL import Image
import os

out = 'crates/bobcat-core/tests/fixtures'

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

# apng-fallback.png is hand-built rather than encoded by Pillow: the layout it
# pins (acTL present, no fcTL before IDAT) is exactly the one an encoder will
# not produce for you.
import zlib, struct

def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack(
        '>I', zlib.crc32(kind + data) & 0xFFFFFFFF)

def scanlines(rgba, w, h):
    return zlib.compress(b''.join(b'\x00' + bytes(rgba) * w for _ in range(h)))

W = H = 4
apng = b'\x89PNG\r\n\x1a\n'
apng += chunk(b'IHDR', struct.pack('>IIBBBBB', W, H, 8, 6, 0, 0, 0))
apng += chunk(b'acTL', struct.pack('>II', 1, 0))
apng += chunk(b'IDAT', scanlines([0, 0, 0, 0], W, H))
apng += chunk(b'fcTL', struct.pack('>IIIIIHHBB', 0, W, H, 0, 0, 1, 10, 0, 0))
apng += chunk(b'fdAT', struct.pack('>I', 1) + scanlines([255, 0, 0, 255], W, H))
apng += chunk(b'IEND', b'')
open(f'{out}/apng-fallback.png', 'wb').write(apng)

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
