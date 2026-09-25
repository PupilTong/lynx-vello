// The `bobcat:diagnostics` ESM: one realm's `console` and `reportError`, the
// same in every realm kind.
//
// Both are written over the two members every realm's core carries under
// `bobcat-internal:host`, `logScriptMessage` and `reportScriptError`. Each
// hands the host one level and one message, and the host sends them to the
// embedder as an engine event named by the realm that made the call. Nothing
// is relayed through another realm: the BTS's `console` and `lynx.reportError`
// reach the embedder without a message to the main thread.
//
// Where the bindings appear is each realm's own business. MTS exports both
// from `bobcat:runtime` as `console` and `_ReportError`, and `lynx.reportError`
// is the latter; `bobcat:bts-runtime` exports this `console` and makes
// `lynx.reportError` this `reportError`; `bobcat:worker` installs this
// `console` on a worker realm's global.
//
// This is the MVP console surface: no formatting directives, no grouping, no
// inspection and no native `alog`.

import { logScriptMessage, reportScriptError } from "bobcat-internal:host";

/**
 * One value as the text a diagnostic carries: an `Error`'s stack, starting
 * with its `String` form when the stack does not already; a string as it is;
 * anything else as JSON where it has one, or its `String` form otherwise.
 */
export function printable(value: unknown): string {
  if (value instanceof Error) {
    const summary = String(value);
    return value.stack?.includes(summary) ? value.stack
      : value.stack ? `${summary}\n${value.stack}` : summary;
  }
  if (typeof value === "string") return value;
  try { return JSON.stringify(value) ?? String(value); }
  catch { return String(value); }
}

/**
 * The five console methods. Each joins its arguments' printable forms with
 * spaces and hands them to the host under its own name as the level.
 */
export const console = Object.fromEntries(
  ["log", "info", "debug", "warn", "error"].map(level => [level,
    (...args: unknown[]) => logScriptMessage(level, args.map(printable).join(" ")),
  ]),
);

/**
 * `lynx.reportError(error, {level})`: reports `error` to the embedder as a
 * diagnostic at the level lynx-core accepts it at, spelled as the console
 * method would spell it — `'warning'` is `"warn"`, `'fatal'` is `"fatal"`,
 * and `'error'`, a level lynx-core does not know and no level at all are
 * `"error"`.
 *
 * A diagnostic and nothing more: it throws nothing into its caller, reaches
 * no `error` event and ends nothing, `"fatal"` included.
 */
export function reportError(error?: unknown, options?: {level?: string}): void {
  const level = options?.level;
  reportScriptError(
    level === "warning" ? "warn" : level === "fatal" ? "fatal" : "error",
    printable(error),
  );
}
