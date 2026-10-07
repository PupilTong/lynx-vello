import {root, useState} from '@lynx-js/react';
import './index.css';

// A red `#open` view fills the page; a tap on it opens the `<dialog>` with
// `showModal()` through the background thread's selector query. The dialog,
// centred by the UA sheet's shrink-to-fit sizing, holds a `<text>` and a green
// 120px `#close` view over the page's centre; its `::backdrop` is yellow and
// covers the rest of the page.
//
// - A tap on `#close` calls `close()`. `catchtap` keeps the tap from reaching
//   the dialog, so only `close` fires, and the page turns blue.
// - A tap on the backdrop targets the dialog, whose `catchtap` calls
//   `requestClose()`: `cancel` then `close` fire, and the page turns magenta.
//   `catchtap` keeps the tap from bubbling to `#open`, the dialog's parent,
//   which would open the dialog again.
//
// `#status` names the same state in text.
function invoke(method) {
  lynx.createSelectorQuery().select('#d').invoke({method}).exec();
}

function App() {
  const [cancelled, setCancelled] = useState(false);
  const [closed, setClosed] = useState(false);
  const status = closed ? (cancelled ? 'cancelled' : 'closed') : 'idle';
  return (
    <view id='open' class={`page ${status}`} bindtap={() => invoke('showModal')}>
      <text id='status'>{status}</text>
      <dialog
        id='d'
        class='dialog'
        catchtap={() => invoke('requestClose')}
        bindcancel={() => setCancelled(true)}
        bindclose={() => setClosed(true)}
      >
        <text class='title'>Dialog</text>
        <view id='close' class='close' catchtap={() => invoke('close')} />
      </dialog>
    </view>
  );
}

root.render(<App />);
