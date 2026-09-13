import {root, useEffect, useGlobalProps, useGlobalPropsChanged, useState} from '@lynx-js/react';

const moduleSeed = lynx.__globalProps.seed;
function App() {
  const props = useGlobalProps();
  const [initialSeed] = useState(props.seed);
  const [clicked, setClicked] = useState(false);
  useGlobalPropsChanged(data => console.log('props-change', data.seed));
  useEffect(() => {
    console.log('props-render', props.color, props.seed, props.keep, initialSeed, moduleSeed);
  }, [props.color, props.seed, props.keep]);
  return <view bindtap={() => setClicked(true)}
    style={{width:'80px',height:'80px',backgroundColor:clicked ? 'purple' : props.color}} />;
}
root.render(<App />);
