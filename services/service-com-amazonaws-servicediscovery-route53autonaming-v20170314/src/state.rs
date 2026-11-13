// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicediscovery-route53autonaming-v20170314 service.
//!
//! Cloud Map's resources form one tree — namespaces hold services, services hold
//! instances — plus the asynchronous `Operation` records every mutating call files
//! and `GetOperation` reads back. Everything the handlers in `src/ops/` answer with
//! is computed from the records below; nothing is canned.
//!
//! Two name indexes exist because the API resolves resources BOTH ways: the control
//! plane addresses a namespace/service by id, while `DiscoverInstances` addresses them
//! by name. They are maintained beside the record maps on every write, so a lookup by
//! either key reaches the one stored record.
//!
//! Every store here is BOUNDED, because it lives in memory for the life of the
//! process and grows on request: a create past a resource quota is refused with the
//! modelled `ResourceLimitExceeded` (`MAX_NAMESPACES`,
//! `MAX_SERVICES_PER_NAMESPACE`, `MAX_INSTANCES_PER_SERVICE`), an attribute past the
//! documented per-service quota with `ServiceAttributesLimitExceededException`
//! (`MAX_SERVICE_ATTRIBUTES`), and the two stores no
//! API can ever shrink — the operation records and the remembered idempotency tokens
//! — evict their oldest entry instead (`MAX_RETAINED_OPERATIONS`,
//! `MAX_RETAINED_IDEMPOTENCY_TOKENS`).

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, HttpProperties, Instance,
    InstanceSummary, Namespace, NamespaceProperties, NamespaceSummary, Operation, Service,
    ServiceAttributes, ServiceSummary, Tag, SOA,
};

/// The region an ARN carries when the request is unsigned, matching the region a
/// generated emulator defaults to (see `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN/`aws.api#service` namespace for this service, used for every ARN it mints.
pub const ARN_NAMESPACE: &str = "servicediscovery";

/// How long an asynchronous operation stays `SUBMITTED` before the clock owes it
/// `SUCCESS`.
///
/// AWS documents no duration for this, so the span is CHOSEN: it sits inside the
/// emulator's 250–750 ms window for a clock-owed transition, so a caller that polls
/// `GetOperation` immediately still observes the non-terminal `SUBMITTED` status the
/// real service reports. The due instant is stored and materialised on read — the
/// service never sleeps for it.
pub const OPERATION_SUCCESS_SPAN: Duration = Duration::from_millis(250);

/// How long a custom health status a client submitted stays owed before the clock
/// applies it to the instance.
///
/// `HealthCheckCustomConfig` documents that the change is NOT applied at the instant of
/// the call: "Cloud Map waits for approximately 30 seconds after receiving an
/// `UpdateInstanceCustomHealthStatus` request before changing the status of the service
/// instance", and "Sending a second or subsequent `UpdateInstanceCustomHealthStatus`
/// request with the same value before 30 seconds has passed doesn't accelerate the
/// change." That wait is the service's own propagation latency rather than a duration a
/// caller or a configuration asks for (`FailureThreshold` is deprecated and "always set
/// to 1"), so the span here is the emulator's 250-750 ms window for a transition the
/// clock owes: a caller reading straight after the update still observes the previous
/// status, as it would against the real service. The due instant is stored and
/// materialised on read — the service never sleeps for it.
pub const CUSTOM_HEALTH_STATUS_SPAN: Duration = Duration::from_millis(250);

/// How long a Route 53 health check's verdict stays owed to a freshly registered
/// instance before the clock reports it.
///
/// `GetInstancesHealthStatus` documents that the status is computed server-side and is
/// not there straight away: "There's a brief delay between when you register an instance
/// and when the health status for the instance is available." AWS's own wait is the
/// health check's failure threshold times its 30-second interval; that is the service's
/// own latency rather than a duration the caller asks for, so the span here is the
/// emulator's 250-750 ms window for a transition the clock owes: a caller reading
/// straight after the registration still observes `UNKNOWN`, the status the real service
/// reports until its checker has reported. The due instant is stored and materialised on
/// read — the service never sleeps for it.
pub const ROUTE53_HEALTH_CHECK_SPAN: Duration = Duration::from_millis(250);

/// The verdict a Route 53 health check reaches in this emulator.
///
/// A `HealthCheckConfig` health check is performed BY the service, by probing the
/// instance's endpoint from outside; this emulator opens no outbound connections, so a
/// probe of that endpoint can never be observed to succeed. The verdict it reports once
/// the check is due is therefore the failing one — never `HEALTHY`, which would claim a
/// successful probe that nothing performed. A client cannot override it:
/// `UpdateInstanceCustomHealthStatus` refuses a service checked this way with
/// `CustomHealthNotFound`.
pub const ROUTE53_HEALTH_CHECK_VERDICT: &str = HEALTH_STATUS_UNHEALTHY;

/// The quota on tags per resource, from the `TooManyTagsException` documentation:
/// "The maximum number of tags that can be applied to a resource is 50."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `TooManyTagsException` message, which is FIXED text: the service reports that
/// the tag quota for the resource was passed and names neither the quota, the resource
/// nor the keys the request carried, so no request value reaches the body.
///
/// One constant for every operation that applies the quota — the tag write and each
/// create that accepts `Tags` — so the quota refusal reads the same wherever it is
/// reached.
pub const TAG_LIMIT_EXCEEDED_MESSAGE: &str = "Allowable tag limit for resource exceeded";

/// How many operation records this store keeps.
///
/// AWS does not publish a retention rule for `Operation` records, so this bound is
/// the EMULATOR's: the store is in memory for the life of the process, and every
/// mutating call files a record, so without a bound the map would grow without
/// limit. When the bound is passed the oldest record that has already reached a
/// terminal status is dropped first, so a caller can still resolve an operation it is
/// polling. `GetOperation` answers `OperationNotFound` for an evicted id, which is
/// the same answer it gives for an id the service never filed.
pub const MAX_RETAINED_OPERATIONS: usize = 1_000;

/// How many `CreatorRequestId` tokens this store remembers, for the same reason as
/// [`MAX_RETAINED_OPERATIONS`]. The oldest token is forgotten first; a retry whose
/// token has been forgotten is served as a fresh request.
pub const MAX_RETAINED_IDEMPOTENCY_TOKENS: usize = 1_000;

/// How many namespaces this emulator holds before refusing to create another with
/// the modelled `ResourceLimitExceeded` ("The resource can't be created because
/// you've reached the quota on the number of resources").
///
/// Real Cloud Map publishes per-account, adjustable quotas; this emulator does not
/// claim to reproduce their values, so the bound here is the EMULATOR's own: a
/// resource store that only ever grows has no other limit, since the records live in
/// the process for its lifetime. The bound is answered with the error the model
/// declares for exactly this case, never by dropping a resource a caller created.
pub const MAX_NAMESPACES: usize = 1_000;

/// How many services one namespace holds, bounded for the reason [`MAX_NAMESPACES`]
/// gives.
pub const MAX_SERVICES_PER_NAMESPACE: usize = 1_000;

/// How many instances one service holds, bounded for the reason [`MAX_NAMESPACES`]
/// gives. Re-registering an instance that already exists is a replace, so it is never
/// refused by this bound.
pub const MAX_INSTANCES_PER_SERVICE: usize = 1_000;

/// How many records a listing returns when the caller names no `MaxResults`.
///
/// From the documented default on `ListNamespacesRequest$MaxResults` — "If you don't
/// specify a value for MaxResults, Cloud Map returns up to 100 namespaces" — which is
/// also the top of the modelled `MaxResults` range (1..100), shared by every list
/// operation in this service.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// How many service-level attributes one service holds, from the documentation of
/// `ServiceAttributes$Attributes`: "You can specify a total of 30 attributes." The
/// same number is the `length` bound the model puts on `ServiceAttributesMap`, which
/// caps ONE request; this constant caps the TOTAL a service accumulates across
/// requests, which is what `ServiceAttributesLimitExceededException` reports.
pub const MAX_SERVICE_ATTRIBUTES: usize = 30;

