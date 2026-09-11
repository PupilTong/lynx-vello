import {lazy, Suspense} from '@lynx-js/react';
import './react-lazy-nested-outer.css';

const Inner = lazy(() => import('./react-lazy-nested-inner.jsx', {with: {mode: 'sync'}}));
export default function Outer() {
  return <view className="nested-outer">
    <Suspense fallback={<view style={{width: '80px', height: '80px', backgroundColor: 'red'}} />}><Inner /></Suspense>
  </view>;
}
