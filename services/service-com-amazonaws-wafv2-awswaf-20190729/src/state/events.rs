// SPDX-License-Identifier: CC0-1.0

//! The API SECURITY events WAF detected in the traffic a web ACL inspected, the
//! clients ("event actors") that produced them, and how a listing selects, orders and
//! pages over them.
//!
//! # What an event is
//!
//! An event is not a resource a caller creates: it is what WAF made of the requests a
//! web ACL inspected. `ListEvents` "retrieves a paginated list of event summaries for
//! your account. Use this operation to monitor and investigate potential threats to the
//! resources that your web ACLs protect", and `EventDetailSummary$ApiSecurityEventDetail`
//! carries the detection itself: the label WAF assigned, its threat category, its
//! confidence, the API patterns the event involves and the actors that produced it. So
//! the records here are written by the DATA plane, where the requests are seen
//! ([`crate::service::api_security`]), and every operation over them is a report
//! computed from what is stored.
//!
//! # Which traffic WAF analyses
//!
//! `WebACL$AnalyticsConfig` is the switch: "security analytics shows the web ACL's
//! request log data … When you enable security analytics, you authorize Amazon Web
//! Services to transfer and store full request logs outside the Amazon Web Services
//! Region where the web ACL is hosted" for "aggregation and processing", and
//! `AnalyticsMode` states what each value does — `CROSS_REGION` "enables security
//! analytics", `DISABLED` "disables security analytics. WAF stops transferring the web
//! ACL's request logs". The events are what that processing produces, so a web ACL whose
//! analytics is disabled contributes none ([`analytics_enabled`]), and the member's own
//! documented defaults decide for a web ACL that configures nothing: "for regional web
//! ACLs, security analytics is disabled by default when you omit this configuration. For
//! web ACLs that protect CloudFront distributions (global web ACLs), security analytics
//! is enabled by default."
//!
//! # What is reported, and what cannot be
//!
//! An actor's `ClientIp` and `AuthFingerprint` are read off the request. Its `Asn`,
//! `AsnOrganization` and `Country` are not: those need the ASN and geolocation databases
//! this emulator holds none of — the same reason `HTTPRequest$Country` is absent from a
//! sampled request — so they are left out rather than invented, and an actor's
//! `TlsFingerprint` likewise, because a request reaching this process in-process
//! completed no TLS handshake of its own to fingerprint.

use super::*;
use crate::ops::types::{
    ApiPattern, ApiPatternFilter, ApiSecurityEventDetail, AnalyticsConfig, EventActorChange,
    EventActorDetail, EventActorDetailSummary, EventActorReference, EventActorSummary,
    EventDetailSummary, EventFilter, EventTimelineEntry, RelatedEvent,
};

/// The namespace of every label WAF's API security detection assigns, from the
/// example `ApiSecurityEventDetail$Label` publishes for one:
/// `awswaf:managed:aws:api-security:credential-stuffing-high`.
pub const API_SECURITY_LABEL_NAMESPACE: &str = "awswaf:managed:aws:api-security:";

/// The detection segment of that same published label: the one API security threat
/// this emulator recognises in the traffic it inspects
/// ([`crate::service::api_security`]).
pub const CREDENTIAL_STUFFING_DETECTION: &str = "credential-stuffing";

/// The `EventCategory` every event of this detection carries: the model declares one
/// value (`smithy_query(query="shape",shape="EventCategory")`), and `ListEvents$Filters`
/// says the same — "EVENT_CATEGORY … the only supported filter value is API_SECURITY".
pub const EVENT_CATEGORY_API_SECURITY: &str = "API_SECURITY";

/// The `ApiSecurityEventDetail$Category` a credential-stuffing event carries: "the API
/// security threat category of the event. For example, `API2:2023`" — the OWASP API
/// Security Top 10 entry for broken authentication, which is the threat class an attack
/// that tries stolen credentials against an API belongs to.
pub const BROKEN_AUTHENTICATION_CATEGORY: &str = "API2:2023";

/// `AnalyticsMode$CROSS_REGION`: the value that "enables security analytics".
pub const ANALYTICS_MODE_CROSS_REGION: &str = "CROSS_REGION";

/// `AnalyticsMode$DISABLED`: the value that "disables security analytics".
pub const ANALYTICS_MODE_DISABLED: &str = "DISABLED";

/// `EventStatus$EVENT_STARTED`: the event has been reported and nothing has been
/// observed on it since.
pub const EVENT_STATUS_STARTED: &str = "EVENT_STARTED";

/// `EventStatus$EVENT_IN_PROGRESS`: the traffic the event is about is still arriving.
pub const EVENT_STATUS_IN_PROGRESS: &str = "EVENT_IN_PROGRESS";

/// `EventStatus$EVENT_ENDED`: the traffic stopped, which is the status that populates
/// `EventDetailSummary$EndTime` ("the time when the event ended. This is not populated
/// while the event is ongoing").
///
/// `EventStatus` declares a fourth value, `EVENT_COMPLETED`. Nothing documents a
/// transition into it that is distinguishable from this one by what WAF observes of the
/// traffic, so no event is reported under it rather than moved there on a span this side
/// invented.
pub const EVENT_STATUS_ENDED: &str = "EVENT_ENDED";

/// `Confidence$LOW`, `$MEDIUM` and `$HIGH`: "the confidence level of the event
/// detection" (`smithy_query(query="shape",shape="Confidence")`).
pub const CONFIDENCE_LOW: &str = "LOW";
/// See [`CONFIDENCE_LOW`].
pub const CONFIDENCE_MEDIUM: &str = "MEDIUM";
/// See [`CONFIDENCE_LOW`].
pub const CONFIDENCE_HIGH: &str = "HIGH";

