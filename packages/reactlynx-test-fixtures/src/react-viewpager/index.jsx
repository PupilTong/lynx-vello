import {root} from '@lynx-js/react';
import './index.css';

// Four coloured pages in a `<viewpager>` that fills the page. The pager
// starts on its first page; a tap anywhere on it turns to the fourth without
// animation, through the background thread's selector query, which is the
// path a card's own `selectTab` call takes.
const COLORS = ['red', 'green', 'blue', 'yellow'];

function App() {
  const turn = () => {
    lynx.createSelectorQuery()
      .select('#pager')
      .invoke({method: 'selectTab', params: {index: 3, smooth: false}})
      .exec();
  };
  return (
    <viewpager id='pager' bindtap={turn}>
      {COLORS.map(color => <viewpager-item key={color} class={color} />)}
    </viewpager>
  );
}

root.render(<App />);
