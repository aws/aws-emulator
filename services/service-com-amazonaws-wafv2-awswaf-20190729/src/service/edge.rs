// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 DATA plane: how a web ACL in front of a protected resource decides one
//! web request.
//!
//! A web ACL is not a document that only gets read back — it is "a set of rules that
//! defines the strategy that WAF uses to inspect and handle web requests"
//! (`WebACL`), so the resource it is associated with answers every request with that
//! strategy applied. This module is that evaluation, and it is the ONE place it
//! happens: the protected resource's own service (an API Gateway stage, say) asks
//! [`decide`] with what the request carries, and renders the answer it gets back.
//! Nothing about a web ACL, a rule group, an IP set or a pattern set changes because
//! a request was evaluated; what the evaluation DOES write is the per-request
//! bookkeeping WAF reports about its own data plane — the sample
//! `GetSampledRequests` draws from, and the rate counts
//! `GetRateBasedStatementManagedKeys` reads the currently rate-limited keys out of.
//! Neither can be computed without recording the requests already evaluated. It also
//! REPORTS the request where the web ACL's own configuration says to: the counters of
//! every dimension that acted ([`super::metrics`]) and the log record its
//! `LoggingConfiguration` delivers ([`super::logs`]).
//!
//! # The order a web ACL decides in
//!
//! 1. The web ACL's rules run in ascending `Priority` (`Rule$Priority`: "WAF
//!    processes rules with lower priority first").
//! 2. A rule whose statement MATCHES adds its `RuleLabels` to the request, under the
//!    web ACL's own label namespace, and then applies its `Action`. `Block`, `Allow`,
//!    `Captcha` and `Challenge` are TERMINATING — they end the evaluation — while
//!    `Count` records the match and lets the next rule run. EITHER WAY the request
//!    enters the sample population of that rule's own dimension, under the
//!    `ActionValue` the rule applied, because the match is what a sample is of. A
//!    label a rule adds is visible to every rule that runs after it, which is what a
//!    `LabelMatchStatement` matches against ("labels that have been added to the web
//!    request by rules that have already run in the web ACL").
//! 3. A rule whose statement runs a rule GROUP — a `ManagedRuleGroupStatement` or a
//!    `RuleGroupReferenceStatement` — runs that group's rules in the group's own
//!    order, each with its own action, and carries the group's
//!    `ScopeDownStatement`, `ExcludedRules`, `RuleActionOverrides` and the web ACL
//!    rule's `OverrideAction`.
//! 4. If no rule terminated, the web ACL's `DefaultAction` decides —
//!    "the action to perform if none of the Rules contained in the WebACL match".
//!
//! # What a decision is on the wire
//!
//! * `Allow` (and the default `Allow`) lets the request through to the origin,
//!   carrying the headers every `CustomRequestHandling` on the way inserted. WAF
//!   prefixes each one with [`INSERTED_HEADER_PREFIX`]
//!   (`CustomHTTPHeader$Name`: "for the header name `sample`, WAF inserts the header
//!   `x-amzn-waf-sample`"), so the origin can tell them from the headers the client
//!   sent.
//! * `Block` is answered at the EDGE: the origin is never reached. With a
//!   `CustomResponse` the caller gets that response's `ResponseCode`, its
//!   `ResponseHeaders` and the web ACL's `CustomResponseBodies` entry its
//!   `CustomResponseBodyKey` names; with none, the protected resource's own service
//!   renders its forbidden answer ([`EdgeDecision::custom_response`] is `None`).
//! * `Captcha` and `Challenge` are terminating too, and a request carrying no
//!   solved token is answered with the status WAF serves the puzzle under
//!   ([`CAPTCHA_STATUS`], [`CHALLENGE_STATUS`]).
//!
//! # What this module does not inspect, and what it answers instead
//!
//! A statement whose criterion is data this emulator holds none of is left UNMET
//! rather than guessed at — a `GeoMatchStatement` (no geolocation database), an
//! `AsnMatchStatement` (no routing table), and a `JA3Fingerprint`/`JA4Fingerprint`
//! or `UriFragment` field (a TLS fingerprint and a URL fragment never reach a
//! server) — and where the model gives such a field a `FallbackBehavior`, that
//! setting is what decides, because it is the caller's own instruction for exactly
//! this case. A managed rule group whose vendor publishes its rule names and
//! actions but not the inspection each rule performs
//! ([`crate::state::MANAGED_RULE_GROUP_CATALOGUE`]) contributes the rules whose
//! inspection the catalogue CAN state and no others: a rule nobody published the
//! criterion of matches nothing rather than matching by guesswork. That is also why
//! a published rule whose criterion is a LITERAL ADDRESS or host the rule hunts for
//! in a request is not among them: this emulator's source states no such address.

use std::collections::HashMap;
use std::net::IpAddr;

use super::sqli;
use crate::ops::types::*;
use crate::state::{
    ComAmazonawsWafv2Awswaf20190729State, ManagedRuleGroupDescriptor, WebAclEntry,
};

/// The prefix WAF puts on every header a `CustomRequestHandling` inserts, from
/// `CustomHTTPHeader$Name`: "when WAF inserts the header into the request, it
/// prefixes this name `x-amzn-waf-`, to avoid confusion with the headers that are
/// already in the request".
pub const INSERTED_HEADER_PREFIX: &str = "x-amzn-waf-";

/// How much of a request BODY WAF inspects at an API Gateway stage when the web ACL
/// raises the limit for no resource type.
///
/// `Body$OversizeHandling` states it: "for CloudFront, API Gateway, Amazon Cognito,
/// App Runner, and Verified Access, the default limit is 16 KB (16,384 bytes), and
/// you can increase the limit for each resource type in the web ACL
/// `AssociationConfig`". A documented limit, so it keeps the documented value.
pub const DEFAULT_BODY_INSPECTION_LIMIT: usize = 16 * 1024;

/// How much of a request's HEADERS WAF inspects, and how many of them: "WAF does not
/// support inspecting the entire contents of request headers when they exceed 8 KB
/// (8192 bytes) or 200 total headers" (`Headers$OversizeHandling`).
pub const HEADER_INSPECTION_LIMIT: usize = 8 * 1024;

/// See [`HEADER_INSPECTION_LIMIT`].
pub const MAX_INSPECTED_HEADERS: usize = 200;

/// The status WAF serves a CAPTCHA puzzle under for a request that carries no solved
/// token: `RuleAction$Captcha` "runs a CAPTCHA check against the web request", and
/// WAF answers such a request with `405` and the CAPTCHA interstitial rather than
/// passing it to the origin.
pub const CAPTCHA_STATUS: u16 = 405;

/// The status WAF serves a silent challenge under, the `Challenge` counterpart of
/// [`CAPTCHA_STATUS`]: WAF answers with `202` and the challenge script.
pub const CHALLENGE_STATUS: u16 = 202;

/// The most requests one web ACL's sample population holds: WAF draws a sample "from
/// among the first 5,000 requests that your Amazon Web Services resource received
/// during the time range" (`GetSampledRequests`), so the log a sample is drawn from
/// is bounded at that figure and the oldest records fall off it.
pub const SAMPLE_POPULATION_CAP: usize = 5_000;

/// How many requests one rate-based aggregation instance was sent in one SECOND.
///
/// The unit the rate counts are kept in: a count per second, oldest first, rather than
/// one entry per request. A rate-based rule only has to tell "at or below the `Limit`"
/// from "over it" across its `EvaluationWindowSec`, and the window is one of four
/// documented values, so the buckets one instance holds are bounded by the longest of
/// them while the COUNT inside each is not bounded at all — which is what keeps a large
/// `Limit` reachable (see [`record_request`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateBucket {
    /// The second the requests arrived in, as whole seconds since the Unix epoch.
    pub second: i64,
    /// How many arrived in it.
    pub requests: u64,
}

/// The most rate-based aggregation instances one rate-based rule keeps counts for.
///
/// An emulator resource guard, NOT an AWS quota, and the one the REQUEST chooses: a
/// rule aggregating on a forwarded address or a header value gets another instance for
/// every distinct value a caller sends, so without a cap the counts would grow with the
/// traffic rather than with the configuration. See [`prune_rate_instances`] for which
/// instance is evicted to make room.
pub const MAX_RATE_INSTANCES_PER_RULE: usize = 4_096;

/// The address WAF treats as the origin of a request that reached this emulator
/// IN PROCESS.
///
/// A request served here never crossed a socket from a remote peer — the emulator's
/// front door hands the whole request to the claiming service — so there is no
/// connection address to report. A request that traversed a proxy still states where
/// it came from, in its forwarded-for header, and that is what
/// [`InspectedRequest::client_ip`] carries when the request has one; with no such
/// header the request came from this machine, which is this address.
pub const IN_PROCESS_CLIENT_IP: &str = "127.0.0.1";

/// The header a request that traversed a proxy reports its origin address in, and the
/// header a `ForwardedIPConfig` names by default ("commonly, this is the
/// X-Forwarded-For (XFF) header").
pub const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The web request WAF inspects, as the protected resource's own service read it off
/// the wire.
///
/// Every field a `FieldToMatch` can name is here, so the evaluation reads the request
/// rather than the wire: the service that owns the resource knows how to read its own
/// request, and this module knows what a web ACL does with one.
#[derive(Debug, Clone, Default)]
pub struct InspectedRequest {
    /// The HTTP method, as `FieldToMatch$Method` inspects it.
    pub method: String,
    /// The path, as `FieldToMatch$UriPath` inspects it ("the part of the URL that
    /// identifies a resource, for example, /images/daily-ad.jpg").
    pub uri_path: String,
    /// The query string WITHOUT its leading `?`, as `FieldToMatch$QueryString`
    /// inspects it.
    pub query_string: String,
    /// The headers in the order they arrived, with the names as the client sent
    /// them; a repeated header appears once per value.
    pub headers: Vec<(String, String)>,
    /// The body as it arrived. How much of it WAF inspects is the web ACL's business
    /// ([`DEFAULT_BODY_INSPECTION_LIMIT`]), not the caller's, so the whole body is
    /// handed over and the truncation happens here.
    pub body: Vec<u8>,
    /// The address this request came from; see [`IN_PROCESS_CLIENT_IP`].
    pub client_ip: String,
}

impl InspectedRequest {
    /// One inbound HTTP request as WAF inspects it — the ONE reading of it, shared by
    /// every protected resource's own service, so no two of them can differ on what a
    /// `FieldToMatch` names.
    ///
    /// `uri_path` and `query_string` are the caller's, not this function's: the path a
    /// web ACL's criteria are written against is the path a client addressing AWS would
    /// have sent, which the service that owns the resource resolves (the path within an
    /// api's stage, within a load balancer's own endpoint), and the emulator's own
    /// addressing prefix is how the request reached this process rather than part of it.
    /// The query string is WITHOUT its leading `?`, as `FieldToMatch$QueryString` states
    /// it, and `body` is the payload already read off the request, so a rule inspecting
    /// `FieldToMatch$Body` and the caller's own reading of it see the SAME bytes.
    pub fn of_http(
        req: &::aws_smithy_runtime_api::http::Request,
        uri_path: &str,
        query_string: &str,
        body: Vec<u8>,
    ) -> Self {
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        Self::of_parts(req.method(), uri_path, query_string, headers, body)
    }

    /// The same reading, for a protected resource's service that read the request off a
    /// SOCKET of its own rather than through this emulator's front door — an endpoint served
    /// on a core-owned listener, where the request arrives as its own method, target and
    /// header list rather than as an already-routed request.
    ///
    /// The fields mean exactly what [`Self::of_http`] states they do, and the address the
    /// request came from is derived the one way ([`client_ip_of`]), so the two callers cannot
    /// differ on what a `FieldToMatch` names.
    pub fn of_parts(
        method: &str,
        uri_path: &str,
        query_string: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            method: method.to_string(),
            uri_path: uri_path.to_string(),
            query_string: query_string.to_string(),
            client_ip: client_ip_of(&headers),
            headers,
            body,
        }
    }

    /// The first value of the header `name` (compared case-insensitively, as HTTP
    /// header names are), or `None` when the request carries none.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// `HTTPRequest$URI`: the path with the query string the client sent, which is the
    /// request line's target.
    fn request_uri(&self) -> String {
        if self.query_string.is_empty() {
            self.uri_path.clone()
        } else {
            format!("{}?{}", self.uri_path, self.query_string)
        }
    }

    /// The request as `GetSampledRequests` reports it.
    ///
    /// A sample is HELD in the store until the population rolls over, so what it holds
    /// is bounded in BYTES as well as in count: the headers are the ones the host
    /// service would have forwarded — "a maximum of 200 headers and at most 8 KB of
    /// header contents" (`Headers$OversizeHandling`) — so the walk stops at whichever
    /// of those two it reaches first, and the reported URI is clamped to the same byte
    /// budget. Without the byte bound a request choosing its own header lengths would
    /// decide how much memory one stored sample costs.
    fn sampled(&self) -> HTTPRequest {
        let mut held = 0_usize;
        let mut headers: Vec<HTTPHeader> = Vec::new();
        for (name, value) in self.headers.iter().take(MAX_INSPECTED_HEADERS) {
            held = held.saturating_add(name.len()).saturating_add(value.len());
            if held > HEADER_INSPECTION_LIMIT {
                break;
            }
            headers.push(HTTPHeader {
                name: Some(name.clone()),
                value: Some(value.clone()),
            });
        }
        HTTPRequest {
            client_ip: Some(self.client_ip.clone()),
            // `HTTPRequest$Country` is "the two-letter country code for the country
            // that the request originated in", which needs a geolocation database
            // this emulator holds none of; see this module's own docs.
            country: None,
            uri: Some(clamped(&self.request_uri(), HEADER_INSPECTION_LIMIT)),
            method: Some(clamped(&self.method, MAX_SAMPLED_METHOD_BYTES)),
            http_version: None,
            headers: Some(headers),
        }
    }
}

/// The address one inbound request came from, as every protected resource's service reads
/// it: the first address its forwarded-for header states, and [`IN_PROCESS_CLIENT_IP`] for a
/// request that states none.
///
/// A request served in process crossed no socket from a remote peer, so there is no
/// connection address; one that traversed a proxy still states where it came from, in its
/// forwarded-for header. THE one reading of it, so no two callers can differ on the address
/// an `IPSetReferenceStatement` or a rate-based rule aggregates on.
pub fn client_ip_of(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(FORWARDED_FOR_HEADER))
        .and_then(|(_, value)| value.split(',').next())
        .map(str::trim)
        .filter(|address| address.parse::<IpAddr>().is_ok())
        .unwrap_or(IN_PROCESS_CLIENT_IP)
        .to_string()
}

/// `value` cut to at most `budget` bytes, on a character boundary.
///
/// What a value HELD in the store is cut to: the request chooses its length, the store
/// keeps it until the population rolls over, so the length one sample costs has to be
/// this side's choice rather than the caller's.
fn clamped(value: &str, budget: usize) -> String {
    if value.len() <= budget {
        return value.to_string();
    }
    let mut end = budget;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// Emulator resource guard, NOT an AWS quota: the longest method name a stored sample
/// holds. A method is a token; anything longer is a caller choosing an allocation.
const MAX_SAMPLED_METHOD_BYTES: usize = 64;

/// Emulator resource guard, NOT an AWS quota: the longest one component of a rate-based
/// `CUSTOM_KEYS` aggregation may be before it becomes part of the stored key.
///
/// A component is a header value, a cookie, a query argument or a path the REQUEST
/// chose, and the key built from it is held until the instance's evaluation window
/// empties, so the length one held instance costs is this side's choice. See
/// [`Evaluation::aggregate_key`].
const MAX_AGGREGATE_KEY_COMPONENT_BYTES: usize = 256;

/// The terminating action a web ACL settled on for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeAction {
    /// The request goes to the origin (`RuleAction$Allow`, `DefaultAction$Allow`).
    Allow,
    /// The request is refused at the edge (`RuleAction$Block`,
    /// `DefaultAction$Block`).
    Block,
    /// The request is answered with a CAPTCHA puzzle (`RuleAction$Captcha`).
    Captcha,
    /// The request is answered with a silent challenge (`RuleAction$Challenge`).
    Challenge,
}

