// SPDX-License-Identifier: CC0-1.0

//! Where the caller a batch of telemetry arrived from is, as RUM reports it on the event.
//!
//! RUM does not only store what a web client sends it: `RumEvent$metadata` is "a JSON
//! serialization of the identity of the user for this session", whose "user information
//! comes from information such as the HTTP user-agent request header", and the model's own
//! extended-metric dimensions name a member of that document no client puts there —
//! `"metadata.countryCode": "CountryCode"`
//! (`smithy_query(query="doc", shape="MetricDefinitionRequest", member="DimensionKeys")`),
//! which is also what `GetAppMonitorData`'s `Country` filter searches
//! (`smithy_query(query="doc", shape="QueryFilter", member="Name")`). So the geolocation of
//! the caller is RUM's own addition to the event's metadata, made once at ingestion
//! ([`super::rum_event::event_document`]): a client that sent only its page's identity is
//! still reported with the location its telemetry came from, and a metric or a query broken
//! down by country has a member to read.
//!
//! Recorded behaviour pins WHICH members RUM adds: the country, the subdivision of that
//! country and the locality, under [`COUNTRY_CODE_MEMBER`], [`SUBDIVISION_CODE_MEMBER`] and
//! [`LOCALITY_NAME_MEMBER`], each a text after the members the client serialized
//! ([`super::rum_event::event_document`] places every member RUM sets at ingestion
//! there). Their VALUES are never written over what the client sent: `metadata` is the
//! client's own document, so a member of one of those names the client did report is
//! reported with the value it arrived with.
//!
//! # Where the values come from
//!
//! The real service resolves them from the ADDRESS the batch arrived from, through a
//! geolocation database. Neither half of that is reachable here: a request this emulator
//! serves comes from this machine (an in-process dispatch, or a loopback connection to the
//! emulator's own endpoint), which is an address no geolocation database places, and no
//! database is consulted off this machine. So the location reported is the ONE location
//! this emulator observes for such a caller ([`OBSERVED_CALLER_LOCATION`]) — a real place,
//! spelled the way the recorded members spell theirs (an ISO 3166-1 alpha-2 country code,
//! the ISO 3166-2 subdivision code within that country without its country prefix, and the
//! locality's name) — and it is the same for every caller, which is what a consumer
//! grouping two events by country or region reads.

/// The metadata member carrying the country the caller's telemetry came from: an ISO 3166-1
/// alpha-2 country code, which is the spelling the model's `Country` filter values and the
/// `CountryCode` dimension are compared against.
pub const COUNTRY_CODE_MEMBER: &str = "countryCode";

/// The metadata member carrying the subdivision of that country: the ISO 3166-2 code of the
/// subdivision, without the country prefix the standard joins it to the country with.
pub const SUBDIVISION_CODE_MEMBER: &str = "subdivisionCode";

/// The metadata member carrying the name of the locality the caller is in.
pub const LOCALITY_NAME_MEMBER: &str = "localityName";

/// One caller location, as the three members RUM adds to an event's metadata report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallerLocation {
    /// [`COUNTRY_CODE_MEMBER`].
    pub country_code: &'static str,
    /// [`SUBDIVISION_CODE_MEMBER`].
    pub subdivision_code: &'static str,
    /// [`LOCALITY_NAME_MEMBER`].
    pub locality_name: &'static str,
}

impl CallerLocation {
    /// This location as the `(member, value)` pairs the enrichment adds, in the order a
    /// RUM event document carries them: the country, then its subdivision, then the
    /// locality — widest first, as the recorded document orders them.
    pub fn members(&self) -> [(&'static str, &'static str); 3] {
        [
            (COUNTRY_CODE_MEMBER, self.country_code),
            (SUBDIVISION_CODE_MEMBER, self.subdivision_code),
            (LOCALITY_NAME_MEMBER, self.locality_name),
        ]
    }
}

/// The location this emulator observes for the caller of every delivered batch: see this
/// module's header for why there is exactly one.
///
/// This emulator's own value — no recorded run's — and a real place: the United States
/// (`US`), the state of Washington (`WA`) and the city of Seattle.
pub const OBSERVED_CALLER_LOCATION: CallerLocation = CallerLocation {
    country_code: "US",
    subdivision_code: "WA",
    locality_name: "Seattle",
};
