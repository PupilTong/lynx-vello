/*
 * QuickJS's private stdlib declaration overlay.
 *
 * Every target receives the same declarations consumed by the pinned QuickJS
 * translation units.  Allocation and abort are subsequently redirected by
 * rust-platform.h; abs and alloca remain compiler builtins and add no C ABI.
 */

#ifndef QJS_PLATFORM_STDLIB_H
#define QJS_PLATFORM_STDLIB_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *pointer, size_t size);
void free(void *pointer);
_Noreturn void abort(void);

#ifdef __cplusplus
}
#endif

/* QuickJS uses both operations internally.  Keep them as compiler operations
 * instead of inventing freestanding or hosted runtime symbols. */
#if defined(__clang__) || defined(__GNUC__)
#define abs(value) __builtin_abs(value)
#define alloca(size) __builtin_alloca(size)
#else
#error "the QuickJS build requires compiler abs and alloca builtins"
#endif

#endif