/// The `length` bound the model puts on `ServiceAttributeKey` (0..255).
///
/// A `ServiceAttributesMap` KEY is the one place this bound is not checked for the
/// handlers: the centralized request walk covers a collection's entry count and a
/// scalar list ELEMENT's or map VALUE's own constraints, but "Map *keys* are not
/// covered" (`vendor/smithy-schema-server/src/constraints.rs`). So
/// `UpdateServiceAttributes`, whose keys are map keys, checks it with
/// [`attribute_key_too_long`], while `DeleteServiceAttributes`, whose keys are list
/// elements, has it checked for it before the handler runs.
pub const MAX_SERVICE_ATTRIBUTE_KEY_LEN: usize = 255;

/// The `InvalidInput` text for an attribute key longer than its modelled `length`
/// bound, or `None` when the key fits.
///
/// `InvalidInput` is documented for exactly this case — "a string value might exceed
/// length constraints" — and the message is built from the request: the bound and the
/// length supplied. Smithy measures a string's `length` in Unicode code points, so a
/// multi-byte key is measured as the caller wrote it rather than in bytes.
pub fn attribute_key_too_long(key: &str) -> Option<String> {
    let actual = key.chars().count();
    (actual > MAX_SERVICE_ATTRIBUTE_KEY_LEN).then(|| {
        format!(
            "An attribute key must be at most {MAX_SERVICE_ATTRIBUTE_KEY_LEN} characters long, \
             but the request supplies one of {actual}."
        )
    })
}

/// What a handler reports when this service was mounted on an engine that holds no
/// state at all.
///
/// A wiring failure, not a bad request — but this service's model declares no
/// server-fault shape (`InvalidInput` is its only service-wide error), and a 501 would
/// claim the operation is unwritten, which is a different and false claim. So the
/// handlers answer with `InvalidInput` carrying this message, which says plainly what
/// went wrong. Unreachable in the assembled emulator, where every service is mounted
/// on the one shared `InMemoryEngine`.
pub const UNREACHABLE_STATE_MESSAGE: &str =
    "the servicediscovery emulator is not mounted on the shared engine, so its stored \
     namespaces, services, instances and operations are not reachable";

/// `OperationStatus` values this service reports (see `OPERATION_STATUS_VALUES`).
pub const OPERATION_STATUS_SUBMITTED: &str = "SUBMITTED";
/// The terminal success status an operation reaches once its due instant passes.
pub const OPERATION_STATUS_SUCCESS: &str = "SUCCESS";

/// `HealthStatus` reported for an instance no health checker has reported on.
pub const HEALTH_STATUS_UNKNOWN: &str = "UNKNOWN";
/// `HealthStatus` of an instance a health checker reported healthy.
pub const HEALTH_STATUS_HEALTHY: &str = "HEALTHY";
/// `HealthStatus` of an instance a health checker reported unhealthy.
pub const HEALTH_STATUS_UNHEALTHY: &str = "UNHEALTHY";

/// The `@pattern` the model declares on `NamespaceNameHttp` (`CreateHttpNamespace$Name`).
pub const NAMESPACE_NAME_HTTP_PATTERN: &str = "^(?!arn:)[!-~]{1,1024}$";

/// The `@pattern` the model declares on `NamespaceNamePrivate`
/// (`CreatePrivateDnsNamespace$Name`).
pub const NAMESPACE_NAME_PRIVATE_PATTERN: &str = "^(?!arn:)[!-~]{1,253}$";

/// The step budget one namespace-name match may spend.
///
/// `fancy-regex` is a BACKTRACKING engine, so a match has no linear-time guarantee in
/// general and the work is bounded explicitly here rather than left to the pattern's
/// shape. Two facts make this budget unreachable in practice, and both are properties
/// of the inputs rather than of the engine: the patterns are MODEL CONSTANTS (never
/// built from a request), and the matched value is a namespace `Name`, whose `@length`
/// maximum (1024 characters at most) the boundary validator enforces before any handler
/// runs. The budget is the backstop for a future model pattern that is not so tame: a
/// match that exhausts it is answered, not retried, and never spins.
const NAMESPACE_NAME_BACKTRACK_LIMIT: usize = 1_000_000;

/// How long a rejected value may be when a validation clause echoes it, matching the
/// framework's own ceiling so a handler-built clause reads like a boundary-built one.
const MAX_ECHOED_VALUE: usize = 256;

/// Why the modelled `@pattern` on a namespace `Name` rejects `name`, worded as the
/// framework's own validation envelope — or `None` when the name satisfies it.
///
/// This check belongs in the handlers rather than at the boundary: the Cloud Map
/// namespace-name patterns open with the negative lookahead `(?!arn:)`, the vendored
/// boundary validator compiles a `@pattern` with the `regex` crate, which has no
/// look-around, and it treats a pattern it cannot compile as matching
/// (`pattern_matches` in `vendor/smithy-schema-server/src/constraints.rs`). So the
/// constraint the model declares would otherwise never be applied to those members.
///
/// The check is the pattern: `pattern` is matched by a look-around-capable engine
/// (`fancy-regex`, as `service-com-amazonaws-odb-odb` already does for the same
/// reason), so the clause — which quotes the model's own source text — cannot state a
/// constraint this function does not apply.
pub fn namespace_name_violation(member: &str, name: &str, pattern: &'static str) -> Option<String> {
    if namespace_name_matches(pattern, name) {
        return None;
    }
    ::aws_smithy_schema_server::envelope(&[::smithy_server_runtime::Violation {
        path: member.to_string(),
        kind: ::smithy_server_runtime::ViolationKind::Pattern(pattern.to_string()),
        value: Some(echoed(name)),
        sensitive: false,
    }])
}

/// The namespace-name patterns of this model, each compiled once.
///
/// Keyed by the pattern source so the compiled form can never drift from the text the
/// clause quotes. Each is built with an explicit backtrack budget
/// ([`NAMESPACE_NAME_BACKTRACK_LIMIT`]), which is what bounds the work one match may
/// do on a backtracking engine. Compilation is fallible and its input is a COMPILE-TIME
/// CONSTANT, never request text, but a pattern that fails to compile is still dropped
/// rather than unwrapped: nothing here panics, on any input.
static NAMESPACE_NAME_REGEXES: LazyLock<HashMap<&'static str, ::fancy_regex::Regex>> =
    LazyLock::new(|| {
        [NAMESPACE_NAME_HTTP_PATTERN, NAMESPACE_NAME_PRIVATE_PATTERN]
            .into_iter()
            .filter_map(|pattern| {
                ::fancy_regex::RegexBuilder::new(pattern)
                    .backtrack_limit(NAMESPACE_NAME_BACKTRACK_LIMIT)
                    .build()
                    .ok()
                    .map(|compiled| (pattern, compiled))
            })
            .collect()
    });

/// Whether `name` satisfies the namespace-name `@pattern` `pattern`.
///
/// A pattern this crate holds no compiled form for, and a match that exhausts its
/// backtrack budget, are both treated as matching — which is what the boundary
/// validator does with a pattern it cannot compile: a value is never refused because of
/// a model pattern nothing can evaluate.
fn namespace_name_matches(pattern: &str, name: &str) -> bool {
    NAMESPACE_NAME_REGEXES
        .get(pattern)
        .is_none_or(|compiled| compiled.is_match(name).unwrap_or(true))
}

