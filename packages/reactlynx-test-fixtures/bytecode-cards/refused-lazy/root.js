// Main-thread root: fetches the lazy bundle `lazy-bundle/refused.lynx.bundle` beside the page
// and shows the fetch's outcome; on success it also runs the bundle's `main-thread` section.
(function () {
  var card = __Card__.split('?')[0].split('#')[0];
  var url = card.slice(0, card.lastIndexOf('/') + 1) + 'lazy-bundle/refused.lynx.bundle';
  var page = __CreatePage('0', 0);
  var pageId = __GetElementUniqueID(page);
  var text = __CreateText(pageId);
  __AppendElement(text, __CreateRawText('refused-lazy pending'));
  __AppendElement(page, text);
  __FlushElementTree();
  lynx.fetchBundle(url, {}).then(function (info) {
    var result;
    if (info.code === 0) {
      try {
        result = 'refused-lazy loaded ' + lynx.loadScript('main-thread', { bundleName: url })();
      } catch (error) {
        result = 'refused-lazy load-failed ' + (error && error.name);
      }
    } else {
      result = 'refused-lazy code:' + info.code;
      console.log('refused-lazy error_msg ' + info.error_msg);
    }
    __ReplaceElements(text, [__CreateRawText(result)], __GetChildren(text));
    __FlushElementTree();
    console.log(result);
  });
})();