/// How many distinct credential sets one protected resource has to have been offered
/// before the detection reports a credential-stuffing event, and the `Confidence` each
/// count carries, strongest first.
///
/// An EMULATOR CHOICE, not an AWS quota: no AWS document states the counts WAF's own
/// detection fires at. What the counts have to satisfy is the shape of the detection —
/// one credential set offered to an API is a sign-in, many distinct ones offered to the
/// same API is the attack, and the more of them the stronger the finding — so the
/// confidence rises with the number of distinct credential sets and the lowest entry is
/// the floor below which nothing is reported at all.
pub const CREDENTIAL_STUFFING_CONFIDENCE: &[(u64, &str)] = &[
    (10, CONFIDENCE_HIGH),
    (5, CONFIDENCE_MEDIUM),
    (3, CONFIDENCE_LOW),
];

/// The floor of [`CREDENTIAL_STUFFING_CONFIDENCE`]: below this many distinct credential
/// sets, the traffic is recorded but no event is reported for it.
pub const CREDENTIAL_STUFFING_FLOOR: u64 = 3;

/// How long after its last observed request an event is reported as ENDED.
///
/// An EMULATOR CHOICE: `EventDetailSummary$EndTime` is "the time when the event ended.
/// This is not populated while the event is ongoing", so an event ends when its traffic
/// stops, and nothing documents how long WAF waits before calling it stopped. The span
/// is read against the event's last observation at REPORT time — a transition the clock
/// alone makes due, owed at the same instant whether or not anybody asked — so no read
/// moves a status that a later caller would then see differently.
pub const EVENT_IDLE_SPAN_MILLIS: i64 = 5 * 60 * 1_000;

/// The longest `TimeWindow` an events listing accepts: "the time window can't be longer
/// than one year" (`ListEvents$TimeWindow`, `ListEventsByWebACL$TimeWindow` and
/// `ListEventActors$TimeWindow` each state it).
pub const MAX_EVENT_WINDOW_MILLIS: i64 = 365 * 24 * 60 * 60 * 1_000;

/// The window a listing that names none reports over: "if you don't specify a time
/// window, WAF returns events from the past year" (`ListEvents$TimeWindow`).
pub const DEFAULT_EVENT_WINDOW_MILLIS: i64 = MAX_EVENT_WINDOW_MILLIS;

/// The largest page an events listing answers with when the request names no `Limit`,
/// and the bound a named one has to lie in: "valid values are 1 to 20"
/// (`ListEvents$Limit`, whose `EventPaginationLimit` is `range 1..20`).
pub const MAX_EVENT_PAGE_SIZE: i32 = 20;

/// How many API patterns one event reports: "the list contains up to 20 API patterns"
/// (`ApiSecurityEventDetail$ApiPatterns`, `length 0..20`).
pub const MAX_EVENT_API_PATTERNS: usize = 20;

/// How many actors one event reports: "the list contains up to 20 event actors"
/// (`ApiSecurityEventDetail$EventActors`, `length 0..20`).
pub const MAX_EVENT_ACTORS: usize = 20;

/// How many timeline entries one event reports: "the list contains up to 32 entries"
/// (`ApiSecurityEventDetail$Timeline`, `length 0..32`).
pub const MAX_EVENT_TIMELINE_ENTRIES: usize = 32;

/// How many related events one actor reports: "the list contains up to 20 events"
/// (`EventActorDetail$Events`, `length 0..20`).
pub const MAX_ACTOR_RELATED_EVENTS: usize = 20;

/// How many attribute changes one actor reports: "the list contains up to 20 entries"
/// (`EventActorDetail$Changes`, `length 0..20`).
pub const MAX_ACTOR_CHANGES: usize = 20;

/// How many event records one web ACL holds at a time.
///
/// An EMULATOR resource guard, NOT an AWS quota, and one the TRAFFIC chooses: a record
/// is opened per protected resource the web ACL decides requests for, so without a cap
/// the store would grow with the traffic rather than with the configuration. The figure
/// is the longest list an events report can answer with
/// (`EventDetailSummaryList`, `length 0..100`), so no reachable record is dropped from a
/// page; the record whose last observation is oldest is the one evicted to make room.
pub const MAX_EVENT_RECORDS_PER_WEB_ACL: usize = 100;

/// How many actors one listing answers with at most: "the operation returns up to the
/// top 100 event actors by event count" (`ListEventActors`, whose
/// `EventActorSummaryList` is `length 0..100`).
pub const MAX_LISTED_EVENT_ACTORS: usize = 100;

/// How many actors one scope holds at a time.
///
/// An EMULATOR resource guard, NOT an AWS quota, and one the TRAFFIC chooses: an actor is
/// opened per distinct credential set the analysed traffic offers, so without a cap the
/// store would grow with that traffic rather than with the configuration. The actor whose
/// last observation is oldest is the one released to make room, and it is released from
/// the events that named it too, so no event names an actor a report cannot resolve.
pub const MAX_EVENT_ACTORS_PER_SCOPE: usize = 1_000;

/// One API pattern the detection observed, as `ApiPattern` reports it.
///
/// `ApiPattern` is "an API pattern to filter events by" and carries the method, the
/// normalized URI path, the port and the domains; the four together are what makes two
/// requests the same pattern, so they are the record's key as well as its report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ApiPatternKey {
    /// `ApiPattern$Method`: "the HTTP method that you want the filter to match. For
    /// example, `GET` or `POST`."
    pub method: String,
    /// `ApiPattern$PathPattern`: "the normalized URI path … For example,
    /// `/api/v1/accounts/{id}`" — see [`normalized_path_pattern`].
    pub path_pattern: String,
    /// `ApiPattern$Port`: "the port that you want the filter to match. For example,
    /// `443`." Zero for a request that named none, which is also the member's own
    /// `@default`.
    pub port: i32,
    /// `ApiPattern$Domains`: "the domains that you want the filter to match" — the host
    /// the request addressed, and empty for a request that addressed none.
    pub domains: Vec<String>,
}

impl ApiPatternKey {
    /// This pattern as the modelled shape a report answers with.
    pub fn reported(&self) -> ApiPattern {
        ApiPattern {
            method: Some(self.method.clone()),
            path_pattern: Some(self.path_pattern.clone()),
            port: self.port,
            domains: Some(self.domains.clone()),
        }
    }
}

