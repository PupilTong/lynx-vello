import {root} from '@lynx-js/react';
import './index.css';

// A `<scroll-coordinator>` 300 by 400 at the page origin, styled by the
// engine's UA sheet beyond the sizes and colours here: a translucent blue
// toolbar 60 tall pinned over a red header 200 tall, and a slot under the
// header holding a vertical `<scroll-view>` of eight 100px items, green and
// yellow in turn. The slot starts at 200 and is 400 - 60 tall, so the fold's
// range is 200 - 60 = 140: a forward drag in the scroll-view folds the header
// first, until the toolbar covers the header's bottom band and the first item
// sits directly under the toolbar, and scrolls the items after that.
const ITEMS = ['green', 'yellow', 'green', 'yellow', 'green', 'yellow', 'green', 'yellow'];

function App() {
  return (
    <scroll-coordinator class='coordinator'>
      <scroll-coordinator-toolbar class='toolbar' />
      <scroll-coordinator-header class='header' />
      <scroll-coordinator-slot>
        <scroll-view scroll-y class='content'>
          {ITEMS.map((color, index) => <view key={index} class={`item ${color}`} />)}
        </scroll-view>
      </scroll-coordinator-slot>
    </scroll-coordinator>
  );
}

root.render(<App />);
