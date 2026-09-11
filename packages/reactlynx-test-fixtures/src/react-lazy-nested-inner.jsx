import {useEffect, useState} from '@lynx-js/react';

export default function Inner() {
  const [color, setColor] = useState('blue');
  useEffect(() => {
    Promise.all([
      import('./react-lazy-nested-value.js'),
      import('./react-lazy-nested-value.js'),
    ]).then(([first, second]) => {
      if (first !== second || first.executions !== 1) throw Error('nested JS module ran twice');
      setColor(first.color);
      console.log('react-lazy-nested-ready');
    });
  }, []);
  return <view style={{width: '80px', height: '80px', backgroundColor: color}} />;
}
