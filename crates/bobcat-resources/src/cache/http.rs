//! HTTP cache semantics, computed from headers alone: whether a response may
//! be stored, whether a stored one is still fresh, what a revalidation
//! sends, and how a `304` refreshes what was stored. RFC 9111, narrowed to a
//! private cache with one user.
//!
//! Nothing here touches storage or the network. That is what makes it
//! testable against literal header blocks, and what lets the disk tier and
//! the transport share one answer to "fetch, revalidate, or serve".

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bobcat_core::resource::CachePolicy;
use http::header::{
    CACHE_CONTROL, DATE, ETAG, EXPIRES, HeaderMap, HeaderName, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED, VARY,
};

/// The response record a stored body carries: everything freshness and
/// revalidation need, and nothing of the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub stored_at: SystemTime,
}

/// A stored response's standing at some instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// May be served without a request.
    Fresh,
    /// Must be revalidated before it is served.
    Stale,
    /// Must not have been stored; a defensive answer for an entry that was.
    Uncacheable,
}

/// What a request should do, given its policy and what the cache holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Serve the stored bytes without a request.
    UseStored,
    /// Send a conditional request; a `304` refreshes the stored entry.
    Revalidate,
    /// Fetch unconditionally, storing the response if it is storable.
    Fetch,
    /// Fetch unconditionally and store nothing.
    FetchNoStore,
    /// `only-if-cached` with nothing usable: the request cannot be served.
    Unavailable,
}

/// The longest a heuristically-dated response stays fresh.
const HEURISTIC_CAP: Duration = Duration::from_hours(24);

impl StoredResponse {
    /// Whether a private cache may store this response at all.
    ///
    /// `no-store` is the explicit refusal. The status list is RFC 9111's
    /// heuristically-cacheable set, which is also the set a client is
    /// allowed to reuse without an explicit lifetime. `Vary: *` names a
    /// response that no later request can be shown to match.
    #[must_use]
    pub fn is_storable(&self) -> bool {
        if !matches!(
            self.status,
            200 | 203 | 204 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501
        ) {
            return false;
        }
        let directives = cache_control(&self.headers);
        if directives.no_store {
            return false;
        }
        !self
            .headers
            .get_all(VARY)
            .iter()
            .any(|value| value.to_str().is_ok_and(|value| value.trim() == "*"))
    }