/// A rejected value as a clause may echo it: itself when short, a
/// character-boundary-safe prefix with an elision when not, so a caller-controlled
/// value cannot amplify a request into an arbitrarily long response.
///
/// The vendored validator keeps the same helper for the clauses IT builds, but it is
/// private and `vendor/` is read-only here, so a handler that builds a clause of its
/// own — which is expected to pass an already-elided `Violation::value` — elides it
/// through this copy, as the other services that do the same thing already do.
fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// A `ResourceDescription` as it is PERSISTED.
///
/// The model gives `Description` no default and length 0..1024, and a resource created
/// with `Description=""` is indistinguishable on a read from one created with no
/// `Description` at all: the member is absent from both. So an empty string carries no
/// description and is not stored, while any non-empty text round-trips unchanged.
pub fn described(description: Option<String>) -> Option<String> {
    description.filter(|text| !text.is_empty())
}

/// The routing policy a `DnsConfig` carries when its creator stated none.
///
/// `DnsConfig$RoutingPolicy` is optional on input and names "the routing policy that you
/// want to apply to all Route 53 DNS records that Cloud Map creates"; a stored service
/// always applies one, and the one it applies when the caller named none is the
/// multivalue answer policy (the other member of the modelled `RoutingPolicy` enum,
/// `WEIGHTED`, is what a creator asks for explicitly).
pub const DEFAULT_ROUTING_POLICY: &str = "MULTIVALUE";

/// The `Properties.DnsProperties.SOA.TTL` a namespace reports when nothing ever set one
/// for it.
///
/// `SOA$TTL` is "the time to live (TTL) for purposes of negative caching" and is modelled
/// `@required` on the `SOA` a namespace read reports, so the member always carries a
/// number; a namespace no creator or update ever gave a TTL — every namespace that holds
/// no SOA record of its own — caches no negative answer for any time at all, the low bound
/// of the modelled `RecordTTL` range (0..=2147483647).
pub const NO_NEGATIVE_CACHING_TTL: i64 = 0;

/// A `DnsConfig` as it is PERSISTED: the configuration the caller stated, plus the
/// members the service itself decides.
///
/// Both are members a read reports whether or not the request carried them, so they are
/// resolved ONCE here — where the record is written — rather than per read path:
///
/// * `RoutingPolicy` — [`DEFAULT_ROUTING_POLICY`] when the request named none.
/// * `NamespaceId` — the namespace that owns the service, which is what this
///   (deprecated) member names; the request may have left it out, or named the namespace
///   by ARN, and the record holds the resolved id either way.
pub fn stored_dns_config(requested: DnsConfig, namespace_id: &str) -> DnsConfig {
    DnsConfig {
        namespace_id: Some(namespace_id.to_string()),
        routing_policy: Some(
            requested
                .routing_policy
                .unwrap_or_else(|| DEFAULT_ROUTING_POLICY.to_string()),
        ),
        dns_records: requested.dns_records,
    }
}

/// Fold a digest into `digits` id characters.
///
/// Cloud Map ids are lowercase alphanumeric, so the digest bytes are folded into base
/// 36 rather than rendered as hex. `digits` is clamped to the digest width, so an id
/// body is at most 32 characters long.
fn fold_id_digits(digest: &[u8], digits: usize) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    digest
        .iter()
        .take(digits.min(digest.len()))
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// How many characters a service `Id` carries behind its `srv-` prefix.
const SERVICE_ID_DIGITS: usize = 16;

/// How many characters an `OperationId` carries in its leading body segment.
///
/// Cloud Map's operation ids are opaque two-segment tokens — a body, a hyphen and a
/// shorter trailing segment (`4yejorelbukcjzpnr6tlmrghsjwpngf4-k95yg2u7` in the API
/// reference's `RegisterInstance`/`GetOperation` examples) — and the model constrains
/// only the overall length (`OperationId`, 0..255). Both segments are lowercase
/// alphanumeric, so the whole token is 41 characters.
const OPERATION_ID_BODY_DIGITS: usize = 32;

/// How many characters an `OperationId` carries in the segment behind its hyphen. See
/// [`OPERATION_ID_BODY_DIGITS`].
const OPERATION_ID_SUFFIX_DIGITS: usize = 8;

/// The `Id` of the service called `name` in the namespace `namespace_id`.
///
/// A service is addressed inside its namespace by its `Name` — the pair is what
/// `ServiceAlreadyExists` is raised for and what `DiscoverInstances` resolves — so the
/// identifier is a FUNCTION of that pair and of nothing else: no counter and no clock
/// is read. Two consequences are what callers rely on: the same name created again in
/// the same namespace, after a `DeleteService` freed it, is answered with the
/// identifier the first create answered with, and the same name under a different
/// namespace gets a different identifier.
pub fn derived_service_id(namespace_id: &str, name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"service");
    // A zero byte neither identifier can contain separates them, so no two distinct
    // pairs hash the same bytes.
    hasher.update(namespace_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(name.as_bytes());
    format!(
        "srv-{}",
        fold_id_digits(&hasher.finalize(), SERVICE_ID_DIGITS)
    )
}

/// What a DNS-namespace update asked for, as an idempotency `request` key.
///
/// `UpdaterRequestId` "allows failed Update…Namespace requests to be retried without
/// the risk of running the operation twice", so the remembered request must identify
/// the whole change: the resolved namespace and every property the call asked to
/// change. The two options are rendered with their `Debug` form so an absent member
/// (which leaves the stored value alone) is never the same key as a member carrying the
/// empty string or a zero TTL.
pub fn dns_namespace_change_request(
    namespace_id: &str,
    description: Option<&str>,
    soa_ttl: Option<i64>,
) -> String {
    format!("{namespace_id}:{description:?}:{soa_ttl:?}")
}

/// The message a namespace update is refused with when the stored namespace is of
/// another `NamespaceType` than the one that update applies to.
///
/// Each `Update*Namespace` operation is documented against ONE namespace kind, and its
/// change shape carries only the properties that kind has (an HTTP namespace has no
/// SOA, a DNS namespace no `HttpName`), so asking the wrong kind of update of a
/// namespace that does exist is a bad request rather than a missing namespace. The
/// message reports the comparison a caller has to make to fix the request: the
/// `NamespaceType` the stored namespace actually has, and the one the operation it
/// called requires. Neither the namespace nor the operation is named — the caller knows
/// both — so one text serves every namespace update.
pub fn wrong_namespace_type_message(stored_type: &str, required_type: &str) -> String {
    format!(
        "Namespace with incorrect type {stored_type} provided. Please provide namespace with \
         type {required_type}."
    )
}

/// The opening of the message a filtered listing refuses unsupported filter conditions
/// with, which is followed by one clause per offending filter
/// ([`filter_condition_clause`]).
const INVALID_FILTER_CONDITIONS: &str = "Invalid filter condition(s) provided: ";

/// One clause of that message: the filter the request named, the conditions that filter
/// supports, and the condition the request asked for.
///
/// A request may state several filters, so the message accumulates one clause per
/// offending filter and each clause ends with its own separator — which is why the
/// separator belongs to the clause rather than to the join.
fn filter_condition_clause(name: &str, supported: &[&str], condition: &str) -> String {
    format!(
        "filter {name} supports condition(s) [{}], but got {condition}; ",
        supported.join(", ")
    )
}

