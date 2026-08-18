/* Private assert surface for the QuickJS C translation units. */

#ifndef QJS_RUST_ASSERT_H
#define QJS_RUST_ASSERT_H

/* Preserve the standard NDEBUG contract while avoiding a second platform
 * assertion ABI. Active invariant failures use the Rust abort hook, and the
 * condition is evaluated exactly once.
 */
#ifdef NDEBUG
#define assert(condition) ((void)0)
#else
#define assert(condition) ((condition) ? (void)0 : qjs_rust_abort())
#endif

#endif
