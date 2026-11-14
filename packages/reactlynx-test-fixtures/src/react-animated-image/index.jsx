import {root, useState} from '@lynx-js/react';
import './index.css';
import gif from './rgb-twice.gif';

// Two `<image>`s of one animated GIF (`rgb-twice.gif`: red 100 ms, green
// 200 ms, blue over the left half 300 ms, and a NETSCAPE count that plays it
// twice) over a red `#status` band, on a white 240px page.
//
// - Top-left, (0, 0) 120 by 120: `#playing`, `loop-count={3}`, which plays three times whatever
//   the file says. It binds `startplay`, `load`, `currentloopcomplete` and `finalloopcomplete`.
// - Top-right, (120, 0) 120 by 120: `#paused`, `autoplay={false}`, which shows the first frame and
//   does not play. It binds the same four events.
// - Bottom half, (0, 120) 240 by 120: `#status`, red until `#playing`'s first `finalloopcomplete`,
//   then green, then blue after its second.
//
// A tap anywhere invokes `startAnimate` on `#playing` and `stopAnimation` on `#paused` through the
// background thread's selector query. `#playing` plays its three loops again; `#paused` was not
// playing and answers the same code 0.
//
// The background thread logs every event as `animated-image <id> <type> <detail JSON>` and every
// method's answer as `animated-image <id> <method> <code> <data>`. The build inlines the GIF as a
// `data:` URL (`dataUriLimit` in `rsbuild.config.js`), so the page needs no file beside the bundle.
function log(...words) {
  console.log(['animated-image', ...words].join(' '));
}

function invoke(id, method) {
  lynx
    .createSelectorQuery()
    .select(`#${id}`)
    .invoke({
      method,
      success: (result) => log(id, method, 0, result),
      fail: (result) => log(id, method, result.code, result.data),
    })
    .exec();
}

function Picture({id, onFinal, ...attributes}) {
  const note = (event) => log(id, event.type, JSON.stringify(event.detail));
  return (
    <image
      id={id}
      class={`picture ${id}`}
      src={gif}
      {...attributes}
      bindstartplay={note}
      bindload={note}
      bindcurrentloopcomplete={note}
      bindfinalloopcomplete={(event) => {
        note(event);
        onFinal?.();
      }}
    />
  );
}

function App() {
  const [finals, setFinals] = useState(0);
  const status = ['', 'once', 'twice'][Math.min(finals, 2)];
  return (
    <view
      class='page'
      bindtap={() => {
        invoke('playing', 'startAnimate');
        invoke('paused', 'stopAnimation');
      }}
    >
      <Picture id='playing' loop-count={3} onFinal={() => setFinals((count) => count + 1)} />
      <Picture id='paused' autoplay={false} />
      <view id='status' class={`status ${status}`} />
    </view>
  );
}

root.render(<App />);