impl EdgeAction {
    /// The `ActionValue` a sample reports this action under
    /// (`SampledHTTPRequest$Action`), and a log record its `action` by.
    pub fn action_value(self) -> &'static str {
        match self {
            Self::Allow => "ALLOW",
            Self::Block => "BLOCK",
            Self::Captcha => "CAPTCHA",
            Self::Challenge => "CHALLENGE",
        }
    }

    /// Whether this action lets the request reach the origin.
    pub fn is_allow(self) -> bool {
        matches!(self, Self::Allow)
    }

    /// The status WAF serves a terminating refusal under, or `None` for an action
    /// that serves no response of its own because the origin answers.
    pub fn default_status(self) -> Option<u16> {
        match self {
            Self::Allow => None,
            // A `Block` with no `CustomResponse` is answered by the protected
            // resource's own service, which words its own forbidden answer: WAF
            // serves `403` for it, and a service with a configurable refusal (API
            // Gateway's `WAF_FILTERED` gateway response) renders that instead.
            Self::Block => Some(403),
            Self::Captcha => Some(CAPTCHA_STATUS),
            Self::Challenge => Some(CHALLENGE_STATUS),
        }
    }
}

/// The response a terminating action serves at the edge, built from the action's own
/// `CustomResponse` and the web ACL's `CustomResponseBodies`.
#[derive(Debug, Clone)]
pub struct EdgeResponse {
    /// `CustomResponse$ResponseCode`: "the HTTP status code to return to the client".
    pub status: u16,
    /// The media type of [`Self::body`], from the `CustomResponseBody$ContentType`
    /// of the named body, or `None` when the action names no body.
    pub content_type: Option<String>,
    /// `CustomResponseBody$Content`: the body WAF serves, empty when the action names
    /// none.
    pub body: Vec<u8>,
    /// `CustomResponse$ResponseHeaders`, in the order they were configured.
    pub headers: Vec<(String, String)>,
}

/// What a web ACL decided about one request.
#[derive(Debug, Clone)]
pub struct EdgeDecision {
    /// The terminating action.
    pub action: EdgeAction,
    /// The headers every `CustomRequestHandling` on the way inserted, already
    /// carrying [`INSERTED_HEADER_PREFIX`]. Empty unless the request was allowed,
    /// counted or challenged through an action that configured some.
    pub inserted_headers: Vec<(String, String)>,
    /// The response a terminating refusal serves, when the action configured a
    /// `CustomResponse`; `None` when the protected resource's own service words the
    /// refusal.
    pub custom_response: Option<EdgeResponse>,
    /// The web ACL rule that decided, or `None` when the `DefaultAction` did.
    pub rule_name: Option<String>,
    /// The rule WITHIN a rule group that decided, as
    /// `SampledHTTPRequest$RuleNameWithinRuleGroup` reports it.
    pub rule_name_within_rule_group: Option<String>,
    /// The fully qualified labels the request carries once evaluation finished.
    pub labels: Vec<String>,
    /// What the web ACL's `LoggingConfiguration` logs about this request, and where it
    /// goes ([`super::logs`]); `None` when the web ACL logs nothing about it.
    ///
    /// The DELIVERY is the protected resource's own service's to make, once it has
    /// answered the request: `super::logs::deliver` writes it, over the in-process call
    /// that write is. A record left undelivered is a request a configured web ACL
    /// enforced without logging, which is what this member exists to prevent.
    pub log_record: Option<super::logs::LoggedRecord>,
}

/// One request a web ACL evaluated, as `GetSampledRequests` reports it.
///
/// The sample is drawn per RULE DIMENSION — `GetSampledRequests$RuleMetricName` is
/// "the metric name assigned to the Rule or RuleGroup dimension for which you want a
/// sample of requests" — so each record carries the dimension it belongs to beside
/// the modelled sample and the instant it was evaluated at.
#[derive(Debug, Clone)]
pub struct SampledRequestRecord {
    /// When the request was evaluated, in milliseconds since the Unix epoch: a
    /// `TimeWindow` selects the records that fall inside it.
    pub observed_at_millis: i64,
    /// The `MetricName` of the rule (or of the web ACL, for a request the
    /// `DefaultAction` decided) this record is a sample of.
    pub metric_name: String,
    /// Exactly what `GetSampledRequests` answers for this request.
    pub sample: SampledHTTPRequest,
}

/// What the evaluation of ONE request recorded about WAF's own reporting of it.
///
/// A `VisibilityConfig` carries two independent switches, so one walk over the rules
/// produces two records of the same evaluation: the sample population
/// `GetSampledRequests` draws from, kept for a dimension whose
/// `SampledRequestsEnabled` asks for it, and the CloudWatch counters, sent for a
/// dimension whose `CloudWatchMetricsEnabled` asks for them. They are collected
/// together because they are recorded at the same points — wherever a dimension ACTED
/// on the request — and written apart because they live in different stores.
#[derive(Debug, Default)]
struct Recorded {
    /// One record per rule dimension that is keeping samples.
    samples: Vec<SampledRequestRecord>,
    /// One entry per rule dimension that is sending metrics, and the action it
    /// applied.
    acted: Vec<super::metrics::ActedDimension>,
    /// One entry per rule that MATCHED the request without ending the evaluation, and
    /// the action it applied: the rule as a record names it, and
    /// [`COUNT_ACTION_VALUE`] or [`EXCLUDED_AS_COUNT_ACTION_VALUE`]. Unlike the two
    /// above, this is kept whatever the dimension's `VisibilityConfig` says, because
    /// it is part of the LOG RECORD of the request ([`super::logs`]) rather than of
    /// WAF's own reporting, and a logging configuration is enabled on the web ACL
    /// rather than per rule.
    non_terminating: Vec<(String, String)>,
}

/// The instants one rate-based aggregation instance has been seen at.
///
/// "WAF monitors web requests and manages keys independently for each unique
/// combination of web ACL, optional rule group, and rate-based rule", and within one
/// rule the counts are kept per aggregation instance — "each unique set of values for
/// the aggregation keys that you specify is a separate aggregation instance" — so the
/// map is keyed by all four.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateInstanceKey {
    /// The scope the web ACL lives in.
    pub scope: String,
    /// The web ACL's id.
    pub web_acl_id: String,
    /// The rate-based rule, as [`rule_path`] names it: the web ACL rule's name, or
    /// the rule-group reference's name and the group rule's name.
    pub rule_path: String,
    /// The aggregation instance: the aggregate keys' values, joined.
    pub aggregate_key: String,
}

/// The rule a rate-based count belongs to: a rule of the web ACL itself, or a rule of
/// a rule group the web ACL references through `referrer`.
pub fn rule_path(referrer: Option<&str>, rule_name: &str) -> String {
    match referrer {
        Some(referrer) => format!("{referrer}/{rule_name}"),
        None => rule_name.to_string(),
    }
}

/// The separator the segments of a group-qualified rule name are joined with; see
/// [`group_rule_name`].
const RULE_NAME_QUALIFIER_SEPARATOR: &str = "#";

/// The name `SampledHTTPRequest$RuleNameWithinRuleGroup` reports for a rule that
/// decided INSIDE a rule group: the segments that name the GROUP, then the rule's
/// group-local name.
///
/// The member states the form — "For managed rule groups, the format for this name is
/// `<vendor name>#<managed rule group name>#<rule name>`. For your own rule groups, the
/// format for this name is `<rule group name>#<rule name>`. If the rule is not in a
/// rule group, this field is absent." So the qualification is whatever names the group
/// the rule lives in (a managed group's vendor and its name, a customer-owned group's
/// name), the group-local name is the last segment, and a rule of the web ACL itself
/// lives in no group and so has no name here at all.
///
/// A segment nothing named is left out rather than joined as an empty one: the value is
/// read by a client that splits it, so a separator with nothing in front of it would
/// name a group that does not exist.
fn group_rule_name(group: &[&str], rule_name: &str) -> String {
    group
        .iter()
        .copied()
        .filter(|segment| !segment.is_empty())
        .chain(::std::iter::once(rule_name))
        .collect::<Vec<&str>>()
        .join(RULE_NAME_QUALIFIER_SEPARATOR)
}

/// Decide one request against the web ACL in front of `resource_arn`, and record what
/// WAF records about its own data plane.
///
/// `None` means no web ACL is associated with that resource, so the resource answers
/// the request as it would with no WAF in front of it at all. `Some` is the decision,
/// and by the time it is returned the request is in the web ACL's sample population
/// (for every rule dimension whose `VisibilityConfig` enables sampling), in the rate
/// counts of every rate-based rule that counted it, and counted into the CloudWatch
/// series of every dimension whose `VisibilityConfig` enables metrics
/// ([`super::metrics`]).
///
/// It is given the shared ENGINE rather than this service's store alone, because what
/// WAF reports about one request spans two stores: its own bookkeeping, and the
/// CloudWatch series a client reads the counters back from. One entry point writes
/// both, so no protected resource's service can serve traffic that is enforced but
/// never reported.
///
/// FIVE phases, and no two stores are ever locked at once. Inspecting a request is work
/// the web ACL's stored rules decide the cost of — a regular expression to compile, a
/// body to scan — so it must not run while every other request to this service waits on
/// the store: the records the evaluation needs are copied out under one shared read, the
/// evaluation runs with the lock released, the bookkeeping it produced is written under a
/// second, exclusive one, and the counters and the log record are published once that lock
/// is gone.
pub fn decide<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_arn: &str,
    request: &InspectedRequest,
) -> Option<EdgeDecision> {
    let state: ::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State> =
        crate::state::service_state(engine)?;
    // Which web ACL is in front of the resource, read under its own shared lock and
    // released again: the evaluation below takes the lock itself, in the five phases its
    // own documentation describes.
    let web_acl_arn = {
        let store = state.lock();
        store.web_acl_associations.get(resource_arn)?.clone()
    };
    decided(engine, &web_acl_arn, Some(resource_arn), request)
}

/// Decide one request against the web ACL `web_acl_arn` names, whichever resource put it
/// in front of itself, and record what WAF records about its own data plane.
///
/// The same evaluation [`decide`] performs, for a protected resource whose association
/// with a web ACL is held by the RESOURCE rather than by `AssociateWebACL`: a CloudFront
/// distribution names its web ACL in its own configuration — "for Amazon CloudFront, don't
/// use `AssociateWebACL`. Instead, use your CloudFront distribution configuration" — so the
/// resource's own service resolves the web ACL and asks about it here.
///
/// `None` when this store holds no web ACL under that ARN, so the resource answers the
/// request as it would with no WAF in front of it at all.
pub fn decide_against<Eng: Send + Sync + 'static>(
    engine: &Eng,
    web_acl_arn: &str,
    request: &InspectedRequest,
) -> Option<EdgeDecision> {
    decided(engine, web_acl_arn, None, request)
}

/// The evaluation both entry points perform, in the phases [`decide`] documents.
///
/// `protected_resource` is the ARN the web ACL was ASSOCIATED with, where an association
/// is what put it in front of the resource; `None` for a resource that names its own web
/// ACL. It decides nothing about the request — only what the log record says about where
/// the request arrived ([`super::logs`]).
fn decided<Eng: Send + Sync + 'static>(
    engine: &Eng,
    web_acl_arn: &str,
    protected_resource: Option<&str>,
    request: &InspectedRequest,
) -> Option<EdgeDecision> {
    let state: ::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State> =
        crate::state::service_state(engine)?;
    // PHASE 1, under a shared read: the web ACL, the entities its rules reference, and the
    // logging configuration in front of it.
    let (entry, catalogue, logging): (WebAclEntry, Catalogue, Option<LoggingConfiguration>) = {
        let store = state.lock();
        let entry = store.web_acl_by_arn(web_acl_arn)?.clone();
        let catalogue = Catalogue::referenced_by(
            &store,
            &entry.scope,
            entry.web_acl.rules.as_deref().unwrap_or_default(),
        );
        // The customer-owned configuration, which is the one `PutLoggingConfiguration`
        // enables "to start logging from a web ACL"; a configuration of another
        // `LogScope` belongs to the service that owns that scope.
        let logging = store.logging_configuration(web_acl_arn, None).cloned();
        (entry, catalogue, logging)
    };
    let now = crate::state::now_millis();

    // The count one aggregation INSTANCE already holds over the window the rule asking
    // looks back over, answered from the store at the moment the rule asks: a lookup of
    // that one key and a sum over its buckets, under a shared lock held for no longer.
    //
    // The counts are NOT copied out ahead of the evaluation: how many instances a
    // rate-based rule holds is chosen by the REQUESTS (each distinct address, header value
    // or key combination is another one, up to
    // [`crate::state::MAX_RATE_INSTANCES_PER_RULE`] per rule), so a copy of them all would
    // make the work one request costs grow with the traffic that came before it. A request
    // needs exactly the instance it belongs to, per rate-based rule it reaches.
    let counted_within = |key: &RateInstanceKey, window: i64| -> u64 {
        let store = state.lock();
        store
            .rate_based_counts
            .get(key)
            .map_or(0, |buckets| count_within(buckets, now, window))
    };

    // PHASE 2, off the lock: the evaluation itself.
    let evaluation = Evaluation {
        request,
        catalogue: &catalogue,
        body_limit: body_inspection_limit(&entry.web_acl),
        scope: entry.scope.clone(),
        web_acl_id: entry.web_acl.id.clone(),
        now,
        counted_within: &counted_within,
    };
    let (decision, recorded, counted) = evaluation.run(&entry.web_acl);

    // PHASE 3, under an exclusive lock: the bookkeeping WAF keeps about the requests it
    // evaluated. Written here, once, for the request that has just been decided —
    // never on a read.
    {
        let mut store = state.lock();
        // The instances nothing will count again are released ONCE for the request,
        // whatever how many rate-based rules counted it.
        if !counted.is_empty() {
            release_spent_rate_instances(&mut store, now);
        }
        for key in counted {
            make_room_for_rate_instance(&mut store, &key);
            let buckets = store.rate_based_counts.entry(key).or_default();
            record_request(buckets, now);
        }
        if !recorded.samples.is_empty() {
            let population = store
                .sampled_requests
                .entry((entry.scope.clone(), entry.web_acl.id.clone()))
                .or_default();
            population.extend(recorded.samples);
            if population.len() > SAMPLE_POPULATION_CAP {
                let overflow = population.len() - SAMPLE_POPULATION_CAP;
                population.drain(..overflow);
            }
        }
    }

    // PHASE 4, with this service's store released: the CloudWatch counters of every
    // dimension that acted on the request. The metric store belongs to another service,
    // so it is locked only once this one's lock is gone and the two can never be held
    // at the same time.
    super::metrics::publish(
        engine,
        &entry.web_acl.name,
        &super::metrics::reported_region(&entry.scope, &entry.web_acl.arn),
        &recorded.acted,
        now,
    );

    // PHASE 5, under the same discipline: what the web ACL's logging configuration logs
    // about this request ([`super::logs`]). The record is composed here, by the
    // evaluation that knows what the web ACL did, and carried on the decision for the
    // protected resource's own service to deliver once it has answered the request.
    let mut decision = decision;
    decision.log_record = super::logs::logged(
        &entry,
        logging.as_ref(),
        protected_resource,
        request,
        &decision,
        &recorded.non_terminating,
        now,
    );
    Some(decision)
}

