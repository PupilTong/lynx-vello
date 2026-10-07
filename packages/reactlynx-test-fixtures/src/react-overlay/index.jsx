import {root, useState} from '@lynx-js/react';
import './index.css';

// A red page fills the 240px view; a tap on it sets `visible` on an
// `<overlay>`. The overlay's one child, a 200px panel, sits at the viewport's
// top-left (the UA sheet positions an overlay's first child there) and paints
// above the page: yellow, turning magenta once `showoverlay` has arrived. Its
// green 120px `#close` view sits at the panel's top-left.
//
// - A tap on `#close` clears `visible`. `catchtap` keeps the tap from bubbling
//   to the page, which would show the overlay again. `dismissoverlay` turns the
//   page blue.
// - A tap on the panel outside `#close` bubbles to the page, whose handler
//   leaves the overlay shown.
function App() {
  const [shown, setShown] = useState(false);
  const [seen, setSeen] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  return (
    <view class={dismissed ? 'page dismissed' : 'page'} bindtap={() => setShown(true)}>
      <overlay
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
