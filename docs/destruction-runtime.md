# BTS application and object destruction

The built-in `bobcat:bts` Worker owns an application lifetime. The creator records
that role explicitly; an ordinary Worker cannot acquire it by using the BTS name
or importing the runtime. When its view is released, the BTS owner stops and reaps
ordinary tasks, invokes the already-loaded `__BobcatDestroyBTS` export, then drops
the realm. An unstarted realm is not created just to destroy it.

The JS export consumes the lifetime before calling the current
`app.callDestroyLifetimeFun` with the app as receiver and zero arguments. A throw,
reentrant call or earlier explicit `__DestroyLifetime` cannot repeat cleanup.
The final call drains ordinary Promise jobs even when the hook throws, but runs
no timer/resource epilogue. Cleanup cannot restart an ended worker. Hooks already
installed during an unfinished entry still run; other workers remain usable.
React reload retains this app lifetime and owns its own component cleanup.

## Object finalization in JavaScript

`getNativeApp().createJSObjectDestructionObserver(callback)` requires exactly one
function and returns an opaque object. Its JS `FinalizationRegistry` retains the
callback as the held value, without retaining the target. Property reads return
undefined and writes report the existing HostObject setter error without storing
the value. The cleanup job calls the function once, with no arguments and an
undefined receiver. Its return value is ignored; a throw goes through
`lynx.reportError` and does not stop other finalizers.

This follows the user's choice to implement the observer with the existing JS
finalization primitive. Native Lynx posts a low-priority task and can wait for a
50 ms timer window; Bobcat deliberately uses JS cleanup-job timing instead.
There is no Rust callback queue, worker index, idle scheduler, timer-window check
or observer-specific host function. Object collection is not a prompt disposal
API and no ordering between independent finalizers is promised.

Once the app lifetime has been consumed, the registry ignores late cleanup jobs,
including jobs drained by a sibling realm's checkpoint. Dropping the realm
releases its registry. The callback body remains framework code; this layer adds
no cross-thread node operation or native platform module.

## Validation boundaries

Real QuickJS tests verify target liveness, one-shot callback delivery, receiver and
arity, rejected arguments/property writes, nonfatal errors, suppression after app
destruction and sibling isolation. Real worker and dual-realm tests verify view
release, explicit destruction, current-hook lookup, reentrancy, thrown hooks and
Promise jobs, cancellation during entry imports, cancellation before source
arrival and ordinary Worker termination. The JS lifecycle test also exercises
repeated destruction notifications.

Compiled React unmount tests remain in the MVP integration layer because they
require the separate compiled-module bootstrap. Neither compiled bundles nor
new fixture copies are part of this layer.

Native observer reference: `lynx/core/runtime/js/bindings/js_app.cc:1475–1507`,
`bindings/js_object_destruction_observer.h`, and `base/src/fml/task_source.cc`.
