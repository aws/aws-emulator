// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-gameliftstreams-gameliftstreams service.
//!
//! This module owns the three persisted resources of Amazon GameLift Streams —
//! applications, stream groups and the stream sessions a stream group hosts —
//! plus the helpers every handler in `src/ops/` shares:
//!
//! * [`ComAmazonawsGameliftstreamsGameliftstreamsState::materialize`] advances every
//!   transition the CLOCK already owed at a given instant (a create finishing, a
//!   delete completing, a placement resolving). Handlers call it first, so a
//!   reporting operation reports what the clock already made due and nothing else;
//!   the advanced record's `LastUpdatedAt` is the instant the transition was DUE,
//!   never the instant it was observed, so two reads of the same resource agree.
//! * [`identifier_id`] resolves the model's `Identifier` shape, which accepts either
//!   a bare resource id or a full ARN for the same resource, and
//!   [`invalid_resource_arn`] is the refusal an identifier already in ARN form gets
//!   when that ARN names no resource of the type the operation addresses.
//! * [`application_arn`] / [`stream_group_arn`] / [`stream_session_arn`] build ARNs
//!   through `engine::emulator_arn`, so the account segment is the canonical one.
//! * [`resolved_arn`] / [`resolved_stream_session_arn`] resolve the identifier a
//!   request passed to the full ARN of the resource it names, and
//!   [`resource_not_found`] / [`stream_session_not_found`] are the ONE place the
//!   service's `ResourceNotFoundException` message is built from it. The wording is
//!   chosen by the class of resource the operation ADDRESSES, so an operation whose
//!   class the service reports without quoting the request — the stream URLs, see
//!   [`stream_url_not_found`] — has its own builder beside them.
//! * [`with_internal_id`] appends the per-request `" (Internal ID: <uuid>)"` suffix
//!   every message this service's HANDLERS answer with carries, so no handler words
//!   that suffix itself.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DefaultApplication, DisplayConfiguration, LocationConfiguration, LocationState,
    PerformanceStatsConfiguration, ReplicationStatus, ResourceNotFoundException,
    RuntimeEnvironment, ValidationException,
};

/// This service's SigV4 signing name, and the `service` segment of its ARNs.
pub const SIGNING_NAME: &str = "gameliftstreams";

/// The region a request that carries no SigV4 credential scope is answered in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a transition the clock owes takes to come due: a create reaching its
/// ready status, a delete completing, a stream placement resolving.
///
/// An emulator-chosen span (the real service takes minutes), deliberately
/// non-zero so a caller that polls immediately after the call that started the
/// transition still observes the transient status and the state errors that go
/// with it.
pub const TRANSITION: Duration = Duration::from_millis(500);

/// A stream group's lifetime. `CreateStreamGroup`'s documentation: "Stream groups
/// expire when they are 365 days old, at which point they can no longer stream
/// sessions. The exact expiration date is indicated by the date value in the
/// `ExpiresAt` field."
pub const STREAM_GROUP_LIFETIME: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// How many applications the store holds before `CreateApplication` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// Amazon GameLift Streams publishes an adjustable per-account, per-Region quota on
/// applications, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is far above any workflow an emulator
/// serves, and it is what stops the store growing without end: every record it
/// holds is released by `DeleteApplication`, which also forgets that application's
/// client tokens, so the token map cannot outgrow this bound either.
pub const APPLICATION_QUOTA: usize = 1_000;

/// How many stream groups the store holds before `CreateStreamGroup` refuses with
/// the modelled `ServiceQuotaExceededException`. The emulator's own bound, for the
/// same reason as [`APPLICATION_QUOTA`]; `DeleteStreamGroup` releases each record
/// and its client tokens.
pub const STREAM_GROUP_QUOTA: usize = 1_000;

/// How many tags ONE resource holds.
///
/// The Amazon Web Services General Reference's tag restrictions — which
/// `TagResource` points at ("Tagging Amazon Web Services Resources in the Amazon
/// Web Services General Reference") — give a maximum of 50 tags per resource, and
/// the model mirrors that bound on every tagging operation's `Tags` map
/// (`length 1..50`). A request that would take a resource past it is refused, so
/// the stored tag map cannot grow without end.
pub const TAGS_PER_RESOURCE: usize = 50;

/// The IPv4 CIDR block of the service VPC a stream group's capacity runs in,
/// reported on every location of a stream group.
///
/// `LocationState::InternalVpcIpv4CidrBlock` is "the CIDR block of the service VPC
/// for this location. Add this CIDR block to your VPC route table to enable traffic
/// routing through the Transit Gateway", so the block belongs to the SERVICE's own
/// network in that location rather than to anything the caller sent: every location
/// runs its own service VPC, and separate VPCs may carry the same private block, so
/// one value describes them all.
///
/// This emulator provisions no network, so the block is its own choice — a private
/// RFC 1918 range, in the `/16` form the member's modelled `Ipv4CidrBlock` pattern
/// accepts.
pub const SERVICE_VPC_IPV4_CIDR_BLOCK: &str = "10.0.0.0/16";

/// How many stream sessions ONE stream group holds at a time.
///
/// A stream session occupies a compute resource the stream group allocated, so the
/// real service bounds concurrent sessions by the group's capacity. This emulator
/// does not allocate compute, so it bounds the per-group session store itself:
/// sessions that have reached a terminal status are released first (oldest first,
/// see [`StreamGroup::make_room_for_a_session`]), and a group whose live sessions
/// fill the bound refuses the start with the modelled `ConflictException` —
/// `StartStreamSession` does not model `ServiceQuotaExceededException`.
pub const SESSIONS_PER_STREAM_GROUP: usize = 100;

/// How many locations ONE stream group holds, after which
/// `AddStreamGroupLocations` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// The model bounds one request's `LocationConfigurations` list at `length 1..100`
/// (`CreateStreamGroup` and `AddStreamGroupLocations` share the shape), so a
/// single stream group can never be configured with more than 100 locations in one
/// call; this is that same bound applied to the group's accumulated set, which is
/// what stops repeated adds growing one group without end.
pub const LOCATIONS_PER_STREAM_GROUP: usize = 100;

/// How long a stream session that has reached a terminal status (`ERROR`,
/// `TERMINATED`) is kept before the clock releases it.
///
/// Emulator-chosen: AWS documents no retention for a stream session record. It is
/// long enough that a caller which terminates a session and then reads it back
/// still sees the terminal status and its `StatusReason`, and short enough that a
/// start/terminate loop cannot grow the store without end. The instant is derived
/// from the record's own `LastUpdatedAt`; nothing sleeps.
///
/// The per-group BOUND wins over this span: a group already at
/// [`SESSIONS_PER_STREAM_GROUP`] releases its oldest terminal session to admit a
/// new one even if that session's retention is not up yet (see
/// [`StreamGroup::make_room_for_a_session`]), because a bound that a caller can
/// defer is not a bound.
pub const TERMINAL_SESSION_RETENTION: Duration = Duration::from_secs(60);

/// How long the credentials of one administrative terminal session into a stream
/// session are good for: "The `StreamUrl` is valid for 60 seconds. After it
/// expires, call this operation again to get a new URL", and the token "is only
/// valid for establishing a new connection within 60 seconds of generation"
/// (`CreateStreamSessionAdminShell` documentation).
pub const ADMIN_SHELL_VALIDITY: Duration = Duration::from_secs(60);

/// How many administrative terminal sessions ONE stream session holds at a time.
///
/// Each one lives for [`ADMIN_SHELL_VALIDITY`] and the clock releases it when that
/// window is up, so the set is already bounded in time; this is the emulator's own
/// bound on how many a caller can hold inside one window — the oldest is released
/// to admit a new one (see [`StreamSession::grant_admin_shell`]), so repeated calls
/// cannot grow one session's record without end. The operation models no quota
/// error, which is why the oldest is released rather than the call refused.
pub const ADMIN_SHELLS_PER_SESSION: usize = 8;

/// How many stream URLs ONE stream group holds at a time.
///
/// Amazon GameLift Streams publishes an adjustable quota on stream URLs rather
/// than a fixed one, and this emulator models no Service Quotas, so this is the
/// emulator's OWN bound on the per-group store — the same role
/// [`SESSIONS_PER_STREAM_GROUP`] plays for stream sessions. Stream URLs that can
/// no longer start a stream session (expired, revoked) are released first to admit
/// a new one (see [`StreamGroup::make_room_for_a_stream_url`]), and a group whose
/// usable stream URLs fill the bound is refused with the modelled
/// `ServiceQuotaExceededException`, which `CreateStreamUrl` declares.
pub const STREAM_URLS_PER_STREAM_GROUP: usize = 100;

/// How many stream sessions a stream URL can start when the request names no
/// `UsageLimit`: "The minimum is 1, and the default is 1" (`UsageLimit`
/// documentation).
pub const DEFAULT_USAGE_LIMIT: i32 = 1;

/// How long a stream session started from a stream URL may run when the request
/// names no `SessionLengthSeconds`: "Valid values are 1-86400 seconds (1 second to
/// 24 hours). The default is 43200 seconds (12 hours)" (`SessionLengthSeconds`
/// documentation on `CreateStreamUrl`).
pub const DEFAULT_SESSION_LENGTH_SECONDS: i32 = 43_200;

/// The pixel budget of a stream's virtual monitor: `Resolution`'s documentation —
/// "The total number of pixels (width x height) must not exceed 2,073,600
/// (equivalent to 1920 x 1080)."
pub const MAX_RESOLUTION_PIXELS: i64 = 2_073_600;

// The enum values this slice stores, from the model's `ApplicationStatus`,
// `StreamGroupStatus`, `StreamGroupLocationStatus`, `StreamSessionStatus`,
// `StreamSessionStatusReason` and `ReplicationStatusType` (see
// `crate::ops::types::shared_types`, which lists every value each accepts).
pub const APPLICATION_INITIALIZED: &str = "INITIALIZED";
pub const APPLICATION_PROCESSING: &str = "PROCESSING";
pub const APPLICATION_READY: &str = "READY";
pub const APPLICATION_DELETING: &str = "DELETING";
pub const STREAM_GROUP_ACTIVATING: &str = "ACTIVATING";
pub const STREAM_GROUP_ACTIVE: &str = "ACTIVE";
/// "UPDATING_LOCATIONS: One or more locations in the stream group are in the
/// process of updating (either activating or deleting)" (`StreamGroupStatus`
/// documentation).
pub const STREAM_GROUP_UPDATING_LOCATIONS: &str = "UPDATING_LOCATIONS";
pub const STREAM_GROUP_DELETING: &str = "DELETING";
pub const STREAM_GROUP_EXPIRED: &str = "EXPIRED";
pub const LOCATION_ACTIVATING: &str = "ACTIVATING";
pub const LOCATION_ACTIVE: &str = "ACTIVE";
pub const LOCATION_REMOVING: &str = "REMOVING";
pub const LOCATION_ERROR: &str = "ERROR";
pub const SESSION_ACTIVATING: &str = "ACTIVATING";
pub const SESSION_ACTIVE: &str = "ACTIVE";
pub const SESSION_CONNECTED: &str = "CONNECTED";
pub const SESSION_PENDING_CLIENT_RECONNECTION: &str = "PENDING_CLIENT_RECONNECTION";
pub const SESSION_RECONNECTING: &str = "RECONNECTING";
pub const SESSION_ERROR: &str = "ERROR";
pub const SESSION_TERMINATING: &str = "TERMINATING";
pub const SESSION_TERMINATED: &str = "TERMINATED";
pub const SESSION_REASON_INTERNAL_ERROR: &str = "internalError";
pub const SESSION_REASON_API_TERMINATED: &str = "apiTerminated";
/// "placementTimeout: Amazon GameLift Streams could not find available stream
/// capacity to start a stream session. Increase the stream capacity in the stream
/// group or wait until capacity becomes available" (`StatusReason` documentation).
pub const SESSION_REASON_PLACEMENT_TIMEOUT: &str = "placementTimeout";
/// "invalidSignalRequest: The WebRTC signal request that was sent is not valid"
/// (`StatusReason` documentation).
pub const SESSION_REASON_INVALID_SIGNAL_REQUEST: &str = "invalidSignalRequest";
pub const REPLICATION_REPLICATING: &str = "REPLICATING";
pub const REPLICATION_COMPLETED: &str = "COMPLETED";
pub const STREAM_URL_ACTIVE: &str = "ACTIVE";
pub const STREAM_URL_EXPIRED: &str = "EXPIRED";
pub const STREAM_URL_REVOKED: &str = "REVOKED";
pub const STREAM_URL_REASON_USER_REVOKED: &str = "userRevoked";
pub const STREAM_URL_REASON_SESSIONS_TERMINATED: &str = "revokedAndSessionsTerminated";
pub const STREAM_URL_REASON_TERMINATING_SESSIONS: &str = "revokedAndTerminatingSessions";
pub const STREAM_URL_REASON_STREAM_GROUP_DELETED: &str = "streamGroupDeleted";
pub const STREAM_URL_REASON_APPLICATION_DELETED: &str = "applicationDeleted";

