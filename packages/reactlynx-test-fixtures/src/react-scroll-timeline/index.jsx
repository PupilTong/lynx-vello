import {root} from '@lynx-js/react';
import './index.css';

// A 300px list over 1000px of rows whose first row reveals along the list's
// `scroll()` timeline. At the boot offset the timeline stands at 0, so the row
// shows its `from` keyframe; had the lowering dropped `animation-timeline`,
// the `auto`-length animation would run on the document timeline and never
// show one.
//
// The third row runs on its own `view()` timeline with keyframes on named
// ranges: its `entry` range ends at the boot offset, a quarter through its
// cover range, where its `entry 100%` keyframe sits. Had the range keyframes
// been dropped, nothing would animate its opacity.
const CLASSES = {0: 'row reveal', 2: 'row enter'};

function App() {
  const rows = [];
  for (let index = 0; index < 10; index++) {
    rows.push(<view key={index} class={CLASSES[index] ?? 'row'} />);
  }
  return (
    <view class='page'>
      <view class='list'>{rows}</view>
    </view>
  );
}

root.render(<App />);
