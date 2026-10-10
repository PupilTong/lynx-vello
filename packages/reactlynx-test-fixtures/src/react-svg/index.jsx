import {root} from '@lynx-js/react';
import './index.css';
import shape from './shape.svg';

// The Lynx `<svg>` component draws one picture from `content` (markup) or `src` (a URL), the way
// web-core's `x-svg` and native do; its children are never read.
//
// A white 240px page in five regions:
//
// - Top-left, (0, 0) 120 by 60: `<svg content={ICON}>` sized by CSS. Its `viewBox="0 0 40 20"`
//   picture is a `<linearGradient>` fill (red to blue) through `url(#fade)` over the top three
//   quarters, a solid green band over the bottom quarter, and a yellow triangle `<path>`.
// - Top-right, (120, 0) 120 by 60: `<svg content={LABEL}>` sized by CSS: a blue rectangle with
//   white `<text>` centred on it by `text-anchor="middle"`.
// - Middle, (0, 60): `<svg src={shape}>` with no CSS size, so it lays out at `shape.svg`'s natural
//   size, 80 by 40 (blue, a yellow circle of radius 6 at the centre of its 40 by 20 `viewBox`);
//   the rest of the band stays white.
// - Bottom-left, (0, 120) 120 by 120: `<image mode="aspectFit">` of `shape.svg` on a magenta
//   background. The 2:1 image is letterboxed to 120 by 60 at (0, 150); the 30px bands above and
//   below stay magenta.
// - Bottom-right, (120, 120) 120 by 120: a `<view>` whose `background-image` is the bundled
//   `tile.svg` (20 by 20: yellow, a green 10px square at its top-left) under
//   `background-repeat: repeat`, six tiles per axis.
//
// The build inlines both assets as `data:` URLs (`dataUriLimit` in `rsbuild.config.js`), so the
// page needs no file beside the bundle.
const ICON = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 20">
  <defs>
    <linearGradient id="fade" x1="0%" x2="100%">
      <stop offset="0%" stop-color="#ff0000"/>
      <stop offset="100%" stop-color="#0000ff"/>
    </linearGradient>
  </defs>
  <rect width="40" height="20" fill="url(#fade)"/>
  <rect y="15" width="40" height="5" fill="#008000"/>
  <path d="M14 13 L20 3 L26 13 Z" fill="#ffff00"/>
</svg>`;

const LABEL = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 60">
  <rect width="120" height="60" fill="#0000ff"/>
  <text x="60" y="38" font-family="sans-serif" font-size="20" fill="#ffffff"
    text-anchor="middle">Lynx</text>
</svg>`;

function App() {
  return (
    <view class='page'>
      <svg id='icon' class='cell left' content={ICON} />
      <svg id='label' class='cell right' content={LABEL} />
      <svg id='shape' class='band' src={shape} />
      <image id='fit' class='fit' src={shape} mode='aspectFit' />
      <view id='tiles' class='tiles' />
    </view>
  );
}

root.render(<App />);
