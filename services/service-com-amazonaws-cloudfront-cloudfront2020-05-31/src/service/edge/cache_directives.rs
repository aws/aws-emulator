// SPDX-License-Identifier: CC0-1.0

//! What the ORIGIN said about storing its own answer, and how long the edge holds that answer
//! because of it.
//!
//! One rule for every answer an origin produces: an answer whose own `Cache-Control` carries a
//! directive that forbids a shared cache from storing it is not held in the edge cache at all,
//! and one that carries none is held for the FRESHNESS LIFETIME the origin stated for it, bounded
//! by the spans the cache behaviour states — never below its `MinTTL`, "the minimum amount of
//! time ... that you want objects to stay in the CloudFront cache before CloudFront sends another
//! request to the origin to see if the object has been updated", and never above its `MaxTTL`,
//! "the maximum amount of time ... that objects stay in the CloudFront cache", which CloudFront
//! "uses ... only when the origin sends `Cache-Control` or `Expires` headers with the object". An
//! origin that sends neither header states no lifetime at all, and such an answer is held for the
//! behaviour's `DefaultTTL`, which CloudFront "uses ... as the object's time to live (TTL) only
//! when the origin does *not* send `Cache-Control` or `Expires` headers with the object". So the
//! stated lifetime decides the span and the configuration bounds it, and the default span is
//! reached only where the origin stated nothing ([`held_for`] is that one decision,
//! [`stated_lifetime`] the reading of the two headers).
//!
//! The exception is stated by the cache behaviour itself — "if your minimum TTL is greater than
//! 0, CloudFront will cache content for at least the duration specified in the cache policy's
//! minimum TTL, even if the `Cache-Control: no-cache`, `no-store`, or `private` directives are
//! present in the origin headers" (`CacheBehavior`) — so a behaviour whose minimum TTL is a span
//! holds such an answer for that span. The exception is about CONTENT: it covers the statuses the
//! edge caches as objects ([`super::is_cacheable_status`]) and not an answer that is an error,
//! which is held only for what the distribution's custom error responses state about it
//! ([`super::custom_errors`]).
//!
//! The three directives are the ones that sentence names, and they are read as PRESENT or absent
//! — "the … directives are present in the origin headers" — so a directive stating what it
//! applies to is the directive being present. The lifetime an answer the origin DOES admit to a
//! shared cache stays fresh for is read off the two headers those TTL members name:
//! `Cache-Control`'s `s-maxage`, the directive addressed to a shared cache, then its `max-age`,
//! then the absolute instant `Expires` carries — a span from the moment the answer was received,
//! and no span at all for an instant already past or an expiry HTTP cannot read.

use ::std::time::{Duration, SystemTime};

/// The response header an origin states a cache directive in.
const CACHE_CONTROL: &str = "cache-control";

/// The response header an origin states the absolute instant its answer goes stale at.
const EXPIRES: &str = "expires";

/// The `Cache-Control` directives that state a freshness lifetime, in the order a SHARED cache
/// reads them: `s-maxage` is the one addressed to a shared cache, so it is read ahead of the
/// `max-age` every cache reads.
const FRESHNESS_LIFETIME: [&str; 2] = ["s-maxage", "max-age"];

/// The directives that forbid a shared cache from storing the answer they are on, as
/// `CacheBehavior` names them: `no-cache`, `no-store` and `private`.
const FORBID_STORAGE: [&str; 3] = ["no-store", "no-cache", "private"];

/// One answer the edge has to decide the residence of: the status it carries, what the origin
/// said about storing it, and the spans the distribution's configuration states.
#[derive(Debug)]
pub struct Residence<'a> {
    /// The status the origin answered with.
    pub status: u16,
    /// The origin's own response headers, lowercased, as the edge serves them.
    pub headers: &'a [(String, String)],
    /// `ErrorCachingMinTTL` for this status, when the distribution's custom error responses name
    /// it ([`super::custom_errors::caching_ttl`]).
    pub error_ttl: Option<Duration>,
    /// How long an answer the behaviour may cache is held when the origin states no freshness
    /// lifetime of its own: the behaviour's `DefaultTTL`, or `None` for a behaviour that holds
    /// such an answer for no time at all.
    pub ttl: Option<Duration>,
    /// The behaviour's MINIMUM TTL, which is what holds content the origin asked no cache to
    /// store, and the floor under a freshness lifetime the origin stated.
    pub min_ttl: Option<Duration>,
    /// The behaviour's MAXIMUM TTL, the ceiling over a freshness lifetime the origin stated, and
    /// `None` for a behaviour whose maximum is no span at all.
    pub max_ttl: Option<Duration>,
    /// When the edge RECEIVED this answer, which the absolute instant an `Expires` carries is a
    /// span from.
    pub received_at: SystemTime,
}