/// The URI path `path` as `ApiPattern$PathPattern` reports it: "the normalized URI path
/// … For example, `/api/v1/accounts/{id}`".
///
/// What the published example normalizes is the part of the path that names ONE
/// resource — an account id — while the parts that name the API itself (`api`, `v1`,
/// `accounts`) are kept as written. So a segment that is entirely digits, or that spells
/// a UUID the way every `EntityId` of this model does
/// (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`), is an identifier and becomes
/// [`PATH_IDENTIFIER_PLACEHOLDER`]; every other segment is left alone, which is why a
/// version segment like `v1` survives although it carries a digit.
pub fn normalized_path_pattern(path: &str) -> String {
    let normalized: Vec<&str> = path
        .split('/')
        .map(|segment| {
            if is_path_identifier(segment) {
                PATH_IDENTIFIER_PLACEHOLDER
            } else {
                segment
            }
        })
        .collect();
    normalized.join("/")
}

/// What one path segment that names a single resource is reported as, from the
/// `ApiPattern$PathPattern` example `/api/v1/accounts/{id}`.
pub const PATH_IDENTIFIER_PLACEHOLDER: &str = "{id}";

/// Whether one path segment names a single resource rather than part of the API's own
/// shape. See [`normalized_path_pattern`].
fn is_path_identifier(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    if segment.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    is_uuid_shaped(segment)
}