/// Release the aggregation instances nothing will count again.
///
/// A rate-based rule's aggregation instances are chosen by the REQUEST — each distinct
/// forwarded address, header value or key combination is another one, and each is held
/// under the value the request chose — so how long each is held has to be this side's
/// choice. An instance whose newest bucket is older than the longest window any rule can
/// look back over can never be counted again, so it is dropped and its key released.
///
/// Called ONCE per request that counted anything, not once per instance it counted.
fn release_spent_rate_instances(store: &mut ComAmazonawsWafv2Awswaf20190729State, now: i64) {
    let oldest = now
        .div_euclid(1_000)
        .saturating_sub(crate::state::MAX_EVALUATION_WINDOW_SEC);
    store
        .rate_based_counts
        .retain(|_, buckets| buckets.iter().any(|bucket| bucket.second >= oldest));
}

/// Make room, if there is none, for one more instance of the rate-based rule `key` belongs
/// to.
///
/// How MANY instances one rule holds is this side's choice for the reason
/// [`release_spent_rate_instances`] gives: they are capped at
/// [`MAX_RATE_INSTANCES_PER_RULE`], the one evicted to make room being the instance whose
/// newest request is oldest.
fn make_room_for_rate_instance(
    store: &mut ComAmazonawsWafv2Awswaf20190729State,
    key: &RateInstanceKey,
) {
    if store.rate_based_counts.contains_key(key) {
        return;
    }
    let mut of_rule: Vec<(RateInstanceKey, i64)> = store
        .rate_based_counts
        .iter()
        .filter(|(held, _)| {
            held.scope == key.scope
                && held.web_acl_id == key.web_acl_id
                && held.rule_path == key.rule_path
        })
        .map(|(held, buckets)| {
            (
                held.clone(),
                buckets.iter().map(|bucket| bucket.second).max().unwrap_or(0),
            )
        })
        .collect();
    if of_rule.len() < MAX_RATE_INSTANCES_PER_RULE {
        return;
    }
    of_rule.sort_by_key(|(_, newest)| *newest);
    let evicting = of_rule.len() + 1 - MAX_RATE_INSTANCES_PER_RULE;
    for (held, _) in of_rule.into_iter().take(evicting) {
        store.rate_based_counts.remove(&held);
    }
}

/// Add one request, at the instant `now`, to the buckets of one aggregation instance,
/// and drop the buckets no evaluation window can reach any more.
///
/// A COUNT per second rather than one entry per request, so the storage one instance
/// costs is bounded by the longest window the model allows rather than by how many
/// requests arrive in it — "valid settings are 60, 120, 300, and 600"
/// (`RateBasedStatement$EvaluationWindowSec`). That is what lets a rule whose `Limit`
/// is large (modelled up to 2,000,000,000) still be reached: nothing here caps the
/// count, only the span it is kept over.
fn record_request(buckets: &mut Vec<RateBucket>, now: i64) {
    let second = now.div_euclid(1_000);
    let oldest = second.saturating_sub(crate::state::MAX_EVALUATION_WINDOW_SEC);
    buckets.retain(|bucket| bucket.second >= oldest);
    match buckets.iter_mut().find(|bucket| bucket.second == second) {
        Some(bucket) => bucket.requests = bucket.requests.saturating_add(1),
        None => buckets.push(RateBucket {
            second,
            requests: 1,
        }),
    }
}

/// How much of a request body the web ACL `web_acl` inspects at an API Gateway stage.
///
/// [`DEFAULT_BODY_INSPECTION_LIMIT`] unless the web ACL's `AssociationConfig` raises
/// it for this resource type — "you can increase the limit for each resource type in
/// the web ACL `AssociationConfig`" — in which case the `SizeInspectionLimit` the
/// configuration names is the limit.
fn body_inspection_limit(web_acl: &WebACL) -> usize {
    web_acl
        .association_config
        .as_ref()
        .and_then(|config| config.request_body.as_ref())
        .and_then(|per_type| per_type.get(API_GATEWAY_RESOURCE_TYPE))
        .and_then(|config| size_inspection_limit(&config.default_size_inspection_limit))
        .unwrap_or(DEFAULT_BODY_INSPECTION_LIMIT)
}

/// The `AssociatedResourceType` an API Gateway stage is configured under
/// (`AssociatedResourceType`: `API_GATEWAY` is one of its values).
pub const API_GATEWAY_RESOURCE_TYPE: &str = "API_GATEWAY";

/// The byte count a `SizeInspectionLimit` names
/// (`smithy_query(query="shape",shape="SizeInspectionLimit")` → `KB_16`, `KB_32`,
/// `KB_48`, `KB_64`), or `None` for a value the enum does not declare.
fn size_inspection_limit(value: &str) -> Option<usize> {
    match value {
        "KB_16" => Some(16 * 1024),
        "KB_32" => Some(32 * 1024),
        "KB_48" => Some(48 * 1024),
        "KB_64" => Some(64 * 1024),
        _ => None,
    }
}

/// The entities a web ACL's rule tree REFERENCES, resolved out of the store once per
/// request.
///
/// A reference statement names an IP set, a regex pattern set or a rule group by ARN,
/// and the referenced entity's own content is what the statement inspects against.
/// Only the ARNs the rule tree NAMES are copied — including, transitively, the ones the
/// rules of a referenced rule group name — so a request pays for the entities its web
/// ACL actually uses rather than for everything the account holds. They are copied
/// under the one shared read the decision is prepared with, so the evaluation reads a
/// consistent snapshot and holds no borrow of the store while it runs.
#[derive(Debug, Default)]
struct Catalogue {
    /// `IPSet$ARN` → the set's declared family and its CIDR entries.
    ip_sets: HashMap<String, (String, Vec<String>)>,
    /// `RegexPatternSet$ARN` → the set's `RegularExpressionList`.
    regex_sets: HashMap<String, Vec<String>>,
    /// `RuleGroup$ARN` → the group's name and its rules, in the group's own order.
    rule_groups: HashMap<String, (String, Vec<Rule>)>,
}

impl Catalogue {
    /// The entities `rules` reference, and the ones their referenced rule groups
    /// reference in turn, out of `scope`.
    fn referenced_by(
        state: &ComAmazonawsWafv2Awswaf20190729State,
        scope: &str,
        rules: &[Rule],
    ) -> Self {
        let mut catalogue = Self::default();
        // A referenced rule group's own rules carry reference statements too, so the
        // walk is a WORKLIST over rule lists rather than a single pass: a rule group
        // may not reference another rule group (the model forbids nesting one inside
        // another), so one level of indirection is all there is, and the ARNs already
        // resolved are what stops a cycle however the records were stored.
        let mut pending: Vec<Vec<Rule>> = vec![rules.to_vec()];
        while let Some(rules) = pending.pop() {
            for arn in referenced_arns(&rules) {
                if let Some(entry) = state
                    .ip_sets
                    .values()
                    .find(|entry| entry.scope == scope && entry.ip_set.arn == arn)
                {
                    catalogue.ip_sets.insert(
                        arn.clone(),
                        (
                            entry.ip_set.ip_address_version.clone(),
                            entry.ip_set.addresses.clone(),
                        ),
                    );
                }
                if let Some(entry) = state.regex_pattern_sets.values().find(|entry| {
                    entry.scope == scope && entry.regex_pattern_set.arn.as_deref() == Some(&arn)
                }) {
                    catalogue.regex_sets.insert(
                        arn.clone(),
                        entry
                            .regex_pattern_set
                            .regular_expression_list
                            .iter()
                            .flatten()
                            .filter_map(|regex| regex.regex_string.clone())
                            .collect(),
                    );
                }
                if catalogue.rule_groups.contains_key(&arn) {
                    continue;
                }
                if let Some(entry) = state
                    .rule_groups
                    .values()
                    .find(|entry| entry.scope == scope && entry.rule_group.arn == arn)
                {
                    let group_rules = entry.rule_group.rules.clone().unwrap_or_default();
                    catalogue.rule_groups.insert(
                        arn.clone(),
                        (entry.rule_group.name.clone(), group_rules.clone()),
                    );
                    pending.push(group_rules);
                }
            }
        }
        catalogue
    }
}

/// Every ARN the reference statements of `rules` name, at any nesting depth.
///
/// One walk for all three reference statements, because an ARN identifies ONE resource
/// of one kind: what the statement meant by it is settled when the ARN is resolved
/// against the store, not here. Walked with an explicit worklist under the same nesting
/// budget the ingress check enforces, so nothing here consumes stack in proportion to a
/// stored tree's depth.
fn referenced_arns(rules: &[Rule]) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    let mut pending: Vec<(&Statement, u32)> = rules
        .iter()
        .map(|rule| (&rule.statement, crate::state::MAX_STATEMENT_NESTING_DEPTH))
        .collect();
    while let Some((statement, depth)) = pending.pop() {
        for arn in [
            statement
                .ip_set_reference_statement
                .as_ref()
                .map(|reference| reference.arn.clone()),
            statement
                .regex_pattern_set_reference_statement
                .as_ref()
                .map(|reference| reference.arn.clone()),
            statement
                .rule_group_reference_statement
                .as_ref()
                .map(|reference| reference.arn.clone()),
        ]
        .into_iter()
        .flatten()
        {
            if !arns.contains(&arn) {
                arns.push(arn);
            }
        }
        if depth == 0 {
            continue;
        }
        let nested = statement
            .and_statement
            .iter()
            .flat_map(|and| and.statements.iter())
            .chain(statement.or_statement.iter().flat_map(|or| or.statements.iter()))
            .chain(
                statement
                    .not_statement
                    .as_deref()
                    .map(|not| not.statement.as_ref()),
            )
            .chain(
                statement
                    .rate_based_statement
                    .as_deref()
                    .and_then(|rate| rate.scope_down_statement.as_deref()),
            )
            .chain(
                statement
                    .managed_rule_group_statement
                    .as_deref()
                    .and_then(|managed| managed.scope_down_statement.as_deref()),
            );
        pending.extend(nested.map(|statement| (statement, depth - 1)));
    }
    arns
}

/// One evaluation of one request: everything the walk over the rules needs that does
/// not change while it runs.
struct Evaluation<'a> {
    /// The request being decided.
    request: &'a InspectedRequest,
    /// The entities its rule tree references.
    catalogue: &'a Catalogue,
    /// How much of the body is inspected ([`body_inspection_limit`]).
    body_limit: usize,
    /// The scope the web ACL lives in, for the rate counts' key.
    scope: String,
    /// The web ACL's id, for the same key.
    web_acl_id: String,
    /// The instant the request is being decided at, in milliseconds since the Unix
    /// epoch.
    now: i64,
    /// The count one aggregation instance ALREADY holds over a window, answered from the
    /// store by the entry point that owns the lock discipline ([`decide`]): a rate-based
    /// rule asks for the one instance this request belongs to, so the work one request
    /// costs does not grow with how many instances the traffic before it created.
    counted_within: &'a dyn Fn(&RateInstanceKey, i64) -> u64,
}

/// What one rule made of the request.
enum RuleOutcome {
    /// The statement did not match, or matched a `Count` action: the next rule runs.
    Continue,
    /// A terminating action: the evaluation ends here.
    Terminate {
        /// The action.
        action: EdgeAction,
        /// Its `CustomResponse`, if it configured one.
        custom_response: Option<CustomResponse>,
        /// Where the rule that decided came from, when a rule of a rule GROUP
        /// decided; [`GroupRuleProvenance::default`] for a rule of the web ACL
        /// itself, which came from no group.
        group: GroupRuleProvenance,
    },
}

/// What a sample of the rule that DECIDED a request reports about the rule group that
/// rule came from.
///
/// Both members are properties of where the rule lives and of what the web ACL said
/// about it, not of the action that ended up being applied, so they travel together
/// with every outcome a group's rule can produce — the ones that terminate the
/// evaluation as much as the ones that only count the request.
#[derive(Debug, Clone, Default)]
struct GroupRuleProvenance {
    /// `SampledHTTPRequest$RuleNameWithinRuleGroup`: the rule within a rule group that
    /// decided, named as [`group_rule_name`] qualifies it.
    inner_rule: Option<String>,
    /// `SampledHTTPRequest$OverriddenAction` — "used only for rule group rules that
    /// have a rule action override in place in the web ACL. This is the action that
    /// the rule group rule is configured for, and not the action that was applied to
    /// the request" — so it is the CONFIGURED action, and only where the web ACL put
    /// something else in its place.
    overridden_action: Option<String>,
}

