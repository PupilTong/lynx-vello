// Dates built from fixed UTC values, independent of the time zone.
var d = new Date(Date.UTC(2020, 1, 29, 12, 30, 15, 250));
var e = new Date(0);
var out = [d.toISOString(), d.getUTCDay(), d.getTime(), e.toISOString(), Date.UTC(1970, 0, 2), new Date(NaN).getTime(), JSON.stringify({ d: e }), new Date('2000-01-01T00:00:00Z').getTime()];
globalThis.__result = JSON.stringify(out.map(String));
