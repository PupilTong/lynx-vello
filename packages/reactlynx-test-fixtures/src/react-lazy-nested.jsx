import {lazy, root, Suspense} from '@lynx-js/react';

const Outer = lazy(() => import('./react-lazy-nested-outer.jsx'));
root.render(<Suspense fallback={<view style={{width: '100px', height: '100px', backgroundColor: 'red'}} />}><Outer /></Suspense>);
