// Main-thread root: one text node, so the page renders; the background script logs.
(function () {
  var page = __CreatePage('0', 0);
  var pageId = __GetElementUniqueID(page);
  var text = __CreateText(pageId);
  __AppendElement(text, __CreateRawText('background-bytecode-min main'));
  __AppendElement(page, text);
  __FlushElementTree();
  console.log('background-bytecode-min main');
})();
