var log = [];
function guarded(fail) {
  try {
    if (fail) throw new Error('fail');
    return 'ok';
  } catch (e) {
    log.push(e.message);
    return 'caught';
  } finally {
    log.push('finally');
  }
}
guarded(true);
guarded(false);