impl Evaluation<'_> {
    /// Walk the web ACL's rules and settle on a terminating action, recording what WAF
    /// reports about the request ([`Recorded`]) and the rate counts the walk incurred.
    ///
    /// The counts this request ADDS come back as the third element, so the walk itself
    /// writes nothing; the counts already recorded are asked for one instance at a time
    /// ([`Self::counted_within`]).
    fn run(
        &self,
        web_acl: &WebACL,
    ) -> (EdgeDecision, Recorded, Vec<RateInstanceKey>) {
        let mut cx = MatchContext {
            request: self.request,
            catalogue: self.catalogue,
            body_limit: self.body_limit,
            labels: Vec::new(),
        };
        let mut inserted: Vec<(String, String)> = Vec::new();
        let mut recorded = Recorded::default();
        let mut counted: Vec<RateInstanceKey> = Vec::new();
        let namespace = web_acl
            .label_namespace
            .clone()
            .unwrap_or_else(|| {
                crate::state::label_namespace(crate::state::WEB_ACL_ARN_KIND, &web_acl.name)
            });

        for rule in web_acl.rules.iter().flatten() {
            let outcome = self.run_rule(
                rule,
                &mut cx,
                &mut inserted,
                &mut counted,
                &namespace,
                &mut recorded,
            );
            if let RuleOutcome::Terminate {
                action,
                custom_response,
                group,
            } = outcome
            {
                let custom = self.rendered_response(web_acl, custom_response.as_ref(), action);
                self.record_outcome(
                    &mut recorded,
                    &rule.visibility_config,
                    action.action_value(),
                    group.overridden_action.clone(),
                    group.inner_rule.clone(),
                    &inserted,
                    &cx.labels,
                );
                report_response_code(
                    &mut recorded.samples,
                    custom.as_ref().map(|response| response.status),
                );
                return (
                    EdgeDecision {
                        action,
                        inserted_headers: if action.is_allow() {
                            inserted
                        } else {
                            Vec::new()
                        },
                        custom_response: custom,
                        rule_name: Some(rule.name.clone()),
                        rule_name_within_rule_group: group.inner_rule,
                        labels: cx.labels,
                        // Composed once the evaluation has settled, by the entry point
                        // that read the web ACL's logging configuration
                        // ([`decided`]).
                        log_record: None,
                    },
                    recorded,
                    counted,
                );
            }
        }

        // "The action to perform if none of the Rules contained in the WebACL match"
        // (`WebACL$DefaultAction`). A `DefaultAction` naming neither member is a web
        // ACL that cannot be created, and WAF's own default for the pair is `Allow`.
        let (action, custom_response) = match (&web_acl.default_action.block, &web_acl.default_action.allow) {
            (Some(block), _) => (EdgeAction::Block, block.custom_response.clone()),
            (None, Some(allow)) => {
                if let Some(handling) = allow.custom_request_handling.as_ref() {
                    insert_headers(&mut inserted, handling);
                }
                (EdgeAction::Allow, None)
            }
            (None, None) => (EdgeAction::Allow, None),
        };
        let custom = self.rendered_response(web_acl, custom_response.as_ref(), action);
        self.record_outcome(
            &mut recorded,
            &web_acl.visibility_config,
            action.action_value(),
            None,
            None,
            &inserted,
            &cx.labels,
        );
        report_response_code(
            &mut recorded.samples,
            custom.as_ref().map(|response| response.status),
        );
        (
            EdgeDecision {
                action,
                inserted_headers: if action.is_allow() {
                    inserted
                } else {
                    Vec::new()
                },
                custom_response: custom,
                rule_name: None,
                rule_name_within_rule_group: None,
                labels: cx.labels,
                // See the sibling construction above.
                log_record: None,
            },
            recorded,
            counted,
        )
    }

    /// Run ONE web ACL rule.
    #[allow(clippy::too_many_arguments)]
    fn run_rule(
        &self,
        rule: &Rule,
        cx: &mut MatchContext<'_>,
        inserted: &mut Vec<(String, String)>,
        counted: &mut Vec<RateInstanceKey>,
        namespace: &str,
        recorded: &mut Recorded,
    ) -> RuleOutcome {
        // A rule that runs a rule GROUP has no `Action` of its own: each of the
        // group's rules carries one, and the web ACL rule's `OverrideAction` is what
        // the web ACL can say about them all.
        if let Some(managed) = rule.statement.managed_rule_group_statement.as_deref() {
            return self.run_group(
                managed_group_rules(managed),
                managed.scope_down_statement.as_deref(),
                managed.excluded_rules.as_deref(),
                managed.rule_action_overrides.as_deref(),
                rule.override_action.as_ref(),
                // What NAMES this group, for the rule name a sample of it reports: a
                // managed group is named by its vendor and its own name.
                &[managed.vendor_name.as_str(), managed.name.as_str()],
                &managed_namespace(managed),
                cx,
                inserted,
                recorded,
                &rule.visibility_config,
            );
        }
        if let Some(reference) = rule.statement.rule_group_reference_statement.as_ref() {
            let (group_name, group) = cx
                .catalogue
                .rule_groups
                .get(&reference.arn)
                .cloned()
                .unwrap_or_default();
            return self.run_referenced_group(
                &rule.name,
                &group_name,
                &group,
                reference.excluded_rules.as_deref(),
                reference.rule_action_overrides.as_deref(),
                rule.override_action.as_ref(),
                cx,
                inserted,
                counted,
                recorded,
                &rule.visibility_config,
            );
        }

        let matched = if let Some(rate) = rule.statement.rate_based_statement.as_deref() {
            self.rate_based_matches(rate, &rule_path(None, &rule.name), cx, counted)
        } else {
            cx.matches(&rule.statement)
        };
        if !matched {
            return RuleOutcome::Continue;
        }
        add_labels(&mut cx.labels, namespace, rule.rule_labels.as_deref());
        let outcome = self.apply_action(rule.action.as_ref(), inserted);
        if let RuleOutcome::Continue = outcome {
            // The rule MATCHED and its action does not terminate the evaluation, so
            // this request is one the rule COUNTED: it belongs to the rule's own
            // dimension exactly as a terminating match does, and the next rule runs.
            if let Some(counted_as) = counting_action_value(rule.action.as_ref()) {
                recorded
                    .non_terminating
                    .push((rule.name.clone(), counted_as.to_string()));
                self.record_outcome(
                    recorded,
                    &rule.visibility_config,
                    counted_as,
                    None,
                    None,
                    inserted,
                    &cx.labels,
                );
            }
        }
        outcome
    }

    /// Run the rules of a MANAGED rule group the web ACL references.
    #[allow(clippy::too_many_arguments)]
    fn run_group(
        &self,
        group_rules: Vec<ManagedGroupRule>,
        scope_down: Option<&Statement>,
        excluded: Option<&[ExcludedRule]>,
        overrides: Option<&[RuleActionOverride]>,
        override_action: Option<&OverrideAction>,
        group: &[&str],
        namespace: &str,
        cx: &mut MatchContext<'_>,
        inserted: &mut Vec<(String, String)>,
        recorded: &mut Recorded,
        visibility: &VisibilityConfig,
    ) -> RuleOutcome {
        // "An optional nested statement that narrows the scope of the web requests
        // that are evaluated by the managed rule group. Requests are only evaluated by
        // the rule group reference statement when they match the scope-down
        // statement" (`ManagedRuleGroupStatement$ScopeDownStatement`).
        if let Some(scope_down) = scope_down {
            if !cx.matches(scope_down) {
                return RuleOutcome::Continue;
            }
        }
        let counted_group = override_action.is_some_and(|action| action.count.is_some());
        for group_rule in group_rules {
            if !group_rule.inspection.matches(cx) {
                continue;
            }
            if let Some(label) = group_rule.label.as_deref() {
                add_label(&mut cx.labels, namespace, label);
            }
            let decided = effective_group_action(
                &group_rule.name,
                &group_rule.action,
                excluded,
                overrides,
                counted_group,
            );
            let named = group_rule_name(group, &group_rule.name);
            if let Some(outcome) = terminating(
                decided.action.as_ref(),
                inserted,
                GroupRuleProvenance {
                    inner_rule: Some(named.clone()),
                    overridden_action: decided.overridden_action.clone(),
                },
            ) {
                return outcome;
            }
            // The group's rule matched and did not terminate, so it COUNTED this
            // request: it belongs to the dimension of the web ACL rule that runs the
            // group, named by the group rule that counted it.
            if let Some(counted_as) = decided.counted_as {
                recorded
                    .non_terminating
                    .push((named.clone(), counted_as.to_string()));
                self.record_outcome(
                    recorded,
                    visibility,
                    counted_as,
                    decided.overridden_action,
                    Some(named),
                    inserted,
                    &cx.labels,
                );
            }
        }
        RuleOutcome::Continue
    }

    /// Run the rules of a CUSTOMER-owned rule group the web ACL references.
    #[allow(clippy::too_many_arguments)]
    fn run_referenced_group(
        &self,
        rule_name: &str,
        group_name: &str,
        group_rules: &[Rule],
        excluded: Option<&[ExcludedRule]>,
        overrides: Option<&[RuleActionOverride]>,
        override_action: Option<&OverrideAction>,
        cx: &mut MatchContext<'_>,
        inserted: &mut Vec<(String, String)>,
        counted: &mut Vec<RateInstanceKey>,
        recorded: &mut Recorded,
        visibility: &VisibilityConfig,
    ) -> RuleOutcome {
        let counted_group = override_action.is_some_and(|action| action.count.is_some());
        for group_rule in group_rules {
            let matched = if let Some(rate) = group_rule.statement.rate_based_statement.as_deref() {
                self.rate_based_matches(
                    rate,
                    &rule_path(Some(rule_name), &group_rule.name),
                    cx,
                    counted,
                )
            } else {
                cx.matches(&group_rule.statement)
            };
            if !matched {
                continue;
            }
            // A rule group's labels carry the GROUP's namespace, not the web ACL's:
            // `RuleGroup$LabelNamespace` is "awswaf:<account ID>:rulegroup:<rule group
            // name>:", which is the prefix a label added by one of its rules acquires.
            add_labels(
                &mut cx.labels,
                &crate::state::label_namespace(crate::state::RULE_GROUP_ARN_KIND, group_name),
                group_rule.rule_labels.as_deref(),
            );
            let decided = effective_group_action(
                &group_rule.name,
                &group_rule.action.clone().unwrap_or_default(),
                excluded,
                overrides,
                counted_group,
            );
            // A customer-owned group is named by its own name alone.
            let named = group_rule_name(&[group_name], &group_rule.name);
            if let Some(outcome) = terminating(
                decided.action.as_ref(),
                inserted,
                GroupRuleProvenance {
                    inner_rule: Some(named.clone()),
                    overridden_action: decided.overridden_action.clone(),
                },
            ) {
                return outcome;
            }
            // As in [`Self::run_group`]: a group rule that matched without
            // terminating counted the request, under the dimension of the web ACL
            // rule that runs the group.
            if let Some(counted_as) = decided.counted_as {
                recorded
                    .non_terminating
                    .push((named.clone(), counted_as.to_string()));
                self.record_outcome(
                    recorded,
                    visibility,
                    counted_as,
                    decided.overridden_action,
                    Some(named),
                    inserted,
                    &cx.labels,
                );
            }
        }
        RuleOutcome::Continue
    }

    /// Apply a web ACL rule's own `Action`.
    ///
    /// Such a rule belongs to no rule group, so a sample of it reports neither the
    /// name within a group nor an overridden action.
    fn apply_action(
        &self,
        action: Option<&RuleAction>,
        inserted: &mut Vec<(String, String)>,
    ) -> RuleOutcome {
        terminating(action, inserted, GroupRuleProvenance::default())
            .unwrap_or(RuleOutcome::Continue)
    }

    /// Whether a rate-based statement rate-limits this request, and the aggregation
    /// instance it counted it under.
    ///
    /// "A rate-based rule counts incoming requests and rate limits requests when they
    /// are coming at too fast a rate": the request counts toward its aggregation
    /// instance when it satisfies the statement's `ScopeDownStatement` (every request
    /// when there is none), and the rule MATCHES when that instance's count over the
    /// `EvaluationWindowSec` immediately preceding now is already at the `Limit` —
    /// this request being the one that takes it over.
    fn rate_based_matches(
        &self,
        rate: &RateBasedStatement,
        rule_path: &str,
        cx: &MatchContext<'_>,
        counted: &mut Vec<RateInstanceKey>,
    ) -> bool {
        if let Some(scope_down) = rate.scope_down_statement.as_deref() {
            if !cx.matches(scope_down) {
                return false;
            }
        }
        // "Web requests that are missing any of the components specified in the
        // aggregation keys are omitted from the rate-based rule evaluation and
        // handling" (`RateBasedStatement$AggregateKeyType`).
        let Some(aggregate_key) = self.aggregate_key(rate, cx) else {
            return false;
        };
        let key = RateInstanceKey {
            scope: self.scope.clone(),
            web_acl_id: self.web_acl_id.clone(),
            rule_path: rule_path.to_string(),
            aggregate_key,
        };
        let window = evaluation_window_millis(rate.evaluation_window_sec);
        let already = (self.counted_within)(&key, window);
        counted.push(key);
        already.saturating_add(1) > rate.limit.max(0) as u64
    }

    /// The aggregation instance this request belongs to for `rate`, or `None` when the
    /// request is missing a component the aggregation names.
    ///
    /// Every component is CLAMPED to [`MAX_AGGREGATE_KEY_COMPONENT_BYTES`] before it
    /// becomes part of the key: a component is a value the request chose, and the key
    /// is HELD in the store until the instance's window empties, so how much one
    /// instance costs to hold has to be this side's choice. Two values that differ only
    /// past the clamp aggregate together, which is the right trade for a rate count.
    fn aggregate_key(&self, rate: &RateBasedStatement, cx: &MatchContext<'_>) -> Option<String> {
        match rate.aggregate_key_type.as_str() {
            // "Count and limit the requests that match the rate-based rule's
            // scope-down statement. With this option, the counted requests aren't
            // further aggregated."
            "CONSTANT" => Some(String::new()),
            "IP" => Some(clamped(
                &self.request.client_ip,
                MAX_AGGREGATE_KEY_COMPONENT_BYTES,
            )),
            "FORWARDED_IP" => {
                let config = rate.forwarded_ip_config.as_ref()?;
                forwarded_addresses(self.request, &config.header_name)
                    .first()
                    .map(|address| clamped(address, MAX_AGGREGATE_KEY_COMPONENT_BYTES))
            }
            "CUSTOM_KEYS" => {
                let mut parts: Vec<String> = Vec::new();
                for custom in rate.custom_keys.iter().flatten() {
                    parts.push(clamped(
                        &self.custom_key(custom, cx)?,
                        MAX_AGGREGATE_KEY_COMPONENT_BYTES,
                    ));
                }
                Some(parts.join("\u{1f}"))
            }
            _ => None,
        }
    }

    /// One component of a `CUSTOM_KEYS` aggregation, or `None` when the request does
    /// not carry it. Clamped by [`Self::aggregate_key`], which is what holds it.
    fn custom_key(
        &self,
        custom: &RateBasedStatementCustomKey,
        cx: &MatchContext<'_>,
    ) -> Option<String> {
        if let Some(header) = custom.header.as_ref() {
            let value = self.request.header(&header.name)?;
            return Some(transformed_string(
                value.as_bytes(),
                &header.text_transformations,
            ));
        }
        if let Some(cookie) = custom.cookie.as_ref() {
            let value = cookies(self.request)
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&cookie.name))
                .map(|(_, value)| value)?;
            return Some(transformed_string(
                value.as_bytes(),
                &cookie.text_transformations,
            ));
        }
        if let Some(argument) = custom.query_argument.as_ref() {
            let value = query_arguments(&self.request.query_string)
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&argument.name))
                .map(|(_, value)| value)?;
            return Some(transformed_string(
                value.as_bytes(),
                &argument.text_transformations,
            ));
        }
        if let Some(query_string) = custom.query_string.as_ref() {
            return Some(transformed_string(
                self.request.query_string.as_bytes(),
                &query_string.text_transformations,
            ));
        }
        if custom.http_method.is_some() {
            return Some(self.request.method.clone());
        }
        if custom.ip.is_some() {
            return Some(self.request.client_ip.clone());
        }
        if custom.forwarded_ip.is_some() {
            return forwarded_addresses(self.request, FORWARDED_FOR_HEADER)
                .first()
                .cloned();
        }
        if let Some(uri_path) = custom.uri_path.as_ref() {
            return Some(transformed_string(
                self.request.uri_path.as_bytes(),
                &uri_path.text_transformations,
            ));
        }
        if let Some(label) = custom.label_namespace.as_ref() {
            // "Use the specified label namespace as an aggregate key": the value is
            // the label the request carries under that namespace, and a request
            // carrying none is omitted from the rule's handling.
            return cx
                .labels
                .iter()
                .find(|carried| label_in_namespace(carried, &label.namespace))
                .cloned();
        }
        // A JA3/JA4 fingerprint and an ASN are data this emulator holds none of (see
        // the module docs), so a request is missing that component.
        None
    }

    /// The custom response a terminating action serves, rendered from the web ACL's
    /// own `CustomResponseBodies`.
    fn rendered_response(
        &self,
        web_acl: &WebACL,
        custom: Option<&CustomResponse>,
        action: EdgeAction,
    ) -> Option<EdgeResponse> {
        let custom = custom?;
        let body = custom
            .custom_response_body_key
            .as_deref()
            .and_then(|key| web_acl.custom_response_bodies.as_ref()?.get(key))
            .cloned();
        let status = u16::try_from(custom.response_code)
            .ok()
            .filter(|status| (200..=599).contains(status))
            .or_else(|| action.default_status())
            .unwrap_or(403);
        Some(EdgeResponse {
            status,
            content_type: body.as_ref().map(|body| media_type(&body.content_type)),
            body: body.map(|body| body.content.into_bytes()).unwrap_or_default(),
            headers: custom
                .response_headers
                .iter()
                .flatten()
                .map(|header| (header.name.clone(), header.value.clone()))
                .collect(),
        })
    }

    /// Record what WAF reports about this request under one rule dimension that ACTED
    /// on it.
    ///
    /// `action` is the `ActionValue` the dimension applied to the request — the
    /// terminating action for the rule that ended the evaluation, and
    /// [`COUNT_ACTION_VALUE`] or [`EXCLUDED_AS_COUNT_ACTION_VALUE`] for a rule that
    /// only counted it. The dimension's `VisibilityConfig` decides what is recorded,
    /// and its two switches are independent: `SampledRequestsEnabled` — "indicates
    /// whether WAF should store a sampling of the web requests that match the rules" —
    /// asks for the sample `GetSampledRequests` draws from, and
    /// `CloudWatchMetricsEnabled` — "indicates whether the associated resource sends
    /// metrics to Amazon CloudWatch" — asks for this action to be counted into the
    /// dimension's own series ([`super::metrics`]). Either turned off, that record is
    /// not kept, and the request is decided either way.
    ///
    /// The response code the request was answered with is reported by
    /// [`report_response_code`] once the evaluation has settled on one, because every
    /// sample recorded here is of the SAME request.
    #[allow(clippy::too_many_arguments)]
    fn record_outcome(
        &self,
        recorded: &mut Recorded,
        visibility: &VisibilityConfig,
        action: &str,
        overridden_action: Option<String>,
        inner_rule: Option<String>,
        inserted: &[(String, String)],
        labels: &[String],
    ) {
        if visibility.cloud_watch_metrics_enabled {
            recorded.acted.push(super::metrics::ActedDimension {
                metric_name: visibility.metric_name.clone(),
                action_value: action.to_string(),
            });
        }
        if !visibility.sampled_requests_enabled {
            return;
        }
        recorded.samples.push(SampledRequestRecord {
            observed_at_millis: self.now,
            metric_name: visibility.metric_name.clone(),
            sample: SampledHTTPRequest {
                request: self.request.sampled(),
                // `SampledHTTPRequest$Weight` is "a value that indicates how one
                // result in the response relates proportionally to other results":
                // every request this emulator evaluated is in the population, so each
                // sample stands for itself.
                weight: 1,
                timestamp: Some(::aws_smithy_types::DateTime::from_millis(self.now)),
                action: Some(action.to_string()),
                rule_name_within_rule_group: inner_rule,
                request_headers_inserted: (!inserted.is_empty()).then(|| {
                    inserted
                        .iter()
                        .map(|(name, value)| HTTPHeader {
                            name: Some(name.clone()),
                            value: Some(value.clone()),
                        })
                        .collect()
                }),
                // Filled in by [`report_response_code`], once the evaluation has
                // settled on the response the request was answered with.
                response_code_sent: None,
                labels: Some(
                    labels
                        .iter()
                        .map(|name| Label { name: name.clone() })
                        .collect(),
                ),
                captcha_response: None,
                challenge_response: None,
                overridden_action,
            },
        });
    }
}

