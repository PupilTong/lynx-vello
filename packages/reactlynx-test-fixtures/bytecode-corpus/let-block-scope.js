// let and const are scoped to their block.
let x = 'outer';
var seen = [];
{
  let x = 'block';
  seen.push(x);
  {
    const x = 'inner';
    seen.push(x);
  }
  seen.push(x);
}
seen.push(x);
if (true) { let y = 1; seen.push(typeof y); }
seen.push(typeof y);
globalThis.__result = JSON.stringify(seen);