/// The `RevocationMode` values `RevokeStreamUrl` accepts, from the model's
/// `RevocationMode` enum.
pub const REVOKE_URL: &str = "REVOKE_URL";
pub const REVOKE_AND_TERMINATE_SESSIONS: &str = "REVOKE_AND_TERMINATE_SESSIONS";

/// The ARN resource-type segment of each resource this service names.
pub const APPLICATION_RESOURCE: &str = "application";
pub const STREAM_GROUP_RESOURCE: &str = "streamgroup";
pub const STREAM_SESSION_RESOURCE: &str = "streamsession";
pub const STREAM_URL_RESOURCE: &str = "streamurl";

/// The `StreamClass` values Amazon GameLift Streams ACCEPTS, in the order the
/// service's own refusal enumerates them.
///
/// Recorded behaviour, and deliberately NOT the model's enum: the model's
/// `com.amazonaws.gameliftstreams#StreamClass` declares two further values —
/// `gen6n_small_win2022` and `gen6n_medium_win2022` — that the live service refuses
/// with a `ValidationException` enumerating exactly the fifteen below. A stream
/// class is checked against THIS set, so the emulator cannot accept a value AWS
/// rejects.
///
/// The source is the recorded refusal of check
/// `gameliftstreams.create-stream-group-invalid-stream-class-rejected`, whose message
/// enumerates these fifteen. Re-check this list against a fresh recording before
/// trusting it again: the day the service starts accepting a value its model already
/// declares, this set — not the model — is what would refuse it.
pub const SERVICE_STREAM_CLASSES: &[&str] = &[
    "gen4n_high",
    "gen4n_ultra",
    "gen4n_win2022",
    "gen5n_high",
    "gen5n_ultra",
    "gen5n_win2022",
    "gen6n_small",
    "gen6n_medium",
    "gen6n_high",
    "gen6n_ultra",
    "gen6n_ultra_win2022",
    "gen6n_pro",
    "gen6n_pro_win2022",
    "gen6e_pro",
    "gen6e_pro_win2022",
];

/// Whether the service accepts `stream_class` (see [`SERVICE_STREAM_CLASSES`]).
pub fn stream_class_accepted(stream_class: &str) -> bool {
    SERVICE_STREAM_CLASSES.contains(&stream_class)
}

/// The refusal text for a `StreamClass` outside [`SERVICE_STREAM_CLASSES`].
///
/// Rendered through the framework's own constraint envelope, so a stream class the
/// MODEL rejects (refused before any handler runs) and one only the SERVICE rejects
/// (refused in the handler) are worded identically — the one difference a caller
/// sees is which values the enumerated set holds.
///
/// Carries no internal-id suffix, unlike [`with_internal_id`]: this is the
/// request-validation envelope, and the recorded refusal of an unknown stream class
/// carries none either.
pub fn stream_class_refusal(stream_class: &str) -> String {
    // One violation always renders, so the fallback is unreachable — but it is a
    // fallback rather than a panic, because this sits on a request path.
    ::aws_smithy_schema_server::envelope(&[stream_class_violation(Some(stream_class))])
        .unwrap_or_else(stream_class_fallback)
}

/// The plain sentence a stream-class refusal falls back to when the framework's
/// constraint envelope renders nothing — one wording for the handler and for
/// `crate::service`'s refusal override, so neither degenerates into the
/// framework's own message, which carries the WIDER model enum.
pub fn stream_class_fallback() -> String {
    format!(
        "{STREAM_CLASS_PATH} must be one of [{}]",
        SERVICE_STREAM_CLASSES.join(", ")
    )
}

/// The constraint violation a `StreamClass` outside [`SERVICE_STREAM_CLASSES`] is,
/// with the enumerated set narrowed to the one the SERVICE accepts. `value` is the
/// offending value as the request spelled it, or `None` when there was none.
///
/// The ONE place that set becomes a violation: both the `CreateStreamGroup` handler
/// (through [`stream_class_refusal`]) and `crate::service`'s refusal override —
/// which narrows the set of the refusal the framework raises for a value the MODEL
/// does not declare — reach it here, so the two cannot drift.
pub fn stream_class_violation(value: Option<&str>) -> ::aws_smithy_schema_server::Violation {
    ::aws_smithy_schema_server::Violation {
        path: STREAM_CLASS_PATH.to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::EnumValue(
            SERVICE_STREAM_CLASSES
                .iter()
                .map(|accepted| (*accepted).to_string())
                .collect(),
        ),
        value: value.map(str::to_string),
        sensitive: false,
    }
}

/// How the validation envelope spells `CreateStreamGroupInput$StreamClass`
/// (lowerCamel, as every constraint path is).
pub const STREAM_CLASS_PATH: &str = "streamClass";

/// How the validation envelope spells the `ResourceArn` label the tagging
/// operations (`TagResource`, `UntagResource`, `ListTagsForResource`) bind — the
/// member `crate::service`'s refusal override answers `BadRequestException` for.
/// Beside [`STREAM_CLASS_PATH`], so the two spellings of "how the envelope names a
/// member" cannot drift.
pub const RESOURCE_ARN_PATH: &str = "resourceArn";

/// The order Amazon GameLift Streams enumerates `StreamUrlStatus` in when it
/// refuses a value outside that enum.
///
/// The SAME four values the model declares — not a narrowing, unlike
/// [`SERVICE_STREAM_CLASSES`] — in the order the SERVICE lists them. The model
/// declares its own member order (`ACTIVE, EXPIRED, REVOKED, LIMIT_REACHED`) and a
/// Smithy enum's member order is not a wire fact, so the order a refusal
/// enumerates is recorded behaviour: the source is the recorded refusal of
/// `ListStreamUrls` with a `Status` outside the enum. Every other enum of this
/// service is enumerated in its declared order, so none of them needs a list here.
pub const SERVICE_STREAM_URL_STATUSES: &[&str] =
    &["EXPIRED", "ACTIVE", "LIMIT_REACHED", "REVOKED"];

/// One constraint violation as THIS service states it.
///
/// The framework's constraint walk produces the violation from the model, and for
/// every rule this service enforces exactly as the model declares it that is also
/// what the service says. The two enumerated sets it does NOT spell the model's way
/// are replaced here, in the one place both refusal paths (the pre-handler check in
/// `crate::service` and its `override_refusal` fallback) reach:
///
/// * `StreamClass`, whose accepted set is NARROWER than the model's enum
///   ([`SERVICE_STREAM_CLASSES`]);
/// * `StreamUrlStatus`, whose set is the model's but whose ORDER is the service's
///   ([`SERVICE_STREAM_URL_STATUSES`]).
pub fn service_violation(
    violation: &::aws_smithy_schema_server::Violation,
) -> ::aws_smithy_schema_server::Violation {
    let ::aws_smithy_schema_server::ViolationKind::EnumValue(allowed) = &violation.kind else {
        return violation.clone();
    };
    if violation.path == STREAM_CLASS_PATH {
        return stream_class_violation(violation.value.as_deref());
    }
    // Matched on the SET the model declared for the member, not on the member's
    // name: the two operations that bind a `Status` query member bind two
    // different enums, and only this one is enumerated out of declared order.
    if !same_values(allowed, SERVICE_STREAM_URL_STATUSES) {
        return violation.clone();
    }
    ::aws_smithy_schema_server::Violation {
        path: violation.path.clone(),
        kind: ::aws_smithy_schema_server::ViolationKind::EnumValue(
            SERVICE_STREAM_URL_STATUSES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        ),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// Whether `stated` and `expected` hold the same values, in any order.
fn same_values(stated: &[String], expected: &[&str]) -> bool {
    stated.len() == expected.len()
        && expected
            .iter()
            .all(|value| stated.iter().any(|held| held == value))
}

/// The id prefix each resource type's own ids carry.
///
/// The id prefix each resource type's own ids carry.
///
/// Published by the model on every identifier member that names the resource
/// ("Example ARN: arn:aws:gameliftstreams:us-west-2:111122223333:streamgroup/sg-1AB2C3De4.
/// Example ID: sg-1AB2C3De4", and `application/a-9ZY8X7Wv6` / `a-9ZY8X7Wv6` for an
/// application), and the same prefixes this emulator mints its ids with. A
/// resource type the model publishes no prefixed example for is absent from this
/// list, and an identifier of that type only has to name a non-empty resource path
/// — a recorded stream session id (`ABC123def4568`) carries no prefix at all.
const RESOURCE_ID_PREFIXES: &[(&str, &str)] = &[
    (APPLICATION_RESOURCE, "a-"),
    (STREAM_GROUP_RESOURCE, "sg-"),
    (STREAM_URL_RESOURCE, "su-"),
];

/// The id prefix `resource_type`'s own ids carry, if the model publishes one.
fn resource_id_prefix(resource_type: &str) -> Option<&'static str> {
    RESOURCE_ID_PREFIXES
        .iter()
        .find(|(kind, _)| *kind == resource_type)
        .map(|(_, prefix)| *prefix)
}

/// Whether `identifier` can name a resource of `resource_type` at all — i.e.
/// whether it resolves to an ARN of that type.
///
/// The model's `Identifier` shape accepts "an Amazon Resource Name (ARN) or ID" for
/// the same resource, and BOTH forms have to resolve to the one ARN the operation
/// addresses: an ARN only does when its resource segment is `<resource_type>/<id>`,
/// and either form's id only does when it is of that type's own form
/// ([`RESOURCE_ID_PREFIXES`]).
pub fn identifier_names_resource(identifier: &str, resource_type: &str) -> bool {
    // A bare identifier IS its own id; an ARN resolves to the id its resource path
    // ends in, and to none at all when that path is of another resource type.
    let Some(id) = identifier_id(identifier, resource_type) else {
        return false;
    };
    match resource_id_prefix(resource_type) {
        Some(prefix) => id.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty()),
        None => !id.is_empty(),
    }
}

