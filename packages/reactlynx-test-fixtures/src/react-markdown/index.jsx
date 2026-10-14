import {root, useEffect, useState} from '@lynx-js/react';

// The Lynx markdown component under both of its tag names, written the way
// each platform's app writes it: `<x-markdown>`, web-core's tag, with
// structured `markdown-style` (an object, which web-core's property setter
// turns into JSON text), a `content-id` and its three events; and
// `<markdown>`, native's tag, with plain content.
//
// The `<x-markdown>`'s content is web-core's `basic.html` fixture
// (`web-elements/tests/fixtures/x-markdown/basic.html`) followed by a link
// and an image, each a paragraph of its own: a tap anywhere on the link's
// line is a tap on its text. The image is a 48 by 24 green PNG inlined as a
// `data:` URL, which markdown-it's `validateLink` admits for images.
//
// Every event the background thread hears is logged as
// `markdown-card <name> <detail JSON>` and named in `#status`; so is the
// answer of a selector-query `invoke` of `getContent` the mount effect makes.
// The page needs no stylesheet: every style is inline.
const IMAGE =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAADAAAAAYCAIAAAAzn+mLAAAAKUlEQVR42u3OAQkAAAjAsPdPYVRrCA4WYDXdIiQkJCQkJCQkJCQk9Cu07FrQH/bwJEkAAAAASUVORK5CYII=';

const CONTENT = '# Title\n\nthis is **bold** and *italic*.\n\n- item 1\n- item 2\n\n'
  + 'inline code: `code`.\n\n```\nblock code\n```\n\n'
  + '[link](https://lynxjs.org/)\n\n'
  + `![img](${IMAGE})`;

function report(name, detail) {
  console.log(`markdown-card ${name} ${JSON.stringify(detail)}`);
}

function App() {
  const [seen, setSeen] = useState([]);
  const note = (name, detail) => {
    report(name, detail);
    setSeen(previous => [...previous, name]);
  };
  useEffect(() => {
    lynx.createSelectorQuery().select('#md').invoke({
      method: 'getContent',
      params: {start: 0, end: 6},
      success: result => note('getContent', result),
      fail: result => note('getContentFailed', result),
    }).exec();
  }, []);
  return (
    <view style={{display: 'flex', flexDirection: 'column', width: '100%', backgroundColor: '#ffffff', padding: '8px', fontSize: '16px', color: '#111111'}}>
      <x-markdown
        id='md'
        content={CONTENT}
        markdown-style={{link: {color: 'ff0000'}, inlineCode: {color: '0000ff'}}}
        content-id='case-1'
        bindlink={event => note('link', event.detail)}
        bindparseEnd={event => note('parseEnd', event.detail)}
        bindimageTap={event => note('imageTap', event.detail)}
      />
      <markdown id='plain' content='a sibling `<markdown>` with **plain** content' />
      <text id='status' style={{marginTop: '8px', color: '#555555'}}>{`seen: ${seen.join(', ')}`}</text>
    </view>
  );
}

root.render(<App />);
