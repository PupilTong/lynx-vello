import {root, useState} from '@lynx-js/react';
import './index.css';

// A red page fills the 240px view, with a 60px `#below` square at its
// bottom-right corner. A tap anywhere on the page sets `visible` on an
// `events-pass-through` `<overlay>`, whose one child is a yellow 120px panel
// at the viewport's top-left, far from `#below`.
//
// `#below` counts the taps it receives and shows, by colour, how many it has
// had and whether `showoverlay` had arrived by then: blue before any, cyan
// after one with the overlay shown, magenta after two with it shown. A tap on
// `#below` bubbles to the page, so the first one also shows the overlay.
//
// With the overlay shown, a second tap on `#below` reaches it only because
// the overlay lets touches outside its content through: a blocking overlay
// would take it, and `#below` would stay cyan.
function App() {
  const [shown, setShown] = useState(false);
  const [seen, setSeen] = useState(false);
  const [taps, setTaps] = useState(0);
  const below = taps === 0 ? 'below' : `below taps-${taps}${seen ? '-seen' : ''}`;
  return (
    <view class='page' bindtap={() => setShown(true)}>
      <view id='below' class={below} bindtap={() => setTaps(taps + 1)} />
      <overlay
        visible={shown}
        events-pass-through={true}
        bindshowoverlay={() => setSeen(true)}
      >
        <view class='panel' />
      </overlay>
    </view>
  );
}

root.render(<App />);