/// The refusal an identifier member carries when it resolves to no ARN of the
/// resource type the operation addresses, or `None` when the operation may go on
/// and resolve it.
///
/// A value the member cannot name a resource with is a malformed VALUE for that
/// member, not a resource that is missing, so it is answered with the modelled
/// `ValidationException` (400) rather than `ResourceNotFoundException` (404) — the
/// two mean opposite things to a caller, and the check is decided from the request
/// alone, so it comes BEFORE the store is read.
///
/// `member` is the input member as the MODEL spells it, which is how the message
/// names it; `resource_type` is the ARN resource type the operation addresses. The
/// text names the ARN rather than the id even when the request sent a bare id,
/// because the ARN is what the identifier had to resolve to. It carries no
/// internal-id suffix, unlike [`with_internal_id`]: it is a request-validation
/// refusal, which this service words without one (see [`stream_class_refusal`]).
pub fn invalid_resource_arn(
    member: &str,
    identifier: &str,
    resource_type: &str,
) -> Option<ValidationException> {
    if identifier_names_resource(identifier, resource_type) {
        return None;
    }
    Some(ValidationException {
        message: format!("'{member}' is not a valid resource ARN of type '{resource_type}'"),
    })
}

/// How the model spells each identifier member [`invalid_resource_arn`] checks, so
/// a refusal naming one cannot drift from the model's spelling.
pub const IDENTIFIER_MEMBER: &str = "Identifier";
pub const APPLICATION_IDENTIFIER_MEMBER: &str = "ApplicationIdentifier";
pub const APPLICATION_IDENTIFIERS_MEMBER: &str = "ApplicationIdentifiers";
pub const DEFAULT_APPLICATION_IDENTIFIER_MEMBER: &str = "DefaultApplicationIdentifier";
pub const STREAM_SESSION_IDENTIFIER_MEMBER: &str = "StreamSessionIdentifier";
pub const STREAM_URL_IDENTIFIER_MEMBER: &str = "StreamUrlIdentifier";
pub const STREAM_GROUP_IDENTIFIER_MEMBER: &str = "StreamGroupIdentifier";

/// Every error message this service's HANDLERS produce ends with a per-request
/// internal id, which is how Amazon GameLift Streams words its errors: the body a
/// client reads is `"<what went wrong> (Internal ID: <uuid>)"`.
///
/// The value is per request, so nothing may be derived from it — it identifies the
/// call in the service's own logs and changes on every call, exactly as a request id
/// does.
///
/// A request the model itself refuses (an absent member, a value outside a declared
/// constraint) is answered by the request-validation envelope BEFORE a handler runs
/// and carries no such suffix; [`stream_class_refusal`] is this service's one
/// handler-side member of that class and matches it.
pub fn with_internal_id(message: impl AsRef<str>) -> String {
    format!("{} (Internal ID: {})", message.as_ref(), internal_id())
}

/// A fresh opaque correlation id, in the canonical 8-4-4-4-12 lowercase-hex form the
/// service's internal ids take.
///
/// Minted per CALL from the wall clock and a process-local counter, mixed the way
/// [`ComAmazonawsGameliftstreamsGameliftstreamsState::mint_id`] mixes a minted
/// resource id — the counter is what makes two ids of the same nanosecond distinct by
/// construction rather than by luck. It identifies one request, so it is never stored
/// and never read back, and it is not part of this service's state: a refused call
/// writes nothing. Minted here rather than by a UUID crate so this service's error
/// wording needs no dependency of its own.
fn internal_id() -> String {
    static NEXT: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let counter = NEXT.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as u64);
    let mut hash = nanos ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut digits = String::with_capacity(32);
    for _ in 0..2 {
        hash = hash
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0xBF58_476D_1CE4_E5B9);
        let mixed = (hash >> 29) ^ hash;
        digits.push_str(&format!("{mixed:016x}"));
    }
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// The refusal text for a `ClientToken` replayed with different request parameters.
pub fn client_token_conflict() -> String {
    with_internal_id("'ClientToken' value has already been used with different request parameters")
}

/// The refusal an application that is not associated with the stream group a
/// request addresses carries.
///
/// "You must have at least one application associated to the stream group"
/// (`StartStreamSession`'s prerequisites), and a stream URL starts its sessions in
/// the stream group too ("This application must be associated with the stream
/// group" — `CreateStreamUrl`'s `ApplicationIdentifier`), so both paths refuse an
/// unassociated application with the same modelled client-input error, worded here
/// once. The text names both resources by the full ARN each one resolves to, in
/// single quotes, which is how the service words this refusal — an `Identifier`
/// member accepts either form, so the ARN is the one spelling of it a caller can
/// always read back.
pub fn application_not_associated(
    application_arn: &str,
    stream_group_arn: &str,
) -> ValidationException {
    ValidationException {
        message: with_internal_id(format!(
            "Application '{application_arn}' is not associated with stream group \
             '{stream_group_arn}'"
        )),
    }
}

/// The refusal a `DefaultApplicationIdentifier` that names no application of the
/// addressed stream group's associations carries.
///
/// The member "only sets the default application in a stream group. To associate a
/// new application to an existing stream group, you must use AssociateApplications"
/// (`UpdateStreamGroup`'s `DefaultApplicationIdentifier`), so the only values it
/// accepts are the applications that group is ALREADY associated with: every other
/// value — including one that names no application at all — is a bad value for the
/// member rather than a resource the caller addressed and could not find, and is
/// answered with the modelled `ValidationException`. The text names the full ARN
/// the identifier resolves to, which is the one spelling of it a caller can always
/// read back, and carries the per-request suffix [`with_internal_id`] appends to
/// every handler-side message of this service.
pub fn default_application_not_associated(application_arn: &str) -> ValidationException {
    ValidationException {
        message: with_internal_id(format!(
            "Application '{application_arn}' cannot be set as default because it is not \
             currently associated with the stream group"
        )),
    }
}

/// The refusal a `DisplayConfiguration` whose `Resolution` breaks one of the two
/// rules the model states in prose carries, or `None` when the operation may go on.
///
/// `Resolution` documents both: "The value must be an even number" (`Width`,
/// `Height`) and the [`MAX_RESOLUTION_PIXELS`] budget. Neither is expressible as a
/// constraint trait, so every operation that accepts a `DisplayConfiguration` —
/// `StartStreamSession` and `CreateStreamUrl`, which configure the same virtual
/// monitor — refuses it here, in this one place, with the modelled
/// `ValidationException`.
pub fn unsupported_resolution(
    configuration: Option<&DisplayConfiguration>,
) -> Option<ValidationException> {
    let resolution = configuration.and_then(|configuration| configuration.resolution.as_ref())?;
    let (width, height) = (resolution.width, resolution.height);
    if width % 2 != 0 || height % 2 != 0 {
        return Some(ValidationException {
            message: with_internal_id(format!(
                "DisplayConfiguration.Resolution must have an even Width and Height, got \
                 {width}x{height}"
            )),
        });
    }
    if i64::from(width) * i64::from(height) > MAX_RESOLUTION_PIXELS {
        return Some(ValidationException {
            message: with_internal_id(format!(
                "DisplayConfiguration.Resolution must not exceed {MAX_RESOLUTION_PIXELS} \
                 pixels, got {width}x{height}"
            )),
        });
    }
    None
}

/// The refusal text for a member documented as an Amazon S3 location whose value is
/// not an S3 URI. Built from the offending member, so every such member words it
/// alike.
pub fn not_an_s3_uri(member: &str) -> String {
    with_internal_id(format!(
        "'{member}' must be a valid S3 URI starting with 's3://'"
    ))
}

