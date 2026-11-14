// Main-thread root: loads the Lepus chunk `counter` twice, then shows what it recorded.
(function () {
  globalThis.__chunkTwiceRuns = [];
  var page = __CreatePage('0', 0);
  var pageId = __GetElementUniqueID(page);
  var first = __LoadLepusChunk('counter', {});
  var second = __LoadLepusChunk('counter', {});
  var result = 'chunk-twice ' + first + ',' + second + ' ' + globalThis.__chunkTwiceRuns.join(',');
  var text = __CreateText(pageId);
  __AppendElement(text, __CreateRawText(result));
  __AppendElement(page, text);
  __FlushElementTree();
  console.log(result);
})();
