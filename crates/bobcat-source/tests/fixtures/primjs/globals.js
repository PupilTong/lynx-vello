var counter = 1;
let label = 'x';
const limit = 3;
function bump() { counter += limit; return label + counter; }
bump();