/// Whether `value` spells a lowercase hyphenated UUID, the form
/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$` pins for every `EntityId` of this
/// model ([`new_uuid`] mints it).
pub fn is_uuid_shaped(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = value.split('-');
    for expected in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != expected
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// What WAF has observed of one potential API security event: the traffic of one
/// protected resource behind one web ACL, and the detection it has reached about it.
///
/// A record exists from the FIRST credential the resource was offered, and becomes an
/// EVENT — acquiring the `EventId` a report names it by — once the detection fires
/// ([`CREDENTIAL_STUFFING_CONFIDENCE`]). A record that has not reached that floor is not
/// an event and no report mentions it, which is why the id is optional rather than
/// minted up front: an event id is the handle `GetEvent` resolves, so one exists exactly
/// when there is an event to resolve.
#[derive(Debug, Clone)]
pub struct ApiSecurityEventRecord {
    /// The `Scope` of the web ACL whose traffic this is: an event is reported in the
    /// namespace its web ACL lives in.
    pub scope: String,
    /// The `Id` of that web ACL.
    pub web_acl_id: String,
    /// `EventDetailSummary$WebAclArn`: "the ARN of the web ACL that the event is
    /// associated with".
    pub web_acl_arn: String,
    /// `EventDetailSummary$ResourceArn`: "the ARN of the protected resource that the
    /// event is associated with", and `None` for a web ACL a resource named itself
    /// rather than one put in front of it by an association.
    pub resource_arn: Option<String>,
    /// The API patterns the traffic has been spread over, oldest first and bounded at
    /// [`MAX_EVENT_API_PATTERNS`].
    pub api_patterns: Vec<ApiPatternKey>,
    /// The ids of the actors the traffic came from, oldest first and bounded at
    /// [`MAX_EVENT_ACTORS`].
    pub actors: Vec<String>,
    /// How many DISTINCT credential sets the resource has been offered, which is what
    /// the detection is judged on. Counted past [`MAX_EVENT_ACTORS`], because the bound
    /// on the reported actor list is a bound on the REPORT and not on what was seen.
    pub credentials_observed: u64,
    /// `EventDetailSummary$StartTime`: "the time when the event started" — the first
    /// request of this traffic, in milliseconds since the Unix epoch.
    pub first_observed_millis: i64,
    /// `EventDetailSummary$LastUpdatedTime`: "the time when the event was last updated"
    /// — the newest request of this traffic.
    pub last_observed_millis: i64,
    /// `EventDetailSummary$EventId`, once the detection has fired; `None` while the
    /// traffic is below the floor.
    pub event_id: Option<String>,
    /// How many requests have been observed since the detection fired, which is what
    /// tells [`EVENT_STATUS_STARTED`] from [`EVENT_STATUS_IN_PROGRESS`].
    pub observed_since_reported: u64,
    /// `ApiSecurityEventDetail$Timeline`: "the changes to the event's label, confidence,
    /// and API patterns over time, ordered from oldest to newest", bounded at
    /// [`MAX_EVENT_TIMELINE_ENTRIES`].
    pub timeline: Vec<EventTimelineEntry>,
}

impl ApiSecurityEventRecord {
    /// The `Confidence` the number of distinct credential sets seen amounts to, or
    /// `None` while that number is below [`CREDENTIAL_STUFFING_FLOOR`] — in which case
    /// there is no detection and so no event.
    pub fn confidence(&self) -> Option<&'static str> {
        CREDENTIAL_STUFFING_CONFIDENCE
            .iter()
            .find(|(floor, _)| self.credentials_observed >= *floor)
            .map(|(_, confidence)| *confidence)
    }

    /// `ApiSecurityEventDetail$Label`: the fully qualified label of this detection at
    /// its current confidence, in the form the member's own example publishes
    /// ([`API_SECURITY_LABEL_NAMESPACE`]).
    pub fn label(&self) -> Option<String> {
        self.confidence().map(|confidence| {
            format!(
                "{API_SECURITY_LABEL_NAMESPACE}{CREDENTIAL_STUFFING_DETECTION}-{}",
                confidence.to_ascii_lowercase()
            )
        })
    }

    /// `EventDetailSummary$EventStatus` at the instant `now`.
    ///
    /// The status is DERIVED and not stored: an event whose traffic has stopped has
    /// ended whether or not anybody asked, so it is read off the record's last
    /// observation against [`EVENT_IDLE_SPAN_MILLIS`] rather than written by a report.
    pub fn status(&self, now: i64) -> &'static str {
        if now.saturating_sub(self.last_observed_millis) >= EVENT_IDLE_SPAN_MILLIS {
            EVENT_STATUS_ENDED
        } else if self.observed_since_reported > 0 {
            EVENT_STATUS_IN_PROGRESS
        } else {
            EVENT_STATUS_STARTED
        }
    }

    /// `EventDetailSummary$EndTime` at the instant `now`: the last request the event saw
    /// once the event has ended, and nothing while it is ongoing ("this is not populated
    /// while the event is ongoing").
    pub fn end_millis(&self, now: i64) -> Option<i64> {
        (self.status(now) == EVENT_STATUS_ENDED).then_some(self.last_observed_millis)
    }

    /// Whether this record is an EVENT: the detection has fired, so a report names it.
    pub fn reported(&self) -> bool {
        self.event_id.is_some()
    }

    /// Whether the event's own span overlaps the window `(from, to)`, which is what a
    /// listing's `TimeWindow` — "the time range for the events to return" — selects on:
    /// an event that was active at any instant of the window falls in it.
    pub fn within(&self, from: i64, to: i64) -> bool {
        self.first_observed_millis <= to && self.last_observed_millis >= from
    }

    /// This event as `EventDetailSummary` reports it, at the instant `now`.
    ///
    /// `include_timeline` carries the request's own `Include` setting: the timeline "is
    /// returned only if you specify `TIMELINE` in the request `Include` setting"
    /// (`ApiSecurityEventDetail$Timeline`), while `GetEvent` — which "retrieves the
    /// details of the specified event … and the event timeline" — always has it.
    pub fn summary(&self, now: i64, include_timeline: bool) -> EventDetailSummary {
        EventDetailSummary {
            event_id: self.event_id.clone().unwrap_or_default(),
            event_status: Some(self.status(now).to_string()),
            event_category: Some(EVENT_CATEGORY_API_SECURITY.to_string()),
            start_time: Some(::aws_smithy_types::DateTime::from_millis(
                self.first_observed_millis,
            )),
            last_updated_time: Some(::aws_smithy_types::DateTime::from_millis(
                self.last_observed_millis,
            )),
            end_time: self
                .end_millis(now)
                .map(::aws_smithy_types::DateTime::from_millis),
            resource_arn: self.resource_arn.clone(),
            web_acl_arn: Some(self.web_acl_arn.clone()),
            api_security_event_detail: Some(ApiSecurityEventDetail {
                label: self.label(),
                category: Some(BROKEN_AUTHENTICATION_CATEGORY.to_string()),
                confidence: self.confidence().map(str::to_string),
                api_patterns: Some(
                    self.api_patterns
                        .iter()
                        .map(ApiPatternKey::reported)
                        .collect(),
                ),
                event_actors: Some(
                    self.actors
                        .iter()
                        .map(|actor_id| EventActorReference {
                            event_actor_id: Some(actor_id.clone()),
                            // `EventActorReference$EventActorValue` is the attribute the
                            // actor was recognised BY, which is the fingerprint of the
                            // credentials it offered: resolved by the caller of this
                            // projection, which holds the actor records.
                            event_actor_value: None,
                        })
                        .collect(),
                ),
                timeline: include_timeline.then(|| self.timeline.clone()),
            }),
        }
    }

    /// This event as one entry of an actor's `Events` list: "an API security event that
    /// an event actor is associated with" (`RelatedEvent`).
    pub fn related(&self) -> RelatedEvent {
        RelatedEvent {
            event_id: self.event_id.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from_millis(
                self.first_observed_millis,
            )),
            label: self.label(),
            category: Some(BROKEN_AUTHENTICATION_CATEGORY.to_string()),
            confidence: self.confidence().map(str::to_string),
            api_patterns: Some(
                self.api_patterns
                    .iter()
                    .map(ApiPatternKey::reported)
                    .collect(),
            ),
        }
    }
}

/// One client the detection has seen, as `EventActorSummary` and
/// `EventActorDetailSummary` report it.
///
/// `EventActorSummary$AuthFingerprint` is "a hash of the authentication credentials that
/// the client of the event actor provides in the request … you can use the fingerprint
/// to match the same client across events and in your WAF logs", so the fingerprint IS
/// the actor's identity and its network attributes are what may change under it — which
/// is exactly what `EventActorChange` records ("a change to the network attributes of
/// the event actor").
#[derive(Debug, Clone)]
pub struct EventActorRecord {
    /// `EventActorSummary$EventActorId`: "the unique identifier of the event actor. You
    /// provide this to `GetEventActor`".
    pub event_actor_id: String,
    /// The hash of the credentials this client offers, which is what recognises it.
    pub auth_fingerprint: String,
    /// `EventActorSummary$ClientIp`: "the client IP address of the event actor", as last
    /// observed.
    pub client_ip: String,
    /// `EventActorSummary$CreatedAt`: "the time when WAF first observed the event
    /// actor", in milliseconds since the Unix epoch.
    pub created_millis: i64,
    /// When the actor was last observed, which orders two actors of equal event count.
    pub last_observed_millis: i64,
    /// `EventActorDetail$Changes`: "the history of changes to the network attributes of
    /// the event actor, ordered from oldest to newest", bounded at
    /// [`MAX_ACTOR_CHANGES`].
    pub changes: Vec<EventActorChange>,
}

impl EventActorRecord {
    /// The actor's latest observed attributes, shared by its listing summary and its
    /// detail ("the latest observed attributes of the event actor",
    /// `EventActorDetail$Summary`).
    ///
    /// `TlsFingerprint`, `Asn`, `AsnOrganization` and `Country` are left out for the
    /// reason this module's own documentation gives: nothing here can observe them.
    pub fn detail_summary(&self) -> EventActorDetailSummary {
        EventActorDetailSummary {
            event_actor_id: self.event_actor_id.clone(),
            client_ip: Some(self.client_ip.clone()),
            tls_fingerprint: None,
            asn: None,
            asn_organization: None,
            country: None,
            auth_fingerprint: Some(self.auth_fingerprint.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from_millis(
                self.created_millis,
            )),
        }
    }

    /// The actor as a listing reports it, with the `EventCount` of the window the
    /// listing asked about: "the number of API security events that the event actor is
    /// associated with in the requested time window".
    pub fn summary(&self, event_count: i32) -> EventActorSummary {
        let latest = self.detail_summary();
        EventActorSummary {
            event_actor_id: latest.event_actor_id,
            client_ip: latest.client_ip,
            tls_fingerprint: latest.tls_fingerprint,
            asn: latest.asn,
            asn_organization: latest.asn_organization,
            country: latest.country,
            auth_fingerprint: latest.auth_fingerprint,
            created_at: latest.created_at,
            event_count,
        }
    }
}

/// Whether WAF analyses the request logs of a web ACL in `scope` carrying
/// `analytics_config`.
///
/// `WebACL$AnalyticsConfig`'s own doc states both the switch and the defaults: security
/// analytics is what transfers and processes the web ACL's request logs, "for regional
/// web ACLs, security analytics is disabled by default when you omit this
/// configuration", and "for web ACLs that protect CloudFront distributions (global web
/// ACLs), security analytics is enabled by default". So an absent configuration is read
/// as the default of its namespace, and a present one as the mode it names.
pub fn analytics_enabled(scope: &str, analytics_config: Option<&AnalyticsConfig>) -> bool {
    match analytics_config {
        Some(config) => config.mode == ANALYTICS_MODE_CROSS_REGION,
        None => scope == SCOPE_CLOUDFRONT,
    }
}

/// The `AnalyticsConfig` a saved web ACL in `scope` carries when the save named none:
/// the documented default of its namespace ([`analytics_enabled`]).
pub fn default_analytics_config(scope: &str) -> AnalyticsConfig {
    AnalyticsConfig {
        mode: if scope == SCOPE_CLOUDFRONT {
            ANALYTICS_MODE_CROSS_REGION.to_string()
        } else {
            ANALYTICS_MODE_DISABLED.to_string()
        },
    }
}

/// How many entries one page of an EVENTS listing holds, for a request whose `Limit` is
/// `limit`.
///
/// [`page_size`] for the narrower bound these two listings declare: "valid values are 1
/// to 20" (`EventPaginationLimit`, `range 1..20`), rather than the `PaginationLimit`
/// range every other paginated operation of this service shares. A `Limit` outside it is
/// REFUSED rather than clamped, for the same reason.
pub fn event_page_size(limit: Option<i32>) -> Result<usize, WAFInvalidParameterException> {
    let requested = match limit {
        None => MAX_EVENT_PAGE_SIZE,
        Some(limit) if (1..=MAX_EVENT_PAGE_SIZE).contains(&limit) => limit,
        Some(limit) => {
            return Err(invalid_parameter_exception(
                FIELD_ENTITY_LIMIT,
                &limit.to_string(),
                &format!("a page holds between 1 and {MAX_EVENT_PAGE_SIZE} event summaries"),
            ));
        }
    };
    Ok(usize::try_from(requested).expect("a page size in 1..=MAX_EVENT_PAGE_SIZE fits a usize"))
}

/// A `TimeWindow` as the pair of epoch-millisecond instants this module compares
/// against, with the end never preceding the start.
pub fn window_millis(window: &TimeWindow) -> (i64, i64) {
    let from = date_time_millis(&window.start_time);
    let to = date_time_millis(&window.end_time);
    (from, to.max(from))
}

/// How a refusal names the `TimeWindow` the request wrote, as the value a caller
/// corrects: the shape's name and its two instants, the way a `MonetizeAction` is named
/// in the same member ([`monetize_action_parameter`]).
pub fn time_window_parameter(window: &TimeWindow) -> String {
    let (from, to) = window_millis(window);
    format!("TimeWindow(startTime={from},endTime={to})")
}

/// The window a listing reports over, or the refusal an unusable one earns.
///
/// Two readings, both documented on the member: a request that names no window reports
/// over [`DEFAULT_EVENT_WINDOW_MILLIS`] ending at the call instant ("if you don't specify
/// a time window, WAF returns events from the past year"), and a window longer than
/// [`MAX_EVENT_WINDOW_MILLIS`] is refused ("the time window can't be longer than one
/// year") rather than narrowed — the request asked about a range WAF does not keep, which
/// is a parameter the caller corrects.
pub fn reported_window(
    window: Option<&TimeWindow>,
    now: i64,
) -> Result<(i64, i64), WAFInvalidParameterException> {
    let Some(window) = window else {
        return Ok((now.saturating_sub(DEFAULT_EVENT_WINDOW_MILLIS), now));
    };
    let (from, to) = window_millis(window);
    if to.saturating_sub(from) > MAX_EVENT_WINDOW_MILLIS {
        return Err(invalid_parameter_exception_without_field(
            &time_window_parameter(window),
            PARAMETER_VALUE_NOT_SUPPORTED_REASON,
        ));
    }
    Ok((from, to))
}

/// Where in `ordered` the page a request's `NextMarker` names begins.
///
/// `NextMarker` is the id of the last entry of the previous page, so the next page
/// starts at the entry after it — and "the marker is valid only for a request with the
/// same `TimeWindow`, `Filters`, `SortBy`, and `SortOrder` settings as the request that
/// returned it" (`ListEvents$NextMarker`): a marker naming no entry of THIS sequence was
/// produced by other settings, so it is refused as the parameter it is rather than
/// silently answering the first page again.
pub fn page_start(
    ordered: &[String],
    marker: Option<&str>,
) -> Result<usize, WAFInvalidParameterException> {
    let Some(marker) = marker else {
        return Ok(0);
    };
    match ordered.iter().position(|id| id == marker) {
        Some(position) => Ok(position + 1),
        None => Err(invalid_parameter_exception_without_field(
            marker,
            FORMAT_NOT_VALID_REASON,
        )),
    }
}

/// Order two events as a listing's `SortBy`/`SortOrder` asks for.
///
/// `EventSortBy` declares the three fields WAF sorts by — `START_TIME`, `END_TIME` and
/// `CONFIDENCE` — and `SortOrder` the direction, whose default is stated on the member:
/// "DESC - Sort in descending order. This is the default". Two events the named field
/// leaves equal are ordered by their ids, so a page boundary names one position however
/// the traffic arrived.
pub fn sorted_event_ids(
    events: &[&ApiSecurityEventRecord],
    sort_by: Option<&str>,
    sort_order: Option<&str>,
    now: i64,
) -> Vec<String> {
    let mut ordered: Vec<&&ApiSecurityEventRecord> = events.iter().collect();
    ordered.sort_by(|left, right| {
        let keyed = match sort_by {
            Some(EVENT_SORT_BY_END_TIME) => left
                .end_millis(now)
                .unwrap_or(i64::MAX)
                .cmp(&right.end_millis(now).unwrap_or(i64::MAX)),
            Some(EVENT_SORT_BY_CONFIDENCE) => confidence_rank(left.confidence())
                .cmp(&confidence_rank(right.confidence())),
            // `START_TIME`, and the field a request that names none is ordered by: the
            // instant an event began is the one every event has.
            _ => left
                .first_observed_millis
                .cmp(&right.first_observed_millis),
        };
        let keyed = match sort_order {
            Some(SORT_ORDER_ASC) => keyed,
            // "DESC … This is the default" (`ListEvents$SortOrder`).
            _ => keyed.reverse(),
        };
        keyed.then_with(|| left.event_id.cmp(&right.event_id))
    });
    ordered
        .into_iter()
        .filter_map(|record| record.event_id.clone())
        .collect()
}

/// `EventSortBy$START_TIME`, `$END_TIME` and `$CONFIDENCE`
/// (`smithy_query(query="shape",shape="EventSortBy")`).
pub const EVENT_SORT_BY_START_TIME: &str = "START_TIME";
/// See [`EVENT_SORT_BY_START_TIME`].
pub const EVENT_SORT_BY_END_TIME: &str = "END_TIME";
/// See [`EVENT_SORT_BY_START_TIME`].
pub const EVENT_SORT_BY_CONFIDENCE: &str = "CONFIDENCE";

/// `SortOrder`'s ascending value, the one direction an events listing has to tell apart:
/// its default is the descending one ([`SORT_ORDER_DESC`], which this module's parent
/// already declares), so every order that is not this one is that
/// (`smithy_query(query="shape",shape="SortOrder")` → `ASC`, `DESC`).
pub const SORT_ORDER_ASC: &str = "ASC";

/// `EventActorSortBy$EVENT_COUNT`: "the only supported value"
/// (`ListEventActors$SortBy`).
pub const EVENT_ACTOR_SORT_BY_EVENT_COUNT: &str = "EVENT_COUNT";

/// `EventFilterKey$EVENT_CATEGORY` and `$RESOURCE_ARN`: the two fields
/// `ListEvents$Filters` narrows by ("filters that narrow the results by event category
/// or by the Amazon Resource Name (ARN) of the protected resource").
pub const EVENT_FILTER_EVENT_CATEGORY: &str = "EVENT_CATEGORY";
/// See [`EVENT_FILTER_EVENT_CATEGORY`].
pub const EVENT_FILTER_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// `EventIncludeField$TIMELINE`: the one additional field an events listing returns on
/// request ("specify `TIMELINE` to return the timeline of each event").
pub const EVENT_INCLUDE_TIMELINE: &str = "TIMELINE";

/// Where a `Confidence` sits on the scale the enum spells out, weakest first, so two
/// events can be ordered by it ([`sorted_event_ids`]).
fn confidence_rank(confidence: Option<&str>) -> u8 {
    match confidence {
        Some(CONFIDENCE_HIGH) => 3,
        Some(CONFIDENCE_MEDIUM) => 2,
        Some(CONFIDENCE_LOW) => 1,
        _ => 0,
    }
}

/// What an events listing asks for.
///
/// `ListEvents` and `ListEventsByWebACL` are ONE report under two addresses — "to
/// retrieve the events for a single web ACL, use `ListEventsByWebACL`" — so they ask it
/// through this one shape, which differs only in whether a web ACL narrows it, and
/// neither handler holds a second reading of a window, a filter, an order or a page.
#[derive(Debug, Default)]
pub struct EventsQuery<'q> {
    /// The namespace the events are reported in.
    pub scope: &'q str,
    /// The web ACL the report is narrowed to, for the listing that names one.
    pub web_acl_arn: Option<&'q str>,
    /// `TimeWindow`: "the time range for the events to return".
    pub window: Option<&'q TimeWindow>,
    /// `Filters`: "filters that narrow the results by event category or by the Amazon
    /// Resource Name (ARN) of the protected resource".
    pub filters: Option<&'q [EventFilter]>,
    /// `ApiPatternFilters`: "filters that narrow the results to events that involve the
    /// specified API patterns".
    pub api_pattern_filters: Option<&'q [ApiPatternFilter]>,
    /// `Limit`: how many summaries one page holds.
    pub limit: Option<i32>,
    /// `NextMarker`: where the page begins.
    pub next_marker: Option<&'q str>,
    /// `SortBy`: "the field that WAF sorts the events by".
    pub sort_by: Option<&'q str>,
    /// `SortOrder`: "the order in which WAF sorts the events".
    pub sort_order: Option<&'q str>,
    /// `Include`: "additional fields to return with each event summary".
    pub include: Option<&'q [String]>,
}

/// Whether the event `record` satisfies every `EventFilter` the request carries.
///
/// Each filter names ONE field and the values to accept for it — "you can specify up to
/// 10 values" — so an event satisfies a filter when that field is one of the filter's
/// values, and satisfies the request when it satisfies every filter the request wrote:
/// the filters narrow by different fields ("by event category or by the ARN of the
/// protected resource"), so they compose rather than alternate. A filter naming a field
/// outside `EventFilterKey` cannot reach here: the member is a modelled enum.
fn matches_event_filters(record: &ApiSecurityEventRecord, filters: &[EventFilter]) -> bool {
    filters.iter().all(|filter| match filter.name.as_str() {
        EVENT_FILTER_RESOURCE_ARN => record
            .resource_arn
            .as_deref()
            .is_some_and(|arn| filter.values.iter().any(|value| value == arn)),
        EVENT_FILTER_EVENT_CATEGORY => filter
            .values
            .iter()
            .any(|value| value == EVENT_CATEGORY_API_SECURITY),
        _ => true,
    })
}

/// Whether the event `record` involves an API pattern any `ApiPatternFilter` of the
/// request matches.
///
/// "If you specify more than one API pattern filter, WAF returns the events that match
/// any of the filters", so the filters ALTERNATE; a filter matches a pattern when every
/// member it STATES matches that pattern, and a member it leaves out (or leaves at the
/// `Port` member's own zero default, or as an empty domain list) narrows nothing.
fn matches_api_pattern_filters(
    record: &ApiSecurityEventRecord,
    filters: &[ApiPatternFilter],
) -> bool {
    filters.iter().any(|filter| {
        record.api_patterns.iter().any(|pattern| {
            filter
                .method
                .as_deref()
                .is_none_or(|method| method.eq_ignore_ascii_case(&pattern.method))
                && filter
                    .path_pattern
                    .as_deref()
                    .is_none_or(|path| path == pattern.path_pattern)
                && (filter.port == 0 || filter.port == pattern.port)
                && filter.domains.as_deref().is_none_or(|domains| {
                    domains.is_empty()
                        || domains
                            .iter()
                            .any(|domain| pattern.domains.iter().any(|held| held == domain))
                })
        })
    })
}

impl ComAmazonawsWafv2Awswaf20190729State {
    /// Whether this store holds a web ACL under `arn` in the namespace `scope`.
    ///
    /// What a well-formed ARN naming no web ACL of THIS scope resolves to is nothing:
    /// an ARN that resolves to another resource kind is not a web ACL ARN, and a web
    /// ACL of the other scope is a different resource. The two operations that address
    /// an events report by web ACL ARN both ask it, so the resolution is one reading.
    pub fn holds_web_acl_arn(&self, scope: &str, arn: &str) -> bool {
        match self.arn_index.get(arn) {
            Some(ResourceRef::WebAcl {
                scope: held,
                id,
            }) => held == scope && self.web_acls.contains_key(&(held.clone(), id.clone())),
            Some(ResourceRef::IpSet { .. })
            | Some(ResourceRef::RegexPatternSet { .. })
            | Some(ResourceRef::RuleGroup { .. })
            | None => false,
        }
    }

    /// Every event this scope holds: the records the detection has fired for, in no
    /// particular order (a listing orders them itself, [`sorted_event_ids`]).
    pub fn api_security_events_in(&self, scope: &str) -> Vec<&ApiSecurityEventRecord> {
        self.api_security_events
            .values()
            .filter(|record| record.scope == scope && record.reported())
            .collect()
    }

    /// The event `event_id` names in `scope`, or `None` when the scope holds none under
    /// that id.
    pub fn api_security_event(
        &self,
        scope: &str,
        event_id: &str,
    ) -> Option<&ApiSecurityEventRecord> {
        self.api_security_events
            .values()
            .find(|record| {
                record.scope == scope && record.event_id.as_deref() == Some(event_id)
            })
    }

    /// How many events of `scope` each actor is associated with inside the window
    /// `(from, to)`: `EventActorSummary$EventCount`, for every actor that is associated
    /// with at least one.
    ///
    /// ONE pass over the scope's events rather than a pass per actor: how many actors a
    /// scope holds and how many events they appear in are both chosen by the TRAFFIC, so
    /// a report whose cost were their product would grow with the traffic that came
    /// before it rather than with the answer it gives.
    pub fn actor_event_counts(
        &self,
        scope: &str,
        from: i64,
        to: i64,
    ) -> ::std::collections::BTreeMap<String, usize> {
        let mut counts: ::std::collections::BTreeMap<String, usize> =
            ::std::collections::BTreeMap::new();
        for record in self
            .api_security_events_in(scope)
            .into_iter()
            .filter(|record| record.within(from, to))
        {
            for actor_id in &record.actors {
                *counts.entry(actor_id.clone()).or_default() += 1;
            }
        }
        counts
    }

    /// The events of `scope` the actor `event_actor_id` is associated with, newest
    /// first and bounded at [`MAX_ACTOR_RELATED_EVENTS`] ("the list contains up to 20
    /// events").
    pub fn actor_related_events(&self, scope: &str, event_actor_id: &str) -> Vec<RelatedEvent> {
        let mut associated: Vec<&ApiSecurityEventRecord> = self
            .api_security_events_in(scope)
            .into_iter()
            .filter(|record| record.actors.iter().any(|actor| actor == event_actor_id))
            .collect();
        associated.sort_by(|left, right| {
            right
                .first_observed_millis
                .cmp(&left.first_observed_millis)
                .then_with(|| left.event_id.cmp(&right.event_id))
        });
        associated
            .into_iter()
            .take(MAX_ACTOR_RELATED_EVENTS)
            .map(ApiSecurityEventRecord::related)
            .collect()
    }

    /// The actor `event_actor_id` names in `scope`, whole, as `GetEventActor` answers
    /// it: its latest attributes, the events it is associated with and the history of
    /// its network attributes.
    pub fn event_actor_detail(
        &self,
        scope: &str,
        event_actor_id: &str,
    ) -> Option<EventActorDetail> {
        let record = self
            .event_actors
            .get(&(scope.to_string(), event_actor_id.to_string()))?;
        Some(EventActorDetail {
            summary: record.detail_summary(),
            events: Some(self.actor_related_events(scope, event_actor_id)),
            changes: Some(record.changes.clone()),
        })
    }

    /// The fingerprint of the credentials the actor `event_actor_id` offered, which is
    /// the attribute an `EventActorReference` names it by
    /// (`EventActorReference$EventActorValue`).
    pub fn event_actor_value(&self, scope: &str, event_actor_id: &str) -> Option<String> {
        self.event_actors
            .get(&(scope.to_string(), event_actor_id.to_string()))
            .map(|record| record.auth_fingerprint.clone())
    }

    /// One event as every report of it answers it: [`ApiSecurityEventRecord::summary`]
    /// with each `EventActorReference` resolved to the value its actor was recognised
    /// by, which only the store holding the actor records can fill in.
    pub fn event_summary(
        &self,
        record: &ApiSecurityEventRecord,
        now: i64,
        include_timeline: bool,
    ) -> EventDetailSummary {
        let mut summary = record.summary(now, include_timeline);
        if let Some(detail) = summary.api_security_event_detail.as_mut() {
            for actor in detail.event_actors.iter_mut().flatten() {
                let Some(actor_id) = actor.event_actor_id.clone() else {
                    continue;
                };
                actor.event_actor_value = self.event_actor_value(&record.scope, &actor_id);
            }
        }
        summary
    }

    /// One page of an events listing, and the `NextMarker` that resumes after it.
    ///
    /// The ONE reading both listings share ([`EventsQuery`]), in the order the members
    /// are documented in: the window the report covers ([`reported_window`]), the
    /// filters that narrow it, the order WAF sorts it in ([`sorted_event_ids`]), and the
    /// page the `Limit` and the `NextMarker` name. Every refusal is the modelled
    /// `WAFInvalidParameterException`, which is what each of these members is corrected
    /// by.
    ///
    /// WAF answers every non-empty page with a `NextMarker` — the id of the LAST summary
    /// on the page even when the page is not full, so a client resumes from it and learns
    /// it reached the end from the empty page that follows.
    pub fn events_page(
        &self,
        query: &EventsQuery<'_>,
        now: i64,
    ) -> Result<(Vec<EventDetailSummary>, Option<String>), WAFInvalidParameterException> {
        let (from, to) = reported_window(query.window, now)?;
        let page_size = event_page_size(query.limit)?;
        let selected: Vec<&ApiSecurityEventRecord> = self
            .api_security_events_in(query.scope)
            .into_iter()
            .filter(|record| {
                query
                    .web_acl_arn
                    .is_none_or(|arn| record.web_acl_arn == arn)
            })
            .filter(|record| record.within(from, to))
            .filter(|record| {
                query
                    .filters
                    .is_none_or(|filters| matches_event_filters(record, filters))
            })
            .filter(|record| {
                query
                    .api_pattern_filters
                    .is_none_or(|filters| matches_api_pattern_filters(record, filters))
            })
            .collect();
        let ordered = sorted_event_ids(&selected, query.sort_by, query.sort_order, now);
        let start = page_start(&ordered, query.next_marker)?;
        let page: Vec<&String> = ordered.iter().skip(start).take(page_size).collect();
        // "Specify `TIMELINE` to return the timeline of each event"
        // (`ListEvents$Include`); a listing that asks for nothing answers "only the
        // default summary fields".
        let include_timeline = query.include.is_some_and(|include| {
            include
                .iter()
                .any(|field| field == EVENT_INCLUDE_TIMELINE)
        });
        let next_marker = page.last().map(|event_id| (*event_id).clone());
        let events = page
            .into_iter()
            .filter_map(|event_id| self.api_security_event(query.scope, event_id))
            .map(|record| self.event_summary(record, now, include_timeline))
            .collect();
        Ok((events, next_marker))
    }

    /// One page of an event-actor listing, and the `NextMarker` that resumes after it.
    ///
    /// "Retrieves a paginated list of the event actors in your account that are
    /// associated with events in the specified time window … The operation returns up to
    /// the top 100 event actors by event count" (`ListEventActors`): an actor appears
    /// when it is associated with at least one event of the window, carries that count
    /// as its `EventCount`, and the order is that count, strongest first — the one field
    /// `EventActorSortBy` declares. Two actors of equal count are ordered by when WAF
    /// first observed them and then by their ids, so a page boundary names one position.
    pub fn event_actors_page(
        &self,
        scope: &str,
        window: &TimeWindow,
        limit: Option<i32>,
        next_marker: Option<&str>,
    ) -> Result<(Vec<EventActorSummary>, Option<String>), WAFInvalidParameterException> {
        let (from, to) = reported_window(Some(window), now_millis())?;
        let page_size = page_size(limit, "event actor summaries")?;
        let counts = self.actor_event_counts(scope, from, to);
        let mut counted: Vec<(&EventActorRecord, usize)> = counts
            .iter()
            .filter_map(|(actor_id, count)| {
                self.event_actors
                    .get(&(scope.to_string(), actor_id.clone()))
                    .map(|record| (record, *count))
            })
            .collect();
        counted.sort_by(|(left, left_count), (right, right_count)| {
            right_count
                .cmp(left_count)
                .then_with(|| left.created_millis.cmp(&right.created_millis))
                .then_with(|| left.event_actor_id.cmp(&right.event_actor_id))
        });
        counted.truncate(MAX_LISTED_EVENT_ACTORS);
        let ordered: Vec<String> = counted
            .iter()
            .map(|(record, _)| record.event_actor_id.clone())
            .collect();
        let start = page_start(&ordered, next_marker)?;
        let page: Vec<&(&EventActorRecord, usize)> =
            counted.iter().skip(start).take(page_size).collect();
        let next_marker = page.last().map(|(record, _)| record.event_actor_id.clone());
        let event_actors = page
            .into_iter()
            .map(|(record, count)| {
                record.summary(i32::try_from(*count).unwrap_or(i32::MAX))
            })
            .collect();
        Ok((event_actors, next_marker))
    }
}
