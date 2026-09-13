import {lazy, root, Suspense, useEffect, useState} from '@lynx-js/react';

const Child = lazy(() => import('./react-lazy-child.jsx', {with: {mode: 'sync'}}));

function App() {
  const [shown, setShown] = useState(lynx.__initData?.showInitial !== false);
  useEffect(() => {
    const events = lynx.getJSModule('GlobalEventEmitter');
    const show = () => setShown(true);
    events.addListener('show-lazy', show);
    console.log('react-lazy-sync-listening');
    return () => events.removeListener('show-lazy', show);
  }, []);
  return <Suspense fallback={<view style={{width: '80px', height: '80px', backgroundColor: 'red'}} />}>
    {shown ? <Child /> : <view style={{width: '80px', height: '80px', backgroundColor: 'green'}} />}
  </Suspense>;
}

root.render(<App />);
