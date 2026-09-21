import {root, useState} from '@lynx-js/react';

// A vertical list of forty cells, plus one edit the card can make to it. The
// point of the fixture is the *protocol*, not the layout: a ReactLynx `<list>`
// never gets its children from the tree the card renders. The framework
// records the children as index operations, writes them with
// `__SetAttribute(list, 'update-list-info', …)`, and files the
// `componentAtIndex`/`enqueueComponent` pair that actually builds and retires
// them — so what this card proves is that the cells arrive at all, in order,
// and that a second batch both removes and inserts.
const CELLS = 40;

// The edit: three cells leave, one new one is appended. `removeAction` is
// therefore `[1, 3, 5]` — ascending *old* indices, which is what makes the
// `position - i` shift observable — and `insertAction` carries the appended
// cell alone.
const DROPPED = [1, 3, 5];

function keys(edited) {
  const rows = [];
  for (let index = 0; index < CELLS; index++) {
    if (edited && DROPPED.includes(index)) continue;
    rows.push(index);
  }
  if (edited) rows.push(CELLS);
  return rows;
}

function App() {
  // Two ways into the same edit: a tap for a full boot, where the handler runs
  // on the background thread, and `lynx.__initData` for the main-thread render
  // a data update re-runs. The main thread has no state store, so its render
  // reads the second alone.
  const [tapped, setTapped] = useState(false);
  const edited = tapped || Boolean(lynx.__initData?.edited);
  return (
    <view style={{display: 'flex', flexDirection: 'column', width: '300px', height: '600px'}}>
      <text id="header" bindtap={() => setTapped(true)}
        style={{height: '40px', backgroundColor: 'teal'}}>edit</text>
      <list id="cells" style={{width: '300px', height: '560px'}}>
        {keys(edited).map(index => (
          <list-item key={`cell-${index}`} item-key={`cell-${index}`}
            estimated-main-axis-size-px={60}
            style={{height: '60px'}}>
            <text>{`row ${index}`}</text>
          </list-item>
        ))}
      </list>
    </view>
  );
}

root.render(<App />);
