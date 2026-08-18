/*
 * Private C-to-Rust platform boundary for every translation unit built by
 * quickjs-rust-bridge. Consume the platform declarations before defining the
 * mappings below: no macro may rewrite a declaration inside a system header,
 * and no process-wide standard-library symbol is replaced.
 */

#ifndef QJS_RUST_PLATFORM_H
#define QJS_RUST_PLATFORM_H

#include <ctype.h>
#include <fenv.h>
#include <inttypes.h>
#include <limits.h>
#include <math.h>
#include <setjmp.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "rust-allocator.h"

#ifdef __cplusplus
extern "C" {
#endif

_Noreturn void qjs_rust_abort(void);
double qjs_rust_atanh(double value);
long qjs_rust_lrint(double value);
void *qjs_rust_memchr(const void *pointer, int byte, size_t length);
char *qjs_rust_strchr(const char *string, int byte);
int qjs_rust_strcmp(const char *left, const char *right);
char *qjs_rust_strrchr(const char *string, int byte);

#if defined(__GNUC__) || defined(__clang__)
#define QJS_PLATFORM_PRINTF_LIKE(format_index, first_argument)                 \
  __attribute__((format(printf, format_index, first_argument)))
#else
#define QJS_PLATFORM_PRINTF_LIKE(format_index, first_argument)
#endif

int qjs_platform_snprintf(char *destination, size_t capacity,
                          const char *format, ...)
    QJS_PLATFORM_PRINTF_LIKE(3, 4);
int qjs_platform_vsnprintf(char *destination, size_t capacity,
                           const char *format, va_list arguments)
    QJS_PLATFORM_PRINTF_LIKE(3, 0);

#undef QJS_PLATFORM_PRINTF_LIKE

#ifdef __cplusplus
}
#endif

/* A C implementation may expose standard functions as macros. Its
 * declarations have already been consumed, so replace any such definitions
 * with the private bridge names used only by these QuickJS translation units.
 */
#undef abort
#undef atanh
#undef lrint
#undef memchr
#undef strchr
#undef strcmp
#undef strrchr
#undef snprintf
#undef vsnprintf

#define abort qjs_rust_abort
#define atanh qjs_rust_atanh
#define lrint qjs_rust_lrint
#define memchr qjs_rust_memchr
#define strchr qjs_rust_strchr
#define strcmp qjs_rust_strcmp
#define strrchr qjs_rust_strrchr
#define snprintf qjs_platform_snprintf
#define vsnprintf qjs_platform_vsnprintf

/* This resolves to the bridge's private header through build.rs's include
 * path. Include it only after the Rust abort hook has been declared.
 */
#include <assert.h>

#endif