/// The `InvalidInput` message for every filter of one request whose `Condition` its
/// `Name` does not support, or `None` when every filter's condition is supported.
///
/// `stated` is the (filter name, condition) pair each filter of the request states, in the
/// order the request stated them, and `supported` is the listing's own condition matrix —
/// `None` from it for a name that listing does not declare, which is refused as a bad
/// filter NAME rather than for its condition.
///
/// One message for the whole request: the clauses are reported together, so a caller that
/// sent several bad filters learns about all of them from one refusal. This is why the
/// condition check runs over every filter before any other filter rule — a request refused
/// here is refused for its conditions alone. The matrix is each listing's; the clause and
/// the envelope are here, so no two filtered listings word this refusal differently.
pub fn unsupported_filter_conditions<'f>(
    stated: impl IntoIterator<Item = (&'f str, &'f str)>,
    supported: impl Fn(&str) -> Option<&'static [&'static str]>,
) -> Option<String> {
    let clauses: Vec<String> = stated
        .into_iter()
        .filter_map(|(name, condition)| {
            let supported = supported(name)?;
            (!supported.contains(&condition))
                .then(|| filter_condition_clause(name, supported, condition))
        })
        .collect();
    invalid_filter_conditions_message(&clauses)
}

/// The envelope [`unsupported_filter_conditions`] states those clauses in, or `None` when
/// there are none.
fn invalid_filter_conditions_message(clauses: &[String]) -> Option<String> {
    (!clauses.is_empty()).then(|| format!("{INVALID_FILTER_CONDITIONS}{}", clauses.concat()))
}

/// The message a namespace create is refused with when the `Name` is already owned by
/// a create that has already completed.
///
/// The text names the create that owns the name and the id a caller can poll, which is
/// the whole content of the refusal: the duplicate is a *request* conflict, not a bad
/// value. `operation` is the operation whose name was duplicated, so the same wording
/// serves every namespace create.
pub fn duplicate_create_message(operation: &str, completed: Option<&str>) -> String {
    match completed {
        Some(operation_id) => {
            format!("Another operation of type {operation} and id {operation_id} has completed")
        }
        // A namespace stored without the id of its create (a record seeded directly
        // into the store) still conflicts; there is just no id to poll.
        None => format!("Another operation of type {operation} has completed"),
    }
}

/// The `ServiceAlreadyExists` message, which is a CONSTANT: the service reports the
/// class of conflict and names neither the service nor the namespace the caller asked
/// for, so no request value reaches the body.
pub const SERVICE_ALREADY_EXISTS_MESSAGE: &str = "Service already exists.";

/// The `InvalidInput` message a service-settings update is refused with when the
/// service belongs to an HTTP namespace.
///
/// `UpdateService` documents the refusal — "You can't call UpdateService and update
/// settings in the following scenarios: When the service is associated with an HTTP
/// namespace" — and an HTTP namespace is the API-only kind: it holds no Route 53 hosted
/// zone, so there are no records or health checks for such a call to update. FIXED text,
/// naming neither the service nor the operation.
pub const API_ONLY_NAMESPACE_UPDATE_MESSAGE: &str =
    "Service in API-only namespace cannot be updated.";

/// A persisted namespace. `ServiceCount` is NOT a field: it is derived from
/// [`ComAmazonawsServicediscoveryRoute53autonamingV20170314State::service_count`] so
/// the namespace and the service map can never disagree.
#[derive(Debug, Clone)]
pub struct NamespaceRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// A `NamespaceType` value (`HTTP`, `DNS_PRIVATE`, `DNS_PUBLIC`).
    pub namespace_type: String,
    pub description: Option<String>,
    pub create_date: SystemTime,
    pub creator_request_id: Option<String>,
    /// `Properties.HttpProperties.HttpName` — the name instances are discovered under.
    pub http_name: Option<String>,
    /// `Properties.DnsProperties.HostedZoneId`, set only for a DNS namespace.
    pub hosted_zone_id: Option<String>,
    /// `Properties.DnsProperties.SOA.TTL` — the TTL "for purposes of negative
    /// caching" of the hosted zone's Start of Authority record, held only for a
    /// namespace whose creator or updater stated one. A namespace that was never given
    /// a TTL reports [`NO_NEGATIVE_CACHING_TTL`] on the `SOA` every namespace read
    /// carries; nothing else is invented for it.
    pub soa_ttl: Option<i64>,
    /// The `OperationId` of the create that owns this name, which is what a later
    /// create of the SAME name is refused with (`DuplicateRequest`).
    pub create_operation_id: Option<String>,
    pub tags: Vec<(String, String)>,
}

/// A persisted service. `InstanceCount` is derived from `instances`, and
/// `InstancesRevision` is bumped on every register/deregister.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub namespace_id: String,
    pub description: Option<String>,
    pub dns_config: Option<DnsConfig>,
    pub health_check_config: Option<HealthCheckConfig>,
    pub health_check_custom_config: Option<HealthCheckCustomConfig>,
    /// A `ServiceType` value (`HTTP`, `DNS`, `DNS_HTTP`).
    pub service_type: String,
    pub create_date: SystemTime,
    pub creator_request_id: Option<String>,
    pub tags: Vec<(String, String)>,
    /// Keyed by `InstanceId`; a `BTreeMap` so every read orders identically.
    pub instances: BTreeMap<String, InstanceRecord>,
    pub instances_revision: i64,
    /// The service-level attributes `UpdateServiceAttributes` writes,
    /// `GetServiceAttributes` reads and `DeleteServiceAttributes` removes keys from.
    /// A `BTreeMap` so every read orders identically; bounded by
    /// [`MAX_SERVICE_ATTRIBUTES`].
    pub attributes: BTreeMap<String, String>,
}

impl ServiceRecord {
    /// Apply to every instance of this service the health status change the clock
    /// already owes it, so a read reports it.
    pub fn apply_due_health_statuses(&mut self, now: SystemTime) {
        for instance in self.instances.values_mut() {
            instance.apply_due_health_status(now);
        }
    }

    /// Whether this service's health checking is the custom kind, the only kind
    /// `UpdateInstanceCustomHealthStatus` can set: "You can use
    /// UpdateInstanceCustomHealthStatus to change the status only for custom health
    /// checks, which you define using HealthCheckCustomConfig … You can't use it to
    /// change the status for Route 53 health checks, which you define using
    /// HealthCheckConfig."
    pub fn custom_health_checked(&self) -> bool {
        self.health_check_custom_config.is_some()
    }

    /// `InstanceCount`, derived from what is stored rather than counted by hand.
    pub fn instance_count(&self) -> i32 {
        i32::try_from(self.instances.len()).unwrap_or(i32::MAX)
    }

    /// Whether the service configures health checking at all. Documented on
    /// `DiscoverInstancesRequest$HealthStatus`: the filter "is ignored for services
    /// that don't have a health check configured, and all instances are returned".
    pub fn health_checked(&self) -> bool {
        self.health_check_config.is_some() || self.health_check_custom_config.is_some()
    }
}

/// A persisted service instance.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub instance_id: String,
    pub attributes: HashMap<String, String>,
    pub creator_request_id: Option<String>,
    /// `CreatedByAccount` — "the ID of the Amazon Web Services account that registered
    /// the instance". Captured when the registration is stored, so a read reports who
    /// registered this instance rather than who is reading it.
    pub created_by_account: String,
    /// A `HealthStatus` value. An instance starts `UNKNOWN`: nothing has reported on
    /// it yet, neither the service's own health checker nor an
    /// `UpdateInstanceCustomHealthStatus` call.
    pub health_status: String,
    /// The health status the clock still owes this instance — the one a client
    /// submitted for a custom health check, or the verdict of the service's Route 53
    /// health check. `None` once it has been applied, and for an instance of a service
    /// that configures no health checking at all.
    pub pending_health_status: Option<PendingHealthStatus>,
}

impl InstanceRecord {
    /// Apply the owed health status when the clock has reached its due instant.
    ///
    /// A read may do this: the status was due to change at `due_at` whether or not
    /// anybody asked, exactly as [`ComAmazonawsServicediscoveryRoute53autonamingV20170314State::operation`]
    /// materialises an operation's terminal status.
    pub fn apply_due_health_status(&mut self, now: SystemTime) {
        if let Some(pending) = &self.pending_health_status {
            if now >= pending.due_at {
                self.health_status = pending.status.clone();
                self.pending_health_status = None;
            }
        }
    }

