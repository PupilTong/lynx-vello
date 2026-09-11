import {root, useEffect, useState} from '@lynx-js/react';

globalThis.reloadModuleExecutions = (globalThis.reloadModuleExecutions || 0) + 1;
function App() {
  const [count, setCount] = useState(lynx.__initData.seed);
  const color = ['green', 'blue', 'purple', 'orange'][count];
  useEffect(() => {
    const events = lynx.getJSModule('GlobalEventEmitter');
    const reload = data => lynx.reload(data, function(...args) {
      if (args.length !== 0 || this !== undefined) throw Error('invalid reload callback ABI');
      console.log('reload-callback', count, lynx.__initData.seed, lynx.__initData.keep, globalThis.reloadModuleExecutions);
    });
    events.addListener('reload-from-js', reload);
    console.log('reload-mount', count, lynx.__initData.keep, globalThis.reloadModuleExecutions);
    return () => {
      events.removeListener('reload-from-js', reload);
      console.log('reload-cleanup', count);
    };
  }, []);
  return <view bindtap={() => setCount(value => value + 1)}
    style={{width:'80px', height:'80px', backgroundColor:color}} />;
}
root.render(<App />);
