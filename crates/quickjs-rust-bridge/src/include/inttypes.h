/* QuickJS's private inttypes declaration overlay. */

#ifndef QJS_PLATFORM_INTTYPES_H
#define QJS_PLATFORM_INTTYPES_H

#include <stdint.h>

/* Derive the format spellings from the compiler's target data model.  In
 * particular, do not assume that int64_t is `long long`: that is false for
 * some hosted ABIs even though it is true for wasm32-unknown-unknown. */
#if !defined(__INT64_FMTd__) || !defined(__UINT64_FMTu__) ||                  \
    !defined(__UINT64_FMTx__)
#error "the QuickJS build requires compiler integer format macros"
#endif

#define PRId64 __INT64_FMTd__
#define PRIu64 __UINT64_FMTu__
#define PRIx64 __UINT64_FMTx__

#endif