/// How long the edge holds `residence` in the distribution's cache, or `None` when it holds it
/// not at all.
pub fn held_for(residence: &Residence<'_>) -> Option<Duration> {
    if forbids_storage(residence.headers) {
        // The origin asked that this answer not be stored, so the only thing that still holds it
        // is the behaviour's own minimum TTL, and only for content.
        return residence
            .min_ttl
            .filter(|_| super::is_cacheable_status(residence.status));
    }
    if let Some(error_ttl) = residence.error_ttl {
        // An ERROR the distribution's custom error responses name is held for the span they state
        // about it, which is the one thing that holds an error at all.
        return Some(error_ttl);
    }
    if !super::is_cacheable_status(residence.status) {
        return None;
    }
    let held = match stated_lifetime(residence.headers, residence.received_at) {
        // The origin stated how long its own answer stays fresh, so the behaviour's minimum and
        // maximum bound THAT span.
        Some(stated) => bounded(stated, residence),
        // It stated nothing, which is the one case the behaviour's default span applies in.
        None => residence.ttl.unwrap_or_default(),
    };
    (!held.is_zero()).then_some(held)
}

/// `stated` bounded by the spans the behaviour states: never below its minimum TTL and never
/// above its maximum.
///
/// A span the behaviour does not state is no span at all, so a behaviour that holds nothing
/// (every TTL zero) holds an answer under a stated lifetime for nothing either.
fn bounded(stated: Duration, residence: &Residence<'_>) -> Duration {
    stated
        .min(residence.max_ttl.unwrap_or_default())
        .max(residence.min_ttl.unwrap_or_default())
}

/// The freshness lifetime the ORIGIN stated for its own answer, or `None` when it stated none —
/// the one reading of the two headers the behaviour's maximum and default TTLs are about.
///
/// The order is a SHARED cache's: the `s-maxage` addressed to one, then the `max-age` every cache
/// reads, then the absolute instant `Expires` carries, which is a span from `received_at` — the
/// moment this answer reached the edge.
fn stated_lifetime(headers: &[(String, String)], received_at: SystemTime) -> Option<Duration> {
    directive_lifetime(headers).or_else(|| expires_lifetime(headers, received_at))
}

/// The freshness lifetime the origin's `Cache-Control` directives state, or `None` when they
/// state none: the first of [`FRESHNESS_LIFETIME`] the header carries with an argument that is a
/// span, a directive whose argument is no such value stating no lifetime.
fn directive_lifetime(headers: &[(String, String)]) -> Option<Duration> {
    FRESHNESS_LIFETIME.iter().find_map(|wanted| {
        directives(headers).find_map(|(name, argument)| {
            name.eq_ignore_ascii_case(wanted)
                .then_some(argument)
                .and_then(delta_seconds)
        })
    })
}

/// One `delta-seconds` argument as a span, or `None` for an argument that is no such value — a
/// directive stating one of those states no lifetime at all. A value too large for the clock is
/// the largest span the clock holds rather than no value.
fn delta_seconds(argument: &str) -> Option<Duration> {
    if argument.is_empty() || !argument.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Duration::from_secs(argument.parse().unwrap_or(u64::MAX)))
}

/// The span an `Expires` the origin stated leaves from `received_at`, or `None` when the origin
/// stated no such header.
///
/// An instant already past leaves no span, and so does an expiry that is no HTTP date at all:
/// HTTP reads a date it cannot parse as one already past. The three date formats are the
/// `httpdate` crate's to read.
fn expires_lifetime(headers: &[(String, String)], received_at: SystemTime) -> Option<Duration> {
    let (_, stated) = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(EXPIRES))?;
    Some(
        ::httpdate::parse_http_date(stated.trim())
            .map(|instant| instant.duration_since(received_at).unwrap_or_default())
            .unwrap_or_default(),
    )
}

/// Whether the origin's own headers forbid a shared cache from storing the answer they describe:
/// a `Cache-Control` carrying one of [`FORBID_STORAGE`].
///
/// The value is a list of directives, each of which may state what it applies to after an `=`, so
/// the name is what is read. A header the origin stated more than once states one list too.
pub fn forbids_storage(headers: &[(String, String)]) -> bool {
    directives(headers).any(|(name, _)| {
        FORBID_STORAGE
            .iter()
            .any(|forbidding| name.eq_ignore_ascii_case(forbidding))
    })
}

