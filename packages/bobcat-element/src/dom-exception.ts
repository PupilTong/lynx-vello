// The `bobcat:dom-exception` ESM: WebIDL's `DOMException`, the exception a
// web API throws by name (`IndexSizeError`, `InvalidStateError`, …). It has
// no host half. `bobcat:canvas` throws it and installs it as a global
// beside the canvas interfaces.
//
// Its prototype chain reaches `Error.prototype`, as WebIDL's does, so
// `instanceof Error` holds and `bobcat:diagnostics` prints it as an error.
// `name` is an own property of each exception rather than a prototype
// accessor.

/** The legacy `code` of each name that has one (WebIDL's error names table). */
const LEGACY_CODES: Record<string, number> = {
  IndexSizeError: 1,
  HierarchyRequestError: 3,
  WrongDocumentError: 4,
  InvalidCharacterError: 5,
  NoModificationAllowedError: 7,
  NotFoundError: 8,
  NotSupportedError: 9,
  InvalidStateError: 11,
  SyntaxError: 12,
  InvalidModificationError: 13,
  NamespaceError: 14,
  InvalidAccessError: 15,
  TypeMismatchError: 17,
  SecurityError: 18,
  NetworkError: 19,
  AbortError: 20,
  URLMismatchError: 21,
  QuotaExceededError: 22,
  TimeoutError: 23,
  InvalidNodeTypeError: 24,
  DataCloneError: 25,
};

/** The legacy constant each code is also named by, in code order. */
const LEGACY_CONSTANTS: [string, number][] = [
  ["INDEX_SIZE_ERR", 1],
  ["DOMSTRING_SIZE_ERR", 2],
  ["HIERARCHY_REQUEST_ERR", 3],
  ["WRONG_DOCUMENT_ERR", 4],
  ["INVALID_CHARACTER_ERR", 5],
  ["NO_DATA_ALLOWED_ERR", 6],
  ["NO_MODIFICATION_ALLOWED_ERR", 7],
  ["NOT_FOUND_ERR", 8],
  ["NOT_SUPPORTED_ERR", 9],
  ["INUSE_ATTRIBUTE_ERR", 10],
  ["INVALID_STATE_ERR", 11],
  ["SYNTAX_ERR", 12],
  ["INVALID_MODIFICATION_ERR", 13],
  ["NAMESPACE_ERR", 14],
  ["INVALID_ACCESS_ERR", 15],
  ["VALIDATION_ERR", 16],
  ["TYPE_MISMATCH_ERR", 17],
  ["SECURITY_ERR", 18],
  ["NETWORK_ERR", 19],
  ["ABORT_ERR", 20],
  ["URL_MISMATCH_ERR", 21],
  ["QUOTA_EXCEEDED_ERR", 22],
  ["TIMEOUT_ERR", 23],
  ["INVALID_NODE_TYPE_ERR", 24],
  ["DATA_CLONE_ERR", 25],
];

/** WebIDL's `DOMException(optional message = "", optional name = "Error")`. */
export class DOMException extends Error {
  /** The legacy code `name` has, or 0. */
  declare readonly code: number;

  constructor(message: unknown = "", name: unknown = "Error") {
    super(`${message}`);
    Object.defineProperty(this, "name", {
      configurable: true,
      enumerable: false,
      writable: false,
      value: `${name}`,
    });
  }
}

Object.defineProperty(DOMException.prototype, "code", {
  configurable: true,
  enumerable: true,
  get(this: DOMException): number {
    return LEGACY_CODES[this.name] ?? 0;
  },
});

for (const [constant, code] of LEGACY_CONSTANTS) {
  const descriptor = { value: code, enumerable: true, writable: false, configurable: false };
  Object.defineProperty(DOMException, constant, descriptor);
  Object.defineProperty(DOMException.prototype, constant, descriptor);
}