    /// The health status the clock still owes this instance, if any.
    pub fn owed_health_status(&self) -> Option<&str> {
        self.pending_health_status
            .as_ref()
            .map(|pending| pending.status.as_str())
    }
}

/// A health status the service has decided on that the clock has not applied yet.
#[derive(Debug, Clone)]
pub struct PendingHealthStatus {
    /// A `HealthStatus` value.
    pub status: String,
    /// When the clock owes the instance this status.
    pub due_at: SystemTime,
}

/// A persisted asynchronous operation, the record `GetOperation` reads.
#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub id: String,
    /// An `OperationType` value (`CREATE_NAMESPACE`, `REGISTER_INSTANCE`, …).
    pub operation_type: String,
    /// An `OperationStatus` value.
    pub status: String,
    pub create_date: SystemTime,
    pub update_date: SystemTime,
    /// When the clock owes this operation its terminal status.
    pub success_due_at: SystemTime,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    /// `OperationTargetType` -> resource id (`NAMESPACE`, `SERVICE`, `INSTANCE`).
    pub targets: HashMap<String, String>,
}

/// A resource one of this service's tagging operations addresses, identified by the
/// record that holds its tags.
///
/// Cloud Map's taggable resources are its namespaces and its services (`Tag` is "a
/// custom key-value pair that's associated with a resource", and both carry a `Tags`
/// member on creation), and `TagResource`, `UntagResource` and `ListTagsForResource`
/// all address them the same way — by `ResourceARN` — so one resolution serves all
/// three rather than each handler knowing the resource tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// The namespace with this `Id`.
    Namespace(String),
    /// The service with this `Id`.
    Service(String),
}

/// Which call an idempotency token was first seen on, so one client's
/// `CreatorRequestId` cannot collide across unrelated operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdempotencyScope {
    CreateHttpNamespace,
    CreatePrivateDnsNamespace,
    CreatePublicDnsNamespace,
    CreateService,
    RegisterInstance,
    UpdateHttpNamespace,
    UpdatePrivateDnsNamespace,
    UpdatePublicDnsNamespace,
}
impl IdempotencyScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::CreateHttpNamespace => "CreateHttpNamespace",
            Self::CreatePrivateDnsNamespace => "CreatePrivateDnsNamespace",
            Self::CreatePublicDnsNamespace => "CreatePublicDnsNamespace",
            Self::CreateService => "CreateService",
            Self::RegisterInstance => "RegisterInstance",
            Self::UpdateHttpNamespace => "UpdateHttpNamespace",
            Self::UpdatePrivateDnsNamespace => "UpdatePrivateDnsNamespace",
            Self::UpdatePublicDnsNamespace => "UpdatePublicDnsNamespace",
        }
    }
}

/// Per-service state for com-amazonaws-servicediscovery-route53autonaming-v20170314. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsServicediscoveryRoute53autonamingV20170314State {
    /// Namespaces by `Id`.
    pub namespaces: HashMap<String, NamespaceRecord>,
    /// `Name` -> namespace `Id`: what `DiscoverInstances` resolves and what makes
    /// `NamespaceAlreadyExists` detectable.
    pub namespace_ids_by_name: HashMap<String, String>,
    /// Services by `Id`.
    pub services: HashMap<String, ServiceRecord>,
    /// (namespace `Id`, service `Name`) -> service `Id`: resolves
    /// `DiscoverInstances`' `ServiceName` and enforces `ServiceAlreadyExists`.
    pub service_ids_by_namespace_and_name: HashMap<(String, String), String>,
    /// Operations by `OperationId`, bounded by [`MAX_RETAINED_OPERATIONS`].
    pub operations: HashMap<String, OperationRecord>,
    /// scoped `CreatorRequestId` -> the id the first call returned, bounded by
    /// [`MAX_RETAINED_IDEMPOTENCY_TOKENS`].
    pub idempotency: HashMap<String, String>,
    /// The idempotency keys in the order they were first remembered, so the oldest
    /// token is the one forgotten when the bound is reached.
    idempotency_order: VecDeque<String>,
    /// Monotonic counter that makes every minted id unique within this emulator.
    id_seq: u64,
}

impl ComAmazonawsServicediscoveryRoute53autonamingV20170314State {
    /// Mint an opaque id of the form `<prefix>-<digits lowercase alphanumerics>`
    /// (just the digits when `prefix` is empty), unique for the lifetime of this store.
    ///
    /// The digits come from a SHA-256 of the monotonic counter and the wall clock, so
    /// an id is opaque, unique for the lifetime of this store and derived from no
    /// request value. `digits` is clamped to the digest width, so an id is at most 32
    /// characters long.
    pub fn next_id(&mut self, prefix: &str, digits: usize) -> String {
        let body = self.mint_digits(prefix, digits);
        if prefix.is_empty() {
            body
        } else {
            format!("{prefix}-{body}")
        }
    }

