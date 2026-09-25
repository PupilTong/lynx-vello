import { __BobcatStartBTS } from "bobcat:bts-runtime";
import { backgroundEntry } from "bobcat-internal:worker";

// The `bobcat:bts` ESM: the BTS bootstrap, preloaded on the group's worker
// runtime. A BTS realm's root module imports it statically after
// `bobcat:worker` and `bobcat:timers`, the way a dedicated worker's root
// module imports that worker's script.
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
// one — a view that named none, or a plain `Worker` that imports this module —
// reads `undefined` and imports nothing.

const entry = backgroundEntry();

__BobcatStartBTS(async () => {
  if (entry !== undefined) await import(entry);
});