    /// The response's standing at `now`.
    ///
    /// An explicit `max-age` beats `Expires`; both are measured from the
    /// response's `Date`, or from when it was stored when it carried none.
    /// `no-cache` means stored but never served without asking. With no
    /// explicit lifetime, the heuristic is a tenth of the response's age
    /// since `Last-Modified`, capped at a day; a response with neither is
    /// simply stale.
    #[must_use]
    pub fn freshness(&self, now: SystemTime) -> Freshness {
        if !self.is_storable() {
            return Freshness::Uncacheable;
        }
        let directives = cache_control(&self.headers);
        if directives.no_cache {
            return Freshness::Stale;
        }
        let date = self
            .headers
            .get(DATE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_http_date)
            .unwrap_or(self.stored_at);
        let lifetime = if let Some(max_age) = directives.max_age {
            Some(max_age)
        } else if let Some(expires) = self.headers.get(EXPIRES) {
            // An unparseable `Expires` means "already expired" (RFC 9111
            // §5.3), which the zero lifetime expresses.
            Some(
                expires
                    .to_str()
                    .ok()
                    .and_then(parse_http_date)
                    .and_then(|expires| expires.duration_since(date).ok())
                    .unwrap_or(Duration::ZERO),
            )
        } else {
            self.headers
                .get(LAST_MODIFIED)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_http_date)
                .and_then(|modified| date.duration_since(modified).ok())
                .map(|age| (age / 10).min(HEURISTIC_CAP))
        };
        match lifetime {
            Some(lifetime) => {
                let age = now.duration_since(date).unwrap_or(Duration::ZERO);
                if age < lifetime {
                    Freshness::Fresh
                } else {
                    Freshness::Stale
                }
            }
            None => Freshness::Stale,
        }
    }

    /// The conditional headers a revalidation sends: `If-None-Match` from the
    /// stored `ETag`, `If-Modified-Since` from the stored `Last-Modified`.
    #[must_use]
    pub fn revalidation_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(etag) = self.headers.get(ETAG) {
            headers.insert(IF_NONE_MATCH, etag.clone());
        }
        if let Some(modified) = self.headers.get(LAST_MODIFIED) {
            headers.insert(IF_MODIFIED_SINCE, modified.clone());
        }
        headers
    }

    /// This response refreshed by a `304`: the stored headers updated with
    /// the ones the `304` carried (RFC 9111 §4.3.4), the body kept, and the
    /// storage time reset to `now`.
    #[must_use]
    pub fn refreshed_by(&self, not_modified: &HeaderMap, now: SystemTime) -> Self {
        let mut headers = self.headers.clone();
        let mut replaced: Vec<HeaderName> = Vec::new();
        for (name, value) in not_modified {
            // Hop-by-hop and validator-only fields of the 304 do not replace
            // what describes the stored body.
            if matches!(
                name.as_str(),
                "content-length" | "content-encoding" | "content-type"
            ) {
                continue;
            }
            if !replaced.contains(name) {
                headers.remove(name);
                replaced.push(name.clone());
            }
            headers.append(name.clone(), value.clone());
        }
        Self {
            status: self.status,
            headers,
            stored_at: now,
        }
    }
}

/// Decides how a request with `policy` is served given the entry the cache
/// holds for it, mirroring the fetch standard's cache modes.
#[must_use]
pub fn plan(policy: CachePolicy, stored: Option<&StoredResponse>, now: SystemTime) -> Plan {
    let standing = stored.map(|stored| stored.freshness(now));
    match policy {
        CachePolicy::NoStore => Plan::FetchNoStore,
        CachePolicy::Reload => Plan::Fetch,
        CachePolicy::NoCache => match standing {
            Some(Freshness::Fresh | Freshness::Stale) => Plan::Revalidate,
            Some(Freshness::Uncacheable) | None => Plan::Fetch,
        },
        CachePolicy::ForceCache => match standing {
            Some(Freshness::Fresh | Freshness::Stale) => Plan::UseStored,
            Some(Freshness::Uncacheable) | None => Plan::Fetch,
        },
        CachePolicy::OnlyIfCached => match standing {
            Some(Freshness::Fresh | Freshness::Stale) => Plan::UseStored,
            Some(Freshness::Uncacheable) | None => Plan::Unavailable,
        },
        _ => match standing {
            Some(Freshness::Fresh) => Plan::UseStored,
            Some(Freshness::Stale) => Plan::Revalidate,
            Some(Freshness::Uncacheable) | None => Plan::Fetch,
        },
    }
}

#[derive(Default)]
struct Directives {
    no_store: bool,
    no_cache: bool,
    max_age: Option<Duration>,
}

