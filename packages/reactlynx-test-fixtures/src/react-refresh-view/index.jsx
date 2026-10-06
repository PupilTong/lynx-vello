import {root} from '@lynx-js/react';
import './index.css';

// Two `<x-refresh-view>`s side by side, each half the page wide and the
// page's full height, styled by the engine's UA sheet beyond their sizes and
// colours. Each holds a red 50px header (with `position: absolute`, as
// web-core's demo card writes it), a yellow `scroll-view` of green 100px
// items 120px apart, and a red 50px footer. Both open on the yellow content:
// no red row at the top or the bottom. A pull on the left one shows its
// header; the right one says `enable-refresh={false}`, so a pull on it moves
// nothing.
function Column(props) {
  return (
    <x-refresh-view class='refresh-view' enable-refresh={props.enableRefresh}>
      <x-refresh-header class='edge header' />
      <scroll-view class='scroll-view' scroll-y bounces>
        <view class='item' />
        <view class='item' />
        <view class='item' />
      </scroll-view>
      <x-refresh-footer class='edge' />
    </x-refresh-view>
  );
}

function App() {
  return (
    <view class='row'>
      <Column enableRefresh={true} />
      <Column enableRefresh={false} />
    </view>
  );
}

root.render(<App />);
