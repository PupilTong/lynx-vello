/*
 * QuickJS's private stdio declaration overlay.
 *
 * snprintf/vsnprintf are the only formatting operations retained by the
 * bridge and are mapped to nanoprintf by rust-platform.h.  QuickJS's FILE,
 * standard-stream, and console diagnostic surface is compiled out by
 * QJS_NO_STDIO_DIAGNOSTICS instead of being modelled as a private ABI.
 */

#ifndef QJS_PLATFORM_STDIO_H
#define QJS_PLATFORM_STDIO_H

#include <stdarg.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

int snprintf(char *destination, size_t capacity, const char *format, ...);
int vsnprintf(char *destination, size_t capacity, const char *format,
              va_list arguments);

#ifdef __cplusplus
}
#endif

#endif