    /// Mint `digits` opaque lowercase-alphanumeric id characters.
    ///
    /// `domain` labels the run so the several segments of one id are independent of
    /// each other; each call also advances the store's counter, so no two runs repeat.
    fn mint_digits(&mut self, domain: &str, digits: usize) -> String {
        self.id_seq = self.id_seq.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(domain.as_bytes());
        hasher.update(self.id_seq.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        fold_id_digits(&hasher.finalize(), digits)
    }

    /// Mint the `OperationId` an asynchronous call reports.
    ///
    /// The token is two hyphen-separated runs of lowercase alphanumerics — see
    /// [`OPERATION_ID_BODY_DIGITS`] — and is opaque: each run is derived from the
    /// store's counter and the clock, never from any request value, so the same
    /// request made twice is answered with two different ids.
    fn next_operation_id(&mut self) -> String {
        format!(
            "{}-{}",
            self.mint_digits("operation", OPERATION_ID_BODY_DIGITS),
            self.mint_digits("operation-suffix", OPERATION_ID_SUFFIX_DIGITS)
        )
    }

    /// Mint the id of the Route 53 hosted zone a DNS namespace carries —
    /// `DnsProperties$HostedZoneId` is "the ID for the Route 53 hosted zone that Cloud
    /// Map creates when you create a namespace".
    ///
    /// Route 53 hosted zone ids are an uppercase alphanumeric string that starts with
    /// `Z`, so the minted digits of [`Self::next_id`] are upper-cased behind that
    /// prefix. The id is opaque: it is derived from the store's counter and the clock,
    /// never from the namespace name.
    pub fn next_hosted_zone_id(&mut self) -> String {
        format!("Z{}", self.next_id("", 20).to_uppercase())
    }

    /// File an operation record and return its `OperationId`.
    ///
    /// The record starts `SUBMITTED` with its success due [`OPERATION_SUCCESS_SPAN`]
    /// later; [`Self::operation`] materialises the transition when the clock reaches it.
    pub fn record_operation(
        &mut self,
        operation_type: &str,
        now: SystemTime,
        targets: impl IntoIterator<Item = (String, String)>,
    ) -> String {
        let id = self.next_operation_id();
        self.operations.insert(
            id.clone(),
            OperationRecord {
                id: id.clone(),
                operation_type: operation_type.to_string(),
                status: OPERATION_STATUS_SUBMITTED.to_string(),
                create_date: now,
                update_date: now,
                success_due_at: now + OPERATION_SUCCESS_SPAN,
                error_code: None,
                error_message: None,
                targets: targets.into_iter().collect(),
            },
        );
        self.evict_oldest_operations();
        id
    }

    /// Keep [`Self::operations`] within [`MAX_RETAINED_OPERATIONS`], dropping the
    /// oldest record that has already reached a terminal status first so a caller
    /// polling a fresh operation can still resolve it.
    ///
    /// A token remembered for an evicted operation is forgotten with it, so a replay
    /// never hands back an id `GetOperation` can no longer resolve.
    fn evict_oldest_operations(&mut self) {
        while self.operations.len() > MAX_RETAINED_OPERATIONS {
            let victim = self
                .operations
                .values()
                .min_by_key(|record| {
                    (
                        record.status == OPERATION_STATUS_SUBMITTED,
                        record.create_date,
                    )
                })
                .map(|record| record.id.clone());
            match victim {
                Some(id) => {
                    self.operations.remove(&id);
                    // Forget every token that produced this id, and keep the order
                    // queue in step with the map so no dead key is ever popped in
                    // place of a live one.
                    let tokens = &mut self.idempotency;
                    let order = &mut self.idempotency_order;
                    tokens.retain(|_, produced| *produced != id);
                    order.retain(|key| tokens.contains_key(key));
                }
                None => break,
            }
        }
    }

    /// Apply the status transition the clock already owes `record`.
    ///
    /// Writing here is what a reporting call may do: the operation was due to succeed
    /// at `success_due_at` whether or not anybody asked, and `update_date` is the
    /// instant it became due rather than the instant somebody noticed, so two readers
    /// of the same record report the same dates.
    fn materialise(record: &mut OperationRecord, now: SystemTime) {
        if record.status == OPERATION_STATUS_SUBMITTED && now >= record.success_due_at {
            record.status = OPERATION_STATUS_SUCCESS.to_string();
            record.update_date = record.success_due_at;
        }
    }

    /// The operation `id`, with any status transition the clock already owes it
    /// applied first.
    pub fn operation(&mut self, id: &str, now: SystemTime) -> Option<&OperationRecord> {
        let record = self.operations.get_mut(id)?;
        Self::materialise(record, now);
        Some(record)
    }

    /// Every stored operation in one stable total order — oldest first, `OperationId`
    /// breaking a tie — each with the status transition the clock already owes it
    /// applied first, exactly as [`Self::operation`] applies it to a single record.
    ///
    /// The order is a property of the records themselves (create instant, then id), not
    /// of the map's iteration order, so the page boundary a `ListOperations`
    /// `NextToken` names stays put as later operations are filed.
    pub fn operations_in_order(&mut self, now: SystemTime) -> Vec<OperationRecord> {
        for record in self.operations.values_mut() {
            Self::materialise(record, now);
        }
        let mut ordered: Vec<OperationRecord> = self.operations.values().cloned().collect();
        ordered
            .sort_by(|left, right| (left.create_date, &left.id).cmp(&(right.create_date, &right.id)));
        ordered
    }

    /// `ServiceCount` for a namespace, derived from the service map.
    pub fn service_count(&self, namespace_id: &str) -> i32 {
        let count = self
            .services
            .values()
            .filter(|service| service.namespace_id == namespace_id)
            .count();
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// Resolve a namespace reference — the members that name one are modelled as
    /// `Arn`, and the docs say a namespace shared with the account is named by its
    /// ARN — to a stored namespace id.
    pub fn resolve_namespace_id(&self, reference: &str) -> Option<String> {
        if self.namespaces.contains_key(reference) {
            return Some(reference.to_string());
        }
        self.namespaces
            .values()
            .find(|namespace| namespace.arn == reference)
            .map(|namespace| namespace.id.clone())
    }

    /// The `NamespaceType` of the stored namespace `namespace_id`, or the empty string
    /// when no namespace is stored under that id.
    pub fn namespace_type(&self, namespace_id: &str) -> String {
        self.namespaces
            .get(namespace_id)
            .map(|namespace| namespace.namespace_type.clone())
            .unwrap_or_default()
    }

    /// Apply to the stored namespace `namespace_id` the change a DNS-namespace update
    /// asks for.
    ///
    /// Both members of a DNS namespace change are OPTIONAL
    /// (`Private`/`PublicDnsNamespaceChange` declare `Description` and `Properties`
    /// without `@required`), so a member the change omits is left exactly as it was and
    /// a call that carries one property updates only that one. A `Description` the
    /// change does carry replaces the stored one — the empty string storing none, as
    /// [`described`] decides for every resource description — and a
    /// `Properties.DnsProperties.SOA.TTL` it carries, the updated TTL for purposes of
    /// negative caching, replaces the stored TTL, which is what a later `GetNamespace`
    /// reports under `Properties.DnsProperties.SOA`.
    ///
    /// Writing through the one stored record is what makes the namespace updates and
    /// every namespace read agree; nothing is cached beside it.
    pub fn apply_dns_namespace_change(
        &mut self,
        namespace_id: &str,
        description: Option<String>,
        soa_ttl: Option<i64>,
    ) {
        if let Some(namespace) = self.namespaces.get_mut(namespace_id) {
            if let Some(description) = description {
                namespace.description = described(Some(description));
            }
            if let Some(ttl) = soa_ttl {
                namespace.soa_ttl = Some(ttl);
            }
        }
    }

    /// Resolve a service reference (its id or its ARN) to a stored service id.
    pub fn resolve_service_id(&self, reference: &str) -> Option<String> {
        if self.services.contains_key(reference) {
            return Some(reference.to_string());
        }
        self.services
            .values()
            .find(|service| service.arn == reference)
            .map(|service| service.id.clone())
    }

    /// The stored resource an `AmazonResourceName` names, or `None` when no resource
    /// in this store carries that ARN.
    ///
    /// The tagging operations are documented against a `ResourceARN`, so the match is
    /// on the ARN the create minted — a resource id is not an ARN and names nothing
    /// here.
    pub fn resolve_tagged_resource(&self, resource_arn: &str) -> Option<TaggedResource> {
        if let Some(namespace) = self
            .namespaces
            .values()
            .find(|namespace| namespace.arn == resource_arn)
        {
            return Some(TaggedResource::Namespace(namespace.id.clone()));
        }
        self.services
            .values()
            .find(|service| service.arn == resource_arn)
            .map(|service| TaggedResource::Service(service.id.clone()))
    }

    /// The tags stored on a resolved resource, in the order they were first set.
    pub fn tags(&self, resource: &TaggedResource) -> Option<&[(String, String)]> {
        match resource {
            TaggedResource::Namespace(id) => {
                self.namespaces.get(id).map(|record| record.tags.as_slice())
            }
            TaggedResource::Service(id) => {
                self.services.get(id).map(|record| record.tags.as_slice())
            }
        }
    }

    /// The tags stored on a resolved resource, for a tagging call to write through.
    pub fn tags_mut(&mut self, resource: &TaggedResource) -> Option<&mut Vec<(String, String)>> {
        match resource {
            TaggedResource::Namespace(id) => {
                self.namespaces.get_mut(id).map(|record| &mut record.tags)
            }
            TaggedResource::Service(id) => self.services.get_mut(id).map(|record| &mut record.tags),
        }
    }

    /// The namespace `DiscoverInstances`' `NamespaceName` names.
    ///
    /// That member is documented as the namespace's `HttpName`, which "in most cases"
    /// equals its `Name`, so the `HttpName` is matched first and the `Name` index is
    /// the fallback.
    pub fn namespace_by_http_name(&self, http_name: &str) -> Option<&NamespaceRecord> {
        if let Some(found) = self
            .namespaces
            .values()
            .find(|namespace| namespace.http_name.as_deref() == Some(http_name))
        {
            return Some(found);
        }
        self.namespace_ids_by_name
            .get(http_name)
            .and_then(|id| self.namespaces.get(id))
    }

    /// The id a previous call with this `CreatorRequestId` returned, if any.
    ///
    /// `request` identifies WHAT was asked for (the namespace name, the service's
    /// namespace and name, the service and instance id, the namespace and the change
    /// asked of it), so a token reused for a
    /// different resource is not treated as a retry of the first one — a retry that
    /// silently stored nothing while answering 200 would be indistinguishable from a
    /// stored write.
    pub fn replayed(
        &self,
        scope: IdempotencyScope,
        token: &str,
        request: &str,
    ) -> Option<String> {
        self.idempotency
            .get(&Self::idempotency_key(scope, token, request))
            .cloned()
    }

    /// The `OperationId` a previous create of the namespace `name` with this
    /// `CreatorRequestId` answered with — but ONLY while that namespace is still
    /// stored.
    ///
    /// Every namespace create is idempotent on its `CreatorRequestId`, so a retry of
    /// the SAME request replays the operation the first call filed instead of creating
    /// a second namespace. Once `DeleteNamespace` has removed the namespace the token
    /// is stale: replaying it would answer success for a create this call never made,
    /// which is a discarded write indistinguishable from a stored one, so the retry is
    /// served as a fresh create. The rule is the same for every namespace type, so it
    /// lives here rather than in one handler.
    pub fn replayed_namespace_create(
        &self,
        scope: IdempotencyScope,
        token: Option<&str>,
        name: &str,
    ) -> Option<String> {
        if !self.namespace_ids_by_name.contains_key(name) {
            return None;
        }
        self.replayed(scope, token?, name)
    }

    /// Remember what a `CreatorRequestId` produced for this request, so a retry of the
    /// SAME request replays it. Keeps the token store within
    /// [`MAX_RETAINED_IDEMPOTENCY_TOKENS`], forgetting the oldest token first.
    pub fn remember(
        &mut self,
        scope: IdempotencyScope,
        token: &str,
        request: &str,
        produced: &str,
    ) {
        let key = Self::idempotency_key(scope, token, request);
        if self
            .idempotency
            .insert(key.clone(), produced.to_string())
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_RETAINED_IDEMPOTENCY_TOKENS {
            match self.idempotency_order.pop_front() {
                Some(forgotten) => {
                    self.idempotency.remove(&forgotten);
                }
                None => break,
            }
        }
    }

    fn idempotency_key(scope: IdempotencyScope, token: &str, request: &str) -> String {
        format!("{}:{token}:{request}", scope.as_str())
    }
}

/// The region the request was signed for, which every ARN this service mints carries.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// An ARN for one of this service's resources, e.g. `namespace/ns-…`.
pub fn service_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, resource)
}

