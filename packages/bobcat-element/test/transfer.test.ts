// Behavior tests for `bobcat:transfer`: Web IDL's conversion of
// `postMessage`'s second argument, the `DataCloneError` the serializer's
// refusals become, and the detaching after a post. The serializer itself is
// the host boundary's and runs in the crate's own tests.

import { describe, expect, it, rstest } from "@rstest/core";

import * as domException from "../src/dom-exception.ts";
rstest.mockRequire("bobcat:dom-exception", () => domException);

import { cloneError, postTransferring, registerTransferables, toObjectSequence, transferOf }
  from "../src/transfer.ts";

describe("transferOf", () => {
  it("reads both overloads, and an empty list as none", () => {
    const a = {};
    const b = {};
    expect(transferOf(undefined)).toBeUndefined();
    expect(transferOf(null)).toBeUndefined();
    expect(transferOf([])).toBeUndefined();
    expect(transferOf({})).toBeUndefined();
    expect(transferOf({ transfer: [] })).toBeUndefined();
    expect(transferOf([a, b])).toEqual([a, b]);
    expect(transferOf(new Set([a]))).toEqual([a]);
    expect(transferOf({ transfer: [b] })).toEqual([b]);
  });

  it("throws a TypeError for what Web IDL refuses", () => {
    expect(() => transferOf(5)).toThrow(TypeError);
    expect(() => transferOf("ab")).toThrow(TypeError);
    expect(() => transferOf([1])).toThrow(TypeError);
    expect(() => transferOf({ transfer: 7 })).toThrow(TypeError);
    expect(() => toObjectSequence({ length: 1, 0: {} })).toThrow(TypeError);
  });
});

describe("cloneError", () => {
  it("maps the serializer's three refusals and nothing else", () => {
    for (const message of ["uncloneable object", "value is not transferable", "duplicate transferable"]) {
      const mapped = cloneError(new TypeError(message)) as domException.DOMException;
      expect(mapped).toBeInstanceOf(domException.DOMException);
      expect(mapped.name).toBe("DataCloneError");
    }
    const other = new TypeError("not a function");
    expect(cloneError(other)).toBe(other);
  });
});

describe("postTransferring", () => {
  const detached: object[] = [];
  const canvas = {};
  const drawn = {};
  registerTransferables({
    id: (value) => (value === canvas ? 7 : value === drawn ? 0 : undefined),
    detach: (value) => { detached.push(value); },
  });

  it("passes the ids in list order and detaches after a post", () => {
    const post = rstest.fn(() => undefined);
    postTransferring([canvas], post);
    expect(post).toHaveBeenCalledWith([7]);
    expect(detached).toEqual([canvas]);
  });

  it("throws and detaches nothing when the host refuses", () => {
    detached.length = 0;
    expect(() => postTransferring([drawn], () => "InvalidStateError"))
      .toThrow(expect.objectContaining({ name: "InvalidStateError" }));
    expect(() => postTransferring([canvas], () => { throw new TypeError("duplicate transferable"); }))
      .toThrow(expect.objectContaining({ name: "DataCloneError" }));
    expect(detached).toEqual([]);
  });
});
