var pattern = /a(b+)c/gi;
var found = pattern.exec('xxABBCxx');
var replaced = 'a-b-c'.replace(/-/g, '+');
