/*
 * C allocation boundary for every translation unit built by
 * quickjs-rust-bridge. This header is force-included by build.rs and consumes
 * the platform allocator headers before defining the macros below. They affect
 * calls in the pinned QuickJS sources and shim without replacing the
 * process-wide libc symbols.
 */

#ifndef QJS_RUST_ALLOCATOR_H
#define QJS_RUST_ALLOCATOR_H

#include <stddef.h>
#include <stdlib.h>
#if defined(__APPLE__)
#include <malloc/malloc.h>
#elif defined(_WIN32)
#include <malloc.h>
#elif defined(__linux__) || defined(__GLIBC__)
#include <malloc.h>
#elif defined(__FreeBSD__)
#include <malloc_np.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

void *qjs_rust_malloc(size_t size);
void *qjs_rust_calloc(size_t count, size_t size);
void *qjs_rust_realloc(void *pointer, size_t size);
void qjs_rust_free(void *pointer);
size_t qjs_rust_malloc_usable_size(const void *pointer);

#ifdef __cplusplus
}
#endif

/*
 * Rust prefixes every allocation with one 16-byte-aligned header. QuickJS
 * adds this value to its usable-size accounting so memory limits include the
 * bridge's metadata instead of silently under-counting it.
 */
#define QJS_RUST_ALLOCATOR_OVERHEAD 16

_Static_assert(_Alignof(max_align_t) <= QJS_RUST_ALLOCATOR_OVERHEAD,
               "the Rust allocator bridge must satisfy malloc alignment");

#define malloc(size) qjs_rust_malloc(size)
#define calloc(count, size) qjs_rust_calloc((count), (size))
#define realloc(pointer, size) qjs_rust_realloc((pointer), (size))
#define free(pointer) qjs_rust_free(pointer)

#endif