/// Whether `uri` names an Amazon S3 location: the `s3://` scheme followed by a
/// bucket name.
///
/// The SCHEME and the BUCKET are what this enforces, and nothing beyond them. The
/// bucket rule is the one this service's model already states for its OTHER S3
/// member, `ApplicationLogOutputUri`
/// (`^s3://([a-zA-Z0-9][a-zA-Z0-9._-]{1,61}[a-zA-Z0-9])(/[a-zA-Z0-9._-]+)*/?$`): a
/// 3-to-63-character bucket that starts and ends alphanumeric, over
/// `[a-zA-Z0-9._-]`. The key portion is deliberately NOT checked against that
/// pattern's `(/[a-zA-Z0-9._-]+)*` tail: an S3 object key admits characters the
/// pattern does not (a space, say), and the member this guards
/// (`ApplicationSourceUri`, `@length 1..1024` and no `@pattern`) is a content
/// location, so refusing a legal key would refuse a request AWS accepts.
pub fn is_s3_uri(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("s3://") else {
        return false;
    };
    let bucket = rest.split('/').next().unwrap_or_default();
    (3..=63).contains(&bucket.len())
        && bucket.starts_with(|first: char| first.is_ascii_alphanumeric())
        && bucket.ends_with(|last: char| last.is_ascii_alphanumeric())
        && bucket.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

/// The characters a minted `Id` is built from. The model's `Id` shape is
/// `length 1..32, pattern ^[a-zA-Z0-9-]+$`.
const ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// One location's share of an application's content replication, as
/// `CreateApplication` reports it in `ReplicationStatuses`.
///
/// Keyed by the stream group that asked for the location, so dropping that group
/// drops exactly its replication rows and leaves the ones another group needs.
#[derive(Debug, Clone)]
pub struct Replication {
    /// ARN of the stream group whose location configuration asked for this.
    pub stream_group_arn: String,
    pub location: String,
    /// When replication to `location` is due to finish.
    pub completes_at: SystemTime,
}

/// A persisted application resource.
#[derive(Debug, Clone)]
pub struct Application {
    pub id: String,
    pub arn: String,
    pub description: String,
    pub runtime_environment: RuntimeEnvironment,
    pub executable_path: String,
    pub application_source_uri: String,
    pub application_log_paths: Vec<String>,
    pub application_log_output_uri: Option<String>,
    /// An `ApplicationStatus` value.
    pub status: String,
    /// An `ApplicationStatusReason` value, set only in `ERROR` status.
    pub status_reason: Option<String>,
    pub replications: Vec<Replication>,
    /// ARNs of the stream groups this application is associated with.
    pub associated_stream_groups: Vec<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// When the clock owes this application's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
}

impl Application {
    /// `ReplicationStatuses` for this application: one row per location, reported
    /// `COMPLETED` once every group that asked for that location has finished
    /// replicating to it. Empty for an application no stream group references.
    ///
    /// Collapsed through an ordered map keyed by location, so the cost is
    /// `n log n` in the stored rows rather than quadratic, and the result is in a
    /// stable order.
    pub fn replication_statuses(&self, now: SystemTime) -> Vec<ReplicationStatus> {
        let mut by_location: ::std::collections::BTreeMap<&str, bool> =
            ::std::collections::BTreeMap::new();
        for replication in &self.replications {
            let completed = now >= replication.completes_at;
            by_location
                .entry(replication.location.as_str())
                .and_modify(|all_completed| *all_completed &= completed)
                .or_insert(completed);
        }
        by_location
            .into_iter()
            .map(|(location, completed)| ReplicationStatus {
                location: Some(location.to_string()),
                status: Some(
                    if completed {
                        REPLICATION_COMPLETED
                    } else {
                        REPLICATION_REPLICATING
                    }
                    .to_string(),
                ),
            })
            .collect()
    }

    /// Advance the transition the clock owed at `now`. Answers `true` when the
    /// record is due to be gone (a `DELETING` application whose deletion completed).
    fn advance(&mut self, now: SystemTime) -> bool {
        let Some(due) = self.status_due_at else {
            return false;
        };
        if now < due {
            return false;
        }
        self.status_due_at = None;
        // `LastUpdatedAt` moves only when the status actually does, so a due
        // instant on a status with no transition cannot rewrite it.
        match self.status.as_str() {
            APPLICATION_INITIALIZED => {
                self.status = APPLICATION_READY.to_string();
                self.last_updated_at = due;
                false
            }
            APPLICATION_DELETING => true,
            _ => false,
        }
    }
}

/// One location of a stream group, as the request configured it.
#[derive(Debug, Clone)]
pub struct LocationSlot {
    pub name: String,
    pub always_on_capacity: Option<i32>,
    pub on_demand_capacity: Option<i32>,
    pub target_idle_capacity: Option<i32>,
    pub maximum_capacity: Option<i32>,
    /// A `StreamGroupLocationStatus` value. Each location carries its OWN status,
    /// because a location added to a group that is already serving is
    /// `ACTIVATING` while the locations beside it stay `ACTIVE`.
    pub status: String,
    /// When the clock owes this location's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
}

impl LocationSlot {
    /// A location as `CreateStreamGroup` or `AddStreamGroupLocations` configured
    /// it, in `ACTIVATING`: "ACTIVATING: Amazon GameLift Streams is preparing the
    /// location. You cannot stream from, scale the capacity of, or remove this
    /// location yet." The clock owes its move to `ACTIVE` one [`TRANSITION`] later,
    /// once the capacity it provisions is in place.
    pub fn activating(configuration: LocationConfiguration, now: SystemTime) -> Self {
        Self {
            name: configuration.location_name,
            always_on_capacity: configuration.always_on_capacity,
            on_demand_capacity: configuration.on_demand_capacity,
            target_idle_capacity: configuration.target_idle_capacity,
            maximum_capacity: configuration.maximum_capacity,
            status: LOCATION_ACTIVATING.to_string(),
            status_due_at: Some(now + TRANSITION),
        }
    }

    /// Start releasing this location: "REMOVING: Amazon GameLift Streams is working
    /// to remove this location. This will release all provisioned capacity for this
    /// location" (`LocationState::Status` documentation). The clock owes the release
    /// one [`TRANSITION`] later, at which point the group no longer holds the
    /// location at all.
    pub fn start_removal(&mut self, now: SystemTime) {
        self.status = LOCATION_REMOVING.to_string();
        self.status_due_at = Some(now + TRANSITION);
    }

    /// How many stream sessions this location can run at the same time.
    ///
    /// The most stream capacity the service may have provisioned here: the
    /// always-on capacity, which "indicates minimum streaming capacity which is
    /// allocated to you and is never released back to the service", plus the
    /// on-demand capacity, "the streaming capacity that Amazon GameLift Streams
    /// can allocate in response to stream requests, and then de-allocate when the
    /// session has terminated" — and never less than "the maximum capacity that
    /// the service can allocate for you", which is what a location configured
    /// through `MaximumCapacity` alone can scale to. A capacity the configuration
    /// left out holds the service's default of zero ("Default is 0 when creating a
    /// stream group or adding a location"), so a location configured through one
    /// member alone is bounded by that one.
    pub fn stream_capacity(&self) -> i32 {
        let scalable = self
            .always_on_capacity
            .unwrap_or(0)
            .max(0)
            // Saturating, not checked: the capacities come from the request, so
            // the arithmetic must not be able to overflow whatever a caller sent.
            .saturating_add(self.on_demand_capacity.unwrap_or(0).max(0));
        scalable.max(self.maximum_capacity.unwrap_or(0).max(0))
    }

    /// Is this location still working towards a status of its own?
    ///
    /// `REMOVING` counts as well as `ACTIVATING` — "REMOVING: Amazon GameLift
    /// Streams is working to remove this location" — so a location on its way out
    /// keeps the group in `UPDATING_LOCATIONS` too.
    fn is_settling(&self) -> bool {
        matches!(self.status.as_str(), LOCATION_ACTIVATING | LOCATION_REMOVING)
    }

    /// Advance the transition the clock owed this location at `now`. Answers `true`
    /// when the location is due to be gone: a `REMOVING` location whose provisioned
    /// capacity the service has released by now leaves the group's set.
    fn advance(&mut self, now: SystemTime) -> bool {
        let Some(due) = self.status_due_at else {
            return false;
        };
        if now < due {
            return false;
        }
        self.status_due_at = None;
        match self.status.as_str() {
            // "ACTIVE: The location is provisioned with initial capacity."
            LOCATION_ACTIVATING => {
                self.status = LOCATION_ACTIVE.to_string();
                false
            }
            LOCATION_REMOVING => true,
            _ => false,
        }
    }
}

/// The message refusing a `LocationConfiguration` this service cannot store, or
/// `None` when it can.
///
/// Every capacity member of `LocationConfiguration` is modelled `@range(min: 0)`.
/// The capacities are stored, echoed and used in this service's capacity
/// arithmetic, so a negative one is refused by the operation that carries it
/// rather than carried into state. Shared by `CreateStreamGroup` and
/// `AddStreamGroupLocations`, which configure a location through the same shape.
pub fn unstorable_location_configuration(configuration: &LocationConfiguration) -> Option<String> {
    for (member, value) in [
        ("AlwaysOnCapacity", configuration.always_on_capacity),
        ("OnDemandCapacity", configuration.on_demand_capacity),
        ("TargetIdleCapacity", configuration.target_idle_capacity),
        ("MaximumCapacity", configuration.maximum_capacity),
    ] {
        if let Some(capacity) = value {
            if capacity < 0 {
                return Some(format!(
                    "LocationConfigurations[{}].{member} must not be negative, got {capacity}",
                    configuration.location_name
                ));
            }
        }
    }
    None
}

/// A persisted stream group, together with the stream sessions it hosts.
#[derive(Debug, Clone)]
pub struct StreamGroup {
    pub id: String,
    pub arn: String,
    pub description: String,
    /// A `StreamClass` value.
    pub stream_class: String,
    pub default_application: Option<DefaultApplication>,
    pub locations: Vec<LocationSlot>,
    /// A `StreamGroupStatus` value.
    pub status: String,
    /// A `StreamGroupStatusReason` value, set only in `ERROR` status.
    pub status_reason: Option<String>,
    /// ARNs of the applications associated with this stream group.
    pub associated_applications: Vec<String>,
    pub sessions: HashMap<String, StreamSession>,
    /// The stream URLs this group hands out, by stream URL id. A stream URL is
    /// scoped to the stream group that runs the sessions it starts — its ARN
    /// carries both ids, and every operation that addresses one names the group
    /// too — so the group owns the record, exactly as it owns its sessions.
    pub stream_urls: HashMap<String, StreamUrl>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub expires_at: SystemTime,
    /// When the clock owes this stream group's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
}

impl StreamGroup {
    /// Does this group hold `location`?
    pub fn has_location(&self, location: &str) -> bool {
        self.locations.iter().any(|slot| slot.name == location)
    }

    /// This group's primary location: "the Amazon Web Services Region location where
    /// you initially created this stream group, known as the primary location"
    /// (`RemoveStreamGroupLocations` documentation).
    ///
    /// That Region is the one the create was answered in, which is the region segment
    /// of the ARN minted with the record — so the primary location is read off the
    /// stored resource rather than held as a second copy of it.
    pub fn primary_location(&self) -> &str {
        arn_region(&self.arn)
    }

    /// Can this group host a stream session right now?
    ///
    /// "Prerequisites: You must have a stream group in ACTIVE status"
    /// (`StartStreamSession` documentation). A group that is updating its
    /// locations is still serving the capacity its provisioned locations hold —
    /// `UPDATING_LOCATIONS` says "one or more locations ... are in the process of
    /// updating", not that the group stopped streaming — so it hosts sessions too,
    /// on the locations [`StreamGroup::can_stream_from`] admits.
    pub fn can_host_sessions(&self) -> bool {
        matches!(
            self.status.as_str(),
            STREAM_GROUP_ACTIVE | STREAM_GROUP_UPDATING_LOCATIONS
        )
    }

    /// Does this group hold `location` in a status that can host a stream?
    ///
    /// Only an `ACTIVE` location can: "ACTIVATING: Amazon GameLift Streams is
    /// preparing the location. You cannot stream from, scale the capacity of, or
    /// remove this location yet."
    pub fn can_stream_from(&self, location: &str) -> bool {
        self.locations
            .iter()
            .any(|slot| slot.name == location && slot.status == LOCATION_ACTIVE)
    }

    /// Make room for one more stream session, releasing the sessions that have
    /// reached a terminal status (oldest first) when the store is at
    /// [`SESSIONS_PER_STREAM_GROUP`].
    ///
    /// Answers `false` when every session the bound allows is still live, which is
    /// the one case the caller must refuse; otherwise the group is below the bound
    /// on return.
    pub fn make_room_for_a_session(&mut self) -> bool {
        if self.sessions.len() < SESSIONS_PER_STREAM_GROUP {
            return true;
        }
        let mut terminal: Vec<(SystemTime, String)> = self
            .sessions
            .values()
            .filter(|session| session.is_terminal())
            .map(|session| (session.last_updated_at, session.id.clone()))
            .collect();
        terminal.sort();
        for (_, id) in terminal {
            self.sessions.remove(&id);
            if self.sessions.len() < SESSIONS_PER_STREAM_GROUP {
                return true;
            }
        }
        false
    }

    /// Make room for one more stream URL, releasing the stream URLs that can no
    /// longer start a stream session (oldest first) when the store is at
    /// [`STREAM_URLS_PER_STREAM_GROUP`].
    ///
    /// Answers `false` when every stream URL the bound allows can still start a
    /// stream, which is the one case the caller must refuse; otherwise the group is
    /// below the bound on return. The same rule
    /// [`StreamGroup::make_room_for_a_session`] applies to sessions.
    pub fn make_room_for_a_stream_url(&mut self) -> bool {
        if self.stream_urls.len() < STREAM_URLS_PER_STREAM_GROUP {
            return true;
        }
        let mut spent: Vec<(SystemTime, String)> = self
            .stream_urls
            .values()
            .filter(|url| url.is_spent())
            .map(|url| (url.created_at, url.id.clone()))
            .collect();
        spent.sort();
        for (_, id) in spent {
            self.stream_urls.remove(&id);
            if self.stream_urls.len() < STREAM_URLS_PER_STREAM_GROUP {
                return true;
            }
        }
        false
    }

    /// One location's `LocationState`: the capacity the request configured, the
    /// location's own status, and what is allocated to it and idle in it right now.
    ///
    /// Every capacity SETTING is reported as a number, including the ones the
    /// configuration left out: an unconfigured setting holds the service's default
    /// of zero ("Default is 0 when creating a stream group or adding a location" —
    /// `LocationConfiguration::OnDemandCapacity`), and a zero is what says the
    /// setting is off ("This setting, if non-zero, indicates minimum streaming
    /// capacity ..." — `AlwaysOnCapacity`). An absent member would instead say the
    /// service has nothing to report for it.
    ///
    /// `AllocatedCapacity` is the always-on capacity the location holds once it is
    /// `ACTIVE` (always-on capacity "is allocated to you and is never released back
    /// to the service" — `CreateStreamGroup` documentation), and `IdleCapacity` is
    /// that allocation less the stream sessions the location is currently running.
    /// `InternalVpcIpv4CidrBlock` is the block of the service VPC this location's
    /// capacity runs in ([`SERVICE_VPC_IPV4_CIDR_BLOCK`]).
    ///
    /// The group's own status wins where it says what every location is doing: a
    /// `DELETING` group is releasing all of them, and an `EXPIRED` group's capacity
    /// is gone, so its locations are not provisioned any more and cannot host a
    /// stream.
    pub fn location_state(&self, slot: &LocationSlot) -> LocationState {
        let location_status = match self.status.as_str() {
            STREAM_GROUP_DELETING => LOCATION_REMOVING,
            STREAM_GROUP_EXPIRED => LOCATION_ERROR,
            _ => slot.status.as_str(),
        };
        let requested = slot.always_on_capacity.unwrap_or(0).max(0);
        let allocated = if location_status == LOCATION_ACTIVE {
            requested
        } else {
            0
        };
        let running = i32::try_from(
            self.sessions
                .values()
                .filter(|session| session.location == slot.name && session.is_running())
                .count(),
        )
        .unwrap_or(i32::MAX);
        LocationState {
            location_name: Some(slot.name.clone()),
            status: Some(location_status.to_string()),
            always_on_capacity: Some(slot.always_on_capacity.unwrap_or(0).max(0)),
            on_demand_capacity: Some(slot.on_demand_capacity.unwrap_or(0).max(0)),
            target_idle_capacity: Some(slot.target_idle_capacity.unwrap_or(0).max(0)),
            maximum_capacity: Some(slot.maximum_capacity.unwrap_or(0).max(0)),
            requested_capacity: Some(requested),
            allocated_capacity: Some(allocated),
            // Saturating, not checked: the capacities come from the request,
            // so the arithmetic must not be able to overflow whatever a
            // caller sent.
            idle_capacity: Some(allocated.saturating_sub(running).max(0)),
            internal_vpc_ipv4_cidr_block: Some(SERVICE_VPC_IPV4_CIDR_BLOCK.to_string()),
            vpc_transit_configuration: None,
        }
    }

    /// `LocationStates` for every location this group holds, in the order the
    /// requests that configured them named them.
    pub fn location_states(&self) -> Vec<LocationState> {
        self.locations
            .iter()
            .map(|slot| self.location_state(slot))
            .collect()
    }

    /// `LocationStates` for the locations `names` lists, skipping any this group
    /// does not hold. Used by `AddStreamGroupLocations`, which reports the
    /// locations THAT CALL configured rather than the group's whole set.
    pub fn location_states_of(&self, names: &[String]) -> Vec<LocationState> {
        names
            .iter()
            .filter_map(|name| self.locations.iter().find(|slot| &slot.name == name))
            .map(|slot| self.location_state(slot))
            .collect()
    }

    /// Is any location of this group still activating or being removed? While one
    /// is, the group reports `UPDATING_LOCATIONS`.
    pub fn has_settling_location(&self) -> bool {
        self.locations.iter().any(LocationSlot::is_settling)
    }

    /// Can this group place one more stream session in `location`, beside the
    /// sessions already placed there?
    ///
    /// Capacity lives in a location, and only a provisioned one holds any:
    /// "ACTIVATING: Amazon GameLift Streams is preparing the location. You cannot
    /// stream from ... this location yet." A session "runs on a compute resource
    /// that a stream group has allocated", so the location can run as many streams
    /// at once as it has stream capacity ([`LocationSlot::stream_capacity`]) and no
    /// more. `placing` is the session being placed, which does not count against
    /// itself.
    fn can_place_in(&self, location: &str, placing: &str) -> bool {
        let Some(slot) = self
            .locations
            .iter()
            .find(|slot| slot.name == location && slot.status == LOCATION_ACTIVE)
        else {
            return false;
        };
        let placed = self
            .sessions
            .values()
            .filter(|session| {
                session.id != placing && session.location == location && session.is_placed()
            })
            .count();
        u64::try_from(slot.stream_capacity()).unwrap_or(0) > placed as u64
    }

    /// Where `session`'s placement lands.
    ///
    /// "If Amazon GameLift Streams is successful in finding capacity for the
    /// stream, the stream session status changes to ACTIVE status"; "if Amazon
    /// GameLift Streams was not successful in finding capacity within the placement
    /// timeout period ..., the stream session status changes to ERROR status and
    /// StartStreamSession returns a StatusReason of placementTimeout". A group that
    /// can no longer host streams at all (it is being deleted, or its capacity has
    /// expired) has no capacity to find either, so it times the placement out the
    /// same way.
    ///
    /// The signal request is judged first: a request that carries no WebRTC offer
    /// the stream server could answer is the modelled `invalidSignalRequest`
    /// rather than a capacity problem.
    fn placement_of(&self, session: &StreamSession) -> Placement {
        if !is_webrtc_offer(&session.signal_request) {
            return Placement::Failed(SESSION_REASON_INVALID_SIGNAL_REQUEST);
        }
        if self.can_host_sessions() && self.can_place_in(&session.location, &session.id) {
            Placement::Placed
        } else {
            Placement::Failed(SESSION_REASON_PLACEMENT_TIMEOUT)
        }
    }

    /// Resolve every placement the clock had already made due at `now`.
    ///
    /// Placement is the group's business rather than the session's, because the
    /// capacity a session is placed on belongs to the group's locations and is
    /// shared with the sessions already placed there. The oldest start is resolved
    /// first, so scarce capacity goes to the stream that asked for it first.
    fn resolve_placements(&mut self, now: SystemTime) {
        let mut due: Vec<(SystemTime, String)> = self
            .sessions
            .values()
            .filter(|session| session.placement_due(now))
            .map(|session| (session.created_at, session.id.clone()))
            .collect();
        due.sort();
        for (_, id) in due {
            let Some(session) = self.sessions.get(&id) else {
                continue;
            };
            let Some(due_at) = session.status_due_at else {
                continue;
            };
            let placement = self.placement_of(session);
            if let Some(session) = self.sessions.get_mut(&id) {
                session.resolve_placement(placement, due_at);
            }
        }
    }

    /// Advance this group and its sessions. Answers `true` when the group is due
    /// to be gone (a `DELETING` group whose deletion completed).
    fn advance(&mut self, now: SystemTime) -> bool {
        // The placements the clock owes first: their outcome is decided by the
        // capacity this group holds right now.
        self.resolve_placements(now);
        for session in self.sessions.values_mut() {
            session.advance(now);
        }
        // A session the clock has retained long enough past its terminal status is
        // released, so `sessions` is bounded by the streams still worth reporting
        // rather than by every `StartStreamSession` ever served.
        self.sessions
            .retain(|_, session| !session.retention_elapsed(now));
        // A location whose release the clock has completed is not part of the group
        // any more, so it stops being reported and stops taking up room in the set.
        self.locations.retain_mut(|slot| !slot.advance(now));
        // "streamGroupDeleted: The stream group was deleted, which revoked the
        // stream URL" (`StreamUrlStatusReason`). The deletion is already under way,
        // so the revocation it causes is owed at the same instant — whether or not
        // anybody reads the stream URL.
        if self.status == STREAM_GROUP_DELETING {
            for url in self.stream_urls.values_mut() {
                url.revoke_for(STREAM_URL_REASON_STREAM_GROUP_DELETED);
            }
        }
        for url in self.stream_urls.values_mut() {
            url.advance(now);
        }
        if let Some(due) = self.status_due_at {
            if now >= due {
                // `LastUpdatedAt` moves only when the status actually does.
                match self.status.as_str() {
                    // A group whose locations have all settled is serving again:
                    // "ACTIVE: The stream group is ready to host streams." While
                    // one has not, the group is still updating its locations and
                    // the clock still owes the move, so the due instant stands.
                    STREAM_GROUP_ACTIVATING | STREAM_GROUP_UPDATING_LOCATIONS => {
                        if !self.has_settling_location() {
                            self.status_due_at = None;
                            self.status = STREAM_GROUP_ACTIVE.to_string();
                            self.last_updated_at = due;
                        }
                    }
                    STREAM_GROUP_DELETING => {
                        self.status_due_at = None;
                        return true;
                    }
                    _ => self.status_due_at = None,
                }
            }
        }
        if self.status == STREAM_GROUP_ACTIVE && now >= self.expires_at {
            self.status = STREAM_GROUP_EXPIRED.to_string();
            self.last_updated_at = self.expires_at;
        }
        false
    }
}

/// A persisted stream session, stored under the stream group that hosts it.
#[derive(Debug, Clone)]
pub struct StreamSession {
    pub id: String,
    pub arn: String,
    pub stream_group_id: String,
    pub application_arn: String,
    pub description: Option<String>,
    pub user_id: Option<String>,
    /// A `StreamSessionStatus` value.
    pub status: String,
    /// A `StreamSessionStatusReason` value.
    pub status_reason: Option<String>,
    /// A `Protocol` value.
    pub protocol: String,
    pub location: String,
    pub signal_request: String,
    /// The WebRTC answer "that the stream server generates in response to the
    /// `SignalRequest`" (`SignalResponse` documentation). The answer is the stream
    /// server's half of the WebRTC handshake, and this emulator runs no stream
    /// server, so nothing fills it in: a value here would name a peer no client
    /// could ever reach.
    pub signal_response: Option<String>,
    pub web_sdk_protocol_url: Option<String>,
    pub connection_timeout_seconds: Option<i32>,
    pub session_length_seconds: Option<i32>,
    pub additional_launch_args: Vec<String>,
    pub additional_environment_variables: HashMap<String, String>,
    /// Whether the session shares its performance statistics with the client, as
    /// the request configured it.
    pub performance_stats_configuration: Option<PerformanceStatsConfiguration>,
    /// The virtual monitor the session was started with (its resolution).
    pub display_configuration: Option<DisplayConfiguration>,
    /// The IAM role the session assumes, as the request named it.
    pub role_arn: Option<String>,
    pub log_file_location_uri: Option<String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// When the clock owes this session's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
    /// The administrative terminal sessions this stream session has handed out and
    /// whose window is not up yet, oldest first (see [`AdminShell`]).
    pub admin_shells: Vec<AdminShell>,
}

/// One administrative terminal session handed out for a stream session.
///
/// "Creates an administrative terminal session with full access to the live runtime
/// environment of the Amazon GameLift Streams stream session. Use the returned
/// credentials (SessionId, StreamUrl and TokenValue) with the Amazon Web Services
/// Systems Manager Session Manager plugin"
/// (`CreateStreamSessionAdminShell` documentation). The record is what makes the
/// documented 60-second window real: the clock releases the grant when it is up,
/// and nothing reads a token back out of it — the token is handed to the caller
/// once and is never stored.
#[derive(Debug, Clone)]
pub struct AdminShell {
    /// The Systems Manager session id this grant reported.
    pub session_id: String,
    /// The Systems Manager websocket endpoint this grant reported.
    pub stream_url: String,
    pub created_at: SystemTime,
    /// When this grant's [`ADMIN_SHELL_VALIDITY`] is up.
    pub expires_at: SystemTime,
}

impl StreamSession {
    /// Is this session occupying stream capacity?
    pub fn is_running(&self) -> bool {
        !matches!(
            self.status.as_str(),
            SESSION_ERROR | SESSION_TERMINATED | SESSION_TERMINATING
        )
    }

    /// Is this session PLACED on stream capacity — running on a compute resource
    /// the stream group allocated to it?
    ///
    /// Only a session whose placement succeeded is: a session that is still
    /// `ACTIVATING` is "searching for available stream capacity" and holds none
    /// yet, and one that has stopped or failed has given its own back. These are
    /// the statuses a placed stream holds, from the model's `StreamSessionStatus`.
    pub fn is_placed(&self) -> bool {
        matches!(
            self.status.as_str(),
            SESSION_ACTIVE
                | SESSION_CONNECTED
                | SESSION_PENDING_CLIENT_RECONNECTION
                | SESSION_RECONNECTING
        )
    }

    /// Has this session reached a status it can never leave?
    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), SESSION_ERROR | SESSION_TERMINATED)
    }

    /// Is this session's [`TERMINAL_SESSION_RETENTION`] up, so the clock owes its
    /// release? `false` for a session that is still live.
    pub fn retention_elapsed(&self, now: SystemTime) -> bool {
        self.is_terminal() && now >= self.last_updated_at + TERMINAL_SESSION_RETENTION
    }

    /// Does the clock owe this session's placement by `now`? A session is
    /// `ACTIVATING` while the service "begins searching for available stream
    /// capacity to run the stream", and the placement resolves one [`TRANSITION`]
    /// later.
    fn placement_due(&self, now: SystemTime) -> bool {
        self.status == SESSION_ACTIVATING && self.status_due_at.is_some_and(|due| now >= due)
    }

    /// Is this session in a status an administrative terminal session can be opened
    /// into? "The stream session must be in one of the following statuses: ACTIVE,
    /// CONNECTED, PENDING_CLIENT_RECONNECTION, or RECONNECTING"
    /// (`CreateStreamSessionAdminShell` documentation) — the statuses of a stream
    /// that is placed and therefore has a live runtime environment.
    pub fn admits_an_admin_shell(&self) -> bool {
        self.is_placed()
    }

    /// Record one administrative terminal session this stream session handed out.
    ///
    /// The oldest grant is released when the record is at
    /// [`ADMIN_SHELLS_PER_SESSION`], so the set is bounded by that count as well as
    /// by [`ADMIN_SHELL_VALIDITY`].
    pub fn grant_admin_shell(&mut self, shell: AdminShell) {
        while self.admin_shells.len() >= ADMIN_SHELLS_PER_SESSION {
            self.admin_shells.remove(0);
        }
        self.admin_shells.push(shell);
    }

    /// Settle this session's placement, which the clock owed at `due`.
    ///
    /// A placed stream is `ACTIVE` and carries no `StatusReason` — that member is
    /// "a short description of the reason the stream session is in ERROR status or
    /// TERMINATED status" — and a placement that failed carries the reason it
    /// failed for. Which outcome applies is the stream group's call
    /// ([`StreamGroup::placement_of`]): it holds the capacity.
    fn resolve_placement(&mut self, placement: Placement, due: SystemTime) {
        self.status_due_at = None;
        match placement {
            Placement::Placed => {
                self.status = SESSION_ACTIVE.to_string();
                self.status_reason = None;
            }
            Placement::Failed(reason) => {
                self.status = SESSION_ERROR.to_string();
                self.status_reason = Some(reason.to_string());
            }
        }
        // `LastUpdatedAt` is the instant the transition was DUE, not the instant
        // it was observed, so two reads of the same session agree.
        self.last_updated_at = due;
    }

    /// Advance the transition the clock owed at `now`.
    ///
    /// The one transition a session owes on its own is the end of a stream that was
    /// asked to stop; the resolution of a placement belongs to the stream group
    /// whose capacity the stream is placed on (see
    /// [`StreamGroup::resolve_placements`]).
    fn advance(&mut self, now: SystemTime) {
        // "The StreamUrl is valid for 60 seconds. After it expires, call this
        // operation again to get a new URL": a terminal session whose window is up
        // is gone whether or not anybody asked, so the grant goes with it.
        self.admin_shells.retain(|shell| now < shell.expires_at);
        let Some(due) = self.status_due_at else {
            return;
        };
        if now < due {
            return;
        }
        if self.status == SESSION_TERMINATING {
            self.status_due_at = None;
            self.status = SESSION_TERMINATED.to_string();
            // The runtime environment the stream ran in is gone with the stream, so
            // the terminal sessions into it are gone too.
            self.admin_shells.clear();
            // `LastUpdatedAt` moves only when the status actually does.
            self.last_updated_at = due;
        }
    }
}