/// Report, on every sample of the request just evaluated, the response code WAF
/// answered it with.
///
/// `SampledHTTPRequest$ResponseCodeSent` is "the response code that was sent for the
/// request", which is a property of the REQUEST rather than of the dimension a sample
/// of it was drawn under: a rule that counted the request sent no response of its own,
/// so what was sent is whatever the action that terminated the evaluation (or the
/// `DefaultAction`) served. `None` where the protected resource's own origin answered,
/// because WAF sent nothing then.
fn report_response_code(samples: &mut [SampledRequestRecord], status: Option<u16>) {
    for record in samples {
        record.sample.response_code_sent = status.map(i32::from);
    }
}

/// How many requests fall in the `window` milliseconds immediately preceding `now`,
/// summed over the per-second buckets that lie in it.
pub fn count_within(buckets: &[RateBucket], now: i64, window: i64) -> u64 {
    let since = now.saturating_sub(window);
    buckets
        .iter()
        .filter(|bucket| {
            let at = bucket.second.saturating_mul(1_000);
            at > since && at <= now
        })
        .fold(0_u64, |total, bucket| total.saturating_add(bucket.requests))
}

/// The milliseconds a rate-based rule looks back over: its `EvaluationWindowSec`, or
/// the member's documented default when it names none ("Default: 300 (5 minutes)").
pub fn evaluation_window_millis(evaluation_window_sec: i64) -> i64 {
    let seconds = if evaluation_window_sec > 0 {
        evaluation_window_sec
    } else {
        crate::state::DEFAULT_EVALUATION_WINDOW_SEC
    };
    seconds.saturating_mul(1_000)
}

/// What the web ACL's overrides made of one rule of a referenced rule group.
struct GroupRuleDecision {
    /// The action in force for the rule, or `None` when the web ACL took the rule's
    /// own action out of force and left it counting the request.
    action: Option<RuleAction>,
    /// The `ActionValue` a sample of this rule reports when the rule did NOT terminate
    /// the evaluation: `COUNT` for an action that counts the request,
    /// `EXCLUDED_AS_COUNT` for a rule the web ACL excluded, and `None` for a rule that
    /// names no action at all, which applies nothing to the request.
    counted_as: Option<&'static str>,
    /// `SampledHTTPRequest$OverriddenAction` — "used only for rule group rules that
    /// have a rule action override in place in the web ACL. This is the action that
    /// the rule group rule is configured for, and not the action that was applied to
    /// the request" — so it is the CONFIGURED action, and only where the web ACL put
    /// something else in its place.
    overridden_action: Option<String>,
}

/// The `ActionValue` a sample reports for a rule that COUNTED the request rather than
/// acting on it: `RuleAction$Count` "enables WAF to count requests that match the
/// rule, without otherwise affecting what happens to the request" (`ActionValue`).
const COUNT_ACTION_VALUE: &str = "COUNT";

/// The `ActionValue` a sample reports for a rule of a referenced rule group that the
/// web ACL EXCLUDED: such a rule is counted instead of acted on
/// (`ExcludedRule`: "the rules in the referenced rule group whose actions are set to
/// Count"), and the enum has a value of its own for that case.
const EXCLUDED_AS_COUNT_ACTION_VALUE: &str = "EXCLUDED_AS_COUNT";

/// The `ActionValue` `action` names, or `None` for a `RuleAction` naming no member at
/// all.
fn action_value(action: &RuleAction) -> Option<&'static str> {
    if action.block.is_some() {
        return Some(EdgeAction::Block.action_value());
    }
    if action.allow.is_some() {
        return Some(EdgeAction::Allow.action_value());
    }
    if action.captcha.is_some() {
        return Some(EdgeAction::Captcha.action_value());
    }
    if action.challenge.is_some() {
        return Some(EdgeAction::Challenge.action_value());
    }
    action.count.as_ref().map(|_| COUNT_ACTION_VALUE)
}

/// The `ActionValue` a sample reports for a rule whose `action` left the evaluation to
/// continue: `COUNT` for an action that counts the request, and `None` for one that
/// names no member, which does nothing to the request and so samples it under nothing.
fn counting_action_value(action: Option<&RuleAction>) -> Option<&'static str> {
    action
        .and_then(|action| action.count.as_ref())
        .map(|_| COUNT_ACTION_VALUE)
}

/// The action a rule of a referenced group takes once the web ACL's overrides are
/// applied, beside what a sample of that rule reports when it only counts the request.
///
/// Three overrides, innermost first: `ExcludedRules` ("the rules in the referenced
/// rule group whose actions are set to `Count`"), `RuleActionOverrides` ("action
/// settings to use in the place of the rule actions that are configured inside the
/// rule group"), and the web ACL rule's own `OverrideAction` set to `Count`, which
/// counts the whole group.
fn effective_group_action(
    name: &str,
    configured: &RuleAction,
    excluded: Option<&[ExcludedRule]>,
    overrides: Option<&[RuleActionOverride]>,
    counted_group: bool,
) -> GroupRuleDecision {
    // `OverriddenAction` reports what the rule is configured for in the place of what
    // was applied, so a rule the web ACL put the SAME action in the place of has
    // nothing to report there.
    let configured_unless = |in_force: Option<&str>| {
        action_value(configured)
            .filter(|value| Some(*value) != in_force)
            .map(str::to_string)
    };
    if counted_group {
        return GroupRuleDecision {
            action: None,
            counted_as: Some(COUNT_ACTION_VALUE),
            overridden_action: configured_unless(Some(COUNT_ACTION_VALUE)),
        };
    }
    if let Some(over) = overrides
        .unwrap_or_default()
        .iter()
        .find(|over| over.name == name)
    {
        let in_force = over.action_to_use.clone();
        return GroupRuleDecision {
            counted_as: counting_action_value(Some(&in_force)),
            overridden_action: configured_unless(action_value(&in_force)),
            action: Some(in_force),
        };
    }
    if excluded
        .unwrap_or_default()
        .iter()
        .any(|excluded| excluded.name == name)
    {
        return GroupRuleDecision {
            action: None,
            counted_as: Some(EXCLUDED_AS_COUNT_ACTION_VALUE),
            overridden_action: configured_unless(Some(EXCLUDED_AS_COUNT_ACTION_VALUE)),
        };
    }
    GroupRuleDecision {
        counted_as: counting_action_value(Some(configured)),
        // The rule's own action is in force, so nothing was put in its place.
        overridden_action: None,
        action: Some(configured.clone()),
    }
}

/// The terminating outcome `action` names, collecting its `CustomRequestHandling`
/// headers on the way; `None` for `Count` (and for an action naming no member), which
/// lets the next rule run.
fn terminating(
    action: Option<&RuleAction>,
    inserted: &mut Vec<(String, String)>,
    group: GroupRuleProvenance,
) -> Option<RuleOutcome> {
    let action = action?;
    if let Some(block) = action.block.as_ref() {
        return Some(RuleOutcome::Terminate {
            action: EdgeAction::Block,
            custom_response: block.custom_response.clone(),
            group,
        });
    }
    if let Some(allow) = action.allow.as_ref() {
        if let Some(handling) = allow.custom_request_handling.as_ref() {
            insert_headers(inserted, handling);
        }
        return Some(RuleOutcome::Terminate {
            action: EdgeAction::Allow,
            custom_response: None,
            group,
        });
    }
    if let Some(captcha) = action.captcha.as_ref() {
        if let Some(handling) = captcha.custom_request_handling.as_ref() {
            insert_headers(inserted, handling);
        }
        return Some(RuleOutcome::Terminate {
            action: EdgeAction::Captcha,
            custom_response: None,
            group,
        });
    }
    if let Some(challenge) = action.challenge.as_ref() {
        if let Some(handling) = challenge.custom_request_handling.as_ref() {
            insert_headers(inserted, handling);
        }
        return Some(RuleOutcome::Terminate {
            action: EdgeAction::Challenge,
            custom_response: None,
            group,
        });
    }
    // `Count` "enables WAF to count requests that match the rule, without otherwise
    // affecting what happens to the request": its own custom request handling still
    // reaches the origin, and the next rule runs.
    if let Some(count) = action.count.as_ref() {
        if let Some(handling) = count.custom_request_handling.as_ref() {
            insert_headers(inserted, handling);
        }
    }
    None
}

/// Add a `CustomRequestHandling`'s headers to the ones the origin will see, each under
/// the prefix WAF gives it.
fn insert_headers(inserted: &mut Vec<(String, String)>, handling: &CustomRequestHandling) {
    for header in &handling.insert_headers {
        let name = format!("{INSERTED_HEADER_PREFIX}{}", header.name);
        if inserted
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        inserted.push((name, header.value.clone()));
    }
}

/// Add the labels a matching rule specifies, each fully qualified under the namespace
/// of the entity whose rule added it.
fn add_labels(labels: &mut Vec<String>, namespace: &str, specified: Option<&[Label]>) {
    for label in specified.unwrap_or_default() {
        add_label(labels, namespace, &label.name);
    }
}

/// Add ONE label, fully qualified, if the request does not already carry it.
fn add_label(labels: &mut Vec<String>, namespace: &str, name: &str) {
    let qualified = format!("{namespace}{name}");
    if !labels.contains(&qualified) {
        labels.push(qualified);
    }
}

/// The namespace a label added by a rule of a managed group carries
/// (`DescribeManagedRuleGroupResponse$LabelNamespace`).
///
/// The publishing vendor states it, so for a group this service's catalogue publishes
/// it is the namespace that catalogue publishes
/// ([`ManagedRuleGroupDescriptor::managed_label_namespace`]) — the same text
/// `DescribeManagedRuleGroup` qualifies the group's labels with, so a
/// `LabelMatchStatement` written against a published label matches the label a
/// matching request carries. For a group no catalogued vendor publishes, which
/// contributes no rules at all, it is the documented syntax over the names the
/// statement carries.
fn managed_namespace(managed: &ManagedRuleGroupStatement) -> String {
    catalogued(&managed.vendor_name, &managed.name)
        .map(ManagedRuleGroupDescriptor::managed_label_namespace)
        .unwrap_or_else(|| {
            format!(
                "awswaf:managed:{}:{}:",
                managed.vendor_name, managed.name
            )
        })
}

/// Whether the fully qualified label `carried` lies in the namespace `namespace`.
fn label_in_namespace(carried: &str, namespace: &str) -> bool {
    let namespace = namespace.trim_end_matches(':');
    carried.starts_with(&format!("{namespace}:")) || carried.contains(&format!(":{namespace}:"))
}

// ===========================================================================
// What a managed rule group inspects
// ===========================================================================

/// One rule of a managed rule group as the data plane runs it: the name and action the
/// catalogue publishes, plus the inspection the rule performs.
struct ManagedGroupRule {
    /// `RuleSummary$Name`.
    name: String,
    /// The action the vendor's rule takes, as the catalogue publishes it.
    action: RuleAction,
    /// The label the rule adds to a matching request, where the catalogue cites one.
    label: Option<String>,
    /// What the rule inspects for.
    inspection: ManagedInspection,
}

/// The rules of the managed group `managed` names, as the data plane runs them.
///
/// The catalogue publishes each group's rule NAMES and actions; the AWS Managed Rules
/// naming convention states, in the name itself, the inspection a rule performs and
/// the request component it performs it on — `SQLi_QUERYARGUMENTS` inspects the query
/// arguments for SQL injection, `GenericLFI_URIPATH` inspects the path for a local
/// file inclusion attempt. So the inspection is read off the published name
/// ([`ManagedInspection::of_rule_name`]) rather than invented per group, and a rule
/// whose name states an inspection this emulator cannot perform matches nothing.
fn managed_group_rules(managed: &ManagedRuleGroupStatement) -> Vec<ManagedGroupRule> {
    let Some(descriptor) = catalogued(&managed.vendor_name, &managed.name) else {
        return Vec::new();
    };
    descriptor
        .rules
        .iter()
        .map(|published| ManagedGroupRule {
            name: published.name.to_string(),
            action: published.action.rule_action(),
            label: published.label().map(str::to_string),
            inspection: ManagedInspection::of_rule_name(published.name),
        })
        .collect()
}

/// The catalogued descriptor of the managed group `vendor_name`/`name` publishes, or
/// `None` when no vendor in the catalogue publishes it.
fn catalogued(vendor_name: &str, name: &str) -> Option<&'static ManagedRuleGroupDescriptor> {
    crate::state::MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .find(|descriptor| descriptor.vendor_name == vendor_name && descriptor.name == name)
}

