// Background script `/app-service.js`: an object with `init`, which `lynx.requireModule` calls.
(function () {
  function init() {
    var doubled = [1, 2, 3].map(function (n) { return n * 2; }).join(',');
    console.log('background-bytecode-min background ' + doubled);
  }
  return { init: init };
})()