/// How a stream session's placement resolved.
///
/// "Placement completion: If Amazon GameLift Streams is successful in finding
/// capacity for the stream, the stream session status changes to ACTIVE status ...
/// If Amazon GameLift Streams was not successful ..., the stream session status
/// changes to ERROR status" (`StartStreamSession` documentation).
#[derive(Debug, Clone, Copy)]
enum Placement {
    /// Capacity was found for the stream.
    Placed,
    /// It was not, for this `StreamSessionStatusReason`.
    Failed(&'static str),
}

/// The one `SignalRequest` value that starts a stream session without carrying a
/// WebRTC offer: "Amazon GameLift Streams also supports setting the field to
/// \"NO_CLIENT_CONNECTION\". This will create a session without needing any browser
/// request or Web SDK integration. The session starts up as usual and waits for a
/// reconnection from a browser" (`SignalRequest` documentation).
pub const NO_CLIENT_CONNECTION: &str = "NO_CLIENT_CONNECTION";

/// Whether `signal_request` carries something a stream server could answer, which
/// is what decides between a placement and the modelled `invalidSignalRequest`
/// failure ("The WebRTC signal request that was sent is not valid").
///
/// `SignalRequest` is "a WebRTC ICE offer string to use when initializing a WebRTC
/// connection. Typically, the offer is a very long JSON string", so both forms a
/// WebRTC client hands out are accepted: the session description as a JSON object,
/// which is what the Web SDK produces (the description itself under `sdp`, and a
/// `type` that must be the offer rather than an answer), and a bare SDP offer. An
/// SDP body is a sequence of `<type>=<value>` lines opening with its protocol
/// version, and SDP has exactly one version — `v=0`. The documented
/// [`NO_CLIENT_CONNECTION`] sentinel carries no offer at all and still starts a
/// session.
pub fn is_webrtc_offer(signal_request: &str) -> bool {
    let signal_request = signal_request.trim();
    if signal_request == NO_CLIENT_CONNECTION {
        return true;
    }
    if let Ok(serde_json::Value::Object(description)) =
        serde_json::from_str::<serde_json::Value>(signal_request)
    {
        if description
            .get("type")
            .is_some_and(|kind| kind.as_str() != Some("offer"))
        {
            return false;
        }
        return description
            .get("sdp")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_session_description);
    }
    is_session_description(signal_request)
}

