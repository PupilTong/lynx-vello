import { root, useEffect, useRef } from '@lynx-js/react';

function App() {
  const ref = useRef(null);
  useEffect(() => {
    ref.current.fields({id: true, dataset: true, query: true}, (node, status) => {
      if (status.code !== 0 || node.id !== 'target' || node.dataset.count !== 7) {
        throw new Error(`React ref fields failed: ${JSON.stringify({node, status})}`);
      }
      node.query.selectAll('.child').fields({tag: true}, (children, nestedStatus) => {
        if (nestedStatus.code !== 0 || children.length !== 1 || children[0].tag !== 'view') {
          throw new Error('rooted React ref query failed');
        }
        ref.current.setNativeProps({
          width: '200px', height: '200px', 'background-color': 'green', role: 'updated',
        }).exec();
        ref.current.fields({attribute: true}, (updated, updateStatus) => {
          if (updateStatus.code !== 0 || updated.attribute.role !== 'updated') {
            throw new Error('query ran ahead of setNativeProps');
          }
          console.log('react-bts-query verified');
        }).exec();
      }).exec();
    }).exec();
  }, []);
  return <view ref={ref} id="target" data-count={7}
    style={{width: '100px', height: '100px', backgroundColor: 'pink'}}>
    <view className="child" />
  </view>;
}

root.render(<App />);
