import {root, runOnMainThread, useEffect, useMainThreadRef} from '@lynx-js/react';
import './index.css';

// A `<canvas width='160' height='60'>` at the top-left of a white 240px page,
// sized by its bitmap. Its main-thread ref transfers control to an
// `OffscreenCanvas`, which the main thread posts to the background thread
// through `lynx.getJSContext()`; the background thread draws on it.
//
// The first draw, four 40px columns:
// - (0, 0) `#a`: red.
// - (40, 0): yellow, then cyan under `globalCompositeOperation = 'multiply'`: green.
// - (80, 0): magenta, then `clearRect(90, 10, 20, 20)`: a transparent hole.
// - (120, 0): black "Hi" in 40px sans-serif on transparent.
// It then reads the canvas back with `getImageData` and logs
// `canvas read <JSON>`: the pixels at (20, 30), (60, 30), (85, 5), (100, 20),
// and how many pixels of the text column are not transparent.
//
// Each tap on the page steps on: the first paints the red column blue from a
// `setTimeout`, the second paints it yellow from a `lynx.requestAnimationFrame`,
// the third resizes the canvas to 80 by 100 and fills it cyan. Each step logs
// `canvas step <n>` once drawn.
function log(...words) {
  console.log(['canvas', ...words].join(' '));
}

let ctx;
let canvas;
let step = 0;

function pixel(image, x, y) {
  const at = (y * image.width + x) * 4;
  return Array.from(image.data.subarray(at, at + 4));
}

function draw(event) {
  canvas = event.data.canvas;
  ctx = canvas.getContext('2d');
  ctx.fillStyle = 'red';
  ctx.fillRect(0, 0, 40, 60);
  ctx.fillStyle = 'yellow';
  ctx.fillRect(40, 0, 40, 60);
  ctx.globalCompositeOperation = 'multiply';
  ctx.fillStyle = 'cyan';
  ctx.fillRect(40, 0, 40, 60);
  ctx.globalCompositeOperation = 'source-over';
  ctx.fillStyle = 'magenta';
  ctx.fillRect(80, 0, 40, 60);
  ctx.clearRect(90, 10, 20, 20);
  ctx.fillStyle = 'black';
  ctx.font = '40px sans-serif';
  ctx.fillText('Hi', 124, 45);
  const image = ctx.getImageData(0, 0, 160, 60);
  let text = 0;
  for (let y = 0; y < 60; y += 1) {
    for (let x = 120; x < 160; x += 1) {
      if (image.data[(y * 160 + x) * 4 + 3] !== 0) text += 1;
    }
  }
  log('read', JSON.stringify({
    canvas: canvas instanceof OffscreenCanvas,
    size: [canvas.width, canvas.height],
    a: pixel(image, 20, 30),
    b: pixel(image, 60, 30),
    c: pixel(image, 85, 5),
    hole: pixel(image, 100, 20),
    text,
  }));
}

function next() {
  if (!ctx) return;
  step += 1;
  const done = step;
  if (done === 1) {
    setTimeout(() => {
      ctx.fillStyle = 'blue';
      ctx.fillRect(0, 0, 40, 60);
      log('step', done);
    }, 0);
  } else if (done === 2) {
    lynx.requestAnimationFrame(() => {
      ctx.fillStyle = 'yellow';
      ctx.fillRect(0, 0, 40, 60);
      log('step', done);
    });
  } else if (done === 3) {
    canvas.width = 80;
    canvas.height = 100;
    ctx.fillStyle = 'cyan';
    ctx.fillRect(0, 0, 80, 100);
    log('step', done);
  }
}

function App() {
  const canvasRef = useMainThreadRef(null);
  function transfer() {
    'main thread';
    canvasRef.current.invoke('transferControlToOffscreen').then((offscreen) => {
      lynx.getJSContext().postMessage({canvas: offscreen}, [offscreen]);
    });
  }
  useEffect(() => {
    lynx.getCoreContext().addEventListener('message', draw);
    runOnMainThread(transfer)();
  }, []);
  return (
    <view class='page' bindtap={next}>
      <canvas class='canvas' width='160' height='60' main-thread:ref={canvasRef} />
    </view>
  );
}

root.render(<App />);
