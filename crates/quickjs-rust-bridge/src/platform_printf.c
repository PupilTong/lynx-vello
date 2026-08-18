/* Private, allocator-free printf subset used by the QuickJS C objects. */

#include <stdarg.h>
#include <stddef.h>

/* Keep the embedded implementation local to this translation unit. QuickJS
 * needs integer, pointer, string, dynamic width, and dynamic precision
 * formatting. Its active runtime paths use neither floating point nor %n.
 */
#define NANOPRINTF_VISIBILITY_STATIC
#define NANOPRINTF_USE_FIELD_WIDTH_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_PRECISION_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_FLOAT_FORMAT_SPECIFIERS 0
#define NANOPRINTF_USE_LARGE_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_SMALL_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_BINARY_FORMAT_SPECIFIERS 0
#define NANOPRINTF_USE_WRITEBACK_FORMAT_SPECIFIERS 0
#define NANOPRINTF_USE_ALT_FORM_FLAG 0
#define NANOPRINTF_USE_FLOAT_SINGLE_PRECISION 0
#define NANOPRINTF_USE_FLOAT_HEX_FORMAT_SPECIFIER 0
#define NANOPRINTF_IMPLEMENTATION
#include "nanoprintf.h"

int qjs_platform_vsnprintf(char *destination, size_t capacity,
                           const char *format, va_list arguments) {
  return npf_vsnprintf(destination, capacity, format, arguments);
}

int qjs_platform_snprintf(char *destination, size_t capacity,
                          const char *format, ...) {
  va_list arguments;
  int length;

  va_start(arguments, format);
  length = qjs_platform_vsnprintf(destination, capacity, format, arguments);
  va_end(arguments);
  return length;
}
