import { __BobcatStartBTS } from "bobcat:bts-runtime";
import { backgroundEntry } from "bobcat-internal:worker";

// The `bobcat:bts` ESM: the BTS bootstrap, preloaded on the group's worker
// runtime. The BTS is the worker whose URL is `bobcat:bts`, so its root module
// imports this module after `bobcat:worker` and `bobcat:timers`, with the
// `await import(<URL>)` every worker's root module imports its script with.
//
// It loads nothing itself. It hands `bobcat:bts-runtime` the function that
// imports the view's BTS entry, and the runtime calls it once the first
// `initialize` message has supplied the page data. So the evaluation of this
// module, and with it the root module's, finishes without waiting for
// anything: the host delivers no posted message to a worker whose root module
// has not finished, and `initialize` is one of them.
//
// The entry URL is read from the host as this module is evaluated. It is the
// view's `background_entry`, already an absolute URL; a realm started without
// one — a view that named none, or a worker at another URL that imports this
// module — reads `undefined` and imports nothing.

const entry = backgroundEntry();

__BobcatStartBTS(async () => {
  if (entry !== undefined) await import(entry);
});
