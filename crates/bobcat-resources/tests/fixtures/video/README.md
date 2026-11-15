# Video fixtures

`rgb.mp4`: 64 × 48, ten frames a second, red (`#ff0000`) for 0.4 s, green
(`#00ff00`) for 0.4 s and blue (`#0000ff`) for 0.4 s, 1.2 s in all; H.264
constrained baseline in MP4, no audio track, 1708 bytes. Its colour is
tagged BT.709 throughout (matrix, primaries, transfer, limited range), so
AVFoundation and Chrome turn its YUV back into the same RGB; untagged, Chrome
read the BT.601 matrix FFmpeg converts with as BT.709 and drew red as about
`[248, 36, 0]`. The macOS player's tests (`src/media/avfoundation_tests.rs`,
`src/media/tests.rs`), the `react-video` card
(`packages/reactlynx-test-fixtures/src/react-video/rgb.mp4`, a copy) and
`crates/bobcat-wasm/scripts/render-check.ts` play it. Made with FFmpeg 8.1.2
and libx264:

```sh
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i color=c=0xff0000:s=64x48:r=10:d=0.4 \
  -f lavfi -i color=c=0x00ff00:s=64x48:r=10:d=0.4 \
  -f lavfi -i color=c=0x0000ff:s=64x48:r=10:d=0.4 \
  -filter_complex "[0][1][2]concat=n=3:v=1:a=0,scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv" \
  -c:v libx264 -profile:v baseline -preset veryslow -g 4 \
  -movflags +faststart -map_metadata -1 -fflags +bitexact -flags:v +bitexact \
  rgb.mp4
```

A colour comes back from H.264's 4:2:0 YUV within a few steps of the one
written, so the tests compare colours with a tolerance.
