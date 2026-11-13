// Main-thread root: loads the custom section `answer__main-thread` and calls its value.
(function () {
  var page = __CreatePage('0', 0);
  var pageId = __GetElementUniqueID(page);
  var answer = lynx.loadScript('answer__main-thread', {});
  var result = 'custom-section-bytecode ' + answer(6, 7);
  var text = __CreateText(pageId);
  __AppendElement(text, __CreateRawText(result));
  __AppendElement(page, text);
  __FlushElementTree();
  console.log(result);
})();