/// Every directive the origin's `Cache-Control` states, as the directive's name and the argument
/// it states after an `=` — empty for a directive that states none. A header the origin stated
/// more than once states one list.
fn directives<'a>(
    headers: &'a [(String, String)],
) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(CACHE_CONTROL))
        .flat_map(|(_, value)| value.split(','))
        .map(|directive| match directive.split_once('=') {
            Some((name, argument)) => (name.trim(), argument.trim()),
            None => (directive.trim(), ""),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One header set, as an origin answer carries it.
    fn headers(stated: &[(&str, &str)]) -> Vec<(String, String)> {
        stated
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    /// WHICH directives forbid storage: each of the three the behaviour's own documentation names,
    /// spelled bare, beside others, in another case and with an argument, against directives that
    /// forbid nothing, a header that is not a cache directive at all, and no header.
    #[test]
    fn the_directives_that_forbid_storage_are_the_three_the_service_names() {
        // (case, the origin's headers, whether they forbid storage)
        type Stated = (&'static str, &'static [(&'static str, &'static str)], bool);
        let cases: [Stated; 12] = [
            ("no header at all", &[], false),
            (
                "a freshness lifetime",
                &[("cache-control", "max-age=120")],
                false,
            ),
            (
                "a shared freshness lifetime",
                &[("cache-control", "s-maxage=60, public")],
                false,
            ),
            ("no-store", &[("cache-control", "no-store")], true),
            ("no-cache", &[("cache-control", "no-cache")], true),
            ("private", &[("cache-control", "private")], true),
            (
                "no-store beside other directives",
                &[("cache-control", "max-age=0, no-store, must-revalidate")],
                true,
            ),
            (
                "a directive in upper case",
                &[("cache-control", "NO-STORE")],
                true,
            ),
            (
                "a qualified private, which is the directive being present",
                &[("cache-control", "private=\"set-cookie\"")],
                true,
            ),
            (
                "the header name in upper case",
                &[("Cache-Control", "no-store")],
                true,
            ),
            (
                "a header that is not a cache directive",
                &[("x-cache-control-note", "no-store")],
                false,
            ),
            (
                "a directive that merely resembles one",
                &[("cache-control", "no-store-note")],
                false,
            ),
        ];
        for (case, stated, expected) in cases {
            let actual = forbids_storage(&headers(stated));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected forbids_storage == {expected}, got {actual}"
            );
        }
    }

    /// One answer whose residence is being decided, under the spans `ttl`, `min_ttl` and `max_ttl`
    /// state, as of [`RECEIVED_AT`].
    fn held(
        status: u16,
        stated: &[(&str, &str)],
        error_ttl: Option<Duration>,
        spans: (Option<Duration>, Option<Duration>, Option<Duration>),
    ) -> Option<Duration> {
        let (ttl, min_ttl, max_ttl) = spans;
        let stated = headers(stated);
        held_for(&Residence {
            status,
            headers: &stated,
            error_ttl,
            ttl,
            min_ttl,
            max_ttl,
            received_at: RECEIVED_AT,
        })
    }

    /// The instant the answers below reached the edge, which an `Expires` row states its own
    /// instant relative to.
    const RECEIVED_AT: SystemTime = SystemTime::UNIX_EPOCH;

    /// How long an answer is held: the span the configuration states for an answer the origin
    /// admits to a cache, nothing for one it asks no cache to store, and the behaviour's minimum
    /// TTL for such an answer when it states one — which holds CONTENT and never an error.
    #[test]
    fn an_answer_the_origin_asks_no_cache_to_store_is_held_only_by_a_minimum_ttl() {
        let hour = Duration::from_secs(3_600);
        let minute = Duration::from_secs(60);
        // (case, status, the origin's headers, the error TTL, the default/minimum/maximum TTLs,
        //  how long the edge holds it)
        type Row = (
            &'static str,
            u16,
            &'static [(&'static str, &'static str)],
            Option<Duration>,
            (Option<Duration>, Option<Duration>, Option<Duration>),
            Option<Duration>,
        );
        let cases: [Row; 9] = [
            (
                "content the origin stated nothing about",
                200,
                &[],
                None,
                (Some(hour), None, Some(hour)),
                Some(hour),
            ),
            (
                "content under a freshness lifetime, which is the span it is held for",
                200,
                &[("cache-control", "max-age=120")],
                None,
                (Some(hour), None, Some(hour)),
                Some(Duration::from_secs(120)),
            ),
            (
                "content the origin asks no cache to store",
                200,
                &[("cache-control", "no-store")],
                None,
                (Some(hour), None, Some(hour)),
                None,
            ),
            (
                "the same content under a behaviour whose minimum TTL is a span",
                200,
                &[("cache-control", "no-store")],
                None,
                (Some(hour), Some(minute), Some(hour)),
                Some(minute),
            ),
            (
                "content a private directive covers",
                200,
                &[("cache-control", "private")],
                None,
                (Some(hour), None, Some(hour)),
                None,
            ),
            (
                "an error the configuration names",
                502,
                &[],
                Some(hour),
                (None, None, None),
                Some(hour),
            ),
            (
                "an error the configuration names, the origin asking no cache to store it",
                502,
                &[("cache-control", "no-store")],
                Some(hour),
                (None, None, None),
                None,
            ),
            (
                "the same error under a behaviour whose minimum TTL is a span",
                502,
                &[("cache-control", "no-store")],
                Some(hour),
                (Some(hour), Some(minute), Some(hour)),
                None,
            ),
            (
                "an error the configuration does not name",
                502,
                &[],
                None,
                (Some(hour), None, Some(hour)),
                None,
            ),
        ];
        for (case, status, stated, error_ttl, spans, expected) in cases {
            let actual = held(status, stated, error_ttl, spans);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the edge to hold it for {expected:?}, got {actual:?}"
            );
        }
    }

    /// WHICH span holds an answer the origin admits to a shared cache: the lifetime the origin
    /// stated, bounded by the behaviour's minimum and maximum, and the behaviour's default only
    /// where the origin stated no lifetime at all.
    ///
    /// The rows differ along what the rule generalises over: the header the lifetime was stated in
    /// (each of the two `Cache-Control` directives, `Expires`, both at once, none), where the
    /// stated span falls against the behaviour's bounds (inside them, under the minimum, over the
    /// maximum), the form of the value (spelled in another case, with surrounding space, an
    /// argument that is no span, an expiry already past, an expiry that is no date), and a
    /// behaviour that holds nothing at all.
    #[test]
    fn a_stated_freshness_lifetime_is_the_span_the_edge_holds_the_answer_for() {
        let minimum = Duration::from_secs(12);
        let maximum = Duration::from_secs(600);
        let default = Duration::from_secs(3_600);
        let spans = (Some(default), Some(minimum), Some(maximum));
        // An absolute instant 100 seconds past [`RECEIVED_AT`], as `Expires` carries one.
        const EXPIRES_IN_100: &str = "Thu, 01 Jan 1970 00:01:40 GMT";
        // Every span of a behaviour that holds nothing at all.
        let holds_nothing = (None, None, None);
        // (case, the origin's headers, the behaviour's default/minimum/maximum TTLs, how long
        //  the edge holds the answer)
        type Row = (
            &'static str,
            &'static [(&'static str, &'static str)],
            (Option<Duration>, Option<Duration>, Option<Duration>),
            Option<Duration>,
        );
        let cases: [Row; 14] = [
            (
                "no lifetime at all: the default span",
                &[],
                spans,
                Some(default),
            ),
            (
                "a max-age inside the bounds",
                &[("cache-control", "max-age=100")],
                spans,
                Some(Duration::from_secs(100)),
            ),
            (
                "an s-maxage, which a shared cache reads ahead of max-age",
                &[("cache-control", "max-age=100, s-maxage=200")],
                spans,
                Some(Duration::from_secs(200)),
            ),
            (
                "a max-age under the minimum, which floors it",
                &[("cache-control", "max-age=1")],
                spans,
                Some(minimum),
            ),
            (
                "a max-age of no time at all, floored the same way",
                &[("cache-control", "max-age=0")],
                spans,
                Some(minimum),
            ),
            (
                "a max-age over the maximum, which caps it",
                &[("cache-control", "max-age=86400")],
                spans,
                Some(maximum),
            ),
            (
                "a directive spelled in upper case, beside another directive",
                &[("Cache-Control", "public, MAX-AGE=100")],
                spans,
                Some(Duration::from_secs(100)),
            ),
            (
                "an argument around space",
                &[("cache-control", "max-age = 100 ")],
                spans,
                Some(Duration::from_secs(100)),
            ),
            (
                "a max-age whose argument is no span, which states no lifetime",
                &[("cache-control", "max-age=soon")],
                spans,
                Some(default),
            ),
            (
                "an expiry the stated instant leaves a span to",
                &[("expires", EXPIRES_IN_100)],
                spans,
                Some(Duration::from_secs(100)),
            ),
            (
                "a max-age beside an expiry, the directive being the one read",
                &[("cache-control", "max-age=200"), ("expires", EXPIRES_IN_100)],
                spans,
                Some(Duration::from_secs(200)),
            ),
            (
                "an expiry already past, floored at the minimum",
                &[("expires", "Thu, 01 Jan 1970 00:00:00 GMT")],
                spans,
                Some(minimum),
            ),
            (
                "an expiry that is no date at all, read as one already past",
                &[("expires", "0")],
                spans,
                Some(minimum),
            ),
            (
                "a behaviour that holds nothing, under a stated lifetime",
                &[("cache-control", "max-age=100")],
                holds_nothing,
                None,
            ),
        ];
        for (case, stated, spans, expected) in cases {
            let actual = held(200, stated, None, spans);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the edge to hold it for {expected:?}, got {actual:?}"
            );
        }
    }
}
