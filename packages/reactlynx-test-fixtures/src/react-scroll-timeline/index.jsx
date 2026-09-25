import {root} from '@lynx-js/react';
import './index.css';

// A 300px list over 1000px of rows whose first row reveals along the list's
// `scroll()` timeline. At the boot offset the timeline stands at 0, so the row
// shows its `from` keyframe; had the lowering dropped `animation-timeline`,
// the `auto`-length animation would run on the document timeline and never
// show one.
function App() {
  const rows = [];
  for (let index = 0; index < 10; index++) {
    rows.push(<view key={index} class={index === 0 ? 'row reveal' : 'row'} />);
  }
  return (
    <view class='page'>
      <view class='list'>{rows}</view>
    </view>
  );
}

root.render(<App />);