/// Which request component a managed rule inspects, as the suffix of its published
/// name states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedComponent {
    /// `_QUERYARGUMENTS`: the value of every query argument.
    QueryArguments,
    /// `_QUERYSTRING`: the whole query string.
    QueryString,
    /// `_BODY`: the body, within the inspection limit.
    Body,
    /// `_COOKIE`: every cookie value.
    Cookie,
    /// `_URIPATH`: the path.
    UriPath,
    /// `_HEADER`: every header value.
    Header,
    /// `_METHOD`: the HTTP method.
    Method,
}

/// What a managed rule looks for, read off its published name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedInspection {
    /// SQL injection in a component, at the sensitivity the rule's name states.
    Sqli(ManagedComponent, bool),
    /// Cross-site scripting in a component.
    Xss(ManagedComponent),
    /// A local-file-inclusion path traversal in a component.
    PathTraversal(ManagedComponent),
    /// A component longer than the limit the rule's name states.
    SizeRestriction(ManagedComponent, usize),
    /// A request with no `User-Agent` header.
    NoUserAgent,
    /// A `Host` header naming the loopback host.
    LocalhostHost,
    /// The `PROPFIND` method.
    PropfindMethod,
    /// A path under an administrative prefix.
    AdminPath,
    /// A rule whose criterion nobody published in a form this emulator can evaluate;
    /// see the module docs.
    Unevaluated,
}

impl ManagedInspection {
    /// The inspection the published rule name `name` states.
    fn of_rule_name(name: &str) -> Self {
        let component = ManagedComponent::of_rule_name(name);
        let (family, _) = name.split_once('_').unwrap_or((name, ""));
        match (family, component) {
            ("SQLi", Some(component)) => Self::Sqli(component, false),
            ("SQLiExtendedPatterns", Some(component)) => Self::Sqli(component, true),
            ("CrossSiteScripting", Some(component)) => Self::Xss(component),
            ("GenericLFI", Some(component)) => Self::PathTraversal(component),
            ("SizeRestrictions", Some(component)) => {
                Self::SizeRestriction(component, size_restriction_limit(component))
            }
            ("NoUserAgent", _) => Self::NoUserAgent,
            ("Host", _) if name == "Host_localhost_HEADER" => Self::LocalhostHost,
            ("PROPFIND", _) => Self::PropfindMethod,
            ("AdminProtection", _) => Self::AdminPath,
            _ => Self::Unevaluated,
        }
    }

    /// Whether this inspection matches the request.
    fn matches(self, cx: &MatchContext<'_>) -> bool {
        match self {
            Self::Sqli(component, extended) => cx
                .component_values(component)
                .iter()
                .any(|value| sqli::is_sql_injection(&url_decode(value), extended)),
            Self::Xss(component) => cx
                .component_values(component)
                .iter()
                .any(|value| sqli::is_cross_site_scripting(&url_decode(value))),
            Self::PathTraversal(component) => cx
                .component_values(component)
                .iter()
                .any(|value| is_path_traversal(&url_decode(value))),
            Self::SizeRestriction(component, limit) => cx
                .component_values(component)
                .iter()
                .any(|value| value.len() > limit),
            Self::NoUserAgent => cx.request.header("user-agent").is_none(),
            Self::LocalhostHost => cx
                .request
                .header("host")
                .is_some_and(|host| LOCALHOST_HOSTS.iter().any(|known| {
                    host.split(':').next().unwrap_or(host).eq_ignore_ascii_case(known)
                })),
            Self::PropfindMethod => cx.request.method.eq_ignore_ascii_case("PROPFIND"),
            Self::AdminPath => {
                let path = cx.request.uri_path.to_ascii_lowercase();
                ADMIN_PATH_PREFIXES
                    .iter()
                    .any(|prefix| path.starts_with(prefix) || path.contains(prefix))
            }
            Self::Unevaluated => false,
        }
    }
}

impl ManagedComponent {
    /// The component the published rule name `name` ends in.
    fn of_rule_name(name: &str) -> Option<Self> {
        let (_, suffix) = name.rsplit_once('_')?;
        match suffix {
            "QUERYARGUMENTS" => Some(Self::QueryArguments),
            "QUERYSTRING" => Some(Self::QueryString),
            "BODY" => Some(Self::Body),
            "COOKIE" => Some(Self::Cookie),
            "URIPATH" => Some(Self::UriPath),
            "HEADER" => Some(Self::Header),
            "METHOD" => Some(Self::Method),
            _ => None,
        }
    }
}

/// The byte limit the `SizeRestrictions_*` rules of the core rule set enforce on each
/// component.
///
/// Source: the WAF Developer Guide's description of `AWSManagedRulesCommonRuleSet`,
/// which states the limit each of those rules enforces — 1,024 bytes for the URI path,
/// 2,048 for the query string, 8 KB for the body and 10,240 for a single cookie header.
fn size_restriction_limit(component: ManagedComponent) -> usize {
    match component {
        ManagedComponent::QueryString | ManagedComponent::QueryArguments => 2_048,
        ManagedComponent::UriPath => 1_024,
        ManagedComponent::Body => 8 * 1_024,
        ManagedComponent::Cookie | ManagedComponent::Header => 10_240,
        ManagedComponent::Method => 1_024,
    }
}

/// The hosts `Host_localhost_HEADER` treats as the loopback host.
const LOCALHOST_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// The path prefixes `AdminProtection_URIPATH` blocks external access to: the
/// administrative pages that rule group "blocks external access to".
const ADMIN_PATH_PREFIXES: &[&str] = &["/admin", "/manager", "/phpmyadmin", "/wp-admin"];

/// Whether `value` carries a path-traversal sequence — the `../` (or `..\`) a local
/// file inclusion attempt walks out of the document root with.
fn is_path_traversal(value: &[u8]) -> bool {
    value
        .windows(3)
        .any(|window| window == b"../" || window == b"..\\")
}

// ===========================================================================
// Matching one statement against one request
// ===========================================================================

/// What matching a statement reads: the request, the entities its references name, the
/// body inspection limit, and the labels the rules that have already run added.
struct MatchContext<'a> {
    /// The request being inspected.
    request: &'a InspectedRequest,
    /// The entities a reference statement resolves through.
    catalogue: &'a Catalogue,
    /// How much of the body is inspected.
    body_limit: usize,
    /// The fully qualified labels the request carries so far.
    labels: Vec<String>,
}

/// The candidate values a `FieldToMatch` yields, and whether the field was larger than
/// WAF inspects.
struct FieldCandidates {
    /// The values to inspect, each on its own: a statement matches when ANY of them
    /// satisfies it.
    values: Vec<Vec<u8>>,
    /// The field's `OversizeHandling` when the field exceeded the inspection limit,
    /// `None` when it did not.
    oversize: Option<String>,
    /// The field's `FallbackBehavior` when the request carries nothing this emulator
    /// can read for it, `None` when the field was read.
    fallback: Option<String>,
}

impl FieldCandidates {
    /// Candidates read normally.
    fn of(values: Vec<Vec<u8>>) -> Self {
        Self {
            values,
            oversize: None,
            fallback: None,
        }
    }

    /// `Some(verdict)` when the field itself decides the statement — because it was
    /// oversize and the rule says what to do about that, or because it could not be
    /// read at all and the rule gave a fallback.
    fn decided(&self) -> Option<bool> {
        let behavior = self.oversize.as_deref().or(self.fallback.as_deref())?;
        match behavior {
            "MATCH" => Some(true),
            "NO_MATCH" => Some(false),
            // `CONTINUE` inspects what is available, which is what the candidates
            // already hold.
            _ => None,
        }
    }
}

impl MatchContext<'_> {
    /// Whether `statement` matches the request.
    ///
    /// A `RateBasedStatement` is not reached here: it is a statement a rule carries at
    /// its top level, and deciding it needs the rate counts, so the rule walk handles
    /// it ([`Evaluation::rate_based_matches`]).
    fn matches(&self, statement: &Statement) -> bool {
        self.matches_within(statement, crate::state::MAX_STATEMENT_NESTING_DEPTH)
    }

    /// [`Self::matches`] with `depth` nesting levels left to descend.
    ///
    /// The logical statements nest, and the tree is the CALLER's: `AndStatement`,
    /// `OrStatement` and `NotStatement` each hold another statement, so the walk over
    /// one is recursive and its depth is whatever was stored. A saved rule tree is
    /// refused on ingress past [`crate::state::MAX_STATEMENT_NESTING_DEPTH`]
    /// (`crate::state::validate_statement_nesting`), so no stored tree is deeper than
    /// that — and this budget is what makes the walk unable to descend further than the
    /// ingress check admits, whatever a tree stored by another path looks like.
    fn matches_within(&self, statement: &Statement, depth: u32) -> bool {
        if depth == 0 {
            return false;
        }
        if let Some(byte_match) = statement.byte_match_statement.as_ref() {
            return self.inspect(&byte_match.field_to_match, |value| {
                positional_match(
                    &transformed(value, &byte_match.text_transformations),
                    &byte_match.search_string,
                    &byte_match.positional_constraint,
                )
            });
        }
        if let Some(sqli_match) = statement.sqli_match_statement.as_ref() {
            // `SqliMatchStatement$SensitivityLevel`: "`LOW` is generally a better
            // choice for resources that already have other protections against SQL
            // injection attacks… Default: `HIGH`".
            let extended = sqli_match
                .sensitivity_level
                .as_deref()
                .unwrap_or("HIGH")
                .eq_ignore_ascii_case("HIGH");
            return self.inspect(&sqli_match.field_to_match, |value| {
                sqli::is_sql_injection(
                    &transformed(value, &sqli_match.text_transformations),
                    extended,
                )
            });
        }
        if let Some(xss_match) = statement.xss_match_statement.as_ref() {
            return self.inspect(&xss_match.field_to_match, |value| {
                sqli::is_cross_site_scripting(&transformed(
                    value,
                    &xss_match.text_transformations,
                ))
            });
        }
        if let Some(size) = statement.size_constraint_statement.as_ref() {
            return self.inspect(&size.field_to_match, |value| {
                compare_size(
                    transformed(value, &size.text_transformations).len() as i64,
                    size.size,
                    &size.comparison_operator,
                )
            });
        }
        if let Some(regex_match) = statement.regex_match_statement.as_ref() {
            // Compiled ONCE for the statement, not once per candidate value: the
            // pattern is the configuration's, the candidates are the request's, and
            // nothing about the compile depends on which candidate it is matched
            // against.
            let compiled = compiled_regexes(std::slice::from_ref(&regex_match.regex_string));
            return self.inspect(&regex_match.field_to_match, |value| {
                let transformed = transformed(value, &regex_match.text_transformations);
                compiled.iter().any(|regex| regex.is_match(&transformed))
            });
        }
        if let Some(reference) = statement.regex_pattern_set_reference_statement.as_ref() {
            let compiled = compiled_regexes(
                self.catalogue
                    .regex_sets
                    .get(&reference.arn)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
            return self.inspect(&reference.field_to_match, |value| {
                let transformed = transformed(value, &reference.text_transformations);
                compiled.iter().any(|regex| regex.is_match(&transformed))
            });
        }
        if let Some(reference) = statement.ip_set_reference_statement.as_ref() {
            return self.ip_set_matches(reference);
        }
        if let Some(label_match) = statement.label_match_statement.as_ref() {
            return self
                .labels
                .iter()
                .any(|carried| label_matches(&label_match.scope, &label_match.key, carried));
        }
        if let Some(and) = statement.and_statement.as_ref() {
            return and
                .statements
                .iter()
                .all(|nested| self.matches_within(nested, depth - 1));
        }
        if let Some(or) = statement.or_statement.as_ref() {
            return or
                .statements
                .iter()
                .any(|nested| self.matches_within(nested, depth - 1));
        }
        if let Some(not) = statement.not_statement.as_deref() {
            return !self.matches_within(&not.statement, depth - 1);
        }
        // A `GeoMatchStatement` or an `AsnMatchStatement` matches on data this
        // emulator holds none of, and a nested `ManagedRuleGroupStatement` or
        // `RuleGroupReferenceStatement` is not a criterion a request satisfies: see
        // the module docs.
        false
    }

    /// Whether any candidate of `field` satisfies `criterion`, with the field's own
    /// oversize and fallback settings deciding where they apply.
    fn inspect(&self, field: &FieldToMatch, criterion: impl Fn(&[u8]) -> bool) -> bool {
        let candidates = self.candidates(field);
        if let Some(decided) = candidates.decided() {
            return decided;
        }
        candidates
            .values
            .iter()
            .any(|value| criterion(value.as_slice()))
    }

    /// The values `field` names in this request.
    fn candidates(&self, field: &FieldToMatch) -> FieldCandidates {
        if let Some(single) = field.single_header.as_ref() {
            return FieldCandidates::of(
                self.request
                    .headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case(&single.name))
                    .map(|(_, value)| value.as_bytes().to_vec())
                    .collect(),
            );
        }
        if let Some(headers) = field.headers.as_ref() {
            let selected = self.selected_headers(&headers.match_pattern);
            let inspected: usize = selected
                .iter()
                .map(|(name, value)| name.len() + value.len())
                .sum();
            let mut candidates = FieldCandidates::of(scoped_pairs(&selected, &headers.match_scope));
            if selected.len() > MAX_INSPECTED_HEADERS || inspected > HEADER_INSPECTION_LIMIT {
                candidates.oversize = Some(headers.oversize_handling.clone());
            }
            return candidates;
        }
        if let Some(cookies) = field.cookies.as_ref() {
            let all = cookies_matching(self.request, &cookies.match_pattern);
            let inspected: usize = all.iter().map(|(name, value)| name.len() + value.len()).sum();
            let mut candidates = FieldCandidates::of(scoped_pairs(&all, &cookies.match_scope));
            if inspected > HEADER_INSPECTION_LIMIT {
                candidates.oversize = Some(cookies.oversize_handling.clone());
            }
            return candidates;
        }
        if let Some(order) = field.header_order.as_ref() {
            let names: Vec<String> = self
                .request
                .headers
                .iter()
                .map(|(name, _)| name.to_ascii_lowercase())
                .collect();
            let mut candidates = FieldCandidates::of(vec![names.join(",").into_bytes()]);
            if names.len() > MAX_INSPECTED_HEADERS {
                candidates.oversize = Some(order.oversize_handling.clone());
            }
            return candidates;
        }
        if let Some(single) = field.single_query_argument.as_ref() {
            return FieldCandidates::of(
                query_arguments(&self.request.query_string)
                    .into_iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case(&single.name))
                    .map(|(_, value)| value.into_bytes())
                    .collect(),
            );
        }
        if field.all_query_arguments.is_some() {
            return FieldCandidates::of(
                query_arguments(&self.request.query_string)
                    .into_iter()
                    .map(|(_, value)| value.into_bytes())
                    .collect(),
            );
        }
        if field.uri_path.is_some() {
            return FieldCandidates::of(vec![self.request.uri_path.as_bytes().to_vec()]);
        }
        if field.query_string.is_some() {
            return FieldCandidates::of(vec![self.request.query_string.as_bytes().to_vec()]);
        }
        if field.method.is_some() {
            return FieldCandidates::of(vec![self.request.method.as_bytes().to_vec()]);
        }
        if let Some(body) = field.body.as_ref() {
            let (inspected, truncated) = self.inspected_body();
            let mut candidates = FieldCandidates::of(vec![inspected]);
            if truncated {
                candidates.oversize = Some(
                    body.oversize_handling
                        .clone()
                        .unwrap_or_else(|| DEFAULT_OVERSIZE_HANDLING.to_string()),
                );
            }
            return candidates;
        }
        if let Some(json) = field.json_body.as_ref() {
            let (inspected, truncated) = self.inspected_body();
            let mut candidates = match json_candidates(&inspected, json) {
                Some(values) => FieldCandidates::of(values),
                None => {
                    // `JsonBody$InvalidFallbackBehavior`: "what WAF should do if it
                    // fails to completely parse the JSON body… Default:
                    // `EVALUATE_AS_STRING`".
                    let behavior = json
                        .invalid_fallback_behavior
                        .clone()
                        .unwrap_or_else(|| "EVALUATE_AS_STRING".to_string());
                    if behavior == "EVALUATE_AS_STRING" {
                        FieldCandidates::of(vec![inspected.clone()])
                    } else {
                        let mut candidates = FieldCandidates::of(Vec::new());
                        candidates.fallback = Some(behavior);
                        candidates
                    }
                }
            };
            if truncated && candidates.fallback.is_none() {
                candidates.oversize = Some(
                    json.oversize_handling
                        .clone()
                        .unwrap_or_else(|| DEFAULT_OVERSIZE_HANDLING.to_string()),
                );
            }
            return candidates;
        }
        // A TLS fingerprint and a URL fragment never reach a server here, so the
        // field's own `FallbackBehavior` decides; see the module docs.
        let fallback = field
            .ja3_fingerprint
            .as_ref()
            .map(|print| print.fallback_behavior.clone())
            .or_else(|| {
                field
                    .ja4_fingerprint
                    .as_ref()
                    .map(|print| print.fallback_behavior.clone())
            })
            .or_else(|| {
                field
                    .uri_fragment
                    .as_ref()
                    .and_then(|fragment| fragment.fallback_behavior.clone())
            });
        FieldCandidates {
            values: Vec::new(),
            oversize: None,
            fallback: Some(fallback.unwrap_or_else(|| "NO_MATCH".to_string())),
        }
    }

    /// The body WAF inspects, and whether the body was longer than that.
    fn inspected_body(&self) -> (Vec<u8>, bool) {
        let body = &self.request.body;
        if body.len() > self.body_limit {
            (body[..self.body_limit].to_vec(), true)
        } else {
            (body.clone(), false)
        }
    }

    /// The headers a `HeaderMatchPattern` selects, in request order.
    fn selected_headers(&self, pattern: &HeaderMatchPattern) -> Vec<(String, String)> {
        self.request
            .headers
            .iter()
            .filter(|(name, _)| included(name, pattern.included_headers.as_deref(), pattern.excluded_headers.as_deref(), pattern.all.is_some()))
            .cloned()
            .collect()
    }

    /// Whether an IP set reference matches the address this request is judged by.
    fn ip_set_matches(&self, reference: &IPSetReferenceStatement) -> bool {
        let Some((version, addresses)) = self.catalogue.ip_sets.get(&reference.arn) else {
            // A reference naming an IP set this service does not hold matches nothing:
            // the set's content is the criterion, and there is none.
            return false;
        };
        // "The configuration for inspecting IP addresses in an HTTP header that you
        // specify, instead of using the IP address that's reported by the web request
        // origin" (`IPSetReferenceStatement$IPSetForwardedIPConfig`). With no such
        // configuration the address is the request's own origin address.
        let Some(config) = reference.ip_set_forwarded_ip_config.as_ref() else {
            return address_in_set(&self.request.client_ip, version, addresses);
        };
        // The entries the POSITION names are the only ones inspected, and they are
        // positions in the header's list as the client wrote it: an entry that is not
        // an address still occupies its place, so one elsewhere in the header never
        // stands in for it.
        let entries = forwarded_entries(self.request, &config.header_name);
        let Some(candidates) = positioned(&entries, &config.position) else {
            // A position the enum does not declare names nothing to inspect, and a
            // criterion with nothing to inspect is not satisfied.
            return false;
        };
        if !candidates
            .iter()
            .any(|entry| entry.parse::<IpAddr>().is_ok())
        {
            // `FallbackBehavior`: "the match status to assign to the web request if the
            // request doesn't have a valid IP address in the specified position" — which
            // covers a request carrying no such header at all.
            return config.fallback_behavior == "MATCH";
        }
        candidates
            .into_iter()
            .any(|address| address_in_set(address, version, addresses))
    }

    /// The values the managed-rule `component` names in this request.
    fn component_values(&self, component: ManagedComponent) -> Vec<Vec<u8>> {
        match component {
            ManagedComponent::QueryArguments => query_arguments(&self.request.query_string)
                .into_iter()
                .map(|(_, value)| value.into_bytes())
                .collect(),
            ManagedComponent::QueryString => {
                vec![self.request.query_string.as_bytes().to_vec()]
            }
            ManagedComponent::Body => vec![self.inspected_body().0],
            ManagedComponent::Cookie => cookies(self.request)
                .into_iter()
                .map(|(_, value)| value.into_bytes())
                .collect(),
            ManagedComponent::UriPath => vec![self.request.uri_path.as_bytes().to_vec()],
            ManagedComponent::Header => self
                .request
                .headers
                .iter()
                .map(|(_, value)| value.as_bytes().to_vec())
                .collect(),
            ManagedComponent::Method => vec![self.request.method.as_bytes().to_vec()],
        }
    }
}

