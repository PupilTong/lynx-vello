import {root} from '@lynx-js/react';
import './index.css';

// Four coloured items in an `<x-swiper>` that fills the 240px page. The
// swiper starts on its second item (`current`); a tap anywhere on it turns to
// the fourth without animation, through the background thread's selector
// query, which is the path a card's own `scrollTo` call takes.
const COLORS = ['red', 'green', 'blue', 'yellow'];

function App() {
  const turn = () => {
    lynx.createSelectorQuery()
      .select('#swiper')
      .invoke({method: 'scrollTo', params: {index: 3, smooth: false}})
      .exec();
  };
  return (
    <x-swiper id='swiper' class='swiper' current={1} bindtap={turn}>
      {COLORS.map(color => <x-swiper-item key={color} class={color} />)}
    </x-swiper>
  );
}

root.render(<App />);
