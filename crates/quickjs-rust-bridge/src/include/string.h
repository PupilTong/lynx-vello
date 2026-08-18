/* QuickJS's private string declaration overlay. */

#ifndef QJS_PLATFORM_STRING_H
#define QJS_PLATFORM_STRING_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void *memchr(const void *pointer, int byte, size_t length);
int memcmp(const void *left, const void *right, size_t length);
void *memcpy(void *destination, const void *source, size_t length);
void *memmove(void *destination, const void *source, size_t length);
void *memset(void *destination, int byte, size_t length);
char *strchr(const char *string, int byte);
int strcmp(const char *left, const char *right);
size_t strlen(const char *string);
char *strrchr(const char *string, int byte);

#ifdef __cplusplus
}
#endif

#endif
