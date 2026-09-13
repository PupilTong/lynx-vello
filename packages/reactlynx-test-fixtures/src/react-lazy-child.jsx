import {useEffect} from '@lynx-js/react';
import './react-lazy-child.css';

export default function Child() {
  useEffect(() => { console.log('react-lazy-ready'); }, []);
  return <view className="lazy-box" />;
}
