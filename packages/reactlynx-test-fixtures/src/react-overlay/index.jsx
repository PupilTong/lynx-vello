import {root, useState} from '@lynx-js/react';
import './index.css';

// A red page fills the 240px view; a tap on it sets `visible` on an
// `<overlay>`. The overlay's one child, a 200px panel, sits at the viewport's
// top-left (the UA sheet positions an overlay's first child there) and paints
// above the page: yellow, turning magenta once `showoverlay` has arrived,
// which also turns the page orange. Its green 120px `#close` view sits at the
// panel's top-left.
//
// The overlay carries `.host`, a counter card's overlay class: an author
// `display: flex` and a translucent green background on the overlay element
// itself. Neither may show: the hidden overlay stays hidden whatever the
// host's `display`, and the shown one tints nothing, so the page outside the
// panel keeps its exact colour.
//
// - A tap on `#close` clears `visible`. `catchtap` keeps the tap from bubbling
//   to the page, which would show the overlay again. `dismissoverlay` turns the
//   page blue.
// - A tap on the panel outside `#close`, or on the page outside the panel,
//   bubbles to the page, whose handler leaves the overlay shown.
function App() {
  const [shown, setShown] = useState(false);
  const [seen, setSeen] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const page = ['page', seen && 'overlay-seen', dismissed && 'dismissed']
    .filter(Boolean)
    .join(' ');
  return (
    <view class={page} bindtap={() => setShown(true)}>
      <overlay
        class='host'
        visible={shown}
        bindshowoverlay={() => setSeen(true)}
        binddismissoverlay={() => setDismissed(true)}
      >
        <view class={seen ? 'panel seen' : 'panel'}>
          <view id='close' class='close' catchtap={() => setShown(false)} />
        </view>
      </overlay>
    </view>
  );
}

root.render(<App />);