/// The `Cache-Control` directives this cache acts on. `s-maxage` is a shared
/// cache's and is ignored; `private` is fine for a private cache.
fn cache_control(headers: &HeaderMap) -> Directives {
    let mut directives = Directives::default();
    for value in headers.get_all(CACHE_CONTROL) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for directive in value.split(',') {
            let directive = directive.trim();
            let (name, argument) = directive
                .split_once('=')
                .map_or((directive, None), |(name, argument)| {
                    (name.trim(), Some(argument.trim().trim_matches('"')))
                });
            if name.eq_ignore_ascii_case("no-store") {
                directives.no_store = true;
            } else if name.eq_ignore_ascii_case("no-cache") {
                directives.no_cache = true;
            } else if name.eq_ignore_ascii_case("max-age")
                && let Some(seconds) = argument.and_then(|argument| argument.parse::<u64>().ok())
            {
                directives.max_age = Some(Duration::from_secs(seconds));
            }
        }
    }
    directives
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Parses an HTTP date: the IMF-fixdate form (`Sun, 06 Nov 1994 08:49:37
/// GMT`), the obsolete RFC 850 form (`Sunday, 06-Nov-94 08:49:37 GMT`), and
/// the `asctime()` form (`Sun Nov  6 08:49:37 1994`), per RFC 9110 §5.6.7.
#[must_use]
pub fn parse_http_date(value: &str) -> Option<SystemTime> {
    let fields: Vec<&str> = value.split_whitespace().collect();
    let (day, month, year, time) = match fields.as_slice() {
        // IMF-fixdate: Sun, 06 Nov 1994 08:49:37 GMT
        [_weekday, day, month, year, time, zone] if zone.eq_ignore_ascii_case("GMT") => (
            day.parse::<u32>().ok()?,
            *month,
            year.parse::<i64>().ok()?,
            *time,
        ),
        // RFC 850: Sunday, 06-Nov-94 08:49:37 GMT
        [_weekday, date, time, zone] if zone.eq_ignore_ascii_case("GMT") => {
            let mut parts = date.split('-');
            let day = parts.next()?.parse::<u32>().ok()?;
            let month = parts.next()?;
            let year = parts.next()?.parse::<i64>().ok()?;
            if parts.next().is_some() {
                return None;
            }
            // Two-digit years: the RFC 9110 rule is "the most recent past
            // year with those two digits" — 1969-2068 is the conventional
            // window that gives.
            let year = if year < 100 {
                if year < 69 { 2000 + year } else { 1900 + year }
            } else {
                year
            };
            (day, month, year, *time)
        }
        // asctime: Sun Nov  6 08:49:37 1994
        [_weekday, month, day, time, year] => (
            day.parse::<u32>().ok()?,
            *month,
            year.parse::<i64>().ok()?,
            *time,
        ),
        _ => return None,
    };
    let month = MONTHS
        .iter()
        .position(|name| name.eq_ignore_ascii_case(month))?
        + 1;
    let mut clock = time.split(':');
    let hour = clock.next()?.parse::<u64>().ok()?;
    let minute = clock.next()?.parse::<u64>().ok()?;
    let second = clock.next()?.parse::<u64>().ok()?;
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 60 || day == 0 || day > 31 {
        return None;
    }
    let days = days_from_civil(year, u32::try_from(month).ok()?, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::try_from(hour * 3600 + minute * 60 + second).ok()?)?;
    if seconds < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).ok()?))
}

/// Formats `time` as an IMF-fixdate.
#[must_use]
pub fn format_http_date(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday, index 3 in a Monday-first week.
    let weekday = WEEKDAYS[usize::try_from((days + 3).rem_euclid(7)).expect("0..7")];
    format!(
        "{weekday}, {day:02} {} {year:04} {:02}:{:02}:{:02} GMT",
        MONTHS[usize::try_from(month - 1).expect("1..=12")],
        remainder / 3600,
        remainder % 3600 / 60,
        remainder % 60
    )
}

