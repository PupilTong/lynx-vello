// Named capture groups and named backreferences.
var m = /(?<year>\d{4})-(?<month>\d{2})/.exec('on 2023-07');
var swapped = '2023-07'.replace(/(?<y>\d+)-(?<m>\d+)/, '$<m>/$<y>');
var repeated = /(?<ch>.)\k<ch>/.test('xyyz');
globalThis.__result = JSON.stringify([m.groups.year, m.groups.month, swapped, repeated, Object.keys(m.groups)]);
