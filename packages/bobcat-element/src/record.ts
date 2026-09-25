// The one reader of the records the host writes, preloaded as the
// `bobcat:record` ESM on both runtimes: the Element PAPI's attribute and
// computed-style answers in an MTS realm, and the native module table a
// worker realm reads from `bobcat-internal:native-modules`.

/**
 * Reads a record the native side wrote: a flat sequence of
 * `<utf16Length>:<text>` fields, the format element-papi's `styleField`
 * writes in the other direction, so a field may contain any character
 * including the delimiter. `String.prototype.slice` counts the units the
 * writer counted, so each field costs one slice and no scan.
 *
 * Nothing here validates the payload. The writer is Bobcat, not a card: a
 * malformed record would be an engine bug, and reporting it as a JavaScript
 * error would only move it further from where it happened.
 */
export function splitRecord(record: string): string[] {
  const fields: string[] = [];
  let rest = record;
  while (rest !== "") {
    const separator = rest.indexOf(":");
    const units = Number(rest.slice(0, separator));
    const body = rest.slice(separator + 1);
    fields.push(body.slice(0, units));
    rest = body.slice(units);
  }
  return fields;
}
