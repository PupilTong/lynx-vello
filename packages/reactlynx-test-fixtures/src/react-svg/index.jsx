import {root} from '@lynx-js/react';
import './index.css';
import shape from './shape.svg';

// A white 240px page in five regions:
//
// - Top-left, (0, 0) 120 by 60: an inline `<svg viewBox="0 0 40 20">` sized by CSS, drawing the
//   same picture as `shape.svg` (blue, a yellow circle of radius 6 at its centre) from child
//   elements.
// - Top-right, (120, 0) 120 by 60: an inline `<svg>` whose `<rect>` fills with `url(#fade)`, a
//   `<linearGradient>` in its `<defs>` (red to blue across the top three quarters), and a solid
//   green band over the bottom quarter.
// - Middle, (0, 60) 240 by 60: an inline `<svg>` sized by its own `width` and `height` attributes
//   (no CSS size): an orange circle with a black stroke at (30, 30), and a purple triangle.
// - Bottom-left, (0, 120) 120 by 120: `<image mode="aspectFit">` of `shape.svg` on a magenta
//   background. The 2:1 image is letterboxed to 120 by 60 at (0, 150); the 30px bands above and
//   below stay magenta.
// - Bottom-right, (120, 120) 120 by 120: a `<view>` whose `background-image` is the bundled
//   `tile.svg` (20 by 20: yellow, a green 10px square at its top-left) under
//   `background-repeat: repeat`, six tiles per axis.
//
// The inline `<svg>`s use presentation attributes only (`fill`, `stop-color`, `viewBox`, ...).
// ReactLynx compiles JSX text to a `raw-text` element rather than a text node, so a `<style>`
// child would carry no sheet, and a `style={{fill}}` is dropped by the engine's CSS parser before
// the SVG is serialised.
//
// The build inlines both assets as `data:` URLs (`dataUriLimit` in `rsbuild.config.js`), so the
// page needs no file beside the bundle.
function App() {
  return (
    <view class='page'>
      <svg id='shape' class='cell left' viewBox='0 0 40 20'>
        <rect width='40' height='20' fill='#0000ff' />
        <circle cx='20' cy='10' r='6' fill='#ffff00' />
      </svg>
      <svg id='gradient' class='cell right' viewBox='0 0 40 20'>
        <defs>
          <linearGradient id='fade' x1='0%' x2='100%'>
            <stop offset='0%' stop-color='#ff0000' />
            <stop offset='100%' stop-color='#0000ff' />
          </linearGradient>
        </defs>
        <rect width='40' height='20' fill='url(#fade)' />
        <rect y='15' width='40' height='5' fill='#008000' />
      </svg>
      <svg id='attributes' class='band' width='240' height='60' viewBox='0 0 240 60'>
        <circle cx='30' cy='30' r='20' fill='#ff8800' stroke='#000000' stroke-width='4' />
        <polygon points='80,50 110,10 140,50' fill='#800080' />
      </svg>
      <image id='fit' class='fit' src={shape} mode='aspectFit' />
      <view id='tiles' class='tiles' />
    </view>
  );
}

root.render(<App />);
