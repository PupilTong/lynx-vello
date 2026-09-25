// `bobcat:diagnostics` over a recording stand-in for the two host members
// every realm's core installs (`crates/bobcat-core/src/realm.rs`).
//
// What is pinned here is what the module decides: the text a value prints
// as, what each console method hands the host, and the level a `reportError`
// reports at. Which realm the host names in the event, and that no other
// realm relays it, is the host's; the crate's own tests run that over live
// realms.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";

rstest.mockRequire("bobcat-internal:host", () => ({
  reportScriptError: (level: string, message: string): undefined => {
    host.reported.push([level, message]);
    return undefined;
  },
  logScriptMessage: (level: string, message: string): undefined => {
    host.logged.push([level, message]);
    return undefined;
  },
}));

import { console, printable, reportError } from "../src/diagnostics.ts";

/** Every `(level, message)` pair each member was handed, in order. */
const host = {
  reported: [] as [string, string][],
  logged: [] as [string, string][],
};

beforeEach(() => {
  host.reported.length = 0;
  host.logged.length = 0;
});

describe("printable", () => {
  it("prints an Error as its stack when the stack starts with its summary", () => {
    const error = new Error("failed");
    expect(error.stack).toContain("Error: failed");
    expect(printable(error)).toBe(error.stack);
  });

  it("puts an Error's summary before a stack that lacks it", () => {
    // QuickJS's stack is only the frames, without the `name: message` line.
    const error = new RangeError("out of range");
    error.stack = "    at step (app:///main.js:1:2)";
    expect(printable(error)).toBe("RangeError: out of range\n    at step (app:///main.js:1:2)");
  });

  it("prints an Error with no stack as its summary", () => {
    const error = new TypeError("bad input");
    delete error.stack;
    expect(printable(error)).toBe("TypeError: bad input");
  });

  it("prints a string as it is", () => {
    expect(printable('a "quoted" line')).toBe('a "quoted" line');
  });

  it("prints any other value as its JSON", () => {
    expect(printable({ value: 1, nested: [true, null] })).toBe('{"value":1,"nested":[true,null]}');
    expect(printable([1, "two"])).toBe('[1,"two"]');
    expect(printable(null)).toBe("null");
    expect(printable(-0.5)).toBe("-0.5");
  });

  it("prints a value JSON has no text for as its String form", () => {
    expect(printable(undefined)).toBe("undefined");
    expect(printable(Symbol("mark"))).toBe("Symbol(mark)");
  });

  it("prints a value JSON refuses as its String form", () => {
    const circular: { self?: unknown } = {};
    circular.self = circular;
    expect(printable(circular)).toBe("[object Object]");
    expect(printable(10n)).toBe("10");
  });
});

describe("console", () => {
  it("hands the host each method's name as the level", () => {
    for (const method of ["log", "info", "debug", "warn", "error"]) {
      console[method]?.(method);
    }
    expect(host.logged).toEqual([
      ["log", "log"], ["info", "info"], ["debug", "debug"], ["warn", "warn"], ["error", "error"],
    ]);
    expect(host.reported).toEqual([]);
  });

  it("joins the printable forms of its arguments with spaces", () => {
    console["log"]?.("count", 2, { value: 1 }, undefined);
    console["warn"]?.();
    expect(host.logged).toEqual([["log", 'count 2 {"value":1} undefined'], ["warn", ""]]);
  });
});

describe("reportError", () => {
  it("spells lynx-core's levels as the console methods, and anything else as error", () => {
    reportError("warned", { level: "warning" });
    reportError("fatal", { level: "fatal" });
    reportError("errored", { level: "error" });
    reportError("unknown", { level: "warn" });
    reportError("no level");
    expect(host.reported).toEqual([
      ["warn", "warned"], ["fatal", "fatal"], ["error", "errored"], ["error", "unknown"],
      ["error", "no level"],
    ]);
    expect(host.logged).toEqual([]);
  });

  it("reports an Error with its stack", () => {
    const error = new Error("render failed");
    reportError(error);
    expect(host.reported).toEqual([["error", error.stack]]);
  });
});