/// The account that created and owns every resource this emulator holds, as the
/// 12-character `AWSAccountId` the ownership members carry.
///
/// Each of those members — `Namespace$ResourceOwner`, `Service$ResourceOwner` /
/// `$CreatedByAccount`, `ServiceAttributes$ResourceOwner`, `Instance$CreatedByAccount`,
/// `Operation$OwnerAccount` and their listing counterparts — is documented as "the ID
/// of the Amazon Web Services account that created/owns" the resource, and names
/// another account ONLY when the namespace was shared in from one. This emulator serves
/// a single account, the one its ARNs carry, so every resource it holds was created by
/// and is owned by that account and every read reports it: the member is a fact of the
/// resource, not a cross-account marker, so it is never omitted.
pub fn owning_account() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The ARN form of a service reference a request named, for an error that answers with
/// the service the caller asked for rather than with a sentence about it.
///
/// Every member that names a service is modelled `Arn` and documented as "the ID or
/// Amazon Resource Name (ARN)", so a caller may name one either way; this renders the
/// one ARN form whichever form the request used. Nothing is looked up: the reference
/// this is built for resolves to no stored service, so the ARN is composed from the
/// request and the region it was signed for, exactly as a create would have minted it.
pub fn service_reference_arn(region: &str, reference: &str) -> String {
    if reference.starts_with(ARN_SCHEME_PREFIX) {
        return reference.to_string();
    }
    service_arn(region, &format!("service/{reference}"))
}

/// What every ARN starts with, so a reference already in ARN form is recognised rather
/// than wrapped in a second one.
const ARN_SCHEME_PREFIX: &str = "arn:";

/// A per-call sort key for one instance id.
///
/// `DiscoverInstances` is documented to return "a randomized list of instances
/// allowing customers to distribute traffic evenly": sorting by this key permutes the
/// list from `nonce` (the caller's wall clock), so the permutation changes between
/// calls without any stored counter being bumped by a read.
pub fn shuffle_key(nonce: u128, instance_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(nonce.to_be_bytes());
    hasher.update(instance_id.as_bytes());
    hasher.finalize().into()
}

/// The separator between a `NextToken`'s scope and its cursor.
///
/// A unit separator cannot appear in a resource id or in an operation name, so splitting
/// a decoded token on it recovers exactly the two fields that were encoded.
const NEXT_TOKEN_SEPARATOR: char = '\u{1f}';

/// An opaque `NextToken` that resumes a paginated read after `cursor`.
///
/// `scope` is the operation that issued it, so a token minted by one paginated read is
/// not honoured as a page boundary by another. The payload is hex-encoded because the
/// member is modelled as an opaque string the caller only ever echoes back.
pub fn encode_next_token(scope: &str, cursor: &str) -> String {
    ::hex::encode(format!("{scope}{NEXT_TOKEN_SEPARATOR}{cursor}"))
}

/// The cursor a `NextToken` carries for `scope`, or `None` when the token is not one
/// this operation issued — which covers a token that is not hex, is not UTF-8, carries
/// no separator, or names another operation's scope. A forged token is therefore
/// refused rather than silently honoured as "start from the beginning".
pub fn decode_next_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (encoded_scope, cursor) = decoded.split_once(NEXT_TOKEN_SEPARATOR)?;
    (encoded_scope == scope).then(|| cursor.to_string())
}

/// A stored instant as the wire `timestamp` the modelled shapes carry.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64()),
        Err(_) => ::aws_smithy_types::DateTime::from_secs(0),
    }
}

/// The modelled `Namespace` for a stored namespace, with `ServiceCount` passed in
/// because it is derived from the service map rather than held on the record.
pub fn namespace_view(record: &NamespaceRecord, service_count: i32) -> Namespace {
    Namespace {
        id: Some(record.id.clone()),
        arn: Some(record.arn.clone()),
        resource_owner: Some(owning_account()),
        name: Some(record.name.clone()),
        r#type: Some(record.namespace_type.clone()),
        description: record.description.clone(),
        service_count: Some(service_count),
        properties: Some(NamespaceProperties {
            // `NamespaceProperties` is "information that's specific to the namespace
            // type", and the DNS side of it — the hosted zone Cloud Map keeps for the
            // namespace and the Start of Authority record of that zone — is part of
            // what a namespace IS rather than something a read leaves out: the
            // structure is reported for every namespace, carrying the properties that
            // namespace actually has.
            dns_properties: Some(crate::ops::types::DnsProperties {
                // The hosted zone Cloud Map created for the namespace, which only a DNS
                // namespace has: a namespace with no hosted zone names none.
                hosted_zone_id: record.hosted_zone_id.clone(),
                // `SOA$TTL` is modelled `@required`, so the SOA a namespace reports
                // always carries its TTL "for purposes of negative caching": the one
                // stored for the namespace, and zero — no negative caching — for a
                // namespace nothing ever set one for, which is every namespace with no
                // SOA record of its own.
                soa: Some(SOA {
                    ttl: record.soa_ttl.unwrap_or(NO_NEGATIVE_CACHING_TTL),
                }),
            }),
            http_properties: record
                .http_name
                .as_ref()
                .map(|http_name| HttpProperties {
                    http_name: Some(http_name.clone()),
                }),
        }),
        create_date: Some(timestamp(record.create_date)),
        creator_request_id: record.creator_request_id.clone(),
    }
}

