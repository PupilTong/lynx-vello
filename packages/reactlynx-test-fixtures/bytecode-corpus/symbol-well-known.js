// Well-known symbols: toPrimitive, toStringTag, iterator, hasInstance, isConcatSpreadable, species.
var money = { [Symbol.toPrimitive](hint) { return hint === 'number' ? 42 : hint === 'string' ? '$42' : 'default'; } };
var tagged = { [Symbol.toStringTag]: 'Custom' };
var spreadable = { length: 2, 0: 'x', 1: 'y', [Symbol.isConcatSpreadable]: true };
var notSpread = [1, 2];
notSpread[Symbol.isConcatSpreadable] = false;
class Plain extends Array { static get [Symbol.species]() { return Array; } }
var mapped = Plain.from([1]).map(x => x);
globalThis.__result = JSON.stringify([+money, `${money}`, money + '', Object.prototype.toString.call(tagged), [0].concat(spreadable), [0].concat(notSpread).length, mapped instanceof Plain]);
