import {lazy, root, Suspense} from '@lynx-js/react';

const Child = lazy(() => import('./react-lazy-child.jsx'));

root.render(<Suspense fallback={<view style={{width: 80, height: 80, backgroundColor: 'red'}} />}><Child /></Suspense>);
