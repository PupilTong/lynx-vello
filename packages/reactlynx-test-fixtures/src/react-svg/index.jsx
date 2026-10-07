import {root, useState} from '@lynx-js/react';
import './index.css';
import shape from './shape.svg';

// Inline markup for `<svg content>`. The `#` of the colours and of
// `url(#fade)`, and the `%` of the gradient offsets, must survive the
// element's trip through a `data:` URL.
const FADE = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 20">'
  + '<defs><linearGradient id="fade" x1="0%" x2="100%">'
  + '<stop offset="0%" stop-color="#ff0000"/><stop offset="100%" stop-color="#0000ff"/>'
  + '</linearGradient></defs>'
  + '<rect width="40" height="20" fill="url(#fade)"/>'
  + '<rect y="15" width="40" height="5" fill="#008000"/>'
  + '</svg>';

// `load` on an `<svg>` reports the element's layout size, here 120 by 60.
function statusClass(side, detail) {
  if (!detail) return `status ${side}`;
  const matched = detail.width === 120 && detail.height === 60;
  return `status ${side} ${matched ? 'matched' : 'mismatched'}`;
}

// A white 240px page in four quadrants:
//
// - Top-left, (0, 0) 120 by 60: `<svg src>` with the bundled `shape.svg` (80 by 40, viewBox 40 by
//   20: blue, a yellow circle of radius 6 at its centre), stretched to the box. Below it, at
//   (0, 60), a 120 by 60 status strip: grey until `load`, then green when the detail is the
//   120 by 60 layout size and red otherwise.
// - Top-right, (120, 0) 120 by 60: `<svg content>` with the inline markup above (a red-to-blue
//   gradient over the top three quarters, a solid green band over the bottom quarter), and its
//   own status strip at (120, 60).
// - Bottom-left, (0, 120) 120 by 120: `<image mode="aspectFit">` of `shape.svg` on a magenta
//   background. The 2:1 image is letterboxed to 120 by 60 at (0, 150); the 30px bands above and
//   below stay magenta.
// - Bottom-right, (120, 120) 120 by 120: a `<view>` whose `background-image` is the bundled
//   `tile.svg` (20 by 20: yellow, a green 10px square at its top-left) under
//   `background-repeat: repeat`, six tiles per axis.
//
// The build inlines both assets as `data:` URLs (`dataUriLimit` in `rsbuild.config.js`), so the
// page needs no file beside the bundle.
function App() {
  const [srcLoad, setSrcLoad] = useState(null);
  const [contentLoad, setContentLoad] = useState(null);
  return (
    <view class='page'>
      <svg id='from-src' class='cell left' src={shape} bindload={(e) => setSrcLoad(e.detail)} />
      <view id='src-status' class={statusClass('left', srcLoad)} />
      <svg
        id='from-content'
        class='cell right'
        content={FADE}
        bindload={(e) => setContentLoad(e.detail)}
      />
      <view id='content-status' class={statusClass('right', contentLoad)} />
      <image id='fit' class='fit' src={shape} mode='aspectFit' />
      <view id='tiles' class='tiles' />
    </view>
  );
}

root.render(<App />);