/// `Body$OversizeHandling`'s own documented default: "Default: `CONTINUE`".
const DEFAULT_OVERSIZE_HANDLING: &str = "CONTINUE";

/// Whether a name/value pair's name is one a match pattern selects: `All` selects
/// every one, `IncludedHeaders`/`IncludedCookies` only those named, and
/// `ExcludedHeaders`/`ExcludedCookies` everything but those named. Names are compared
/// case-insensitively, as HTTP header and cookie names are addressed.
fn included(
    name: &str,
    included_names: Option<&[String]>,
    excluded_names: Option<&[String]>,
    all: bool,
) -> bool {
    if let Some(included_names) = included_names {
        return included_names
            .iter()
            .any(|wanted| wanted.eq_ignore_ascii_case(name));
    }
    if let Some(excluded_names) = excluded_names {
        return !excluded_names
            .iter()
            .any(|unwanted| unwanted.eq_ignore_ascii_case(name));
    }
    all
}

/// The candidates a `MapMatchScope` yields over selected name/value pairs: `KEY`
/// inspects the names, `VALUE` the values, `ALL` both.
fn scoped_pairs(pairs: &[(String, String)], scope: &str) -> Vec<Vec<u8>> {
    let mut values: Vec<Vec<u8>> = Vec::new();
    for (name, value) in pairs {
        if scope == "KEY" || scope == "ALL" {
            values.push(name.as_bytes().to_vec());
        }
        if scope == "VALUE" || scope == "ALL" || scope.is_empty() {
            values.push(value.as_bytes().to_vec());
        }
    }
    values
}

/// Every cookie the request's `Cookie` header carries, name and value.
fn cookies(request: &InspectedRequest) -> Vec<(String, String)> {
    request
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .flat_map(|(_, value)| value.split(';'))
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

/// The cookies a `CookieMatchPattern` selects.
fn cookies_matching(
    request: &InspectedRequest,
    pattern: &CookieMatchPattern,
) -> Vec<(String, String)> {
    cookies(request)
        .into_iter()
        .filter(|(name, _)| {
            included(
                name,
                pattern.included_cookies.as_deref(),
                pattern.excluded_cookies.as_deref(),
                pattern.all.is_some(),
            )
        })
        .collect()
}

/// The query arguments of `query_string`, name and percent-decoded value, in the order
/// the client sent them.
fn query_arguments(query_string: &str) -> Vec<(String, String)> {
    query_string
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (
                percent_decoded(name),
                percent_decoded(value),
            )
        })
        .collect()
}

/// `value` with its percent-escapes decoded and `+` read as a space, which is how a
/// query argument is written on the wire.
fn percent_decoded(value: &str) -> String {
    let replaced = value.replace('+', " ");
    ::percent_encoding::percent_decode_str(&replaced)
        .decode_utf8_lossy()
        .into_owned()
}

/// The addresses the forwarded-for header `header_name` lists, in header order.
///
/// A forwarded-for header is "a comma-separated list of IP addresses", so each entry
/// is trimmed and kept only when it parses as an address — an entry that is not one is
/// not an address WAF could match. Where a POSITION within the list decides which entry
/// is inspected, [`forwarded_entries`] is what reads it, because dropping an entry would
/// move the ones behind it.
fn forwarded_addresses(request: &InspectedRequest, header_name: &str) -> Vec<String> {
    forwarded_entries(request, header_name)
        .into_iter()
        .filter(|entry| entry.parse::<IpAddr>().is_ok())
        .collect()
}

/// The entries the forwarded-for header `header_name` lists, in header order, trimmed
/// and kept whether or not they are addresses.
///
/// `Position` is "the position in the header to search for the IP address", so the list
/// it indexes is the one the client wrote: an entry that is not an address occupies its
/// place and makes that position hold no valid address. An EMPTY entry is punctuation
/// rather than an element, so a trailing or doubled comma adds no position.
fn forwarded_entries(request: &InspectedRequest, header_name: &str) -> Vec<String> {
    request
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(header_name))
        .flat_map(|(_, value)| value.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// How many forwarded addresses an `ANY` position inspects: "If the header contains
/// more than 10 IP addresses, WAF inspects the last 10"
/// (`IPSetForwardedIPConfig$Position`).
const MAX_INSPECTED_FORWARDED_ENTRIES: usize = 10;

/// The entries of a forwarded-for header `position` inspects, or `None` for a position
/// the `ForwardedIPPosition` enum does not declare.
///
/// `FIRST` is "the first IP address in the list of IP addresses in the header", `LAST`
/// "the last IP address in the list" and `ANY` "all IP addresses in the header", bounded
/// by [`MAX_INSPECTED_FORWARDED_ENTRIES`].
fn positioned<'a>(entries: &'a [String], position: &str) -> Option<Vec<&'a String>> {
    match position {
        "FIRST" => Some(entries.first().into_iter().collect()),
        "LAST" => Some(entries.last().into_iter().collect()),
        "ANY" => Some(
            entries[entries.len().saturating_sub(MAX_INSPECTED_FORWARDED_ENTRIES)..]
                .iter()
                .collect(),
        ),
        _ => None,
    }
}

/// Whether `address` is in the CIDR ranges of an IP set of family `version`.
fn address_in_set(address: &str, version: &str, addresses: &[String]) -> bool {
    let Ok(address) = address.parse::<IpAddr>() else {
        return false;
    };
    // "All addresses must be specified using Classless Inter-Domain Routing (CIDR)
    // notation" (`IPSet$Addresses`), and `IPAddressVersion` fixes the family the
    // entries belong to, so an address of the other family is in no range of the set.
    match (version, address) {
        ("IPV4", IpAddr::V4(_)) | ("IPV6", IpAddr::V6(_)) => {}
        _ => return false,
    }
    addresses.iter().any(|range| {
        range
            .parse::<::ipnet::IpNet>()
            .is_ok_and(|net| net.contains(&address))
    })
}

/// Whether a `LabelMatchStatement` with this `scope` and `key` matches the fully
/// qualified label `carried`.
///
/// `LabelMatchStatement$Key` states what the key may be: with `Scope` `LABEL` it "must
/// include the name and can include any number of preceding namespace specifications
/// and prefix up to providing the fully qualified label name", so a key is a TRAILING
/// run of the qualified label's colon-separated components; with `Scope` `NAMESPACE`
/// it "can include any number of contiguous namespace strings", so a key is a run of
/// components anywhere in the label's namespace.
pub fn label_matches(scope: &str, key: &str, carried: &str) -> bool {
    let key = key.trim_matches(':');
    match scope {
        "LABEL" => carried == key || carried.ends_with(&format!(":{key}")),
        "NAMESPACE" => {
            let namespace = match carried.rsplit_once(':') {
                Some((namespace, _)) => namespace,
                None => return false,
            };
            namespace == key
                || namespace.starts_with(&format!("{key}:"))
                || namespace.ends_with(&format!(":{key}"))
                || namespace.contains(&format!(":{key}:"))
        }
        _ => false,
    }
}

/// Whether `value` satisfies a byte match statement's `PositionalConstraint` against
/// `search`.
fn positional_match(value: &[u8], search: &[u8], constraint: &str) -> bool {
    match constraint {
        "EXACTLY" => value == search,
        "STARTS_WITH" => value.starts_with(search),
        "ENDS_WITH" => value.ends_with(search),
        "CONTAINS" => contains(value, search),
        // "The value of the specified part of the web request must include the value
        // of SearchString, and SearchString must contain only alphanumeric characters
        // or underscore (A-Z, a-z, 0-9, or _). In addition, SearchString must be a
        // word, which means that both of the following are true: SearchString is at
        // the beginning of the specified part of the web request or is preceded by a
        // character other than an alphanumeric character or underscore… SearchString
        // is at the end… or is followed by a character other than an alphanumeric
        // character or underscore."
        "CONTAINS_WORD" => contains_word(value, search),
        _ => false,
    }
}

/// Whether `value` contains `search`.
fn contains(value: &[u8], search: &[u8]) -> bool {
    if search.is_empty() {
        return true;
    }
    if search.len() > value.len() {
        return false;
    }
    value.windows(search.len()).any(|window| window == search)
}

/// Whether `value` contains `search` as a WORD; see [`positional_match`].
fn contains_word(value: &[u8], search: &[u8]) -> bool {
    if search.is_empty() || search.len() > value.len() {
        return false;
    }
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    (0..=value.len() - search.len()).any(|start| {
        if &value[start..start + search.len()] != search {
            return false;
        }
        let before_ok = start == 0 || !is_word(value[start - 1]);
        let end = start + search.len();
        let after_ok = end == value.len() || !is_word(value[end]);
        before_ok && after_ok
    })
}

/// Whether `length` stands in the relation `operator` to `size`
/// (`SizeConstraintStatement$ComparisonOperator`).
fn compare_size(length: i64, size: i64, operator: &str) -> bool {
    match operator {
        "EQ" => length == size,
        "NE" => length != size,
        "LE" => length <= size,
        "LT" => length < size,
        "GE" => length >= size,
        "GT" => length > size,
        _ => false,
    }
}

/// The patterns of `patterns` compiled, dropping each one the engine cannot compile.
///
/// `Regex$RegexString` is a regular expression, compiled through
/// [`crate::state::compile_regex`] — the one place this service turns a
/// caller-supplied expression into something that matches, so an expression a save
/// accepted is an expression this matches with. A pattern that does not compile
/// matches nothing; the save path refuses one, so a stored set holds none.
///
/// Compiled once per statement, which is why this answers a vector rather than
/// matching: the compile is the expensive half and the candidate values of one request
/// are all matched against the same expressions.
fn compiled_regexes(patterns: &[String]) -> Vec<::regex::bytes::Regex> {
    patterns
        .iter()
        .filter_map(|pattern| crate::state::compile_regex(pattern))
        .collect()
}

/// The media type a `CustomResponseBody$ContentType` names, as a response header value
/// (`ResponseContentType`: `APPLICATION_JSON`, `TEXT_HTML`, `TEXT_PLAIN`).
fn media_type(content_type: &str) -> String {
    match content_type {
        "APPLICATION_JSON" => "application/json",
        "TEXT_HTML" => "text/html",
        _ => "text/plain",
    }
    .to_string()
}

