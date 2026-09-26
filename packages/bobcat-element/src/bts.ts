// Imported for their effects, first: the BTS's global scope and its timers
// come from these two imports of its own. The engine installs neither in any
// worker realm, so `self`, `name`, `postMessage`, `onmessage`, `setTimeout`
// and the rest exist in the BTS realm because this module imports them, and
// they are in place before anything imported after them is evaluated.
import "bobcat:worker";
import "bobcat:timers";
import { __BobcatStartBTS } from "bobcat:bts-runtime";

// The `bobcat:bts` ESM: the BTS bootstrap, preloaded on the group's worker
// runtime. The BTS is the worker whose URL is `bobcat:bts`, and a worker's
// root module is the module at its URL, so this module is the BTS realm's
// root module.
//
// It loads nothing itself. It hands `bobcat:bts-runtime` the function that
// imports the view's BTS entry, and the runtime calls it with the first
// `initialize` message once that message has supplied the page data, the
// screen and the native modules. So the evaluation of this module, and with it
// the BTS's boot, finishes without waiting for anything: the host delivers no
// posted message to a worker whose root module has not finished, and
// `initialize` is one of them.
//
// The entry URL is the one `initialize` carries: the view's
// `background_entry`, already an absolute URL, which the MTS realm posts as
// boot connects the BTS. A view that named none posts `undefined`, and
// nothing is imported; a worker at another URL that imports this module is
// posted no `initialize` at all.

__BobcatStartBTS(async ({ entry }) => {
  if (entry !== undefined) await import(entry);
});
