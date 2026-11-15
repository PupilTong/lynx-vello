import {root, useState} from '@lynx-js/react';
import './index.css';
import clip from './rgb.mp4';

// One muted `<video>` of `rgb.mp4` (64 by 48: red 0.4 s, green 0.4 s, blue
// 0.4 s) over a red `#status` band, on a white 240px page.
//
// - Top, (0, 0) 240 by 180: `#clip`, which opens paused on its first frame and binds `firstframe`,
//   `ended` and `error`. Its box is the clip's ratio, so `object-fit: contain` fills it.
// - Bottom, (0, 180) 240 by 60: `#status`, red until `#clip`'s `ended`, then green.
//
// A tap anywhere invokes `play` on `#clip` through the background thread's selector query; it
// plays once and ends on its last frame.
//
// The background thread logs every event as `video <id> <type> <detail JSON>` and every method's
// answer as `video <id> <method> <code> <data JSON>`. The build inlines the clip as a `data:` URL
// (`dataUriLimit` in `rsbuild.config.js`), so the page needs no file beside the bundle.
function log(...words) {
  console.log(['video', ...words].join(' '));
}

function invoke(id, method) {
  lynx
    .createSelectorQuery()
    .select(`#${id}`)
    .invoke({
      method,
      success: (result) => log(id, method, 0, JSON.stringify(result)),
      fail: (result) => log(id, method, result.code, JSON.stringify(result.data)),
    })
    .exec();
}

function App() {
  const [ended, setEnded] = useState(false);
  const note = (event) => log('clip', event.type, JSON.stringify(event.detail));
  return (
    <view class='page' bindtap={() => invoke('clip', 'play')}>
      <video
        id='clip'
        class='clip'
        src={clip}
        muted
        bindfirstframe={note}
        binderror={note}
        bindended={(event) => {
          note(event);
          setEnded(true);
        }}
      />
      <view id='status' class={`status ${ended ? 'ended' : ''}`} />
    </view>
  );
}

root.render(<App />);