/// The modelled `NamespaceSummary` for a stored namespace — the same record
/// [`namespace_view`] renders, minus the members `NamespaceSummary` does not carry
/// (`CreatorRequestId`), so a listing and `GetNamespace` can never disagree.
pub fn namespace_summary_view(record: &NamespaceRecord, service_count: i32) -> NamespaceSummary {
    let namespace = namespace_view(record, service_count);
    NamespaceSummary {
        id: namespace.id,
        arn: namespace.arn,
        // The same account `Namespace` reports, so a listing and `GetNamespace` can
        // never disagree about who owns the namespace.
        resource_owner: namespace.resource_owner,
        name: namespace.name,
        r#type: namespace.r#type,
        description: namespace.description,
        service_count: namespace.service_count,
        properties: namespace.properties,
        create_date: namespace.create_date,
    }
}

/// The page size a listing serves for a request whose `MaxResults` is `max_results`.
///
/// An absent `MaxResults` asks for as many records as the documented default allows
/// ([`MAX_LIST_PAGE_SIZE`]), never an empty page. A caller-supplied bound is already
/// inside the modelled `@range(1..100)` by the time a handler runs; clamping again
/// costs nothing and keeps this total.
pub fn page_size(max_results: Option<i32>) -> usize {
    let bound = max_results
        .unwrap_or(MAX_LIST_PAGE_SIZE)
        .clamp(1, MAX_LIST_PAGE_SIZE);
    usize::try_from(bound).unwrap_or(1)
}

/// One page of an ordered `(key, record)` sequence, plus the `NextToken` that continues
/// it — `None` when the page reached the end.
///
/// The token names the KEY OF THE FIRST RECORD NOT RETURNED, scoped to the operation
/// that issued it by [`encode_next_token`], and a resumed call starts at the first key
/// at or after that cursor, so resuming stays well defined even when the record the
/// token named was deleted between the two calls: the scan simply continues at the next
/// key. Together with a stable key order that is what makes a paging client see each
/// record exactly once.
pub fn page<'a, T: Clone + 'a>(
    ordered: impl IntoIterator<Item = (&'a String, &'a T)>,
    resume_at: Option<&str>,
    page_size: usize,
    scope: &str,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if resume_at.is_some_and(|from| key.as_str() < from) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(encode_next_token(scope, key)));
        }
        records.push(record.clone());
    }
    (records, None)
}

/// The modelled `Service` for a stored service.
pub fn service_view(record: &ServiceRecord) -> Service {
    Service {
        id: Some(record.id.clone()),
        arn: Some(record.arn.clone()),
        resource_owner: Some(owning_account()),
        name: Some(record.name.clone()),
        namespace_id: Some(record.namespace_id.clone()),
        description: record.description.clone(),
        instance_count: Some(record.instance_count()),
        // `DnsConfig` describes "the Route 53 DNS records that you want Cloud Map to
        // create when you register an instance", which every service has — one that
        // registers no record simply has none of them — so the member is part of the
        // service's shape rather than something a read leaves out: it carries the stored
        // configuration, and an EMPTY one (no record, and so none of the members a
        // stored configuration resolves for itself either) for a service that registers
        // no record at all.
        dns_config: Some(record.dns_config.clone().unwrap_or_default()),
        r#type: Some(record.service_type.clone()),
        health_check_config: record.health_check_config.clone(),
        health_check_custom_config: record.health_check_custom_config.clone(),
        create_date: Some(timestamp(record.create_date)),
        creator_request_id: record.creator_request_id.clone(),
        created_by_account: Some(owning_account()),
    }
}

/// The modelled `ServiceSummary` for a stored service — the same record
/// [`service_view`] renders, minus the members `ServiceSummary` does not carry
/// (`NamespaceId`, `CreatorRequestId`), so a listing and `GetService` can never
/// disagree.
pub fn service_summary_view(record: &ServiceRecord) -> ServiceSummary {
    let service = service_view(record);
    ServiceSummary {
        id: service.id,
        arn: service.arn,
        // The same accounts `Service` reports, so a listing and `GetService` can never
        // disagree about who owns the namespace or created the service.
        resource_owner: service.resource_owner,
        name: service.name,
        r#type: service.r#type,
        description: service.description,
        instance_count: service.instance_count,
        dns_config: service.dns_config,
        health_check_config: service.health_check_config,
        health_check_custom_config: service.health_check_custom_config,
        create_date: service.create_date,
        created_by_account: service.created_by_account,
    }
}

/// The modelled `ServiceAttributes` for a stored service.
///
/// `ServiceAttributesMap` is modelled `length 1..30`, so a service carrying no
/// attributes reports the member ABSENT rather than an empty map, which no value of
/// that shape can be.
pub fn service_attributes_view(record: &ServiceRecord) -> ServiceAttributes {
    ServiceAttributes {
        service_arn: Some(record.arn.clone()),
        // The account that created the namespace the service belongs to, as
        // `Service$ResourceOwner` reports it.
        resource_owner: Some(owning_account()),
        attributes: if record.attributes.is_empty() {
            None
        } else {
            Some(
                record
                    .attributes
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        },
    }
}

/// The modelled `Tag` list for a stored tag list, in the stored order.
pub fn tag_views(tags: &[(String, String)]) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Apply a `TagList` onto a resource's stored tags, returning the result.
///
/// A tag is "a custom key-value pair", so a key identifies at most one tag: setting a
/// key that is already stored REPLACES its value in place (leaving every other tag
/// alone and keeping the stored order stable for a reader), and a key that is not
/// stored yet is appended. The incoming list is applied in order, so a key repeated
/// within one request ends up carrying the last value given for it.
///
/// Returns the would-be tag set WITHOUT storing it, so a caller can refuse the whole
/// call — with the modelled `TooManyTagsException` — before any of it is persisted.
/// Both sides are bounded: the stored set by [`MAX_TAGS_PER_RESOURCE`] and the
/// incoming list by the model's `TagList` maximum of 200 entries, which the request
/// walk refuses before a handler runs.
pub fn tags_after_applying(
    stored: &[(String, String)],
    incoming: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    let mut merged = stored.to_vec();
    for (key, value) in incoming {
        match merged.iter_mut().find(|(stored_key, _)| *stored_key == key) {
            Some(existing) => existing.1 = value,
            None => merged.push((key, value)),
        }
    }
    merged
}

/// The modelled `Instance` for a stored instance.
pub fn instance_view(record: &InstanceRecord) -> Instance {
    Instance {
        id: record.instance_id.clone(),
        creator_request_id: record.creator_request_id.clone(),
        attributes: Some(record.attributes.clone()),
        // The account that registered the instance, read from its record: it is stored
        // when the registration is, rather than recomputed on the way out.
        created_by_account: Some(record.created_by_account.clone()),
    }
}

/// The modelled `InstanceSummary` for a stored instance — the same record
/// [`instance_view`] renders, minus the member `InstanceSummary` does not carry
/// (`CreatorRequestId`), so a listing and `GetInstance` can never disagree about an
/// instance's attributes or its registering account.
pub fn instance_summary_view(record: &InstanceRecord) -> InstanceSummary {
    let instance = instance_view(record);
    InstanceSummary {
        id: Some(instance.id),
        attributes: instance.attributes,
        created_by_account: instance.created_by_account,
    }
}

/// The modelled `Operation` for a stored operation record.
pub fn operation_view(record: &OperationRecord) -> Operation {
    Operation {
        id: Some(record.id.clone()),
        owner_account: Some(owning_account()),
        r#type: Some(record.operation_type.clone()),
        status: Some(record.status.clone()),
        error_message: record.error_message.clone(),
        error_code: record.error_code.clone(),
        create_date: Some(timestamp(record.create_date)),
        update_date: Some(timestamp(record.update_date)),
        targets: Some(record.targets.clone()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicediscoveryRoute53autonamingV20170314State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicediscoveryRoute53autonamingV20170314State>)
}
