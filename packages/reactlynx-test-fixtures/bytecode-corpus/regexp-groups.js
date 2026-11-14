// Capture groups, non-capturing groups, backreferences and exec results.
var m = /(\d+)-(?:x|y)-(\w)\1?/.exec('ab 12-x-q12 cd');
var date = '2024-03-15'.match(/(\d{4})-(\d{2})-(\d{2})/);
var all = [];
var re = /(a)(b)?/g, r;
while ((r = re.exec('ab a ab')) !== null) all.push([r.index, r[1], r[2] === undefined ? null : r[2]]);
globalThis.__result = JSON.stringify([m[0], m[1], m[2], m.index, date.slice(1), all, /(a)|b/.exec('b')[1] === undefined]);
