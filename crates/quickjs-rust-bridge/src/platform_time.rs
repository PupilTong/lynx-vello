//! Rust time hooks used by the `QuickJS` C build.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Local, Utc};

static RANDOM_SEED_FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

const GREGORIAN_400_YEAR_CYCLE_MILLISECONDS: i128 = 146_097 * 86_400_000;

fn positive_div_ceil(dividend: i128, divisor: i128) -> i128 {
    debug_assert!(dividend > 0);
    debug_assert!(divisor > 0);
    1 + (dividend - 1) / divisor
}

fn timezone_query_datetime(epoch_milliseconds: i64) -> Option<DateTime<Utc>> {
    if let Some(datetime) = DateTime::from_timestamp_millis(epoch_milliseconds) {
        return Some(datetime);
    }

    let timestamp = i128::from(epoch_milliseconds);
    let minimum = i128::from(DateTime::<Utc>::MIN_UTC.timestamp_millis());
    let maximum = i128::from(DateTime::<Utc>::MAX_UTC.timestamp_millis());
    let mapped = if timestamp < minimum {
        let cycles = positive_div_ceil(minimum - timestamp, GREGORIAN_400_YEAR_CYCLE_MILLISECONDS);
        timestamp + cycles * GREGORIAN_400_YEAR_CYCLE_MILLISECONDS
    } else {
        let cycles = positive_div_ceil(timestamp - maximum, GREGORIAN_400_YEAR_CYCLE_MILLISECONDS);
        timestamp - cycles * GREGORIAN_400_YEAR_CYCLE_MILLISECONDS
    };

    i64::try_from(mapped)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
}

fn nonzero_random_seed(seed: u64) -> u64 {
    if seed == 0 { 1 } else { seed }
}

fn fallback_random_seed(timestamp_microseconds: i64, sequence: u64) -> u64 {
    nonzero_random_seed(timestamp_microseconds.cast_unsigned() ^ sequence)
}

fn utc_minus_local_minutes(local_minus_utc_seconds: i32) -> i32 {
    -local_minus_utc_seconds / 60
}

/// Returns whole milliseconds since the Unix epoch for `QuickJS` `Date.now()`.
#[unsafe(no_mangle)]
pub extern "C" fn qjs_rust_epoch_time_milliseconds() -> i64 {
    Utc::now().timestamp_millis()
}

/// Returns a non-zero initial state for `QuickJS` `Math.random()`.
#[unsafe(no_mangle)]
pub extern "C" fn qjs_rust_random_seed() -> u64 {
    getrandom::u64().map_or_else(
        |_| {
            let sequence = RANDOM_SEED_FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
            fallback_random_seed(Utc::now().timestamp_micros(), sequence)
        },
        nonzero_random_seed,
    )
}

/// Returns UTC-minus-local in whole minutes for the supplied Unix timestamp.
///
/// `chrono::Local` uses the platform time-zone database on native targets and
/// the host JavaScript `Date` implementation on browser Wasm. Chrono's date
/// range is slightly narrower than ECMAScript's, so queries in the remaining
/// valid ECMAScript fringe are shifted by whole Gregorian 400-year cycles into
/// Chrono's nearest same-side window, preserving their calendar and DST phase.
#[unsafe(no_mangle)]
pub extern "C" fn qjs_rust_timezone_offset_minutes(epoch_milliseconds: i64) -> i32 {
    let Some(datetime) = timezone_query_datetime(epoch_milliseconds) else {
        return 0;
    };
    let local = datetime.with_timezone(&Local);
    utc_minus_local_minutes(local.offset().local_minus_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_timezone_queries_into_chronos_range_by_gregorian_cycles() {
        const ECMASCRIPT_DATE_LIMIT_MILLISECONDS: i64 = 8_640_000_000_000_000;
        let minimum = i128::from(DateTime::<Utc>::MIN_UTC.timestamp_millis());
        let maximum = i128::from(DateTime::<Utc>::MAX_UTC.timestamp_millis());

        for timestamp in [
            -ECMASCRIPT_DATE_LIMIT_MILLISECONDS,
            ECMASCRIPT_DATE_LIMIT_MILLISECONDS,
            i64::MIN,
            i64::MAX,
        ] {
            assert!(DateTime::from_timestamp_millis(timestamp).is_none());
            let mapped = timezone_query_datetime(timestamp)
                .expect("a Gregorian-cycle mapping should fit Chrono's range")
                .timestamp_millis();
            let mapped = i128::from(mapped);
            assert_eq!(
                (i128::from(timestamp) - mapped).rem_euclid(GREGORIAN_400_YEAR_CYCLE_MILLISECONDS),
                0
            );
            if timestamp.is_negative() {
                assert!(minimum <= mapped);
                assert!(mapped < minimum + GREGORIAN_400_YEAR_CYCLE_MILLISECONDS);
            } else {
                assert!(maximum - GREGORIAN_400_YEAR_CYCLE_MILLISECONDS < mapped);
                assert!(mapped <= maximum);
            }
        }

        assert_eq!(
            timezone_query_datetime(0)
                .expect("the Unix epoch is representable")
                .timestamp_millis(),
            0
        );
    }

    #[test]
    fn normalizes_zero_random_seed() {
        assert_eq!(nonzero_random_seed(0), 1);
        assert_eq!(nonzero_random_seed(42), 42);
    }

    #[test]
    fn fallback_random_seeds_distinguish_the_same_timestamp() {
        let first = fallback_random_seed(1_723_968_000_000_000, 0);
        let second = fallback_random_seed(1_723_968_000_000_000, 1);

        assert_ne!(first, 0);
        assert_ne!(second, 0);
        assert_ne!(first, second);
    }

    #[test]
    fn timezone_offset_uses_ecmascripts_sign() {
        assert_eq!(utc_minus_local_minutes(8 * 60 * 60), -480);
        assert_eq!(utc_minus_local_minutes(-(5 * 60 * 60 + 30 * 60)), 330);
    }
}