/// The candidates a `JsonBody` field yields from `body`, or `None` when `body` is not
/// JSON this emulator could parse.
fn json_candidates(body: &[u8], json: &JsonBody) -> Option<Vec<Vec<u8>>> {
    let document: ::serde_json::Value = ::serde_json::from_slice(body).ok()?;
    let mut roots: Vec<&::serde_json::Value> = Vec::new();
    if let Some(paths) = json.match_pattern.included_paths.as_deref() {
        // "Match only the specified include paths", each written as a JSON pointer.
        for path in paths {
            if let Some(found) = document.pointer(path) {
                roots.push(found);
            }
        }
    } else {
        roots.push(&document);
    }
    let mut values: Vec<Vec<u8>> = Vec::new();
    for root in roots {
        collect_json(root, &json.match_scope, &mut values);
    }
    Some(values)
}

/// Collect the keys, the values or both out of a JSON document, as a
/// `JsonMatchScope` asks.
///
/// Walked with an EXPLICIT work stack and an explicit depth budget, never by
/// recursion: the document is the request's own body, so its nesting is the caller's
/// choice, and a walk that recursed on it would consume stack in proportion to that
/// choice. Past [`MAX_JSON_INSPECTION_DEPTH`] the walk stops descending, and it stops
/// collecting at [`MAX_JSON_CANDIDATES`], so neither the stack nor the candidate list
/// grows with what the body carries.
fn collect_json(root: &::serde_json::Value, scope: &str, into: &mut Vec<Vec<u8>>) {
    let keys_wanted = scope == "KEY" || scope == "ALL";
    let values_wanted = scope == "VALUE" || scope == "ALL" || scope.is_empty();
    let mut pending: Vec<(&::serde_json::Value, usize)> = vec![(root, 0)];
    while let Some((value, depth)) = pending.pop() {
        if into.len() >= MAX_JSON_CANDIDATES {
            return;
        }
        match value {
            ::serde_json::Value::Object(members) => {
                for (key, member) in members {
                    if keys_wanted {
                        into.push(key.as_bytes().to_vec());
                    }
                    if depth < MAX_JSON_INSPECTION_DEPTH {
                        pending.push((member, depth + 1));
                    }
                }
            }
            ::serde_json::Value::Array(members) => {
                if depth < MAX_JSON_INSPECTION_DEPTH {
                    pending.extend(members.iter().map(|member| (member, depth + 1)));
                }
            }
            other => {
                if values_wanted {
                    into.push(json_scalar(other).into_bytes());
                }
            }
        }
    }
}

/// Emulator resource guard, NOT an AWS quota: the deepest a `JsonBody` field's document
/// is walked. The nesting is the caller's, so the depth the walk follows is this side's
/// choice.
const MAX_JSON_INSPECTION_DEPTH: usize = 100;

/// Emulator resource guard, NOT an AWS quota: the most candidate values one `JsonBody`
/// field yields. The body is already truncated to the inspection limit, and this bounds
/// the list built out of it as well.
const MAX_JSON_CANDIDATES: usize = 2_000;

/// A JSON scalar as the text WAF inspects it as: a string's own characters, and any
/// other scalar's JSON spelling.
fn json_scalar(value: &::serde_json::Value) -> String {
    match value {
        ::serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

// ===========================================================================
// Text transformations
// ===========================================================================

/// `value` with the statement's `TextTransformations` applied in ascending `Priority`
/// ("WAF performs all transformations on the content… starting from the lowest
/// priority setting").
///
/// A `Type` this emulator does not perform leaves the content as it was: a
/// transformation applied wrongly would change what every criterion over it decides,
/// so an unperformed one is the identity rather than an approximation.
pub fn transformed(value: &[u8], transformations: &[TextTransformation]) -> Vec<u8> {
    let mut ordered: Vec<&TextTransformation> = transformations.iter().collect();
    ordered.sort_by_key(|transformation| transformation.priority);
    let mut current = value.to_vec();
    for transformation in ordered {
        current = apply_transformation(&current, &transformation.r#type);
    }
    current
}

/// [`transformed`], as the text a rate-based aggregate key carries.
fn transformed_string(value: &[u8], transformations: &[TextTransformation]) -> String {
    String::from_utf8_lossy(&transformed(value, transformations)).into_owned()
}

/// One `TextTransformationType` applied to `value`.
fn apply_transformation(value: &[u8], transformation: &str) -> Vec<u8> {
    match transformation {
        "LOWERCASE" => value.to_ascii_lowercase(),
        "UPPERCASE" => value.to_ascii_uppercase(),
        "URL_DECODE" | "URL_DECODE_UNI" => url_decode(value),
        "HTML_ENTITY_DECODE" => html_entity_decode(value),
        "COMPRESS_WHITE_SPACE" => compress_white_space(value),
        "REMOVE_WHITESPACE" => value
            .iter()
            .copied()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect(),
        "REMOVE_NULLS" => value.iter().copied().filter(|byte| *byte != 0).collect(),
        "REPLACE_NULLS" => value
            .iter()
            .copied()
            .map(|byte| if byte == 0 { b' ' } else { byte })
            .collect(),
        "TRIM" => trim(value, true, true),
        "TRIM_LEFT" => trim(value, true, false),
        "TRIM_RIGHT" => trim(value, false, true),
        "NORMALIZE_PATH" | "NORMALIZE_PATH_WIN" => normalize_path(value),
        "BASE64_DECODE" | "BASE64_DECODE_EXT" => base64_decode(value).unwrap_or_else(|| value.to_vec()),
        // `NONE` is "no transformation", and a type this emulator does not perform is
        // left alone; see this function's own documentation.
        _ => value.to_vec(),
    }
}

/// `value` with its percent-escapes decoded, which is what `URL_DECODE` does ("decode
/// a URL-encoded value").
fn url_decode(value: &[u8]) -> Vec<u8> {
    let replaced: Vec<u8> = value
        .iter()
        .copied()
        .map(|byte| if byte == b'+' { b' ' } else { byte })
        .collect();
    ::percent_encoding::percent_decode(&replaced).collect()
}

/// `value` with its HTML character references decoded, which is what
/// `HTML_ENTITY_DECODE` does ("decode HTML entities").
///
/// Through the maintained decoder rather than a hand-written table: a reference can be
/// named, decimal or hexadecimal, and what the transformation has to reproduce is what a
/// browser would make of the text.
fn html_entity_decode(value: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(value);
    ::html_escape::decode_html_entities(text.as_ref())
        .into_owned()
        .into_bytes()
}

/// `value` with each run of whitespace collapsed to one space, which is what
/// `COMPRESS_WHITE_SPACE` does ("replace multiple spaces with one space").
fn compress_white_space(value: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(value.len());
    let mut in_space = false;
    for byte in value.iter().copied() {
        if byte.is_ascii_whitespace() {
            if !in_space {
                out.push(b' ');
            }
            in_space = true;
        } else {
            out.push(byte);
            in_space = false;
        }
    }
    out
}

/// `value` without its leading and/or trailing whitespace.
fn trim(value: &[u8], left: bool, right: bool) -> Vec<u8> {
    let mut start = 0;
    let mut end = value.len();
    if left {
        while start < end && value[start].is_ascii_whitespace() {
            start += 1;
        }
    }
    if right {
        while end > start && value[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
    }
    value[start..end].to_vec()
}

/// `value` with its `.` and `..` path segments resolved and its separator runs
/// collapsed, which is what `NORMALIZE_PATH` does.
fn normalize_path(value: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(value).replace('\\', "/");
    let absolute = text.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in text.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let joined = segments.join("/");
    if absolute {
        format!("/{joined}").into_bytes()
    } else {
        joined.into_bytes()
    }
}

/// `value` decoded from standard base64, or `None` when it is not base64 at all.
///
/// Through the maintained coder rather than a hand-written one, with the padding-optional
/// alphabet: `BASE64_DECODE` is "decode a base64-encoded string", and a value an
/// attacker encoded does not have to be padded.
fn base64_decode(value: &[u8]) -> Option<Vec<u8>> {
    use ::base64::Engine as _;
    let trimmed: Vec<u8> = value
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    ::base64::engine::general_purpose::STANDARD
        .decode(&trimmed)
        .or_else(|_| {
            ::base64::engine::general_purpose::STANDARD_NO_PAD.decode(&trimmed)
        })
        .ok()
}

#[cfg(test)]
mod tests {
    //! Unit coverage for the pieces of the evaluation whose rules are statements about
    //! BYTES rather than about stored records: the positional constraints, the text
    //! transformations, label matching and the forwarded-address list.

    use super::*;

    #[test]
    fn positional_constraints_follow_the_modelled_vocabulary() {
        let cases: &[(&str, &[u8], &[u8], bool)] = &[
            ("EXACTLY", b"admin", b"admin", true),
            ("EXACTLY", b"administrator", b"admin", false),
            ("STARTS_WITH", b"administrator", b"admin", true),
            ("ENDS_WITH", b"the-admin", b"admin", true),
            ("CONTAINS", b"an-admin-page", b"admin", true),
            ("CONTAINS_WORD", b"an admin page", b"admin", true),
            ("CONTAINS_WORD", b"anadminpage", b"admin", false),
        ];
        for (constraint, value, search, expected) in cases {
            let actual = positional_match(value, search, constraint);
            assert_eq!(
                actual, *expected,
                "case {constraint:?} over {:?}: expected {expected}, got {actual}",
                String::from_utf8_lossy(value)
            );
        }
    }

    #[test]
    fn transformations_run_in_priority_order() {
        let transformations = vec![
            TextTransformation {
                priority: 1,
                r#type: "LOWERCASE".to_string(),
            },
            TextTransformation {
                priority: 0,
                r#type: "URL_DECODE".to_string(),
            },
        ];
        let actual = transformed(b"%41%42c", &transformations);
        assert_eq!(
            actual,
            b"abc".to_vec(),
            "case url-decode then lowercase: expected b\"abc\", got {:?}",
            String::from_utf8_lossy(&actual)
        );
    }

    #[test]
    fn label_match_reads_the_key_as_a_trailing_run_of_components() {
        let carried = "awswaf:123456789012:webacl:acl:tagged";
        let cases: &[(&str, &str, bool)] = &[
            ("LABEL", "tagged", true),
            ("LABEL", "acl:tagged", true),
            ("LABEL", "awswaf:123456789012:webacl:acl:tagged", true),
            ("LABEL", "other", false),
            ("NAMESPACE", "webacl:acl", true),
            ("NAMESPACE", "awswaf", true),
            ("NAMESPACE", "tagged", false),
        ];
        for (scope, key, expected) in cases {
            let actual = label_matches(scope, key, carried);
            assert_eq!(
                actual, *expected,
                "case {scope}/{key}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn a_group_rule_name_is_the_group_then_the_rule() {
        // `SampledHTTPRequest$RuleNameWithinRuleGroup`: `<vendor name>#<managed rule
        // group name>#<rule name>` for a managed group, `<rule group name>#<rule name>`
        // for one of the account's own, and a segment nothing named is left out rather
        // than joined as an empty one.
        let cases: &[(&[&str], &str, &str)] = &[
            (
                &["AWS", "AWSManagedRulesSQLiRuleSet"],
                "SQLi_QUERYARGUMENTS",
                "AWS#AWSManagedRulesSQLiRuleSet#SQLi_QUERYARGUMENTS",
            ),
            (&["own-guard"], "deny-upload", "own-guard#deny-upload"),
            (&[""], "deny-upload", "deny-upload"),
            (&[], "deny-upload", "deny-upload"),
        ];
        for (group, rule_name, expected) in cases {
            let actual = group_rule_name(group, rule_name);
            assert_eq!(
                actual, *expected,
                "case {group:?} holding {rule_name:?}: expected {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    #[test]
    fn forwarded_addresses_keep_header_order_and_drop_non_addresses() {
        let request = InspectedRequest {
            headers: vec![(
                "X-Forwarded-For".to_string(),
                "203.0.113.7, not-an-ip, 198.51.100.9".to_string(),
            )],
            ..InspectedRequest::default()
        };
        let actual = forwarded_addresses(&request, "x-forwarded-for");
        assert_eq!(
            actual,
            vec!["203.0.113.7".to_string(), "198.51.100.9".to_string()],
            "case a header with one unparseable entry: got {actual:?}"
        );
    }

    #[test]
    fn base64_and_path_transformations_follow_their_documented_effect() {
        let decoded = apply_transformation(b"YWJj", "BASE64_DECODE");
        assert_eq!(
            decoded,
            b"abc".to_vec(),
            "case BASE64_DECODE of YWJj: got {:?}",
            String::from_utf8_lossy(&decoded)
        );
        let normalized = apply_transformation(b"/a/./b/../c", "NORMALIZE_PATH");
        assert_eq!(
            normalized,
            b"/a/c".to_vec(),
            "case NORMALIZE_PATH: got {:?}",
            String::from_utf8_lossy(&normalized)
        );
    }

    #[test]
    fn a_json_body_is_walked_to_a_bounded_depth_and_yields_a_bounded_list() {
        // The nesting and the width of a body are the CALLER's, so the walk's depth and
        // the list it builds are bounded on this side. The document is built as a VALUE
        // rather than parsed, because a parser has a nesting limit of its own and the
        // bound under test is this walk's: nested one level past the depth budget, so a
        // walk that followed the whole document would reach the leaf and a bounded one
        // does not.
        let mut deep = ::serde_json::json!("leaf");
        for _ in 0..=MAX_JSON_INSPECTION_DEPTH {
            deep = ::serde_json::Value::Array(vec![deep]);
        }
        let mut values: Vec<Vec<u8>> = Vec::new();
        collect_json(&deep, "ALL", &mut values);
        assert!(
            values.is_empty(),
            "case a body nested one level past the depth budget: expected the walk to \
             stop before the leaf, got {:?}",
            values
                .iter()
                .take(4)
                .map(|value| String::from_utf8_lossy(value).into_owned())
                .collect::<Vec<String>>()
        );

        // A body nested to exactly the budget IS walked to its leaves, so the bound
        // stops the walk one level further out and nowhere sooner.
        let mut reachable = ::serde_json::json!("leaf");
        for _ in 0..MAX_JSON_INSPECTION_DEPTH {
            reachable = ::serde_json::Value::Array(vec![reachable]);
        }
        let mut values: Vec<Vec<u8>> = Vec::new();
        collect_json(&reachable, "ALL", &mut values);
        assert_eq!(
            values,
            vec![b"leaf".to_vec()],
            "case a body nested to exactly the depth budget: expected its leaf among \
             the candidates, got {} candidate(s)",
            values.len()
        );

        // And a wide body stops at the candidate cap rather than at the depth budget.
        let wide = ::serde_json::Value::Array(vec![
            ::serde_json::json!("leaf");
            MAX_JSON_CANDIDATES * 2
        ]);
        let mut values: Vec<Vec<u8>> = Vec::new();
        collect_json(&wide, "ALL", &mut values);
        assert!(
            values.len() <= MAX_JSON_CANDIDATES,
            "case a body carrying {} values: expected at most {MAX_JSON_CANDIDATES} \
             candidates, got {}",
            MAX_JSON_CANDIDATES * 2,
            values.len()
        );

        // A body within both budgets yields its keys and its values.
        let document: ::serde_json::Value =
            ::serde_json::json!({ "outer": { "inner": ["alpha", 7] } });
        let mut values: Vec<Vec<u8>> = Vec::new();
        collect_json(&document, "ALL", &mut values);
        let rendered: Vec<String> = values
            .iter()
            .map(|value| String::from_utf8_lossy(value).into_owned())
            .collect();
        for expected in ["outer", "inner", "alpha", "7"] {
            assert!(
                rendered.iter().any(|value| value == expected),
                "case a nested object with scope ALL: expected {expected:?} among the \
                 candidates, got {rendered:?}"
            );
        }
    }
}
