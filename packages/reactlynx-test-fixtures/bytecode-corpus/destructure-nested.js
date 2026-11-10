// Nested object and array patterns.
var data = { user: { name: 'n', tags: ['t1', 't2'], addr: { city: 'c' } }, list: [{ id: 1 }, { id: 2 }] };
var { user: { name, tags: [, second], addr: { city } }, list: [{ id: firstId }, ...restItems] } = data;
globalThis.__result = JSON.stringify([name, second, city, firstId, restItems]);