/// Whether `text` is an SDP body: its first line is SDP's single version line.
fn is_session_description(text: &str) -> bool {
    text.trim_start()
        .lines()
        .next()
        .is_some_and(|version| version.trim_end() == "v=0")
}

/// A persisted stream URL, stored under the stream group that runs the stream
/// sessions it starts.
///
/// The record is what every stream-URL operation reads: `CreateStreamUrl` mints it,
/// `GetStreamUrl` reports it, and `RevokeStreamUrl` moves it to `REVOKED`.
#[derive(Debug, Clone)]
pub struct StreamUrl {
    pub id: String,
    pub arn: String,
    pub stream_group_id: String,
    pub stream_group_arn: String,
    pub application_arn: String,
    pub description: Option<String>,
    /// The shareable locator this stream URL hands out, built once with the record
    /// (see [`shareable_stream_url`]) so two reads of the same stream URL agree.
    pub stream_url: String,
    /// A `Protocol` value.
    pub protocol: String,
    pub locations: Vec<String>,
    pub session_length_seconds: i32,
    pub additional_launch_args: Vec<String>,
    pub additional_environment_variables: HashMap<String, String>,
    /// The virtual monitor a stream session started from this URL gets, as the
    /// request configured it.
    pub display_configuration: Option<DisplayConfiguration>,
    /// The IAM role such a session assumes, as the request named it.
    pub role_arn: Option<String>,
    pub usage_limit: i32,
    pub remaining_uses: i32,
    /// A `StreamUrlStatus` value.
    pub status: String,
    /// A `StreamUrlStatusReason` value. "Amazon GameLift Streams populates this
    /// value when the status is REVOKED" (`StatusReason` documentation).
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
}

impl StreamUrl {
    /// Can this stream URL still start a stream session? Only an `ACTIVE` one can:
    /// "EXPIRED: The stream URL has passed its expiration time and can no longer
    /// start stream sessions. REVOKED: The stream URL was revoked and can no longer
    /// start stream sessions" (`StreamUrlStatus` documentation).
    pub fn is_usable(&self) -> bool {
        self.status == STREAM_URL_ACTIVE
    }

    /// Has this stream URL reached a status it can never leave? The complement of
    /// [`Self::is_usable`], and what makes a record releasable when the per-group
    /// bound needs room.
    pub fn is_spent(&self) -> bool {
        !self.is_usable()
    }

    /// Revoke this stream URL for `reason`, one of the model's
    /// `StreamUrlStatusReason` values.
    ///
    /// "Revoking a stream URL is permanent. The status of the stream URL changes to
    /// REVOKED" (`RevokeStreamUrl` documentation) — so a URL already `REVOKED` keeps
    /// the reason it was first revoked for, which is what makes the operation
    /// `@idempotent`.
    pub fn revoke_for(&mut self, reason: &str) {
        if self.status == STREAM_URL_REVOKED {
            return;
        }
        self.status = STREAM_URL_REVOKED.to_string();
        self.status_reason = Some(reason.to_string());
    }

    /// Advance this stream URL to the status the clock already owed it at `now`.
    ///
    /// The one transition the clock owes is expiry: "The number of minutes after
    /// creation that the stream URL remains valid. After this period, the status of
    /// the stream URL changes to EXPIRED and it can no longer start stream sessions"
    /// (`UrlExpiresAfterMinutes` documentation). `LIMIT_REACHED` is reached by USING
    /// the URL, and a use starts a stream session in the hosted web player rather
    /// than through this API, so no call here spends one: `RemainingUses` reports
    /// the uses the record still holds.
    fn advance(&mut self, now: SystemTime) {
        if self.is_usable() && now >= self.expires_at {
            self.status = STREAM_URL_EXPIRED.to_string();
        }
    }
}

/// The shareable locator a stream URL hands out: what an end user opens to play a
/// stream session in the hosted web player.
///
/// Amazon GameLift Streams publishes no format for the value, so it is built from
/// the record itself — the Region the stream URL was created in, the stream group
/// and stream URL ids its ARN carries, and the opaque token minted with it. No two
/// stream URLs can therefore share a locator, and the locator cannot be derived
/// from the ids alone: "Treat the stream URL as a secret. Anyone who has it can
/// start a stream session until the stream URL expires, is revoked, or reaches its
/// usage limit" (`StreamUrl` documentation). The hosted web player the locator
/// addresses is a browser client of the real service, not part of this emulator.
pub fn shareable_stream_url(region: &str, group_id: &str, url_id: &str, token: &str) -> String {
    format!("https://streams.{SIGNING_NAME}.{region}.amazonaws.com/{group_id}/{url_id}?token={token}")
}

/// How many characters the opaque token a shareable stream URL carries holds.
pub const STREAM_URL_TOKEN_LENGTH: usize = 32;

/// How many characters the opaque suffix of an administrative terminal session's
/// `SessionId` holds.
pub const ADMIN_SHELL_SESSION_SUFFIX_LENGTH: usize = 16;

/// The `SessionId` one administrative terminal session reports: "an Amazon Web
/// Services Systems Manager session identifier that uniquely identifies the
/// requested terminal session".
///
/// Amazon GameLift Streams publishes no format for the value, so it is built the
/// way a Session Manager session id is: the target the session is into — here the
/// stream session's own id — and an opaque part that makes one grant distinct from
/// the next. `suffix` comes from the store's mint, so two grants for one stream
/// session never share an id and nothing about the request can be read back out of
/// it.
pub fn admin_shell_session_id(stream_session_id: &str, suffix: &str) -> String {
    format!("{stream_session_id}-{suffix}")
}

