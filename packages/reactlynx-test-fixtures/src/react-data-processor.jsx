import {root, useEffect, useInitData, useState} from '@lynx-js/react';

globalThis.processorModuleExecutions = (globalThis.processorModuleExecutions || 0) + 1;
lynx.registerDataProcessors({
  dataProcessors: {
    named(data) {
      const result = {};
      if ('rawColor' in data) result.color = {first:'green', second:'blue', third:'purple'}[data.rawColor];
      if ('rawSeed' in data) result.seed = data.rawSeed + 10;
      if ('rawKeep' in data) result.keep = `${data.rawKeep}-named`;
      console.log('named-processor-run', result.color ?? 'absent');
      return result;
    },
  },
  defaultDataProcessor(data) {
    const result = {};
    // A processor returns the data consumed by the runtime in this call.
    if ('rawColor' in data) result.color = {first:'green', second:'blue', third:'purple'}[data.rawColor];
    if ('rawSeed' in data) result.seed = data.rawSeed + 1;
    if ('rawKeep' in data) result.keep = `${data.rawKeep}-processed`;
    console.log('processor-run', result.color ?? 'absent');
    return result;
  },
});

function App() {
  const data = useInitData();
  const [initialSeed] = useState(data.seed);
  const [clicked, setClicked] = useState(false);
  useEffect(() => {
    if ('rawColor' in data || 'rawSeed' in data || 'rawKeep' in data) throw Error('raw data reached BTS');
    console.log('processed-data', data.color, data.seed ?? 'absent', data.keep ?? 'absent', initialSeed, globalThis.processorModuleExecutions);
  }, [data]);
  return <view bindtap={() => setClicked(true)}
    style={{width:'80px', height:'80px', backgroundColor:clicked ? 'orange' : data.color}} />;
}
root.render(<App />);
