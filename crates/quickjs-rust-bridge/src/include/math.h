/* QuickJS's private math declaration overlay. */

#ifndef QJS_PLATFORM_MATH_H
#define QJS_PLATFORM_MATH_H

#if !defined(__clang__) && !defined(__GNUC__)
#error "the QuickJS build requires compiler floating-point builtins"
#endif

/* These are compile-time constants/predicates and do not introduce a C ABI. */
#define INFINITY (__builtin_inff())
#define NAN (__builtin_nanf(""))
#define isfinite(value) (__builtin_isfinite(value))
#define isnan(value) (__builtin_isnan(value))
#define signbit(value) (__builtin_signbit(value))

#ifdef __cplusplus
extern "C" {
#endif

double acos(double value);
double acosh(double value);
double asin(double value);
double asinh(double value);
double atan(double value);
double atan2(double y, double x);
double atanh(double value);
double cbrt(double value);
double ceil(double value);
double cos(double value);
double cosh(double value);
double exp(double value);
double expm1(double value);
double fabs(double value);
double floor(double value);
double fmax(double left, double right);
double fmin(double left, double right);
double fmod(double numerator, double denominator);
double hypot(double x, double y);
double log(double value);
double log10(double value);
double log1p(double value);
double log2(double value);
long lrint(double value);
double pow(double base, double exponent);
double round(double value);
double sin(double value);
double sinh(double value);
double sqrt(double value);
double tan(double value);
double tanh(double value);
double trunc(double value);

#ifdef __cplusplus
}
#endif

#endif