/// Days since 1970-01-01 for a proleptic Gregorian civil date (Howard
/// Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date of a day count since 1970-01-01 (Hinnant's
/// `civil_from_days`), as `(year, month, day)`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use http::header::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                HeaderValue::from_str(value).expect("a header value"),
            );
        }
        headers
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn stored(status: u16, pairs: &[(&str, &str)], stored_at: u64) -> StoredResponse {
        StoredResponse {
            status,
            headers: headers(pairs),
            stored_at: at(stored_at),
        }
    }

    const EPOCH_DATE: &str = "Thu, 01 Jan 1970 00:00:00 GMT";

    #[test]
    fn http_dates_round_trip_through_every_form() {
        let expected = at(784_111_777);
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(expected)
        );
        assert_eq!(
            parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"),
            Some(expected)
        );
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), Some(expected));
        assert_eq!(format_http_date(expected), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(format_http_date(UNIX_EPOCH), EPOCH_DATE);
        assert_eq!(parse_http_date(EPOCH_DATE), Some(UNIX_EPOCH));
        for seconds in [0, 86_399, 951_782_400, 1_709_251_199, 4_102_444_800] {
            let time = at(seconds);
            assert_eq!(
                parse_http_date(&format_http_date(time)),
                Some(time),
                "{seconds}"
            );
        }
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 PST"), None);
        assert_eq!(parse_http_date("Sun, 32 Nov 1994 08:49:37 GMT"), None);
        assert_eq!(parse_http_date("garbage"), None);
        assert_eq!(parse_http_date("Sun, 06 Foo 1994 08:49:37 GMT"), None);
    }

    #[test]
    fn storability_follows_status_no_store_and_vary() {
        assert!(stored(200, &[], 0).is_storable());
        assert!(stored(404, &[], 0).is_storable());
        assert!(!stored(500, &[], 0).is_storable());
        assert!(!stored(302, &[], 0).is_storable());
        assert!(!stored(200, &[("cache-control", "private, no-store")], 0).is_storable());
        assert!(!stored(200, &[("vary", "*")], 0).is_storable());
        assert!(stored(200, &[("vary", "Accept-Encoding")], 0).is_storable());
    }

    #[test]
    fn max_age_beats_expires_and_is_measured_from_the_date_header() {
        let response = stored(
            200,
            &[
                ("date", "Sun, 06 Nov 1994 08:49:37 GMT"),
                ("cache-control", "public, max-age=60"),
                ("expires", "Sun, 06 Nov 1994 09:49:37 GMT"),
            ],
            784_111_777 + 1000,
        );
        assert_eq!(response.freshness(at(784_111_777 + 59)), Freshness::Fresh);
        assert_eq!(response.freshness(at(784_111_777 + 60)), Freshness::Stale);
    }

    #[test]
    fn expires_alone_and_a_broken_expires_are_honoured() {
        let response = stored(
            200,
            &[
                ("date", "Sun, 06 Nov 1994 08:49:37 GMT"),
                ("expires", "Sun, 06 Nov 1994 09:49:37 GMT"),
            ],
            0,
        );
        assert_eq!(response.freshness(at(784_111_777 + 3599)), Freshness::Fresh);
        assert_eq!(response.freshness(at(784_111_777 + 3600)), Freshness::Stale);
        let broken = stored(200, &[("expires", "0")], 100);
        assert_eq!(broken.freshness(at(100)), Freshness::Stale);
    }

    #[test]
    fn the_heuristic_lifetime_is_a_tenth_of_the_age_capped_at_a_day() {
        let response = stored(
            200,
            &[
                ("date", "Sun, 06 Nov 1994 08:49:37 GMT"),
                ("last-modified", "Sat, 05 Nov 1994 08:49:37 GMT"),
            ],
            0,
        );
        // A day since modification: fresh for 8640 seconds.
        assert_eq!(response.freshness(at(784_111_777 + 8639)), Freshness::Fresh);
        assert_eq!(response.freshness(at(784_111_777 + 8640)), Freshness::Stale);
        let ancient = stored(
            200,
            &[
                ("date", "Sun, 06 Nov 1994 08:49:37 GMT"),
                ("last-modified", "Thu, 01 Jan 1970 00:00:00 GMT"),
            ],
            0,
        );
        assert_eq!(
            ancient.freshness(at(784_111_777 + 86_399)),
            Freshness::Fresh
        );
        assert_eq!(
            ancient.freshness(at(784_111_777 + 86_400)),
            Freshness::Stale
        );
        assert_eq!(
            stored(200, &[], 0).freshness(at(0)),
            Freshness::Stale,
            "no lifetime at all"
        );
    }

    #[test]
    fn no_cache_is_stored_but_always_revalidated_and_stored_at_stands_in_for_date() {
        let response = stored(200, &[("cache-control", "no-cache, max-age=3600")], 500);
        assert_eq!(response.freshness(at(500)), Freshness::Stale);
        let undated = stored(200, &[("cache-control", "max-age=10")], 500);
        assert_eq!(undated.freshness(at(509)), Freshness::Fresh);
        assert_eq!(undated.freshness(at(510)), Freshness::Stale);
        assert_eq!(
            stored(500, &[("cache-control", "max-age=10")], 0).freshness(at(0)),
            Freshness::Uncacheable
        );
    }

    #[test]
    fn revalidation_headers_and_304_refresh() {
        let response = stored(
            200,
            &[
                ("etag", "\"v1\""),
                ("last-modified", "Sun, 06 Nov 1994 08:49:37 GMT"),
                ("content-type", "text/css"),
                ("cache-control", "max-age=1"),
            ],
            0,
        );
        let conditional = response.revalidation_headers();
        assert_eq!(conditional.get(IF_NONE_MATCH).unwrap(), "\"v1\"");
        assert_eq!(
            conditional.get(IF_MODIFIED_SINCE).unwrap(),
            "Sun, 06 Nov 1994 08:49:37 GMT"
        );
        assert!(stored(200, &[], 0).revalidation_headers().is_empty());

        let refreshed = response.refreshed_by(
            &headers(&[
                ("cache-control", "max-age=600"),
                ("etag", "\"v1\""),
                ("content-type", "text/plain"),
            ]),
            at(99),
        );
        assert_eq!(refreshed.stored_at, at(99));
        assert_eq!(
            refreshed.headers.get("cache-control").unwrap(),
            "max-age=600"
        );
        assert_eq!(
            refreshed.headers.get("content-type").unwrap(),
            "text/css",
            "the stored body's description is kept"
        );
        assert_eq!(refreshed.freshness(at(99 + 599)), Freshness::Fresh);
    }

    #[test]
    fn plans_mirror_the_fetch_cache_modes() {
        let fresh = stored(200, &[("cache-control", "max-age=100")], 0);
        let stale = stored(200, &[("cache-control", "max-age=0")], 0);
        let uncacheable = stored(500, &[], 0);
        let now = at(50);
        assert_eq!(
            plan(CachePolicy::Default, Some(&fresh), now),
            Plan::UseStored
        );
        assert_eq!(
            plan(CachePolicy::Default, Some(&stale), now),
            Plan::Revalidate
        );
        assert_eq!(
            plan(CachePolicy::Default, Some(&uncacheable), now),
            Plan::Fetch
        );
        assert_eq!(plan(CachePolicy::Default, None, now), Plan::Fetch);
        assert_eq!(
            plan(CachePolicy::NoStore, Some(&fresh), now),
            Plan::FetchNoStore
        );
        assert_eq!(plan(CachePolicy::Reload, Some(&fresh), now), Plan::Fetch);
        assert_eq!(
            plan(CachePolicy::NoCache, Some(&fresh), now),
            Plan::Revalidate
        );
        assert_eq!(plan(CachePolicy::NoCache, None, now), Plan::Fetch);
        assert_eq!(
            plan(CachePolicy::ForceCache, Some(&stale), now),
            Plan::UseStored
        );
        assert_eq!(plan(CachePolicy::ForceCache, None, now), Plan::Fetch);
        assert_eq!(
            plan(CachePolicy::OnlyIfCached, Some(&stale), now),
            Plan::UseStored
        );
        assert_eq!(
            plan(CachePolicy::OnlyIfCached, None, now),
            Plan::Unavailable
        );
    }

    #[test]
    fn civil_conversions_agree_across_leap_years() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        for days in (-100_000..100_000).step_by(997) {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(
                days_from_civil(
                    year,
                    u32::try_from(month).unwrap(),
                    u32::try_from(day).unwrap()
                ),
                days
            );
        }
    }
}