/// The `StreamUrl` one administrative terminal session reports: "an Amazon Web
/// Services Systems Manager WebSocket connection endpoint for the requested
/// terminal session".
///
/// The form is the one Systems Manager hands a Session Manager client for a
/// session's data channel — the regional `ssmmessages` websocket endpoint, the
/// session's own id and the publish-subscribe role the client opens the channel
/// with. The runtime environment at the far end of that channel is the stream
/// session's own, which this emulator does not run: the grant, its id and its
/// 60-second window are real state, the interactive terminal is not.
pub fn admin_shell_stream_url(region: &str, session_id: &str) -> String {
    format!(
        "wss://ssmmessages.{region}.amazonaws.com/v1/data-channel/{session_id}\
         ?role=publish_subscribe"
    )
}

/// The `TokenValue` one administrative terminal session reports: "an Amazon Web
/// Services Systems Manager authentication token that authenticates your access to
/// the session ID and WebSocket URL".
///
/// The value is a secret handed to the caller, so nothing here constructs it: it
/// comes from the one component that generates secrets, the emulator's credential
/// store. It authenticates a Systems Manager data channel rather than a request to
/// this emulator, so it is NOT a credential of the emulator and must never be
/// accepted as one — which is why the minted entry is removed from the store again
/// as soon as its secret has been read.
///
/// `None` when the store minted nothing, which leaves the operation to refuse
/// rather than report a token it made up.
pub fn mint_admin_shell_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .to_string();
    // The terminal session's token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    (!token.is_empty()).then_some(token)
}

/// Per-service state for com-amazonaws-gameliftstreams-gameliftstreams. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGameliftstreamsGameliftstreamsState {
    /// Applications by application id.
    pub applications: HashMap<String, Application>,
    /// Stream groups by stream group id; each one owns its stream sessions.
    pub stream_groups: HashMap<String, StreamGroup>,
    /// `"<operation>:<ClientToken>"` -> what that token already created, so a
    /// replayed `@idempotencyToken` answers with the first result instead of
    /// creating a second resource, and the same token sent with DIFFERENT
    /// parameters is refused instead of quietly creating one. The operation name is
    /// part of the key because one client may reuse a token value across unrelated
    /// creates.
    pub idempotency: HashMap<String, TokenRecord>,
    /// Monotonic counter mixed into every minted id, which is what makes two
    /// minted ids distinct by construction rather than by luck.
    mint_counter: u64,
}

/// What one `ClientToken` has already done: which resource it created, and a
/// [`Fingerprint`] of the parameters it created that resource with.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    /// Id of the resource this token created.
    pub resource_id: String,
    /// Canonical rendering of the request parameters of the call that created it.
    pub fingerprint: String,
}

/// What a request's `ClientToken` means for the call carrying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenUse {
    /// No token, or a token this operation has not served before: do the work.
    Fresh,
    /// This token already created a resource with these very parameters: answer
    /// with that resource (the id), and create nothing.
    Replay(String),
    /// This token already created a resource with DIFFERENT parameters, which is
    /// the one case an idempotency token refuses rather than serves.
    Conflict,
}

/// A canonical rendering of the request parameters an `@idempotencyToken` covers,
/// used to tell a genuine retry (same parameters) from a token reused with
/// different ones.
///
/// Every value is length-prefixed, so no sequence of values can render as another
/// sequence, and a map's entries are sorted by key, so two requests carrying the
/// same tags in a different order fingerprint alike — a `HashMap`'s iteration order
/// is not a request parameter. The `ClientToken` itself is never part of it: it is
/// the key the fingerprint is stored under.
#[derive(Debug, Default)]
pub struct Fingerprint {
    rendered: String,
}

impl Fingerprint {
    /// An empty fingerprint.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a present text member.
    pub fn text(&mut self, member: &str, value: &str) {
        // The member NAME is length-prefixed too, so a synthetic key a collection
        // builds (`"<member>.<position>"`) can never render as a differently-named
        // member: the canonical form is collision-free by construction.
        self.rendered.push_str(&member.len().to_string());
        self.rendered.push(':');
        self.rendered.push_str(member);
        self.rendered.push('=');
        self.rendered.push_str(&value.len().to_string());
        self.rendered.push(':');
        self.rendered.push_str(value);
        self.rendered.push(';');
    }

    /// Add a member that may be absent. An absent member renders differently from
    /// any present value, including the empty string.
    pub fn optional(&mut self, member: &str, value: Option<&str>) {
        match value {
            Some(value) => self.text(member, value),
            None => {
                self.rendered.push_str(&member.len().to_string());
                self.rendered.push(':');
                self.rendered.push_str(member);
                self.rendered.push_str("=-;");
            }
        }
    }

    /// Add a numeric member that may be absent.
    pub fn number(&mut self, member: &str, value: Option<i32>) {
        self.optional(member, value.map(|value| value.to_string()).as_deref());
    }

    /// Add a list member, in the order the request sent it: a list's order is part
    /// of the request.
    pub fn list(&mut self, member: &str, values: &[String]) {
        self.text(member, &values.len().to_string());
        for (position, value) in values.iter().enumerate() {
            self.text(&format!("{member}.{position}"), value);
        }
    }

    /// Add a map member, sorted by key.
    pub fn map(&mut self, member: &str, values: &HashMap<String, String>) {
        let mut entries: Vec<(&String, &String)> = values.iter().collect();
        entries.sort();
        self.text(member, &entries.len().to_string());
        for (key, value) in entries {
            self.text(&format!("{member}.{key}"), value);
        }
    }

    /// The rendered fingerprint. Takes the builder: a fingerprint is built once
    /// per request and read once.
    pub fn finish(self) -> String {
        self.rendered
    }
}

impl ComAmazonawsGameliftstreamsGameliftstreamsState {
    /// Mint a fresh `Id` with `prefix`: five pseudo-random characters followed by
    /// this store's mint counter in base62, so the result is unique within the
    /// store and matches the model's `Id` shape.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        let mut id = String::with_capacity(prefix.len() + 11);
        id.push_str(prefix);
        id.push_str(&self.random_chars(5));
        let base = ID_ALPHABET.len() as u64;
        let mut counter = self.mint_counter;
        let mut digits = Vec::new();
        loop {
            digits.push(ID_ALPHABET[(counter % base) as usize]);
            counter /= base;
            if counter == 0 {
                break;
            }
        }
        digits.reverse();
        for digit in digits {
            id.push(char::from(digit));
        }
        id
    }

    /// A fresh opaque token of `length` characters over [`ID_ALPHABET`], for a value
    /// a caller must not be able to guess (the token a shareable stream URL
    /// carries). Minted from the same stream as [`Self::mint_id`], so two tokens of
    /// the same nanosecond differ by construction rather than by luck.
    pub fn mint_token(&mut self, length: usize) -> String {
        self.random_chars(length)
    }

    /// `count` characters of this store's pseudo-random stream, mixed from the wall
    /// clock and the mint counter.
    fn random_chars(&mut self, count: usize) -> String {
        self.mint_counter += 1;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        let mut hash = nanos ^ self.mint_counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut characters = String::with_capacity(count);
        for _ in 0..count {
            hash = hash
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(0xBF58_476D_1CE4_E5B9);
            let mixed = (hash >> 29) ^ hash;
            characters.push(char::from(
                ID_ALPHABET[(mixed % ID_ALPHABET.len() as u64) as usize],
            ));
        }
        characters
    }

    /// Apply every transition the clock had already made due at `now`, and drop
    /// the resources whose deletion completed.
    ///
    /// Every handler calls this before it reads or writes, so a reporting
    /// operation reports the state the clock owed at the instant it ran — and
    /// nothing else.
    pub fn materialize(&mut self, now: SystemTime) {
        self.applications
            .retain(|_, application| !application.advance(now));
        self.stream_groups.retain(|_, group| !group.advance(now));
        self.revoke_stream_urls_of_deleted_applications();
        self.forget_dead_idempotency_keys();
    }

    /// Revoke every stream URL whose application this store no longer holds.
    ///
    /// "applicationDeleted: The application was deleted, which revoked the stream
    /// URL" (`StreamUrlStatusReason`), and "if you delete the stream group or
    /// application that backs the stream URL, this operation updates the status of
    /// the stream URL to REVOKED" (`GetStreamUrl` documentation). The delete already
    /// happened, so the revocation it causes is owed at the same instant whether or
    /// not anybody reads the stream URL. The stream-group half of that sentence is
    /// applied by [`StreamGroup::advance`], which can see its own status; this half
    /// needs both stores, so it lives here.
    fn revoke_stream_urls_of_deleted_applications(&mut self) {
        let live: HashSet<&String> = self
            .applications
            .values()
            .map(|application| &application.arn)
            .collect();
        for group in self.stream_groups.values_mut() {
            for url in group.stream_urls.values_mut() {
                if url.is_usable() && !live.contains(&url.application_arn) {
                    url.revoke_for(STREAM_URL_REASON_APPLICATION_DELETED);
                }
            }
        }
    }

    /// Drop the idempotency entries whose resource is gone.
    ///
    /// A `ClientToken` can only answer with the resource it created, so an entry
    /// whose resource has been deleted can never be used again. Releasing it here
    /// means the table is bounded by the LIVE resources rather than by the number
    /// of creates this emulator has ever served: it shrinks on exactly the
    /// caller-driven path (`Delete*`, then this) that releases the resources.
    fn forget_dead_idempotency_keys(&mut self) {
        let applications = &self.applications;
        let stream_groups = &self.stream_groups;
        self.idempotency
            .retain(|key, record| match key.split_once(':').map(|(name, _)| name) {
                Some("CreateApplication") => applications.contains_key(&record.resource_id),
                Some("CreateStreamGroup") => stream_groups.contains_key(&record.resource_id),
                Some("StartStreamSession") => stream_groups
                    .values()
                    .any(|group| group.sessions.contains_key(&record.resource_id)),
                Some("CreateStreamUrl") => stream_groups
                    .values()
                    .any(|group| group.stream_urls.contains_key(&record.resource_id)),
                // An operation this slice records nothing for: there is nothing to
                // judge the entry by, so leave it rather than drop it blindly.
                _ => true,
            });
    }

    /// The stored application an `Identifier` names, by bare id or by ARN.
    pub fn application(&self, identifier: &str) -> Option<&Application> {
        self.applications
            .get(&identifier_id(identifier, APPLICATION_RESOURCE)?)
    }

    /// The stored application an `Identifier` names, for mutation.
    pub fn application_mut(&mut self, identifier: &str) -> Option<&mut Application> {
        let id = identifier_id(identifier, APPLICATION_RESOURCE)?;
        self.applications.get_mut(&id)
    }

    /// The stored stream group an `Identifier` names, by bare id or by ARN.
    pub fn stream_group(&self, identifier: &str) -> Option<&StreamGroup> {
        self.stream_groups
            .get(&identifier_id(identifier, STREAM_GROUP_RESOURCE)?)
    }

    /// The stored stream group an `Identifier` names, for mutation.
    pub fn stream_group_mut(&mut self, identifier: &str) -> Option<&mut StreamGroup> {
        let id = identifier_id(identifier, STREAM_GROUP_RESOURCE)?;
        self.stream_groups.get_mut(&id)
    }

    /// The stored stream URL the two identifiers of a stream-URL operation name: the
    /// stream group that owns it, and the stream URL itself (either of them by bare
    /// id or by ARN).
    pub fn stream_url(&self, group_identifier: &str, url_identifier: &str) -> Option<&StreamUrl> {
        let id = identifier_id(url_identifier, STREAM_URL_RESOURCE)?;
        self.stream_group(group_identifier)?.stream_urls.get(&id)
    }

    /// The stored stream URL those two identifiers name, for mutation.
    pub fn stream_url_mut(
        &mut self,
        group_identifier: &str,
        url_identifier: &str,
    ) -> Option<&mut StreamUrl> {
        let id = identifier_id(url_identifier, STREAM_URL_RESOURCE)?;
        self.stream_group_mut(group_identifier)?
            .stream_urls
            .get_mut(&id)
    }

    /// What this call's `ClientToken` means: a token no previous call of
    /// `operation` carried is [`TokenUse::Fresh`], one that created a resource with
    /// the same `fingerprint` is a [`TokenUse::Replay`] of that resource, and one
    /// that created a resource with a different fingerprint is a
    /// [`TokenUse::Conflict`] — "the request is idempotent, which ensures that an
    /// API request completes only once" (`ClientToken` documentation), so the same
    /// token cannot stand for two different requests.
    pub fn client_token_use(
        &self,
        operation: &str,
        client_token: Option<&str>,
        fingerprint: &str,
    ) -> TokenUse {
        let Some(token) = client_token else {
            return TokenUse::Fresh;
        };
        match self.idempotency.get(&idempotency_key(operation, token)) {
            Some(record) if record.fingerprint == fingerprint => {
                TokenUse::Replay(record.resource_id.clone())
            }
            Some(_) => TokenUse::Conflict,
            None => TokenUse::Fresh,
        }
    }

    /// The tags of the taggable resource an `Arn` names.
    ///
    /// "You can assign tags to the following Amazon GameLift Streams resource
    /// types: Application StreamGroup" (`TagResource`'s documentation), so the ARN
    /// is resolved against those two stores and nothing else: an ARN of any other
    /// resource type (a stream session, say) names nothing taggable and answers
    /// `None`, as does an ARN whose resource this store no longer holds.
    ///
    /// An `Arn` is always the full ARN form (`pattern
    /// ^arn:aws:gameliftstreams:([^: ]*):([0-9]{12}):([^: ]*)$`) — unlike an
    /// `Identifier`, a bare id is not one, so a non-ARN resolves nothing.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&HashMap<String, String>> {
        if !resource_arn.starts_with("arn:") {
            return None;
        }
        if let Some(application) = self.application(resource_arn) {
            return Some(&application.tags);
        }
        self.stream_group(resource_arn).map(|group| &group.tags)
    }

    /// The tags of the taggable resource an `Arn` names, for mutation — the same
    /// resolution as [`Self::tags_of`], for the operations that assign or remove
    /// tags rather than report them.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut HashMap<String, String>> {
        if !resource_arn.starts_with("arn:") {
            return None;
        }
        // Which store holds it is decided by an immutable lookup first, so only one
        // mutable borrow of `self` is ever live.
        if self.application(resource_arn).is_some() {
            return self
                .application_mut(resource_arn)
                .map(|application| &mut application.tags);
        }
        self.stream_group_mut(resource_arn)
            .map(|group| &mut group.tags)
    }

    /// Record which resource a `ClientToken` created, and the fingerprint of the
    /// request that created it.
    pub fn remember(
        &mut self,
        operation: &str,
        client_token: Option<&str>,
        id: &str,
        fingerprint: &str,
    ) {
        if let Some(token) = client_token {
            self.idempotency.insert(
                idempotency_key(operation, token),
                TokenRecord {
                    resource_id: id.to_string(),
                    fingerprint: fingerprint.to_string(),
                },
            );
        }
    }
}

fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}:{client_token}")
}

/// One page of a `List*` operation over `ids`, and the `NextToken` that continues
/// it.
///
/// The ids are sorted, so `after` — the last id of the previous page — is all the
/// token has to carry: a page boundary stays correct when the store changes between
/// calls, which an offset could not promise. `None` for the token means this was
/// the last page. Shared by every paginated `List*` so the two pagers cannot drift.
pub fn page_of_ids(
    mut ids: Vec<String>,
    after: &str,
    page_size: usize,
) -> (Vec<String>, Option<String>) {
    ids.sort();
    let mut page: Vec<String> = ids
        .into_iter()
        .filter(|id| after.is_empty() || id.as_str() > after)
        .take(page_size + 1)
        .collect();
    if page.len() > page_size {
        page.truncate(page_size);
        let next = page.last().cloned();
        (page, next)
    } else {
        (page, None)
    }
}

/// The resource id an `Identifier` names.
///
/// The model's `Identifier` shape accepts either form for the same resource
/// (`pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^: ]*):([0-9]{12}):([^: ]*)$)$`),
/// so a bare id is returned as given and an ARN is resolved to the last segment of
/// its resource path — but only when that path is of `resource_type`, so an
/// application ARN never resolves a stream group.
pub fn identifier_id(identifier: &str, resource_type: &str) -> Option<String> {
    if !identifier.starts_with("arn:") {
        return Some(identifier.to_string());
    }
    let mut fields = identifier.splitn(6, ':');
    if fields.next() != Some("arn") {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next() != Some(SIGNING_NAME) {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let path = fields
        .next()?
        .strip_prefix(resource_type)?
        .strip_prefix('/')?;
    path.split('/').next_back().map(str::to_string)
}

/// The region segment of an ARN this service minted, which for every resource it
/// holds is the region the request that created the resource was answered in.
fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
}

/// One resource's ARN: `arn:aws:gameliftstreams:<region>:<account>:<type>/<path>`.
fn resource_arn(region: &str, resource_type: &str, path: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("{resource_type}/{path}"))
}

/// The ARN of an application: `arn:aws:gameliftstreams:<region>:<account>:application/<id>`.
pub fn application_arn(region: &str, id: &str) -> String {
    resource_arn(region, APPLICATION_RESOURCE, id)
}

/// The ARN of a stream group: `arn:aws:gameliftstreams:<region>:<account>:streamgroup/<id>`.
pub fn stream_group_arn(region: &str, id: &str) -> String {
    resource_arn(region, STREAM_GROUP_RESOURCE, id)
}

/// The ARN of a stream session, which carries the hosting stream group:
/// `arn:aws:gameliftstreams:<region>:<account>:streamsession/<group id>/<session id>`.
pub fn stream_session_arn(region: &str, group_id: &str, session_id: &str) -> String {
    child_arn(region, STREAM_SESSION_RESOURCE, group_id, session_id)
}

/// The ARN of a stream URL, which carries the stream group that runs its sessions:
/// "Format is arn:aws:gameliftstreams:[AWS Region]:[AWS account]:streamurl/[stream
/// group resource ID]/[stream URL resource ID]" (`Arn` documentation on
/// `CreateStreamUrl`).
pub fn stream_url_arn(region: &str, group_id: &str, url_id: &str) -> String {
    child_arn(region, STREAM_URL_RESOURCE, group_id, url_id)
}

/// One stream-group-scoped resource's ARN: `<type>/<stream group id>/<resource id>`,
/// the composite form both the stream sessions and the stream URLs of a group take.
fn child_arn(region: &str, resource_type: &str, group_id: &str, id: &str) -> String {
    resource_arn(region, resource_type, &format!("{group_id}/{id}"))
}

/// The ARN the `Identifier` a request passed resolves to, for the resource type
/// the operation addresses.
///
/// The model's `Identifier` shape accepts either form for the same resource, so an
/// identifier already in ARN form IS the resolved ARN, and a bare id resolves
/// against the region the request is answered in and this emulator's account. The
/// identifier does NOT have to name a resource the store holds: resolution is a
/// property of the request alone, which is what lets a not-found message name the
/// ARN that is missing.
///
/// No kind check is performed, unlike [`identifier_id`]: an identifier already in
/// ARN form is reported as the caller wrote it, even when its resource type is not
/// the one the operation addresses, because that string IS the ARN the request
/// named and the message reports what the request asked for.
pub fn resolved_arn(region: &str, identifier: &str, resource_type: &str) -> String {
    if identifier.starts_with("arn:") {
        return identifier.to_string();
    }
    resource_arn(region, resource_type, identifier)
}

/// The composite stream session ARN the two path labels of a session-scoped
/// operation resolve to: `…:streamsession/<stream group id>/<stream session id>`.
///
/// A `StreamSessionIdentifier` already in ARN form is its own resolved ARN;
/// otherwise the enclosing stream group's id comes from the operation's
/// `Identifier` label, whether that label is a bare id or a stream group ARN. An
/// `Identifier` that is an ARN of some OTHER resource names no stream group this
/// service can resolve, and the message still has to carry a well-formed ARN, so
/// its last path segment stands in for the group id rather than being embedded
/// whole.
pub fn resolved_stream_session_arn(
    region: &str,
    group_identifier: &str,
    session_identifier: &str,
) -> String {
    resolved_child_arn(
        region,
        STREAM_SESSION_RESOURCE,
        group_identifier,
        session_identifier,
    )
}

/// The composite ARN a stream-group-scoped resource's two identifiers resolve to.
fn resolved_child_arn(
    region: &str,
    resource_type: &str,
    group_identifier: &str,
    identifier: &str,
) -> String {
    if identifier.starts_with("arn:") {
        return identifier.to_string();
    }
    let group_id = identifier_id(group_identifier, STREAM_GROUP_RESOURCE).unwrap_or_else(|| {
        group_identifier
            .rsplit(['/', ':'])
            .next()
            .unwrap_or(group_identifier)
            .to_string()
    });
    child_arn(region, resource_type, &group_id, identifier)
}

/// The service's `ResourceNotFoundException` for a resource-scoped operation: it
/// names the ARN the request's identifier resolved to and carries a per-request
/// correlation id.
///
/// The model types `ResourceNotFoundException` with a single required `Message` and
/// `@httpError 404` and says nothing about the text, but the text the service emits
/// is built from the REQUEST — the resolved ARN of the identifier the caller
/// passed — so every not-found path in this crate goes through here (or through
/// [`stream_session_not_found`]) rather than wording its own.
pub fn resource_not_found(arn: &str) -> ResourceNotFoundException {
    not_found("Resource", arn)
}

/// The service's `ResourceNotFoundException` for the session-scoped operations
/// (`GetStreamSession`, `TerminateStreamSession`, `CreateStreamSessionConnection`,
/// `CreateStreamSessionAdminShell`), which name the stream session — the resource
/// the caller addressed — even when it is the enclosing stream group that is absent.
pub fn stream_session_not_found(arn: &str) -> ResourceNotFoundException {
    not_found("Stream session", arn)
}

/// The service's `ResourceNotFoundException` for the stream-URL operations
/// (`GetStreamUrl`, `RevokeStreamUrl`), whose addressed resource is the stream URL
/// the two path labels name.
///
/// A stream URL is a secret a caller distributes to end users ("Treat the stream URL
/// as a secret" — the `StreamUrl` member's documentation), so this class of miss is
/// reported as a bare sentence: it names NO identifier and NO ARN, and carries only
/// the per-request correlation id [`with_internal_id`] appends. Nothing about the
/// request is quoted back, which is why it is worded here rather than through
/// [`not_found`].
pub fn stream_url_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: with_internal_id("Stream URL not found"),
    }
}

/// `<subject> '<arn>' does not exist (Internal ID: <uuid>)`, through the one
/// [`with_internal_id`] every handler-side message of this service is worded by.
fn not_found(subject: &str, arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: with_internal_id(format!("{subject} '{arn}' does not exist")),
    }
}

/// The region a request is answered in: the region of its SigV4 credential scope,
/// or [`DEFAULT_REGION`] when it carries none.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A `SystemTime` as the `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGameliftstreamsGameliftstreamsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGameliftstreamsGameliftstreamsState>)
}
