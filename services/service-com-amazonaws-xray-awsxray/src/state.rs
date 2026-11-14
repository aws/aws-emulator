// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-xray-awsxray service.
//!
//! X-Ray's ingest -> query -> read loop is entirely state-derived:
//! `PutTraceSegments` stores each accepted segment document VERBATIM under its
//! trace, and every reader (`BatchGetTraces`, `GetTraceSummaries`) recomputes
//! its answer from those stored documents — nothing is echoed from the request
//! that stored them. Groups are a keyed map plus an ARN index, so `GetGroup`
//! resolves a group by either selector; the account-level `Default` group is
//! DERIVED rather than stored ([`default_group`]), because it exists in every
//! region without being created and cannot be created or deleted. Sampling
//! rules are a map keyed by `RuleName`, holding what `CreateSamplingRule`
//! stored; `GetSamplingRules` lists that map and `DeleteSamplingRule` removes
//! from it, by either selector. The account's service-managed
//! [`DEFAULT_SAMPLING_RULE_NAME`] rule is DERIVED rather than stored
//! ([`default_sampling_rule`]), exactly as the `Default` group is, so a fresh
//! store already lists it; only an `UpdateSamplingRule` of it ever stores a
//! record under that name, and no delete may remove it. Indexing
//! rules are a fixed, seeded keyed map: the account's `Default` rule exists
//! before the first call, and `UpdateIndexingRule` rewrites its configuration in
//! place for `GetIndexingRules` to report.
//!
//! [`filter_expression`] is the `FilterExpression` grammar `CreateGroup` and
//! `UpdateGroup` validate against before either writes anything.
//!
//! A group's tags are keyed by tag key ON its record, so `CreateGroup`,
//! `TagResource`, `UntagResource` and `ListTagsForResource` read and write ONE
//! set per resource rather than a tag store beside it, and the restrictions the
//! model documents for a tag are written down once, in
//! [`validate_tag`]/[`validate_tag_key`].
//!
//! The segment-document schema this module parses is the one `PutTraceSegments`
//! documents (`smithy_query(query="doc",shape="PutTraceSegments")`): `name`,
//! `id`, `trace_id` and `start_time` are required, `end_time`/`in_progress` say
//! whether the segment is complete, and `error`/`fault`/`throttle`, `http`,
//! `parent_id`, `annotations`, `cause` and `subsegments` are optional. Two more
//! members come from the X-Ray Segment Documents reference that
//! `PutTraceSegments` links to, because the service graph is derived from them:
//! `type`, which is `subsegment` on a subsegment sent as its own document, and
//! `namespace`, which is `aws` for an AWS SDK call and `remote` for any other
//! call that left the process.
//!
//! Two things a reader reports are DERIVED from those documents rather than
//! carried by them, and both are derived here so that `BatchGetTraces` and
//! `GetTraceSummaries` cannot disagree:
//!
//! * a subsegment that names a downstream service (`namespace: "remote"`) gets
//!   an INFERRED top-level segment synthesised for that downstream service
//!   ([`inferred_segments`]), which is the service X-Ray attributes the call to,
//!   and which the call's own namespace TYPES ([`inferred_service_type`])
//!   because a service nothing reported for carries no `origin`;
//! * every entity (a segment, or a subsegment inside one) records an
//!   [`Outcome`], read from the `fault`/`error`/`throttle` fields its document
//!   DECLARED: the one [`Classification`] a COUNTER splits it
//!   into (a fault outranks a throttle, which outranks an error), and whether it
//!   recorded a 400-series error at all. A recorded `http.response.status` is
//!   data the document carries, reported as `Http$HttpStatus` and selectable
//!   through the `http.status` keyword, and it never stands in for a flag the
//!   document did not set. The trace's
//!   `HasFault`/`HasError`/`HasThrottle` flags and its root causes are read off
//!   those outcomes.
//!
//! Daemon telemetry (`PutTelemetryRecords`, "used by the Amazon Web Services
//! X-Ray daemon to upload telemetry") is kept the same way: each submitted
//! `TelemetryRecord` is stored VERBATIM under the reporting source the request
//! identifies, beside the running totals accumulated over every record that
//! source ever had accepted — including records that have since fallen out of
//! its [`MAX_TELEMETRY_RECORDS_PER_SOURCE`] window. No modelled operation reads
//! telemetry back — `PutTelemetryRecordsResult` carries no members and X-Ray
//! publishes no telemetry reader — so the store is what the ingest asserts, and
//! it is bounded like the trace store is.
//!
//! A trace retrieval is the same loop over a longer clock: `StartTraceRetrieval`
//! stores one [`TraceRetrievalRecord`] per accepted call under the token it
//! minted, and every reader of that token (`ListRetrievedTraces`,
//! `GetRetrievedTracesGraph`) recomputes BOTH its `RetrievalStatus` — from the
//! instants stored on the record, through
//! [`TraceRetrievalRecord::status`] — and its traces — from the stored segment
//! documents of the requested ids inside the requested window, through
//! [`ComAmazonawsXrayAwsxrayState::retrieved_traces`]. `CancelTraceRetrieval` is
//! the one call that moves a retrieval's status by hand.
//!
//! Every instant a segment document carries is checked where the document is
//! read ([`MAX_SEGMENT_EPOCH_SECONDS`]), so no stored `start_time` / `end_time`
//! can be a value a reader cannot convert to the modelled `Timestamp`; the
//! conversion itself goes through [`timestamp_of`].

pub mod encryption_key;
pub mod filter;
pub mod group_scope;
pub mod graphs;
pub mod pagination;
pub mod query_window;
pub mod retrievals;
pub mod sampling_rules;
pub mod sampling_statistics;
pub mod span_delivery;

// Each part above owns one concern of this service's state and is re-exported
// here, so every handler keeps reaching the store through `crate::state::…` and
// no caller has to know which part a record, a bound or a wording lives in.
pub use encryption_key::*;
pub use graphs::*;
pub use pagination::*;
pub use query_window::*;
pub use retrievals::*;
pub use sampling_rules::*;
pub use sampling_statistics::*;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::ops::types::{
    EncryptionConfig, Group, GroupSummary, IndexingRule, IndexingRuleValue, Insight,
    InsightsConfiguration, ProbabilisticRuleValue, TelemetryRecord,
};

/// The region an ARN carries when the request's SigV4 scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from the model's `awsService.arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "xray";

/// `UnprocessedTraceSegment$ErrorCode` for a document that cannot be read as a
/// JSON segment object at all.
///
/// The model documents `ErrorCode` only as "The error that caused processing to
/// fail" and publishes no value set, so the VALUES are X-Ray's own, as its
/// recorded `PutTraceSegments` responses state them: a document that cannot be
/// read is `ParseError`, and a required member that is absent is `Missing` plus
/// that member's name in PascalCase (`MissingName` for an absent `name`). A
/// member that is present but unusable — wrong JSON type, or a `trace_id` that
/// is not a well-formed trace id — follows the same construction as `Invalid`
/// plus the member name, and a document refused because its trace is full takes
/// [`SEGMENT_ERROR_LIMIT_EXCEEDED`]. The `Message` beside the code RESTATES it,
/// which is the one wording X-Ray uses for every per-document rejection:
/// `Invalid segment. ErrorCode: <ErrorCode>`. So the code is where the fault is
/// named, and [`segment_member_code`] is the single place a member's code is
/// built.
pub const SEGMENT_ERROR_PARSE: &str = "ParseError";

// ---------------------------------------------------------------------------
// Encryption configuration
//
// `PutEncryptionConfig` replaces the whole document and `GetEncryptionConfig`
// reports what is stored, so both read one record ([`EncryptionConfigRecord`])
// rather than keeping a copy each.
// ---------------------------------------------------------------------------

/// `EncryptionConfig$Type` for encryption with a caller-selected KMS key, from
/// [`crate::ops::types::ENCRYPTION_TYPE_VALUES`].
pub const ENCRYPTION_TYPE_KMS: &str = "KMS";

/// `EncryptionConfig$Type` for X-Ray's default encryption, from
/// [`crate::ops::types::ENCRYPTION_TYPE_VALUES`].
pub const ENCRYPTION_TYPE_NONE: &str = "NONE";

/// `EncryptionConfig$Status` while a submitted configuration is being applied,
/// from [`crate::ops::types::ENCRYPTION_STATUS_VALUES`].
pub const ENCRYPTION_STATUS_UPDATING: &str = "UPDATING";

/// `EncryptionConfig$Status` once the stored configuration is in force, from
/// [`crate::ops::types::ENCRYPTION_STATUS_VALUES`].
pub const ENCRYPTION_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a configuration reports `UPDATING` before the clock owes it
/// `ACTIVE`.
///
/// CHOSEN, not documented: X-Ray publishes no duration for applying a new
/// encryption configuration, so this is a value inside the project's 250–750 ms
/// band for a transition the clock owes. It is deliberately non-zero, because
/// `EncryptionConfig$Status` documents a state a caller can observe — "While the
/// status is UPDATING, X-Ray may encrypt data with a combination of the new and
/// old settings" — so a read taken straight after the put must still see it.
pub const ENCRYPTION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// What X-Ray answers a `PutEncryptionConfig` that selects KMS encryption
/// without naming a key with.
///
/// FIXED text: the refusal states the condition that was broken — a key is
/// needed for this type of encryption — and interpolates nothing from the
/// request, so it is the same sentence whatever else the request carried.
pub const ENCRYPTION_KEY_ID_REQUIRED_MESSAGE: &str = "Key ID is required for type KMS";

/// The number of digits in an Amazon Web Services account id, which the account
/// field of an ARN carries.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The number of dash-separated groups in the id KMS mints for a single-region
/// key.
const KEY_ID_GROUPS: usize = 5;

/// The number of hexadecimal digits after the `mrk-` prefix of the id KMS mints
/// for a multi-region key.
const MULTI_REGION_KEY_ID_DIGITS: usize = 32;

/// Whether `candidate` is a KMS key id (`PutEncryptionConfig$KeyId`: "Key ID -
/// The KMS key ID of the key").
///
/// The emulator resolves no KMS key, so only the FORM is judged, and it is judged
/// no more narrowly than the id's two shapes: [`KEY_ID_GROUPS`] non-empty groups
/// of hexadecimal digits for a single-region key — which covers both the
/// canonical 8-4-4-4-12 grouping and the differently grouped example that
/// member's own documentation gives — or `mrk-` and
/// [`MULTI_REGION_KEY_ID_DIGITS`] hexadecimal digits for a multi-region key.
fn is_kms_key_id(candidate: &str) -> bool {
    let hexadecimal = |group: &str| {
        !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    if let Some(digits) = candidate.strip_prefix("mrk-") {
        return digits.len() == MULTI_REGION_KEY_ID_DIGITS && hexadecimal(digits);
    }
    let mut groups = 0usize;
    for group in candidate.split('-') {
        if !hexadecimal(group) {
            return false;
        }
        groups += 1;
    }
    groups == KEY_ID_GROUPS
}

/// Whether `candidate` is a KMS alias path: `alias/` followed by a non-empty
/// name of the characters KMS's alias name allows (`^[a-zA-Z0-9:/_-]+$`), as in
/// `PutEncryptionConfig$KeyId`'s "Alias - The name of the key. For example,
/// alias/MyKey".
fn is_kms_alias(candidate: &str) -> bool {
    candidate.strip_prefix("alias/").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/_-".contains(&byte))
    })
}

/// The six colon-delimited fields of an ARN —
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — or `None` when
/// `candidate` is not in that form at all.
///
/// One definition of "split an ARN" for this module: the callers differ only in
/// which fields they then require.
fn arn_fields(candidate: &str) -> Option<[&str; 6]> {
    let mut split = candidate.splitn(6, ':');
    let fields = [
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
    ];
    (fields[0] == "arn").then_some(fields)
}

/// Whether `candidate` is the full ARN of a KMS key or alias
/// (`arn:<partition>:kms:<region>:<account>:key/<key id>` or `…:alias/<name>`),
/// the third form `PutEncryptionConfig$KeyId` documents.
fn is_kms_arn(candidate: &str) -> bool {
    let Some([_, partition, service, region, account, resource]) = arn_fields(candidate) else {
        return false;
    };
    service == "kms"
        && !partition.is_empty()
        && !region.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && (is_kms_alias(resource) || resource.strip_prefix("key/").is_some_and(is_kms_key_id))
}

/// Whether `key_id` selects a KMS key in one of the three forms
/// `PutEncryptionConfig$KeyId` documents — an alias, a key id, or the full ARN of
/// either. Anything else is refused: the emulator holds no KMS keys, so the form
/// of the selector is all there is to judge.
pub fn is_kms_key_selector(key_id: &str) -> bool {
    is_kms_alias(key_id) || is_kms_key_id(key_id) || is_kms_arn(key_id)
}

/// The ARN of the KMS key a submitted `KeyId` selector names, as a caller in
/// `region` reads it.
///
/// The configuration a reader reports IDENTIFIES the key that protects the
/// account's trace data, and an identifier that names a key only inside one
/// account and region does not identify it: a bare key id or alias is the same
/// text in every account. So the stored configuration holds the FULLY QUALIFIED
/// form — the partition, the region the configuration was put in and the account
/// this service serves — whatever form the request used, and a selector that
/// already carries those fields is kept exactly as submitted, including the
/// partition, region and account it names.
pub fn kms_key_arn(key_id: &str, region: &str) -> String {
    if is_kms_arn(key_id) {
        return key_id.to_string();
    }
    // The two unqualified forms `PutEncryptionConfig$KeyId` documents: an alias
    // path, which an ARN carries as its resource unchanged, and a key id, whose
    // resource is `key/<id>`.
    let resource = if is_kms_alias(key_id) {
        key_id.to_string()
    } else {
        format!("key/{key_id}")
    };
    ::emulator_server::emulator_arn("kms", region, &resource)
}

/// The encryption configuration this service reports: what `PutEncryptionConfig`
/// last stored, and when the clock owes it `ACTIVE`.
#[derive(Debug, Clone)]
pub struct EncryptionConfigRecord {
    /// `EncryptionConfig$Type`: [`ENCRYPTION_TYPE_KMS`] or
    /// [`ENCRYPTION_TYPE_NONE`].
    pub encryption_type: String,
    /// `EncryptionConfig$KeyId`: the ARN of the key the caller selected, which
    /// is what IDENTIFIES it ([`kms_key_arn`]); `None` under
    /// [`ENCRYPTION_TYPE_NONE`].
    pub key_id: Option<String>,
    /// When `UPDATING` -> `ACTIVE` falls due; `None` once the configuration is
    /// active. See [`ENCRYPTION_SETTLE_AFTER`].
    pub active_due: Option<Instant>,
}

impl Default for EncryptionConfigRecord {
    /// Until a configuration is put, X-Ray encrypts with its own key: `Type`
    /// `NONE`, `Status` `ACTIVE`, and no `KeyId`.
    fn default() -> Self {
        Self {
            encryption_type: ENCRYPTION_TYPE_NONE.to_string(),
            key_id: None,
            active_due: None,
        }
    }
}

impl EncryptionConfigRecord {
    /// Materialise the `UPDATING` -> `ACTIVE` transition the clock owes by `now`.
    pub fn settle(&mut self, now: Instant) {
        if self.active_due.is_some_and(|due| due <= now) {
            self.active_due = None;
        }
    }

    /// This record as the modelled `EncryptionConfig`, as of `now`.
    pub fn to_config(&self, now: Instant) -> EncryptionConfig {
        let status = if self.active_due.is_some_and(|due| due > now) {
            ENCRYPTION_STATUS_UPDATING
        } else {
            ENCRYPTION_STATUS_ACTIVE
        };
        EncryptionConfig {
            key_id: self.key_id.clone(),
            status: Some(status.to_string()),
            r#type: Some(self.encryption_type.clone()),
        }
    }
}

/// `UnprocessedTraceSegment$ErrorCode` for a document refused because its trace
/// already holds [`MAX_SEGMENTS_PER_TRACE`] segments.
pub const SEGMENT_ERROR_LIMIT_EXCEEDED: &str = "LimitExceeded";

/// The `ErrorCode` for one segment member at fault: `<prefix><Member>`, the
/// member's snake_case document name rendered in PascalCase, as `MissingName`
/// is built from `name`.
fn segment_member_code(prefix: &str, member: &str) -> String {
    let mut code = prefix.to_string();
    for word in member.split('_') {
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            code.extend(first.to_uppercase());
            code.push_str(characters.as_str());
        }
    }
    code
}

/// Whether `value` is a well-formed X-Ray trace id: `1-<8 hex>-<24 hex>`, the
/// form `PutTraceSegments` documents for a segment's `trace_id` and the only
/// form a stored trace is indexed under (`TraceId` is 1..=35 characters, which
/// this form occupies exactly).
///
/// The form itself is read through the ONE shared reading of it
/// (`service_common::xray_trace`), which a service that merely CARRIES a trace
/// header asks as well, so no two services judge one id differently.
pub fn is_trace_id(value: &str) -> bool {
    ::service_common::xray_trace::is_trace_id(value)
}

/// What a READ asked for a trace id that is not of [`is_trace_id`]'s form is
/// refused with: X-Ray's template, naming the value the request carried and the
/// rule it broke, and nothing else.
///
/// Written once because every reader that takes a `TraceId` refuses the same
/// malformed id the same way, whichever member carried it.
pub fn malformed_trace_id_message(trace_id: &str) -> String {
    format!("Invalid trace id {trace_id}: incorrect id format")
}

/// What a read naming more trace ids than [`MAX_TRACE_IDS_PER_CALL`] is refused
/// with: X-Ray's fixed phrase, which states the limit itself — the model
/// publishes none, so the refusal is the only place a client can read it.
pub fn exceeding_maximum_query_size_message() -> String {
    format!("Exceeding maximum query size: {MAX_TRACE_IDS_PER_CALL}")
}

// ---------------------------------------------------------------------------
// Bounds on caller-driven work and growth
//
// Every limit below exists so that no request, and no sequence of requests, can
// make this service do or hold unbounded work: the per-call document count, the
// segments one trace accumulates, the number of traces and groups kept, and the
// number of segment documents one response carries are each capped, and the
// trace store evicts its least recently ingested trace once full (real X-Ray
// expires traces too, so the cap is also closer to the service).
// ---------------------------------------------------------------------------

/// The most segment documents one `PutTraceSegments` call accepts. A larger
/// batch is refused outright rather than reported document by document, so a
/// single request can neither drive unbounded parsing nor amplify into an
/// unbounded response.
pub const MAX_SEGMENT_DOCUMENTS_PER_CALL: usize = 500;

/// The most segments one trace holds. A further NEW segment is refused as an
/// `UnprocessedTraceSegment` and marks the trace
/// [`limit_exceeded`](TraceRecord::limit_exceeded), which is what `Trace$LimitExceeded`
/// reports.
pub const MAX_SEGMENTS_PER_TRACE: usize = 1_000;

/// The most subsegments one segment document contributes, counting every level
/// of nesting, so one document can neither drive unbounded parsing nor hold
/// unbounded state. Further ones are dropped.
pub const MAX_SUBSEGMENTS_PER_SEGMENT: usize = 1_000;

/// The most traces the store keeps. Ingesting a new trace past this evicts the
/// least recently ingested one.
pub const MAX_TRACES: usize = 1_000;

/// The most groups the store keeps; `CreateGroup` past this is refused.
pub const MAX_GROUPS: usize = 250;
/// The most telemetry records one `PutTelemetryRecords` call accepts.
///
/// `TelemetryRecordList` carries no modelled length bound, so the batch is
/// bounded here — as `PutTraceSegments`'s is — rather than letting one call
/// drive unbounded work and growth.
pub const MAX_TELEMETRY_RECORDS_PER_CALL: usize = 500;

/// The most telemetry records kept per reporting source: a rolling window of the
/// most recently submitted ones. Older records fall out of the window, while the
/// source's running [`TelemetryTotals`] keep counting every record accepted.
pub const MAX_TELEMETRY_RECORDS_PER_SOURCE: usize = 1_000;

/// The most reporting sources the telemetry store keeps. A report from a new
/// source past this evicts the one that has gone longest without reporting.
pub const MAX_TELEMETRY_SOURCES: usize = 500;

/// The most user-applied tags one taggable resource holds.
///
/// Published by the service, not chosen here: the `Tag` shape's own
/// documentation (`smithy_query(query="shape",shape="Tag")`) says "Maximum
/// number of user-applied tags per resource: 50". A `TagResource` call that
/// would carry a resource past it is refused with `TooManyTagsException`, and a
/// `CreateGroup` with `InvalidRequestException` (the only 400 it models).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The prefix reserved for Amazon Web Services' own tag keys.
///
/// From `TagResource$Tags`' documentation: "Don't use aws: as a prefix for
/// keys; it's reserved for Amazon Web Services use. You cannot edit or delete
/// system tags." So a caller may neither apply nor remove a key carrying it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

// ---------------------------------------------------------------------------
// Trace segment destination (Transaction Search)
// ---------------------------------------------------------------------------

/// The destination an account sends `PutTraceSegments` and OTLP data to before
/// `UpdateTraceSegmentDestination` ever ran: `TraceSegmentDestination$XRay`.
///
/// The `CloudWatchLogs` destination is what Transaction Search requires
/// (`smithy_query(query="doc",shape="UpdateTraceSegmentDestination")`), and
/// Transaction Search is opt-in, so an account that never asked for it reports
/// the X-Ray destination.
pub const DEFAULT_TRACE_SEGMENT_DESTINATION: &str = "XRay";

/// `TraceSegmentDestination$CloudWatchLogs`: the destination Transaction Search
/// requires, and the only one a `CloudWatchLogsConfiguration` applies to (that
/// member's documentation: "Specify this only when `Destination` is
/// `CloudWatchLogs`").
pub const CLOUD_WATCH_LOGS_DESTINATION: &str = "CloudWatchLogs";

/// `TraceSegmentDestinationStatus$ACTIVE`: the configured destination is in
/// effect.
pub const DESTINATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `TraceSegmentDestinationStatus$PENDING`: a destination change was accepted
/// and is not in effect yet.
pub const DESTINATION_STATUS_PENDING: &str = "PENDING";

/// How long a destination change reports `PENDING` before the clock owes it
/// `ACTIVE`.
///
/// Neither the model nor the AWS documentation states a duration for the
/// switch, so this span is chosen: non-zero, so a caller reading straight back
/// still observes the `PENDING` the real service reports, and short enough that
/// nothing has to wait long for `ACTIVE`.
pub const DESTINATION_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The indexing rule every account already holds, before any call.
///
/// Transaction Search ships one rule, named `Default`, and this service's model
/// publishes no operation that creates or deletes an indexing rule
/// (`smithy_query(query="list_operations")` has only `GetIndexingRules` and
/// `UpdateIndexingRule`), so this is the only name [`IndexingRuleStore`] holds.
pub const DEFAULT_INDEXING_RULE_NAME: &str = "Default";

/// The `DesiredSamplingPercentage` the [`DEFAULT_INDEXING_RULE_NAME`] rule
/// carries until `UpdateIndexingRule` rewrites it: the 1% of spans Transaction
/// Search indexes out of the box.
pub const DEFAULT_INDEXING_SAMPLING_PERCENTAGE: f64 = 1.0;

/// The largest `DesiredSamplingPercentage` an indexing rule accepts.
///
/// `ProbabilisticRuleValueUpdate$DesiredSamplingPercentage` targets
/// `NullableDouble` and the model states no range, but the member is documented
/// as a "sampling percentage of traceIds": a percentage outside 0..=100 names no
/// sampling rate, so it is refused rather than stored.
pub const MAX_SAMPLING_PERCENTAGE: f64 = 100.0;

/// The most trace ids one `BatchGetTraces` call accepts.
///
/// `TraceIdList` carries no modelled length bound, so the bound is the
/// service's: X-Ray's `BatchGetTraces` "retrieves a list of traces specified by
/// ID" five at a time, and a longer list is refused as an invalid request
/// rather than partly served.
pub const MAX_TRACE_IDS_PER_CALL: usize = 5;

/// The most segment documents one `BatchGetTraces` response carries. A trace
/// that does not fit in what is left of the budget is deferred to the next page
/// rather than truncated, so a page always makes progress and a whole trace is
/// always readable ([`MAX_SEGMENTS_PER_TRACE`] segments fit the budget exactly).
pub const MAX_SEGMENTS_PER_BATCH_RESPONSE: usize = MAX_SEGMENTS_PER_TRACE;

/// The deepest `subsegments` nesting one segment document may carry.
///
/// Every reader walks that tree RECURSIVELY — the classifications a summary
/// unions, the frames a root cause names, the entity index an `edge(…)` filter
/// resolves through — so its depth is caller-driven work on the read path, and
/// the one kind a reader cannot recover from: exhausting the stack aborts the
/// process instead of failing the request. A document nested deeper than this is
/// therefore refused at ingest, like any other member it cannot use, and every
/// walk is bounded by this constant by construction. Deep enough for the nesting
/// an instrumented call tree produces (an SDK subsegment inside a handler
/// subsegment inside a segment is three).
pub const MAX_SUBSEGMENT_DEPTH: usize = 16;

/// The most root causes one trace reports per class
/// ([`TraceRecord::root_causes`]).
///
/// A root cause costs a walk of the trace, so without a cap a trace holding
/// [`MAX_SEGMENTS_PER_TRACE`] failing segments would make one read do work in
/// the square of its size. Past the cap the answer is the first causes derived,
/// which degrades the answer rather than the service.
pub const MAX_ROOT_CAUSES_PER_TRACE: usize = 32;

/// The most frames one response-time root cause's chain descends through
/// ([`TraceRecord::root_causes`] for `RootCauseKind::ResponseTime`).
///
/// The descent takes one step per frame, and a stored trace can grow in two
/// dimensions at once — [`MAX_SEGMENTS_PER_TRACE`] segments, each
/// [`MAX_SUBSEGMENT_DEPTH`] levels deep — so this caps their product rather than
/// either factor. Past it the chain reports the frames it reached.
pub const MAX_CRITICAL_PATH_FRAMES: usize = MAX_SEGMENTS_PER_TRACE + MAX_SUBSEGMENT_DEPTH;

/// How much of a trace's response time one component below its root segment must
/// have been running for to be reported as the trace's response-time root cause
/// ([`TraceRecord::root_causes`] for `RootCauseKind::ResponseTime`).
///
/// A response-time root cause names the component the time went to, so there has
/// to be one: a trace whose root segment spent its span on nothing in particular
/// — several short children, or no child at all — has no such component, and
/// reports no cause. X-Ray publishes no threshold, so "most of the response
/// time" is this emulator's reading of it.
pub const RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE: f64 = 0.5;

/// The most sampling rules the store keeps; `CreateSamplingRule` past this is
/// refused with the modelled `RuleLimitExceededException`.
///
/// The model documents no quota, so this is the emulator's bound on growth, in
/// the same spirit as [`MAX_GROUPS`].
pub const MAX_SAMPLING_RULES: usize = 500;

/// The most trace retrievals the store keeps. A new retrieval past this evicts
/// the one that started longest ago (expired ones are dropped first).
pub const MAX_TRACE_RETRIEVALS: usize = 250;

/// How long a started trace retrieval stays readable.
///
/// "Retrievals will time out after 60 minutes" — the `StartTraceRetrieval`
/// documentation (`smithy_query(query="doc",shape="StartTraceRetrieval")`), so
/// the value is the service's, not this emulator's choice.
pub const TRACE_RETRIEVAL_TIMEOUT_SECONDS: f64 = 60.0 * 60.0;

/// How many `StartTraceRetrieval` calls may arrive back-to-back before the
/// account's request budget is empty and the next one is throttled.
///
/// `ThrottledException` is "the request exceeds the maximum number of requests
/// per second" and the model publishes no number, so the budget below is this
/// emulator's: it bounds how much retrieval work one caller can start at once,
/// and refills at [`TRACE_RETRIEVAL_RATE_PER_SECOND`] so a steady caller is
/// never throttled.
pub const TRACE_RETRIEVAL_BURST: u32 = 3;

/// How fast the `StartTraceRetrieval` budget refills, in requests per second.
///
/// Slower than the burst, so a caller that fires several retrievals at once runs
/// the budget down and is told so, while a caller that starts one retrieval at a
/// time is never throttled.
pub const TRACE_RETRIEVAL_RATE_PER_SECOND: f64 = 3.0;

/// `RetrievalStatus` for a retrieval that was accepted but has not begun
/// reading, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_SCHEDULED: &str = "SCHEDULED";

/// `RetrievalStatus` while a retrieval is reading, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_RUNNING: &str = "RUNNING";

/// `RetrievalStatus` once a retrieval's results are readable, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_COMPLETE: &str = "COMPLETE";

/// `RetrievalStatus` for a retrieval `CancelTraceRetrieval` stopped, from
/// [`RETRIEVAL_STATUS_VALUES`](crate::ops::types::RETRIEVAL_STATUS_VALUES).
pub const RETRIEVAL_STATUS_CANCELLED: &str = "CANCELLED";

/// How long a started retrieval reports `SCHEDULED` before the clock owes it
/// `RETRIEVAL_STATUS_RUNNING`.
///
/// CHOSEN, not documented: X-Ray publishes no duration for scheduling a
/// retrieval, so this is a value inside the project's 250–750 ms band for a
/// transition the clock owes. It is deliberately non-zero because
/// `GetRetrievedTracesGraph` documents the states a caller can observe — "Retry
/// the request after the status changes from RUNNING or SCHEDULED to COMPLETE" —
/// so a read taken straight after the start must still see the retrieval before
/// it runs.
pub const TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS: f64 = 0.25;

/// How long a retrieval reports `RETRIEVAL_STATUS_RUNNING` before the clock owes
/// it `RETRIEVAL_STATUS_COMPLETE`.
///
/// CHOSEN for the same reason as [`TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS`], and
/// non-zero for the same one: "the API will return an empty response" while the
/// status is not `COMPLETE` (`ListRetrievedTraces`), which a caller can only
/// observe if the running state lasts.
pub const TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS: f64 = 0.25;

/// `TraceFormatType` for the X-Ray segment documents this service ingests, from
/// [`TRACE_FORMAT_TYPE_VALUES`](crate::ops::types::TRACE_FORMAT_TYPE_VALUES).
pub const TRACE_FORMAT_XRAY: &str = "XRAY";

/// `TraceFormatType` for OpenTelemetry spans, from
/// [`TRACE_FORMAT_TYPE_VALUES`](crate::ops::types::TRACE_FORMAT_TYPE_VALUES).
pub const TRACE_FORMAT_OTEL: &str = "OTEL";

/// How far past the present a `[StartTime, EndTime]` bound may land before the
/// window is refused as not naming a time the data could have.
///
/// A whole day is far more than any clock skew, which is deliberate: the bound
/// exists to refuse a value that cannot be a timestamp of recorded traffic at
/// all — a millisecond-scale epoch value read as seconds lands thousands of
/// years out — not to second-guess a caller asking for a window that reaches a
/// little past now.
pub const MAX_WINDOW_FUTURE_SKEW_SECONDS: f64 = 24.0 * 60.0 * 60.0;

/// The most entries one `Histogram` in a service graph carries. The model
/// publishes no bound, so this caps the response a caller can amplify out of
/// widely-spread segment durations.
pub const MAX_HISTOGRAM_ENTRIES: usize = 50;

/// How far from the epoch an instant a segment document carries may be, in
/// seconds.
///
/// A `start_time` or `end_time` beyond this — and any value that is not finite
/// — is not a time a trace could have been recorded at (a millisecond-scale
/// epoch value read as seconds already lands past it, the same mistake
/// [`validate_window`] refuses for a request window) and is not a value every
/// reader's modelled `Timestamp` can carry, so the document is refused at
/// ingest rather than stored and converted later. 1e12 seconds is about 31,000
/// years, which covers every instant in seconds of epoch time and no
/// millisecond-scale one.
pub const MAX_SEGMENT_EPOCH_SECONDS: f64 = 1e12;

/// Per-service state for com-amazonaws-xray-awsxray. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsXrayAwsxrayState {
    /// Ingested traces, keyed by `trace_id`, at most [`MAX_TRACES`] of them.
    pub traces: BTreeMap<String, TraceRecord>,
    /// Groups, keyed by [`GroupKey`] — the region they live in and their
    /// case-sensitive `GroupName` — at most [`MAX_GROUPS`] of them.
    ///
    /// The reserved [`DEFAULT_GROUP_NAME`] group is NOT here: it is derived
    /// ([`default_group`]), so a fresh store already answers for it, and only an
    /// `UpdateGroup` of it ever stores a record under that name.
    pub groups: BTreeMap<GroupKey, GroupRecord>,
    /// `GroupARN` -> the [`GroupKey`] it names, so a reader can resolve a group
    /// by either selector. A group ARN embeds its region and carries a unique
    /// suffix, so one ARN names at most one record anywhere in this store.
    pub group_arn_to_name: BTreeMap<String, GroupKey>,
    /// Feeds the unique suffix of a minted group ARN; never reset, so two groups
    /// of the same name created in sequence still get distinct ARNs.
    pub group_seq: u64,
    /// Ingest counter, stamped on a trace so the oldest can be evicted.
    pub ingest_seq: u64,
    /// Sampling rules, keyed by their case-sensitive `RuleName`, at most
    /// [`MAX_SAMPLING_RULES`] of them. A rule's ARN is derived from its name, so
    /// no second index is needed to resolve one by ARN.
    pub sampling_rules: BTreeMap<String, StoredSamplingRule>,
    /// Started trace retrievals, keyed by the `RetrievalToken` that was minted
    /// for them, at most [`MAX_TRACE_RETRIEVALS`] of them.
    pub retrievals: BTreeMap<String, TraceRetrievalRecord>,
    /// Feeds the unique prefix of a minted `RetrievalToken`; never reset, so two
    /// retrievals of the same traces still get distinct tokens.
    pub retrieval_seq: u64,
    /// The account's `StartTraceRetrieval` request budget.
    pub retrieval_rate: RateLimiter,
    /// Insights, keyed by their `InsightId`.
    ///
    /// X-Ray MINTS insights itself, from anomaly detection over ingested traces
    /// ("When fault rates go outside of the expected range, X-Ray creates an
    /// insight" — `smithy_query(query="shape",shape="Insight")`); no operation in
    /// the API creates one. This emulator does not run that analysis, so this is
    /// the map an insight generator would write and `GetInsight` /
    /// `GetInsightSummaries` read: an `InsightId` it does not hold names no
    /// insight, which is exactly what `GetInsight` refuses.
    pub insights: BTreeMap<String, Insight>,
    /// Daemon telemetry, keyed by the reporting source the request identified,
    /// at most [`MAX_TELEMETRY_SOURCES`] of them.
    pub telemetry: BTreeMap<TelemetrySource, TelemetryReporter>,
    /// Report counter, stamped on a reporter so the least recently reporting one
    /// can be evicted; never reset.
    pub telemetry_seq: u64,
    /// The encryption configuration `PutEncryptionConfig` stores and
    /// `GetEncryptionConfig` reports; X-Ray's default until one is put.
    pub encryption: EncryptionConfigRecord,
    /// Where `PutTraceSegments` and OTLP data go, as last set by
    /// `UpdateTraceSegmentDestination`; its [`Default`] is the account default
    /// every reader answers until something changes it.
    pub trace_segment_destination: TraceSegmentDestinationRecord,
    /// The account's indexing rules, which `GetIndexingRules` reports and
    /// `UpdateIndexingRule` rewrites.
    pub indexing_rules: IndexingRuleStore,
    /// The sampling results services reported through `GetSamplingTargets`, the
    /// reservoir shares computed from them, and when the account's sampling-rule
    /// configuration last changed.
    pub sampling: SamplingStatisticsStore,
}

/// Who reported telemetry: the `PutTelemetryRecords` request's own reporter
/// identity (`EC2InstanceId`, `Hostname`, `ResourceARN`), which is what
/// distinguishes one daemon's counters from another's.
///
/// A member carrying the empty string identifies nothing, so it is held as
/// absent: each of the three is modelled `len 0..N`
/// (`smithy_query(query="operation",operation="PutTelemetryRecords")`), so a
/// daemon that sends `""` for a field it does not know reports as the same
/// source as one that omits it.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TelemetrySource {
    /// `PutTelemetryRecordsRequest$EC2InstanceId`.
    pub ec2_instance_id: Option<String>,
    /// `PutTelemetryRecordsRequest$Hostname`.
    pub hostname: Option<String>,
    /// `PutTelemetryRecordsRequest$ResourceARN`.
    pub resource_arn: Option<String>,
}

impl TelemetrySource {
    /// The source a request identifies, with an empty-string member held as
    /// absent.
    pub fn new(
        ec2_instance_id: Option<&str>,
        hostname: Option<&str>,
        resource_arn: Option<&str>,
    ) -> Self {
        let identifier = |value: Option<&str>| {
            value
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        Self {
            ec2_instance_id: identifier(ec2_instance_id),
            hostname: identifier(hostname),
            resource_arn: identifier(resource_arn),
        }
    }
}

/// The account's trace segment destination, as stored.
///
/// `GetTraceSegmentDestination` answers from this record and
/// `UpdateTraceSegmentDestination` is the only thing that changes it, so the
/// reported destination is always the one that was last configured — never a
/// value echoed from the request being answered.
#[derive(Debug, Clone)]
pub struct TraceSegmentDestinationRecord {
    /// The configured destination: one of
    /// [`TRACE_SEGMENT_DESTINATION_VALUES`](crate::ops::types::TRACE_SEGMENT_DESTINATION_VALUES).
    pub destination: String,
    /// When the accepted change becomes effective, i.e. when the clock owes the
    /// `PENDING` -> `ACTIVE` move. `None` once the destination is in effect.
    pub active_at: Option<Instant>,
    /// `CloudWatchLogsConfiguration$SpansLogGroupKmsKeyId`, as last supplied
    /// with a `CloudWatchLogs` destination, and dropped when the service is
    /// pointed anywhere else. Kept so the configuration survives the call that
    /// set it; no modelled output returns it, so
    /// [`ComAmazonawsXrayAwsxrayState::set_trace_segment_destination`]'s own
    /// test is what pins it.
    pub spans_log_group_kms_key_id: Option<String>,
}

impl Default for TraceSegmentDestinationRecord {
    fn default() -> Self {
        Self {
            destination: DEFAULT_TRACE_SEGMENT_DESTINATION.to_string(),
            active_at: None,
            spans_log_group_kms_key_id: None,
        }
    }
}

impl TraceSegmentDestinationRecord {
    /// The `TraceSegmentDestinationStatus` this record reports: `PENDING` while
    /// an accepted change is still owed, `ACTIVE` once it is in effect.
    pub fn status(&self) -> &'static str {
        if self.active_at.is_some() {
            DESTINATION_STATUS_PENDING
        } else {
            DESTINATION_STATUS_ACTIVE
        }
    }
}

/// The indexing rules this service holds, keyed by rule name.
///
/// The set of rules is FIXED: X-Ray models no create and no delete for one, so
/// the store is seeded with [`DEFAULT_INDEXING_RULE_NAME`] at construction and
/// only ever has a rule's configuration rewritten under it. That is why a read
/// never has to materialise anything — the `Default` rule is there before the
/// first call, exactly as it is on a fresh account.
#[derive(Debug)]
pub struct IndexingRuleStore {
    rules: BTreeMap<String, IndexingRuleRecord>,
}

impl Default for IndexingRuleStore {
    fn default() -> Self {
        let record = IndexingRuleRecord {
            name: DEFAULT_INDEXING_RULE_NAME.to_string(),
            modified_at_secs: now_epoch_secs(),
            desired_sampling_percentage: DEFAULT_INDEXING_SAMPLING_PERCENTAGE,
        };
        Self {
            rules: BTreeMap::from([(record.name.clone(), record)]),
        }
    }
}

/// One reporting source's telemetry: the records it submitted, how much it has
/// submitted, and the totals recomputed from every record accepted.
#[derive(Debug, Clone, Default)]
pub struct TelemetryReporter {
    /// The source these records came from.
    pub source: TelemetrySource,
    /// The most recent [`MAX_TELEMETRY_RECORDS_PER_SOURCE`] records, in
    /// submission order, each stored exactly as submitted.
    pub records: Vec<TelemetryRecord>,
    /// How many records this source has had accepted, including any that have
    /// since fallen out of [`records`](Self::records).
    pub records_accepted: u64,
    /// How many `PutTelemetryRecords` calls this source has made, counting one
    /// that carried an empty batch.
    pub reports: u64,
    /// The sums of every accepted record's counters.
    pub totals: TelemetryTotals,
    /// When this source last reported, in [`ComAmazonawsXrayAwsxrayState::telemetry_seq`]
    /// ticks. Orders eviction once the telemetry store is full.
    pub last_report_seq: u64,
}

/// The running sums of a reporting source's telemetry counters: one field per
/// `NullableInteger` member of `TelemetryRecord` and of its
/// `BackendConnectionErrors`
/// (`smithy_query(query="closure",operation="PutTelemetryRecords")`).
///
/// Held as `i64` and added saturatingly, so no sequence of calls can overflow a
/// total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TelemetryTotals {
    /// Sum of `TelemetryRecord$SegmentsReceivedCount`.
    pub segments_received: i64,
    /// Sum of `TelemetryRecord$SegmentsSentCount`.
    pub segments_sent: i64,
    /// Sum of `TelemetryRecord$SegmentsSpilloverCount`.
    pub segments_spillover: i64,
    /// Sum of `TelemetryRecord$SegmentsRejectedCount`.
    pub segments_rejected: i64,
    /// Sum of `BackendConnectionErrors$TimeoutCount`.
    pub backend_timeouts: i64,
    /// Sum of `BackendConnectionErrors$ConnectionRefusedCount`.
    pub backend_connection_refused: i64,
    /// Sum of `BackendConnectionErrors$HTTPCode4XXCount`.
    pub backend_http_code_4xx: i64,
    /// Sum of `BackendConnectionErrors$HTTPCode5XXCount`.
    pub backend_http_code_5xx: i64,
    /// Sum of `BackendConnectionErrors$UnknownHostCount`.
    pub backend_unknown_host: i64,
    /// Sum of `BackendConnectionErrors$OtherCount`.
    pub backend_other: i64,
}

impl TelemetryTotals {
    /// Add one record's counters. An absent counter contributes nothing, which
    /// is what `NullableInteger` being optional means.
    fn add(&mut self, record: &TelemetryRecord) {
        let accumulate = |total: &mut i64, count: Option<i32>| {
            if let Some(count) = count {
                *total = total.saturating_add(i64::from(count));
            }
        };
        accumulate(&mut self.segments_received, record.segments_received_count);
        accumulate(&mut self.segments_sent, record.segments_sent_count);
        accumulate(
            &mut self.segments_spillover,
            record.segments_spillover_count,
        );
        accumulate(&mut self.segments_rejected, record.segments_rejected_count);
        if let Some(errors) = record.backend_connection_errors.as_ref() {
            accumulate(&mut self.backend_timeouts, errors.timeout_count);
            accumulate(
                &mut self.backend_connection_refused,
                errors.connection_refused_count,
            );
            accumulate(&mut self.backend_http_code_4xx, errors.http_code4_xx_count);
            accumulate(&mut self.backend_http_code_5xx, errors.http_code5_xx_count);
            accumulate(&mut self.backend_unknown_host, errors.unknown_host_count);
            accumulate(&mut self.backend_other, errors.other_count);
        }
    }
}

impl IndexingRuleStore {
    /// Every stored rule, in rule-name order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &IndexingRuleRecord> {
        self.rules.values()
    }

    /// The stored rules from `name` onward, in rule-name order — what a page
    /// resumed from a `NextToken` starts at.
    pub fn iter_from(&self, name: &str) -> impl Iterator<Item = &IndexingRuleRecord> {
        self.rules
            .range::<str, _>((
                ::std::ops::Bound::Included(name),
                ::std::ops::Bound::Unbounded,
            ))
            .map(|(_, rule)| rule)
    }

    /// Whether a rule of this exact name is stored.
    pub fn contains(&self, name: &str) -> bool {
        self.rules.contains_key(name)
    }

    /// Rewrite the named rule's probabilistic configuration and restamp its
    /// `ModifiedAt`, answering the stored record — or `None` when no rule of
    /// that name exists, which is what `ResourceNotFoundException` reports.
    ///
    /// The store neither grows nor shrinks here: an unknown name is refused
    /// rather than created, because X-Ray has no operation that adds a rule.
    pub fn update_probabilistic(
        &mut self,
        name: &str,
        desired_sampling_percentage: f64,
    ) -> Option<&IndexingRuleRecord> {
        let record = self.rules.get_mut(name)?;
        record.desired_sampling_percentage = desired_sampling_percentage;
        record.modified_at_secs = now_epoch_secs();
        Some(record)
    }
}

/// One indexing rule, as stored. `GetIndexingRules` and `UpdateIndexingRule`
/// both answer from this record, never from the request that wrote it.
#[derive(Debug, Clone)]
pub struct IndexingRuleRecord {
    /// The rule's name.
    pub name: String,
    /// When the rule was last modified, in whole seconds of epoch time —
    /// `IndexingRule$ModifiedAt` is documented "in Unix time seconds".
    pub modified_at_secs: i64,
    /// The configured `DesiredSamplingPercentage`.
    pub desired_sampling_percentage: f64,
}

impl IndexingRuleRecord {
    /// This record as the modelled `IndexingRule`.
    ///
    /// `ActualSamplingPercentage` is "applied sampling percentage of traceIds",
    /// which AWS may hold below the configured one "to ensure completeness of
    /// data". This emulator ingests through `PutTraceSegments` and applies no
    /// such limit, so the applied percentage IS the configured one.
    pub fn to_indexing_rule(&self) -> IndexingRule {
        IndexingRule {
            name: Some(self.name.clone()),
            modified_at: Some(::aws_smithy_types::DateTime::from_secs(
                self.modified_at_secs,
            )),
            rule: Some(IndexingRuleValue::Probabilistic(ProbabilisticRuleValue {
                desired_sampling_percentage: self.desired_sampling_percentage,
                actual_sampling_percentage: Some(self.desired_sampling_percentage),
            })),
        }
    }
}

/// The current instant in whole seconds of epoch time.
///
/// The crate keeps ONE clock: this is [`now_epoch_seconds`] truncated, for the
/// members X-Ray documents "in Unix time seconds".
pub fn now_epoch_secs() -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let seconds = now_epoch_seconds().trunc() as i64;
    seconds
}

/// One trace: every segment document submitted under a single `trace_id`.
#[derive(Debug, Clone, Default)]
pub struct TraceRecord {
    /// The trace's `trace_id`.
    pub id: String,
    /// Its segments, in first-submission order.
    pub segments: Vec<SegmentRecord>,
    /// Segment id -> its index in [`segments`](Self::segments), so ingesting a
    /// document is a lookup rather than a scan of everything stored so far.
    segment_index: BTreeMap<String, usize>,
    /// The earliest `start_time` across [`segments`](Self::segments).
    pub first_start: f64,
    /// The latest `end_time` across [`segments`](Self::segments), taking an
    /// in-progress segment's `start_time` as its latest known instant.
    pub last_end: f64,
    /// Set once a segment was refused because this trace reached
    /// [`MAX_SEGMENTS_PER_TRACE`]; reported as `Trace$LimitExceeded`.
    pub limit_exceeded: bool,
    /// When this trace was last ingested into, in [`ComAmazonawsXrayAwsxrayState::ingest_seq`]
    /// ticks. Orders eviction once the store is full.
    pub ingest_seq: u64,
    /// When a segment of this trace was last accepted, in seconds of epoch
    /// time: the trace's UPDATE time, which is the instant
    /// `GetTraceSummaries$TimeRangeType` `Event` windows a trace by. Written at
    /// ingest, so it is the service's own clock rather than anything a document
    /// carried.
    pub last_updated_at: f64,
}

/// One segment document, with the fields a reader needs lifted out of it and the
/// document itself kept byte-for-byte as submitted.
#[derive(Debug, Clone, Default)]
pub struct SegmentRecord {
    /// The segment's `id`.
    pub id: String,
    /// The segment's `parent_id`; `None` makes this a root segment.
    pub parent_id: Option<String>,
    /// The segment's `name` — the service that handled the request.
    pub name: String,
    /// The submitted document, verbatim. `BatchGetTraces` returns exactly this.
    pub document: String,
    /// `start_time`, in floating point seconds of epoch time.
    pub start_time: f64,
    /// `end_time`, absent while the segment is in progress.
    pub end_time: Option<f64>,
    /// `in_progress`: the segment was started but is not complete.
    pub in_progress: bool,
    /// `error`: the segment recorded a client-side (400 series) error.
    pub error: bool,
    /// `fault`: the segment recorded a server-side (500 series) fault.
    pub fault: bool,
    /// `throttle`: the segment recorded a throttling (429) error.
    pub throttle: bool,
    /// The segment's `http` block, when it carries one.
    pub http: Option<HttpRecord>,
    /// The segment's `annotations`, as submitted.
    pub annotations: BTreeMap<String, serde_json::Value>,
    /// The segment's `origin` — the resource type that ran the service.
    pub origin: Option<String>,
    /// The segment's `account_id`, when it names one.
    pub account_id: Option<String>,
    /// The segment's `user` — the authenticated user the request came from
    /// (`TraceSummary$Users`).
    pub user: Option<String>,
    /// The segment's `resource_arn` (`TraceSummary$ResourceARNs`).
    pub resource_arn: Option<String>,
    /// The document's `type`. `Some("subsegment")` marks a SUBSEGMENT sent as
    /// its own document: it records work inside its parent's service, so it
    /// reports no service of its own.
    pub segment_type: Option<String>,
    /// The document's `namespace`: `aws` for an AWS SDK call, `remote` for any
    /// other call that left the process, absent for in-process work.
    pub namespace: Option<String>,
    /// The exceptions the segment's `cause` block carries.
    pub exceptions: Vec<CauseException>,
    /// The `subsegments` this document carries inline, recursively — the
    /// entities inside it that a root cause walks — at most
    /// [`MAX_SUBSEGMENTS_PER_SEGMENT`] of them and at most
    /// [`MAX_SUBSEGMENT_DEPTH`] levels deep.
    pub subsegments: Vec<SubsegmentRecord>,
    /// Whether this segment was SYNTHESISED by the service for a downstream
    /// service a stored document only called (`namespace: "remote"`), rather than
    /// submitted. Its document carries `"inferred": true`.
    pub inferred: bool,
    /// The service type of the node this segment stands for when the service
    /// INFERRED it from a call out of another service
    /// ([`inferred_service_type`]); `None` for a segment a service submitted,
    /// whose type is the `origin` it carried. Read through
    /// [`SegmentRecord::service_type`].
    pub inferred_type: Option<String>,
}

/// One subsegment, as carried inline in its segment's `subsegments` array, with
/// the same fields a root cause and a filter expression read off a segment.
///
/// A subsegment records work done INSIDE the segment's service, so — unlike a
/// segment — it names a service of its own only when its `namespace` says the
/// work left the process.
#[derive(Debug, Clone, Default)]
pub struct SubsegmentRecord {
    /// The subsegment's `id`, when it carries one.
    pub id: Option<String>,
    /// The subsegment's `name` — the operation, or the downstream service it
    /// called.
    pub name: String,
    /// The subsegment's `namespace`: `aws`, `remote`, or absent.
    pub namespace: Option<String>,
    /// `start_time`, when the subsegment carries one.
    pub start_time: Option<f64>,
    /// `end_time`, absent while the subsegment is in progress.
    pub end_time: Option<f64>,
    /// `in_progress`: started but not complete.
    pub in_progress: bool,
    /// `error`: a client-side (400 series) error.
    pub error: bool,
    /// `fault`: a server-side (500 series) fault.
    pub fault: bool,
    /// `throttle`: a throttling (429) error.
    pub throttle: bool,
    /// The subsegment's `http` block, when it carries one.
    pub http: Option<HttpRecord>,
    /// The subsegment's `annotations`, as submitted.
    pub annotations: BTreeMap<String, serde_json::Value>,
    /// The exceptions this subsegment's `cause` block carries.
    pub exceptions: Vec<CauseException>,
    /// The subsegments nested under this one.
    pub subsegments: Vec<SubsegmentRecord>,
}

/// One exception from an entity's `cause.exceptions` — the `type` X-Ray reports
/// as `RootCauseException$Name`, and its `message`.
#[derive(Debug, Clone, Default)]
pub struct CauseException {
    /// The exception's `type`.
    pub name: Option<String>,
    /// The exception's `message`.
    pub message: Option<String>,
}

/// What one entity's outcome was. EXCLUSIVE: an entity is a fault, a throttle,
/// an error or none of the three, never two at once — X-Ray reclassifies rather
/// than unions, so a document carrying both `throttle` and `error` is a
/// throttle, and one carrying `fault` is a fault. A trace's
/// `HasFault`/`HasError`/`HasThrottle` are the union of its entities'
/// classifications, so a trace can still report two of them when two entities
/// were classified differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// Neither a fault, a throttle nor an error.
    Ok,
    /// A client-side (400 series) error that is not a throttle.
    Error,
    /// A throttling (429) error.
    Throttle,
    /// A server-side (500 series) fault.
    Fault,
}

/// Classify one entity from the flags its own document declared.
///
/// The outcome of an entity is the `fault` / `error` / `throttle` booleans its
/// document SET. A recorded `http.response.status` is data the document carries
/// — reported as `Http$HttpStatus` and selectable through the `http.status`
/// keyword — and not one of those flags, so a document that declares none of
/// them recorded none, whatever status it also recorded; the status-to-flag
/// convention belongs to the instrumentation that writes the document, not to
/// the service that reads it. This is where the three become EXCLUSIVE, in the
/// order X-Ray reclassifies them: a fault outranks a throttle, which outranks
/// an error.
fn classify(fault: bool, error: bool, throttle: bool) -> Classification {
    if fault {
        Classification::Fault
    } else if throttle {
        Classification::Throttle
    } else if recorded_error(error, throttle) {
        Classification::Error
    } else {
        Classification::Ok
    }
}

/// The parts of a segment's `http` block a `TraceSummary` reports.
#[derive(Debug, Clone, Default)]
pub struct HttpRecord {
    /// `http.request.client_ip`.
    pub client_ip: Option<String>,
    /// `http.request.method`.
    pub http_method: Option<String>,
    /// `http.request.url`.
    pub url: Option<String>,
    /// `http.request.user_agent`.
    pub user_agent: Option<String>,
    /// `http.response.status`.
    pub response_status: Option<i32>,
}

/// A stored group. `GetGroup` / `GetGroups` answer from this record, never from
/// the request that created it.
#[derive(Debug, Clone, Default)]
pub struct GroupRecord {
    /// The region this group lives in. Part of the record, not context around
    /// it, so [`ComAmazonawsXrayAwsxrayState::key`] can rebuild the exact key a
    /// lookup resolved and a write can never address another region's record.
    pub region: String,
    /// The group's case-sensitive name.
    pub group_name: String,
    /// The ARN minted when the group was created, and persisted since.
    pub group_arn: String,
    /// The group's filter expression, when it has one.
    pub filter_expression: Option<String>,
    /// The group's insights configuration, when it has one.
    pub insights_configuration: Option<InsightsConfigurationRecord>,
    /// The group's tags, keyed by tag key — so applying a key already present
    /// REPLACES its value rather than storing the key twice. Tags live here, on
    /// the resource record, rather than in a tag store beside it, so a group has
    /// exactly one set of tags whichever operation reads or writes them.
    pub tags: BTreeMap<String, String>,
}

/// A group's `InsightsConfiguration`, as stored.
#[derive(Debug, Clone, Default)]
pub struct InsightsConfigurationRecord {
    /// Whether insights are enabled for the group.
    pub insights_enabled: Option<bool>,
    /// Whether insight notifications are enabled for the group.
    pub notifications_enabled: Option<bool>,
}

/// Why a `(RuleName, RuleARN)` pair names no stored sampling rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingRuleLookupError {
    /// Neither selector was supplied.
    NoSelector,
    /// Both selectors were supplied: `SamplingRule$RuleName` and
    /// `SamplingRule$RuleARN` are documented as "either name or ARN, but not
    /// both".
    BothSelectors,
    /// A selector was supplied but no rule matches it.
    NotFound,
}

impl SamplingRuleLookupError {
    /// What to tell the caller. Lives here, beside
    /// [`ComAmazonawsXrayAwsxrayState::resolve_sampling_rule_name`], so every
    /// operation that selects a rule words one refusal the same way, and never
    /// renders an internal Rust type.
    pub fn message(self) -> &'static str {
        match self {
            Self::NoSelector => "either RuleName or RuleARN must be specified",
            Self::BothSelectors => {
                "specify a sampling rule by either RuleName or RuleARN, but not both"
            }
            Self::NotFound => SAMPLING_RULE_NOT_FOUND_MESSAGE,
        }
    }
}

/// What X-Ray answers when a selector names no sampling rule.
///
/// A FIXED wording that names neither selector, as the service's recorded
/// `DeleteSamplingRule` / `UpdateSamplingRule` refusals state it: the refusal
/// reports that there is no such rule, not what the request asked for.
pub const SAMPLING_RULE_NOT_FOUND_MESSAGE: &str = "Sampling rule does not exist";

/// What X-Ray answers when a created rule's `RuleName` is already in use — the
/// same fixed shape of wording, from its recorded `CreateSamplingRule`
/// refusals.
pub const SAMPLING_RULE_EXISTS_MESSAGE: &str = "Sampling rule already exists";

/// The `UnprocessedStatistics$Message` for a statistics document reported
/// against a sampling rule the account does not hold.
///
/// A DIFFERENT sentence from [`SAMPLING_RULE_NOT_FOUND_MESSAGE`], and
/// deliberately so: that one is the message of a whole-request refusal
/// (`DeleteSamplingRule`, `UpdateSamplingRule`), while this is the per-document
/// note inside a `GetSamplingTargets` 200 — a report the service declined to
/// process rather than a call it refused. FIXED text: the entry names the rule
/// in its own `RuleName` member, so nothing of the request reaches the sentence.
pub const UNKNOWN_SAMPLING_RULE_MESSAGE: &str = "Unknown rule";

/// What X-Ray answers when a `ResourceARN` handed to one of the tagging
/// operations names no resource this account holds.
///
/// The template states the resource the caller asked about and that it was not
/// found, with the ARN echoed VERBATIM — undecorated, so a client that parses
/// the ARN back out of the message reads the bytes it sent. Single-homed here so
/// `TagResource`, `UntagResource` and `ListTagsForResource` cannot drift into
/// three sentences for one error class.
#[must_use]
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} not found")
}

/// What identifies one group in the store: the region it lives in and its
/// case-sensitive `GroupName`.
///
/// An X-Ray group is a REGIONAL resource — its ARN carries the region, and the
/// same name in another region is another group — so the region is part of the
/// key, not context around it. Every lookup and every write goes through this
/// one key type, so a caller addressing a group in its own region can neither
/// read nor remove a record belonging to another.
pub type GroupKey = (String, String);

/// The name of the group every X-Ray account and region holds without anyone
/// creating it.
///
/// `CreateGroupRequest$GroupName` says so in the model
/// (`smithy_query(query="shape",shape="CreateGroupRequest")`): "Default is a
/// reserved name and names must be unique".
pub const DEFAULT_GROUP_NAME: &str = "Default";

/// `arn:aws:xray:<region>:<account>:group/<GroupName>/<12 hex digits>` — the ONE
/// place a group ARN's shape is written, so a minted ARN and the reserved
/// group's derived one cannot drift apart.
///
/// X-Ray's `GroupARN` is "generated based on the GroupName"; `seed`
/// distinguishes two ARNs carrying the same name, and is mixed with the name
/// into the 48-bit suffix (exactly 12 hexadecimal digits) by `std`'s default
/// hasher, whose algorithm `std` does not promise to keep stable across
/// releases — so the suffix is an opaque unique id rather than a value to depend
/// on. No clock enters the value.
///
/// Seed `0` is reserved for the derived [`DEFAULT_GROUP_NAME`] group and
/// [`ComAmazonawsXrayAwsxrayState::mint_group_arn`] only ever seeds from a
/// counter it has already advanced past zero, so no minted ARN can collide with
/// the reserved group's.
fn group_arn(region: &str, group_name: &str, seed: u64) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    group_name.hash(&mut hasher);
    seed.hash(&mut hasher);
    let suffix = hasher.finish() & 0xffff_ffff_ffff;
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("group/{group_name}/{suffix:012x}"),
    )
}

/// The resource an ARN of a group names — the prefix of the resource half of
/// every ARN [`group_arn`] mints, and so what tells a group's ARN from another
/// resource's.
const GROUP_ARN_RESOURCE: &str = "group/";

/// Whether `candidate` is of the form a group's ARN has: the ARN prefix, this
/// service's namespace, and a resource half naming a group.
///
/// What is judged is ARN SYNTAX, not existence: a value of this form may still
/// name no stored group ([`GroupLookupError::NotFound`]), while a value that is
/// not of it could never have named one and is a malformed parameter
/// ([`GroupLookupError::InvalidArn`]). The region and the account are not judged
/// — an ARN minted in another region is a well-formed ARN that resolves to
/// nothing here.
fn is_group_arn(candidate: &str) -> bool {
    let fields: Vec<&str> = candidate.splitn(6, ':').collect();
    matches!(fields.as_slice(), ["arn", _, ARN_NAMESPACE, _, _, resource] if resource.starts_with(GROUP_ARN_RESOURCE))
}

/// The reserved [`DEFAULT_GROUP_NAME`] group, as a caller in `region` reads it.
///
/// DERIVED, never stored: the group exists in every account and region without
/// being created, so there is no record to insert and no request can remove it.
/// Its ARN is a pure function of the reserved name and the region, which is what
/// keeps it stable across reads — two `GetGroups` calls answer the same ARN —
/// while a caller signing for another region reads that region's ARN.
pub fn default_group(region: &str) -> GroupRecord {
    GroupRecord {
        region: region.to_string(),
        group_name: DEFAULT_GROUP_NAME.to_string(),
        group_arn: group_arn(region, DEFAULT_GROUP_NAME, 0),
        // The reserved group selects every trace, so nothing is configured on
        // it: no filter expression and no insights configuration. Neither is
        // invented here.
        filter_expression: None,
        insights_configuration: None,
        tags: BTreeMap::new(),
    }
}

/// What a read whose `FilterExpression` spent its whole work budget
/// ([`filter::MAX_QUERY_STEPS`]) answers.
///
/// The request is well formed; what it would COST is the problem, so every
/// reader that evaluates an expression lifts this one sentence into its own
/// modelled THROTTLING error rather than into one that blames the input — and it
/// is an error rather than a short answer, because a derivation that stopped
/// halfway is indistinguishable from one that found nothing more.
pub const FILTER_BUDGET_SPENT_MESSAGE: &str = "the FilterExpression could not be evaluated over every trace in [StartTime, EndTime) within the work one request may do; narrow the time range or simplify the expression";

/// The refusal X-Ray answers when a request does not supply EXACTLY ONE group
/// selector — neither of them, or both at once.
///
/// A group is identified by exactly one of `GroupName` or `GroupARN`, so a
/// request carrying both states two identities at once and one carrying neither
/// names no group at all; X-Ray words both halves of that one rule the same way
/// ("Provide either GroupName or GroupARN") rather than servicing one selector
/// or widening the call to every group.
pub const CONFLICTING_GROUP_SELECTORS_MESSAGE: &str = "Provide either GroupName or GroupARN";

/// The refusal X-Ray answers for a `GroupARN` that is not of the form a group's
/// ARN has: fixed text naming the MEMBER, because the value cannot be a group's
/// ARN whatever groups exist. Judged before any lookup, so a malformed value is
/// never reported as a group that does not exist.
pub const INVALID_GROUP_ARN_MESSAGE: &str = "Invalid GroupARN";

/// The refusal X-Ray answers when a read that SCOPES a derivation to a group is
/// given a group that does not exist.
///
/// Entirely fixed text, the closing full stop included: it names neither
/// selector, so no value from the request — and nothing about how this emulator
/// represents a group — reaches the caller. Stated once here, because every read
/// that resolves a group to scope itself by answers the same sentence for the
/// same mistake: the derivations over a group's traffic, and the group a trace
/// query scopes itself to ([`group_scope`]). The group RESOURCE's own reads word
/// it differently ([`GroupLookupError::message`]), which is the service's own
/// distinction and not this emulator's.
pub const GROUP_NOT_FOUND_MESSAGE: &str = "Group not found.";

/// The reserved [`DEFAULT_GROUP_NAME`] group `resource_arn` names, when it names
/// that group's derived ARN.
///
/// The region comes out of the ARN itself (`arn:aws:xray:<region>:…`) and the
/// whole ARN is then re-derived and compared, so only the exact ARN this
/// emulator would answer for that region's reserved group matches.
fn default_group_for_arn(resource_arn: &str) -> Option<GroupRecord> {
    let region = resource_arn.split(':').nth(3)?;
    let reserved = default_group(region);
    (reserved.group_arn == resource_arn).then_some(reserved)
}

/// Why a `(GroupName, GroupARN)` pair names no single stored group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupLookupError {
    /// Neither selector was supplied.
    NoSelector,
    /// BOTH selectors were supplied, so the request states two identities.
    ConflictingSelectors,
    /// The `GroupARN` supplied is not of the form a group's ARN has
    /// ([`is_group_arn`]), so it could never have named one: a malformed
    /// parameter, judged before any lookup.
    InvalidArn,
    /// A selector was supplied but no group matches it.
    NotFound,
}

impl GroupLookupError {
    /// What to tell the caller. Lives here, beside
    /// [`ComAmazonawsXrayAwsxrayState::find_group`], so `GetGroup` and
    /// `DeleteGroup` cannot drift into two wordings of one refusal.
    ///
    /// A lookup that matched nothing is answered with the fixed text X-Ray uses
    /// on the group resource's own reads, `Group not found`: it names neither
    /// selector, so no value from the request — and nothing about how this
    /// emulator represents one — reaches the caller. A read that SCOPES a
    /// derivation to a group words the same mistake with a closing full stop
    /// ([`GROUP_NOT_FOUND_MESSAGE`]), which is the service's own distinction
    /// between the two surfaces.
    pub fn message(self) -> &'static str {
        match self {
            // One sentence for both halves of "exactly one selector": a request
            // naming neither is as unresolvable as one naming two, and X-Ray
            // states them identically (see
            // [`CONFLICTING_GROUP_SELECTORS_MESSAGE`]).
            Self::NoSelector | Self::ConflictingSelectors => CONFLICTING_GROUP_SELECTORS_MESSAGE,
            // A value that is not an ARN at all is a malformed parameter, so the
            // refusal names the MEMBER that carried it rather than the resource
            // the request was looking for.
            Self::InvalidArn => INVALID_GROUP_ARN_MESSAGE,
            Self::NotFound => "Group not found",
        }
    }
}

/// Why a tag a request carries cannot be applied to, or removed from, a
/// resource. Becomes the `InvalidRequestException` message of whichever tagging
/// operation carried it, so `CreateGroup`, `TagResource` and `UntagResource`
/// cannot drift into three readings of one documented restriction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRejection {
    /// The key carries [`RESERVED_TAG_KEY_PREFIX`].
    ReservedKeyPrefix {
        /// The refused key.
        key: String,
    },
    /// A key or a value carries a character outside the documented set.
    InvalidCharacter {
        /// `"key"` or `"value"` — which half of the tag is at fault.
        member: &'static str,
        /// The refused text.
        text: String,
        /// The first character of it that is not allowed.
        character: char,
    },
}

impl TagRejection {
    /// What to tell the caller: the member at fault and the restriction it broke.
    pub fn message(&self) -> String {
        match self {
            Self::ReservedKeyPrefix { key } => format!(
                "tag key `{key}` uses the `{RESERVED_TAG_KEY_PREFIX}` prefix, which is reserved \
                 for Amazon Web Services use"
            ),
            Self::InvalidCharacter {
                member,
                text,
                character,
            } => format!(
                "tag {member} `{text}` carries {character:?}, which is not one of the letters, \
                 numbers, space and `_ . : / = + - @` a tag {member} may use"
            ),
        }
    }
}

/// Whether `c` is a character a tag key or a tag value may carry.
///
/// `TagResource$Tags` publishes the set — "Valid values for key and value: a-z,
/// A-Z, 0-9, space, and the following characters: _ . : / = + - and @" — and the
/// `Tag` shape widens its alphanumeric half to "Unicode letters, numbers",
/// which is what [`char::is_alphanumeric`] answers.
fn is_tag_character(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ' ' | '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
}

/// Refuse a tag key a caller may not name.
///
/// The key's length bound (1..=128 characters) is modelled on `TagKey`, so the
/// request boundary refuses an empty or over-long key before any handler runs;
/// what is left is the character set and the reserved prefix.
pub fn validate_tag_key(key: &str) -> Result<(), TagRejection> {
    if let Some(character) = key.chars().find(|c| !is_tag_character(*c)) {
        return Err(TagRejection::InvalidCharacter {
            member: "key",
            text: key.to_string(),
            character,
        });
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Err(TagRejection::ReservedKeyPrefix {
            key: key.to_string(),
        });
    }
    Ok(())
}

/// Refuse a whole tag whose key or value is outside what the model documents.
pub fn validate_tag(key: &str, value: &str) -> Result<(), TagRejection> {
    validate_tag_key(key)?;
    if let Some(character) = value.chars().find(|c| !is_tag_character(*c)) {
        return Err(TagRejection::InvalidCharacter {
            member: "value",
            text: value.to_string(),
            character,
        });
    }
    Ok(())
}

/// Why a submitted segment document could not be stored. Becomes one
/// `UnprocessedTraceSegment` in the `PutTraceSegments` response.
#[derive(Debug, Clone)]
pub struct SegmentRejection {
    /// The rejected document's `id`, when it carried a usable one.
    pub id: Option<String>,
    /// [`SEGMENT_ERROR_PARSE`], [`SEGMENT_ERROR_LIMIT_EXCEEDED`], or a member
    /// code built by [`segment_member_code`].
    pub error_code: String,
    /// The code, restated: `Invalid segment. ErrorCode: <ErrorCode>`.
    pub message: String,
}

impl SegmentRejection {
    /// A rejection naming `error_code`, with the `Message` X-Ray states beside
    /// every per-document rejection — the code, restated.
    fn new(id: Option<&str>, error_code: String) -> Self {
        Self {
            id: id.map(str::to_string),
            message: format!("Invalid segment. ErrorCode: {error_code}"),
            error_code,
        }
    }

    /// A document that could not be read as a JSON segment object.
    fn unparseable() -> Self {
        Self::new(None, SEGMENT_ERROR_PARSE.to_string())
    }

    /// A document missing a documented required member.
    fn missing(id: Option<&str>, member: &str) -> Self {
        Self::new(id, segment_member_code("Missing", member))
    }

    /// A document whose member is present but unusable — the wrong JSON type, a
    /// value that is not of the form the member documents, or an instant no
    /// trace could have been recorded at.
    fn invalid(id: Option<&str>, member: &str) -> Self {
        Self::new(id, segment_member_code("Invalid", member))
    }

    /// A document refused because its trace is full.
    fn trace_full(id: &str) -> Self {
        Self::new(Some(id), SEGMENT_ERROR_LIMIT_EXCEEDED.to_string())
    }
}

/// A JSON object's member, read as a string.
type JsonObject = serde_json::Map<String, serde_json::Value>;

/// A required string field, distinguishing "absent" from "wrong type".
fn required_string(
    object: &JsonObject,
    member: &str,
    id: Option<&str>,
) -> Result<String, SegmentRejection> {
    match object.get(member) {
        None | Some(serde_json::Value::Null) => Err(SegmentRejection::missing(id, member)),
        Some(serde_json::Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(SegmentRejection::invalid(id, member)),
    }
}

/// A required epoch-seconds field, distinguishing "absent" from "wrong type"
/// and from an instant no trace could carry.
fn required_seconds(
    object: &JsonObject,
    member: &str,
    id: Option<&str>,
) -> Result<f64, SegmentRejection> {
    match object.get(member) {
        None | Some(serde_json::Value::Null) => Err(SegmentRejection::missing(id, member)),
        Some(serde_json::Value::Number(number)) => {
            let seconds = number
                .as_f64()
                .ok_or_else(|| SegmentRejection::invalid(id, member))?;
            epoch_instant(id, member, seconds)
        }
        Some(_) => Err(SegmentRejection::invalid(id, member)),
    }
}

/// One instant a segment document carries, refused when it is not a time a
/// trace could have been recorded at.
///
/// Checked HERE, where the document is read, so no unusable instant is ever
/// stored: every reader converts a stored instant to the modelled `Timestamp`,
/// and a value that is not finite, or is further from the epoch than
/// [`MAX_SEGMENT_EPOCH_SECONDS`], has no `Timestamp` to convert to. It is the
/// same per-document refusal as any other unusable member, so it takes the same
/// `Invalid<Member>` code.
fn epoch_instant(id: Option<&str>, member: &str, seconds: f64) -> Result<f64, SegmentRejection> {
    if !seconds.is_finite() || seconds.abs() > MAX_SEGMENT_EPOCH_SECONDS {
        return Err(SegmentRejection::invalid(id, member));
    }
    Ok(seconds)
}

/// An optional string field; a wrong-typed value is ignored rather than fatal,
/// because none of these fields is documented as required.
fn optional_string(object: &JsonObject, member: &str) -> Option<String> {
    object
        .get(member)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// An optional boolean flag, absent meaning `false`.
fn flag(object: &JsonObject, member: &str) -> bool {
    object
        .get(member)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Parse one submitted segment document into the trace it belongs to and the
/// record stored under it. `document` is kept verbatim on the record.
pub fn parse_segment_document(document: &str) -> Result<(String, SegmentRecord), SegmentRejection> {
    let value: serde_json::Value =
        serde_json::from_str(document).map_err(|_| SegmentRejection::unparseable())?;
    let object = value
        .as_object()
        .ok_or_else(SegmentRejection::unparseable)?;

    // The segment's own id first: a rejection reports it when the document
    // carried one, so a client can tell which document failed.
    let reported_id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let id_ref = reported_id.as_deref();

    // The four fields `PutTraceSegments` documents as required.
    let trace_id = required_string(object, "trace_id", id_ref)?;
    let id = required_string(object, "id", id_ref)?;
    let name = required_string(object, "name", id_ref)?;
    let start_time = required_seconds(object, "start_time", id_ref)?;

    // A `trace_id` that is not of the documented form names no trace X-Ray
    // could index, so the document is refused per-document — the rest of the
    // batch still lands — rather than stored under an id no reader can ask for.
    if !is_trace_id(&trace_id) {
        return Err(SegmentRejection::invalid(id_ref, "trace_id"));
    }

    // "Specify either an end_time or in_progress" — a document with neither is
    // treated as in progress rather than refused, so a partial trace is still
    // readable.
    let end_time = match object.get("end_time") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(number)) => match number.as_f64() {
            Some(seconds) => Some(epoch_instant(id_ref, "end_time", seconds)?),
            None => None,
        },
        Some(_) => return Err(SegmentRejection::invalid(id_ref, "end_time")),
    };
    let in_progress = flag(object, "in_progress") || end_time.is_none();

    let segment = SegmentRecord {
        id,
        parent_id: optional_string(object, "parent_id"),
        name,
        document: document.to_string(),
        start_time,
        end_time,
        in_progress,
        error: flag(object, "error"),
        fault: flag(object, "fault"),
        throttle: flag(object, "throttle"),
        http: parse_http(object.get("http")),
        annotations: parse_annotations(object),
        origin: optional_string(object, "origin"),
        account_id: optional_string(object, "account_id"),
        user: optional_string(object, "user"),
        resource_arn: optional_string(object, "resource_arn"),
        segment_type: optional_string(object, "type"),
        namespace: optional_string(object, "namespace"),
        exceptions: parse_cause(object.get("cause")),
        subsegments: parse_subsegments(id_ref, object.get("subsegments"), 1, &mut 0)?,
        // NOT read from the document: `inferred` and the type that goes with it
        // are service-assigned, and the segment schema `PutTraceSegments`
        // documents has no such member, so a submitted document cannot claim to
        // be a service the trace only called. [`inferred_segment`] is the one
        // place they are set; a submitted segment is typed by its `origin`.
        inferred: false,
        inferred_type: None,
    };
    Ok((trace_id, segment))
}

/// An entity's `annotations`, as submitted.
fn parse_annotations(object: &JsonObject) -> BTreeMap<String, serde_json::Value> {
    object
        .get("annotations")
        .and_then(serde_json::Value::as_object)
        .map(|annotations| {
            annotations
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The exceptions an entity's `cause` block carries. `cause` is either an object
/// with an `exceptions` array or a bare cause id, and only the former names
/// exceptions.
fn parse_cause(value: Option<&serde_json::Value>) -> Vec<CauseException> {
    let Some(exceptions) = value
        .and_then(serde_json::Value::as_object)
        .and_then(|cause| cause.get("exceptions"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    exceptions
        .iter()
        .filter_map(serde_json::Value::as_object)
        .map(|exception| CauseException {
            // X-Ray reports the exception's `type` as the root cause
            // exception's `Name` (`RootCauseException$Name`).
            name: optional_string(exception, "type"),
            message: optional_string(exception, "message"),
        })
        .collect()
}

/// An entity's `subsegments`, recursively, at `depth` levels below the segment.
///
/// Lenient about one subsegment where the document itself is strict: a
/// subsegment is part of an already accepted document, so an entry that is not
/// an object, or carries no usable `name`, is dropped rather than refusing the
/// whole document — only the segment's own members are documented as required.
/// The COUNT one document contributes is bounded by
/// [`MAX_SUBSEGMENTS_PER_SEGMENT`] (`budget`, shared across the whole nesting),
/// and a tree DEEPER than [`MAX_SUBSEGMENT_DEPTH`] is refused per document,
/// which is what bounds every later walk of it: the readers recurse once per
/// level, and a stack this deep is the one caller-driven cost a reader cannot
/// recover from.
fn parse_subsegments(
    id: Option<&str>,
    value: Option<&serde_json::Value>,
    depth: usize,
    budget: &mut usize,
) -> Result<Vec<SubsegmentRecord>, SegmentRejection> {
    let Some(array) = value.and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    if depth > MAX_SUBSEGMENT_DEPTH {
        return Err(SegmentRejection::invalid(id, "subsegments"));
    }
    let mut out = Vec::new();
    for object in array.iter().filter_map(serde_json::Value::as_object) {
        if *budget >= MAX_SUBSEGMENTS_PER_SEGMENT {
            break;
        }
        let Some(name) = optional_string(object, "name") else {
            continue;
        };
        *budget += 1;
        let end_time = object.get("end_time").and_then(serde_json::Value::as_f64);
        out.push(SubsegmentRecord {
            id: optional_string(object, "id"),
            name,
            namespace: optional_string(object, "namespace"),
            start_time: object.get("start_time").and_then(serde_json::Value::as_f64),
            end_time,
            in_progress: flag(object, "in_progress") || end_time.is_none(),
            error: flag(object, "error"),
            fault: flag(object, "fault"),
            throttle: flag(object, "throttle"),
            http: parse_http(object.get("http")),
            annotations: parse_annotations(object),
            exceptions: parse_cause(object.get("cause")),
            subsegments: parse_subsegments(id, object.get("subsegments"), depth + 1, budget)?,
        });
    }
    Ok(out)
}

/// Lift the `http.request` / `http.response` fields a `TraceSummary` reports.
fn parse_http(value: Option<&serde_json::Value>) -> Option<HttpRecord> {
    let http = value?.as_object()?;
    let request = http.get("request").and_then(serde_json::Value::as_object);
    let response = http.get("response").and_then(serde_json::Value::as_object);
    let request_field = |member: &str| request.and_then(|object| optional_string(object, member));
    Some(HttpRecord {
        client_ip: request_field("client_ip"),
        http_method: request_field("method"),
        url: request_field("url"),
        user_agent: request_field("user_agent"),
        response_status: response
            .and_then(|object| object.get("status"))
            .and_then(serde_json::Value::as_i64)
            .and_then(|status| i32::try_from(status).ok()),
    })
}

/// The `namespace` that makes a subsegment a call OUT of the service it sits in,
/// so the work it covers is a downstream service's.
pub const REMOTE_NAMESPACE: &str = "remote";

/// The `namespace` that makes a subsegment a call out through the AWS SDK, so
/// the work it covers is an AWS service's.
pub const AWS_NAMESPACE: &str = "aws";

/// The service type of a node X-Ray INFERRED from a call out of an instrumented
/// service, which the calling entity's `namespace` names, or `None` when the
/// entity names no downstream service at all.
///
/// `Service$Type` documents the vocabulary: [`SERVICE_TYPE_REMOTE`] is "a
/// downstream service of indeterminate type", which is what a
/// [`REMOTE_NAMESPACE`] entity records, and `AWS::<name>` is what
/// "downstream calls to Amazon Web Services ... that didn't target a specific
/// table" report, which is what an [`AWS_NAMESPACE`] entity records. An entity
/// with neither namespace recorded in-process work and infers no service.
///
/// This is the crate's ONE namespace-to-service-type derivation, so every
/// surface that reports the identity of an inferred service — a service graph's
/// nodes, a trace summary's `ServiceIds`, a root cause's services, a
/// `service(name, type)` filter expression — types that service the same way.
pub fn inferred_service_type(name: &str, namespace: Option<&str>) -> Option<String> {
    match namespace? {
        REMOTE_NAMESPACE => Some(SERVICE_TYPE_REMOTE.to_string()),
        AWS_NAMESPACE => Some(format!("AWS::{name}")),
        _ => None,
    }
}

/// The id of the segment synthesised for the remote subsegment `remote_id` of
/// `trace_id`: 16 hexadecimal digits, as a segment id is.
///
/// Service-assigned and derived only from what it identifies, so it is the same
/// on every read and distinct per remote subsegment — re-submitting the same
/// document replaces the inferred segment it already derived instead of adding a
/// second one. The mixing is [`service_common::stable_hex_id`], the one
/// deterministic id helper the service crates share: this id travels on the
/// wire, so it has to be reproducible across builds as well as across reads,
/// which `std`'s default hasher does not promise. It is still an opaque id, not
/// a value to depend on.
fn inferred_segment_id(trace_id: &str, remote_id: &str) -> String {
    ::service_common::stable_hex_id(&["inferred", trace_id, remote_id])
}

/// The segments X-Ray synthesises for the downstream services `segment` only
/// CALLED: one per subsegment in its tree whose `namespace` is
/// [`REMOTE_NAMESPACE`] and that carries an id to parent the synthesised segment
/// on.
///
/// Each is a top-level segment of the same trace whose `parent_id` is the remote
/// subsegment's id and whose document carries `"inferred": true`, so a reader
/// building a service map sees the downstream node the caller recorded. Its
/// times, `http` block, outcome flags and `cause` are the subsegment's: the
/// remote subsegment is the only record of what the downstream service did.
///
/// The tree is walked with an explicit worklist rather than by recursion, so the
/// nesting a submitted document carries costs heap and not call stack; the order
/// is the recursion's, a node then its children left to right.
pub fn inferred_segments(trace_id: &str, segment: &SegmentRecord) -> Vec<SegmentRecord> {
    let mut out = Vec::new();
    if segment.inferred {
        // A synthesised segment never derives further segments from itself. It
        // carries no subsegments either, so this is belt and braces: only
        // `inferred_segment` sets the flag, never a submitted document.
        return out;
    }
    let mut pending: Vec<&SubsegmentRecord> = segment.subsegments.iter().rev().collect();
    while let Some(subsegment) = pending.pop() {
        if let (Some(remote_id), true) = (
            subsegment.id.as_deref(),
            subsegment.namespace.as_deref() == Some(REMOTE_NAMESPACE),
        ) {
            out.push(inferred_segment(
                trace_id,
                remote_id,
                subsegment,
                segment.start_time,
            ));
        }
        pending.extend(subsegment.subsegments.iter().rev());
    }
    out
}

/// One inferred segment: the record, and the document a reader returns for it.
///
/// `timed_from` is the calling segment's own `start_time`, which is when a
/// remote subsegment that carries no `start_time` of its own is timed from: it
/// is the only instant known for it.
fn inferred_segment(
    trace_id: &str,
    remote_id: &str,
    remote: &SubsegmentRecord,
    timed_from: f64,
) -> SegmentRecord {
    let start_time = remote.start_time.unwrap_or(timed_from);
    let id = inferred_segment_id(trace_id, remote_id);
    let mut document = serde_json::Map::new();
    document.insert("id".to_string(), serde_json::json!(id));
    document.insert("name".to_string(), serde_json::json!(remote.name));
    document.insert("trace_id".to_string(), serde_json::json!(trace_id));
    document.insert("parent_id".to_string(), serde_json::json!(remote_id));
    document.insert("start_time".to_string(), serde_json::json!(start_time));
    match remote.end_time {
        Some(end_time) => {
            document.insert("end_time".to_string(), serde_json::json!(end_time));
        }
        None => {
            document.insert("in_progress".to_string(), serde_json::json!(true));
        }
    }
    document.insert("inferred".to_string(), serde_json::json!(true));
    for (member, set) in [
        ("fault", remote.fault),
        ("error", remote.error),
        ("throttle", remote.throttle),
    ] {
        if set {
            document.insert(member.to_string(), serde_json::json!(true));
        }
    }
    if let Some(http) = &remote.http {
        document.insert("http".to_string(), http_document(http));
    }
    if !remote.exceptions.is_empty() {
        document.insert("cause".to_string(), cause_document(&remote.exceptions));
    }

    SegmentRecord {
        id,
        parent_id: Some(remote_id.to_string()),
        name: remote.name.clone(),
        document: serde_json::Value::Object(document).to_string(),
        start_time,
        end_time: remote.end_time,
        in_progress: remote.in_progress,
        error: remote.error,
        fault: remote.fault,
        throttle: remote.throttle,
        http: remote.http.clone(),
        annotations: BTreeMap::new(),
        origin: None,
        account_id: None,
        user: None,
        resource_arn: None,
        // An inferred segment stands for a downstream SERVICE, not for work
        // inside one, so it is no `type: "subsegment"` document; and it carries
        // no `namespace` of its own — the namespace that produced it was the
        // remote subsegment's.
        segment_type: None,
        namespace: None,
        exceptions: remote.exceptions.clone(),
        subsegments: Vec::new(),
        inferred: true,
        // The node this segment stands for reported nothing itself, so it has no
        // `origin`: what types it is the call that inferred it.
        inferred_type: inferred_service_type(&remote.name, remote.namespace.as_deref()),
    }
}

/// An `http` block, as a segment document carries it.
fn http_document(http: &HttpRecord) -> serde_json::Value {
    let mut request = serde_json::Map::new();
    for (member, value) in [
        ("client_ip", &http.client_ip),
        ("method", &http.http_method),
        ("url", &http.url),
        ("user_agent", &http.user_agent),
    ] {
        if let Some(value) = value {
            request.insert(member.to_string(), serde_json::json!(value));
        }
    }
    let mut block = serde_json::Map::new();
    if !request.is_empty() {
        block.insert("request".to_string(), serde_json::Value::Object(request));
    }
    if let Some(status) = http.response_status {
        block.insert(
            "response".to_string(),
            serde_json::json!({ "status": status }),
        );
    }
    serde_json::Value::Object(block)
}

/// A `cause` block, as a segment document carries it.
fn cause_document(exceptions: &[CauseException]) -> serde_json::Value {
    let exceptions: Vec<serde_json::Value> = exceptions
        .iter()
        .map(|exception| {
            let mut entry = serde_json::Map::new();
            if let Some(name) = &exception.name {
                entry.insert("type".to_string(), serde_json::json!(name));
            }
            if let Some(message) = &exception.message {
                entry.insert("message".to_string(), serde_json::json!(message));
            }
            serde_json::Value::Object(entry)
        })
        .collect();
    serde_json::json!({ "exceptions": exceptions })
}

/// Whether an entity classified `class` can ANCHOR a root cause of that class:
/// it recorded the outcome, and its own `cause` block named at least one
/// exception to attribute the outcome to.
///
/// A fault or error root cause is the attribution of an outcome to an exception
/// a `cause` block named ([`RootCauseException`](CauseException) is "the
/// exception associated with a root cause"), so an entity that declared a flag
/// and submitted no exception gives the service nothing to attribute and
/// anchors no root cause. This is the ONE place that condition is written, so
/// every reader of a derived root cause — a trace summary's lists and the
/// filter language's `rootcause.*` keywords — agrees on which entities have one.
fn anchors_root_cause(
    class: Classification,
    recorded: Classification,
    exceptions: &[CauseException],
) -> bool {
    recorded == class && !exceptions.is_empty()
}

/// One entity's recorded outcome.
///
/// Two facts, because the model reports two: the EXCLUSIVE class the counters
/// split a call into ([`Classification`]), and whether the entity recorded a
/// 400-series error at all. They disagree exactly where X-Ray does: a throttled
/// request is counted under `ErrorStatistics$ThrottleCount` rather than
/// `$OtherCount`, and a document that declared both a throttle and an error is a
/// throttle alone.
#[derive(Debug, Clone, Copy)]
pub struct Outcome {
    /// The one class this entity is counted as.
    pub class: Classification,
    /// Whether it recorded a 400-series error ([`recorded_error`]).
    pub error_400: bool,
}

impl SubsegmentRecord {
    /// The `http.response.status` this subsegment recorded, if any.
    pub fn http_status(&self) -> Option<i32> {
        self.http.as_ref().and_then(|http| http.response_status)
    }

    /// This subsegment's outcome, from the flags it declared.
    pub fn classification(&self) -> Classification {
        classify(self.fault, self.error, self.throttle)
    }

    /// Whether this subsegment recorded a 400-series error.
    pub fn is_error(&self) -> bool {
        recorded_error(self.error, self.throttle)
    }

    /// This subsegment's recorded outcome: its exclusive class, and whether it
    /// recorded a 400-series error.
    pub fn outcome(&self) -> Outcome {
        Outcome {
            class: self.classification(),
            error_400: self.is_error(),
        }
    }

    /// Whether this subsegment declared a 500-series fault.
    pub fn is_fault(&self) -> bool {
        self.fault
    }

    /// Whether this subsegment declared a throttling error.
    pub fn is_throttle(&self) -> bool {
        self.throttle
    }

    /// Whether this subsegment is a call OUT of the service it sits in.
    pub fn is_remote(&self) -> bool {
        self.namespace.as_deref() == Some(REMOTE_NAMESPACE)
    }

    /// How long this subsegment ran, in seconds; `None` unless it recorded both
    /// of the instants a duration needs.
    pub fn duration(&self) -> Option<f64> {
        let span = self.end_time? - self.start_time?;
        (span.is_finite() && span >= 0.0).then_some(span)
    }

    /// This subsegment as one frame of a root cause's entity path.
    fn entity(&self) -> RootCauseEntityView {
        RootCauseEntityView {
            name: self.name.clone(),
            exceptions: self.exceptions.clone(),
            remote: self.is_remote(),
            coverage: None,
        }
    }

    /// This subsegment as one frame of a root cause's entity path, covering
    /// `coverage` of the trace's response time.
    fn entity_covering(&self, coverage: Option<f64>) -> RootCauseEntityView {
        RootCauseEntityView {
            coverage,
            ..self.entity()
        }
    }

    /// Every entity in this subsegment's tree, itself first.
    ///
    /// Recurses once per level of nesting, which the ingest bounds at
    /// [`MAX_SUBSEGMENT_DEPTH`] ([`parse_subsegments`]). `depth` re-states that
    /// bound HERE, so the walk's stack is bounded by this module rather than by
    /// what a request managed to store: past the bound the walk stops instead of
    /// recursing, which no stored tree can reach.
    fn walk<'a>(&'a self, depth: usize, out: &mut Vec<&'a Self>) {
        out.push(self);
        if depth >= MAX_SUBSEGMENT_DEPTH {
            return;
        }
        for child in &self.subsegments {
            child.walk(depth + 1, out);
        }
    }

    /// The entity path from this subsegment down to the deepest entity inside it
    /// that ANCHORS a root cause of `class` ([`anchors_root_cause`]), or `None`
    /// when its tree holds none.
    ///
    /// A REMOTE subsegment is not walked: what happened on the other side of a
    /// call out belongs to the downstream service, which is the inferred segment
    /// synthesised for it ([`inferred_segments`]) and carries its own root cause.
    ///
    /// `depth` bounds the recursion at [`MAX_SUBSEGMENT_DEPTH`], the same bound
    /// the ingest refuses a deeper document with.
    fn root_cause_path(
        &self,
        class: Classification,
        depth: usize,
    ) -> Option<Vec<RootCauseEntityView>> {
        if self.is_remote() {
            return None;
        }
        if depth < MAX_SUBSEGMENT_DEPTH {
            for child in &self.subsegments {
                if let Some(mut path) = child.root_cause_path(class, depth + 1) {
                    path.insert(0, self.entity());
                    return Some(path);
                }
            }
        }
        anchors_root_cause(class, self.classification(), &self.exceptions)
            .then(|| vec![self.entity()])
    }
}

/// Whether a recorded outcome is a 400-series error: the `error` flag the
/// document set, unless the same document also declared a `throttle`.
///
/// X-Ray reclassifies a declared throttle rather than unioning it, so a document
/// whose flags say `throttle: true, error: true` comes back as a throttle and
/// NOT as an error. Nothing else is consulted: `TraceSummary$HasError` reports
/// what the documents declared, not what their recorded statuses imply.
fn recorded_error(error: bool, throttle: bool) -> bool {
    error && !throttle
}

impl SegmentRecord {
    /// The `http.response.status` this segment recorded, if any.
    pub fn http_status(&self) -> Option<i32> {
        self.http.as_ref().and_then(|http| http.response_status)
    }

    /// This segment's own outcome, from the flags it declared.
    pub fn classification(&self) -> Classification {
        classify(self.fault, self.error, self.throttle)
    }

    /// How long this segment took, in seconds; `None` while it is in progress.
    pub fn duration(&self) -> Option<f64> {
        self.end_time.map(|end| end - self.start_time)
    }

    /// The latest instant this segment is known to have reached: its `end_time`
    /// when it has one, else its `start_time`.
    pub fn latest(&self) -> f64 {
        self.end_time.unwrap_or(self.start_time)
    }

    /// The outcomes recorded anywhere in this segment's document — the segment
    /// itself and every subsegment inside it.
    pub fn outcomes(&self) -> Vec<Outcome> {
        let mut entities = Vec::new();
        for subsegment in &self.subsegments {
            subsegment.walk(1, &mut entities);
        }
        let mut out = vec![self.outcome()];
        out.extend(entities.iter().map(|entity| entity.outcome()));
        out
    }

    /// Those outcomes' exclusive classes, which is what a counter reads.
    pub fn classifications(&self) -> Vec<Classification> {
        self.outcomes()
            .into_iter()
            .map(|outcome| outcome.class)
            .collect()
    }

    /// Whether this segment recorded a 400-series error — the `error` flag it
    /// declared, a throttle apart.
    pub fn is_error(&self) -> bool {
        recorded_error(self.error, self.throttle)
    }

    /// This segment's recorded outcome: its exclusive class, and whether it
    /// recorded a 400-series error.
    pub fn outcome(&self) -> Outcome {
        Outcome {
            class: self.classification(),
            error_400: self.is_error(),
        }
    }

    /// Whether this segment declared a 500-series fault — the `fault` flag it
    /// carried.
    pub fn is_fault(&self) -> bool {
        self.fault
    }

    /// Whether this segment declared a throttling error — the `throttle` flag it
    /// carried.
    pub fn is_throttle(&self) -> bool {
        self.throttle
    }

    /// Whether this document is a SUBSEGMENT sent on its own, which the X-Ray
    /// segment-document schema marks with `type: "subsegment"`.
    ///
    /// Such a document records work INSIDE its parent's service — a function
    /// call, a query, a request that left the process — so it is not a service
    /// of the graph in its own right.
    pub fn is_subsegment(&self) -> bool {
        self.segment_type.as_deref() == Some("subsegment")
    }

    /// This segment as one frame of a root cause's entity path. An inferred
    /// segment stands for a service this trace only called, so its frame is
    /// remote.
    fn entity(&self) -> RootCauseEntityView {
        RootCauseEntityView {
            name: self.name.clone(),
            exceptions: self.exceptions.clone(),
            remote: self.inferred,
            coverage: None,
        }
    }

    /// This segment as one frame of a root cause's entity path, covering
    /// `coverage` of the trace's response time.
    fn entity_covering(&self, coverage: Option<f64>) -> RootCauseEntityView {
        RootCauseEntityView {
            coverage,
            ..self.entity()
        }
    }

    /// The service type this segment's identity reports: the type the call that
    /// INFERRED the service gave it ([`inferred_service_type`]) when no service
    /// reported this segment, and otherwise the `origin` the reporting document
    /// carried.
    ///
    /// Every projection of a service identity reads this, so an inferred service
    /// is typed the same whichever surface reports it.
    pub fn service_type(&self) -> Option<String> {
        self.inferred_type
            .clone()
            .or_else(|| self.origin.clone())
    }

    /// This segment's service identity, as a root cause reports it.
    fn service(&self, entity_path: Vec<RootCauseEntityView>) -> RootCauseServiceView {
        RootCauseServiceView {
            name: self.name.clone(),
            r#type: self.service_type(),
            account_id: self.account_id.clone(),
            inferred: self.inferred,
            entity_path,
        }
    }

    /// The entity path from this segment down to the deepest entity inside it
    /// that ANCHORS a root cause of `class` ([`anchors_root_cause`]), or `None`
    /// when it holds none.
    ///
    /// The walk is bounded by [`MAX_SUBSEGMENT_DEPTH`], the depth the ingest
    /// refuses a deeper document with, so the stack it costs is this module's
    /// choice rather than the request's.
    fn root_cause_path(&self, class: Classification) -> Option<Vec<RootCauseEntityView>> {
        for child in &self.subsegments {
            if let Some(mut path) = child.root_cause_path(class, 1) {
                path.insert(0, self.entity());
                return Some(path);
            }
        }
        anchors_root_cause(class, self.classification(), &self.exceptions)
            .then(|| vec![self.entity()])
    }

    /// The frames from this segment down to the entity with id `entity_id`, when
    /// its tree holds one.
    ///
    /// Bounded by [`MAX_SUBSEGMENT_DEPTH`] the same way.
    fn frames_to(&self, entity_id: &str) -> Option<Vec<RootCauseEntityView>> {
        fn frames(
            subsegments: &[SubsegmentRecord],
            entity_id: &str,
            depth: usize,
        ) -> Option<Vec<RootCauseEntityView>> {
            if depth > MAX_SUBSEGMENT_DEPTH {
                return None;
            }
            for subsegment in subsegments {
                if subsegment.id.as_deref() == Some(entity_id) {
                    return Some(vec![subsegment.entity()]);
                }
                if let Some(mut path) = frames(&subsegment.subsegments, entity_id, depth + 1) {
                    path.insert(0, subsegment.entity());
                    return Some(path);
                }
            }
            None
        }

        if self.id == entity_id {
            return Some(vec![self.entity()]);
        }
        frames(&self.subsegments, entity_id, 1).map(|mut path| {
            path.insert(0, self.entity());
            path
        })
    }
}

/// One service of a root cause: the chain of them is the path through the
/// service graph the fault travelled (`FaultRootCause$Services`).
#[derive(Debug, Clone)]
pub struct RootCauseServiceView {
    /// The service's name.
    pub name: String,
    /// Its service type: the `origin` a reporting service carried, or the type
    /// the call that inferred the service gave it
    /// ([`SegmentRecord::service_type`]).
    pub r#type: Option<String>,
    /// Its `account_id`, when the document named one.
    pub account_id: Option<String>,
    /// Whether this service was inferred from a call to it rather than
    /// instrumented itself.
    pub inferred: bool,
    /// The frames inside this service the root cause ran through.
    pub entity_path: Vec<RootCauseEntityView>,
}

/// One frame of a root cause's entity path.
#[derive(Debug, Clone)]
pub struct RootCauseEntityView {
    /// The entity's name.
    pub name: String,
    /// The exceptions this entity's own `cause` block carried.
    pub exceptions: Vec<CauseException>,
    /// Whether this entity is a call out of its service (a dependency frame)
    /// rather than work inside it (a pipeline frame).
    pub remote: bool,
    /// How much of the trace's response time this frame ran for, as a fraction
    /// of it — `ResponseTimeRootCauseEntity$Coverage`, "the relative amount of
    /// time that the component was running". Only a response-time root cause
    /// reports it; the fault and error entity shapes carry no `Coverage` member,
    /// so their frames leave it unset.
    pub coverage: Option<f64>,
}

/// Which root causes a reader is asking for — the three lists X-Ray reports on a
/// `TraceSummary`. Two are read off the outcome a submitted document declared;
/// the third is read off the TIMING those documents recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootCauseKind {
    /// `TraceSummary$FaultRootCauses`.
    Fault,
    /// `TraceSummary$ErrorRootCauses`.
    Error,
    /// `TraceSummary$ResponseTimeRootCauses`.
    ResponseTime,
}

impl RootCauseKind {
    /// The entity classification this kind of root cause is built from, for the
    /// two kinds a declared outcome builds; `None` for the response-time kind,
    /// which is built from the recorded timing instead.
    pub fn classification(self) -> Option<Classification> {
        match self {
            Self::Fault => Some(Classification::Fault),
            Self::Error => Some(Classification::Error),
            Self::ResponseTime => None,
        }
    }
}

/// One root cause: which services and entities the outcome travelled through,
/// and whether the trace's client saw it.
#[derive(Debug, Clone)]
pub struct RootCauseView {
    /// Root first, the service that recorded the outcome last.
    pub services: Vec<RootCauseServiceView>,
    /// Whether the trace's own root segment recorded the same outcome, which is
    /// what makes a root cause client-impacting.
    pub client_impacting: bool,
}

impl TraceRecord {
    /// Widen the trace's bounds to include one newly appended segment. O(1), so
    /// an ingest never re-walks what is already stored.
    fn extend_bounds(&mut self, segment: &SegmentRecord) {
        let latest = segment.end_time.unwrap_or(segment.start_time);
        if self.segments.is_empty() {
            self.first_start = segment.start_time;
            self.last_end = latest;
        } else {
            self.first_start = self.first_start.min(segment.start_time);
            self.last_end = self.last_end.max(latest);
        }
    }

    /// Recompute the trace's time bounds from its segments.
    ///
    /// Only a REPLACED segment needs this — replacing can move a bound inwards,
    /// which [`extend_bounds`](Self::extend_bounds) cannot — and the walk is
    /// bounded by [`MAX_SEGMENTS_PER_TRACE`].
    fn recompute_bounds(&mut self) {
        self.first_start = self
            .segments
            .iter()
            .map(|segment| segment.start_time)
            .fold(f64::INFINITY, f64::min);
        self.last_end = self
            .segments
            .iter()
            .map(|segment| segment.end_time.unwrap_or(segment.start_time))
            .fold(f64::NEG_INFINITY, f64::max);
        if self.segments.is_empty() {
            self.first_start = 0.0;
            self.last_end = 0.0;
        }
    }

    /// "The length of time in seconds between the start time of the earliest
    /// segment that started and the end time of the last segment that
    /// completed" (`Trace$Duration` / `TraceSummary$Duration`).
    pub fn duration(&self) -> Option<f64> {
        (!self.segments.is_empty()).then_some(self.last_end - self.first_start)
    }

    /// The trace's root segment: the first one with no `parent_id`.
    pub fn root(&self) -> Option<&SegmentRecord> {
        self.segments
            .iter()
            .find(|segment| segment.parent_id.is_none())
    }

    /// Whether any stored segment of this trace STARTED inside the half-open
    /// window [`start`, `end`) — the selection every windowed derivation of this
    /// crate makes, segment by segment.
    ///
    /// Asked BEFORE a group's `FilterExpression` is evaluated over a trace, so a
    /// trace that cannot contribute to the answer costs none of the query budget
    /// ([`filter::QueryBudget`]) however large the store grows.
    pub fn touches_window(&self, start: f64, end: f64) -> bool {
        self.segments
            .iter()
            .any(|segment| segment.start_time >= start && segment.start_time < end)
    }

    /// The segment stored under `id`, in O(log n) through the id index rather
    /// than a scan \u2014 which is how a graph resolves a segment's `parent_id`.
    pub fn segment(&self, id: &str) -> Option<&SegmentRecord> {
        self.segment_index
            .get(id)
            .and_then(|at| self.segments.get(*at))
    }

    /// "The length of time in seconds between the start and end times of the
    /// root segment" (`TraceSummary$ResponseTime`).
    pub fn response_time(&self) -> Option<f64> {
        let root = self.root()?;
        Some(root.end_time? - root.start_time)
    }

    /// Whether any entity of this trace was classified `class`.
    ///
    /// An entity is a segment or a subsegment inside one, and each is classified
    /// EXCLUSIVELY ([`Classification`]), so a document carrying both `throttle`
    /// and `error` contributes a throttle and no error.
    pub fn has(&self, class: Classification) -> bool {
        self.segments
            .iter()
            .flat_map(SegmentRecord::classifications)
            .any(|recorded| recorded == class)
    }

    /// Whether the ROOT segment document recorded `class` — the root segment
    /// itself, or an entity inside the document that carried it.
    ///
    /// The scope matters, and it is the model's: `TraceSummary$HasFault` is
    /// documented as "The root segment document has a 500 series error" and
    /// `$HasError` as "…a 400 series error"
    /// (`smithy_query(query="doc",shape="TraceSummary",member="HasFault")`),
    /// while `$HasThrottle` is "One or more of the segment documents has a 429
    /// throttling error" — so the first two are read off the root document and
    /// only the third is a union over the trace.
    pub fn root_document_has(&self, class: Classification) -> bool {
        self.root()
            .is_some_and(|root| root.classifications().contains(&class))
    }

    /// Whether the root segment document recorded a 400-series error
    /// (`TraceSummary$HasError`): the root segment itself, or an entity inside
    /// its document.
    ///
    /// Read off each entity's [`Outcome::error_400`] rather than its exclusive
    /// class, because the two differ where a document declared an error inside a
    /// segment that is itself counted as something else ([`recorded_error`]).
    pub fn has_error(&self) -> bool {
        self.root().is_some_and(|root| {
            root.outcomes().iter().any(|outcome| outcome.error_400)
        })
    }

    /// Whether the root segment document recorded a 500-series fault
    /// (`TraceSummary$HasFault`).
    pub fn has_fault(&self) -> bool {
        self.root_document_has(Classification::Fault)
    }

    /// Whether ANY segment document recorded a throttling error
    /// (`TraceSummary$HasThrottle`).
    pub fn has_throttle(&self) -> bool {
        self.has(Classification::Throttle)
    }

    /// Whether any segment is still in progress (`TraceSummary$IsPartial`).
    pub fn is_partial(&self) -> bool {
        self.segments.iter().any(|segment| segment.in_progress)
    }

    /// The root causes of `kind` this trace recorded
    /// (`TraceSummary$FaultRootCauses` / `$ErrorRootCauses`), or the one the
    /// recorded timing explains ([`response_time_root_causes`](Self::response_time_root_causes))
    /// for `RootCauseKind::ResponseTime`.
    ///
    /// One per segment whose own tree holds an entity that ANCHORS a cause of
    /// that class — one that recorded the outcome AND named the exceptions it is
    /// attributed to ([`anchors_root_cause`]). Its `Services` are the chain of
    /// services from the trace's root down to that segment, each carrying the
    /// frames inside it the outcome ran through — the frame that called out (a
    /// dependency frame, `Remote`) or the work it did itself (a pipeline frame)
    /// — and the deepest frame carries the exceptions its own `cause` block
    /// named. A trace whose documents declared an outcome but submitted no
    /// exception to attribute it to reports NO root cause of that class: the
    /// lists are derived from what the documents carry, and there is nothing
    /// here to diagnose it with.
    ///
    /// Bounded in both dimensions a trace can grow in: at most
    /// [`MAX_ROOT_CAUSES_PER_TRACE`] causes are derived, and each chain walks
    /// each of the trace's segments at most once, resolving every hop through
    /// one entity index built here rather than by re-scanning the segments per
    /// hop. A trace that recorded more outcomes than the cap therefore reports
    /// the first of them rather than making one read do super-linear work.
    pub fn root_causes(&self, kind: RootCauseKind) -> Vec<RootCauseView> {
        let Some(class) = kind.classification() else {
            return self.response_time_root_causes();
        };
        let client_impacting = self
            .root()
            .is_some_and(|root| root.classification() == class);
        let owners = self.entity_owners();
        let mut causes = Vec::new();
        for (index, segment) in self.segments.iter().enumerate() {
            if causes.len() >= MAX_ROOT_CAUSES_PER_TRACE {
                break;
            }
            if let Some(path) = segment.root_cause_path(class) {
                causes.push(RootCauseView {
                    services: self.service_chain(index, path, &owners),
                    client_impacting,
                });
            }
        }
        causes
    }

    /// The response-time root cause of this trace
    /// (`TraceSummary$ResponseTimeRootCauses`), derived from the TIMING the
    /// stored documents recorded rather than from an outcome any of them
    /// declared.
    ///
    /// A trace's response time is its root segment's own span, and what explains
    /// that span is the chain of components the time was spent in: starting at
    /// the root segment, each step descends into the child entity that ran
    /// LONGEST — a subsegment of the entity in hand, or the segment another
    /// service reported under it — until no child recorded a span. That chain is
    /// the cause's `Services`, root first, each carrying the frames inside it the
    /// time ran through: the frame that called out of its service (`Remote`, a
    /// dependency frame) or the work it did itself (a pipeline frame), with each
    /// frame's `Coverage` the part of the response time it ran for.
    ///
    /// A cause is reported only when the chain reaches a component BELOW the
    /// root segment that ran for at least
    /// [`RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE`] of the response time: a trace
    /// whose time is not attributable to one component inside it has no
    /// response-time root cause, which is why most traces report none. An
    /// in-progress root segment has no response time yet, so it has none either.
    ///
    /// Bounded: the index of child segments is built once, the descent takes at
    /// most [`MAX_CRITICAL_PATH_FRAMES`] steps, and each step looks only at the
    /// children of the entity in hand.
    fn response_time_root_causes(&self) -> Vec<RootCauseView> {
        let Some(root_at) = self
            .segments
            .iter()
            .position(|segment| segment.parent_id.is_none() && !segment.inferred)
        else {
            return Vec::new();
        };
        let Some(response_time) = self.segments[root_at]
            .duration()
            .filter(|span| span.is_finite() && *span > 0.0)
        else {
            return Vec::new();
        };
        let covers = |span: Option<f64>| span.map(|span| (span / response_time).clamp(0.0, 1.0));

        // Which segments hang off which entity id, so a descent resolves the
        // next service by lookup instead of re-scanning the trace per step.
        let mut children_of: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (at, segment) in self.segments.iter().enumerate() {
            if let Some(parent_id) = segment.parent_id.as_deref() {
                children_of.entry(parent_id).or_default().push(at);
            }
        }

        // The chain, root first: which segment each frame belongs to, so the
        // frames can be grouped into one service per segment below.
        let mut frames: Vec<(usize, RootCauseEntityView)> = vec![(
            root_at,
            self.segments[root_at].entity_covering(covers(Some(response_time))),
        )];
        let mut at = root_at;
        let mut standing_on: Option<&SubsegmentRecord> = None;
        let mut leaf_coverage = 0.0_f64;
        for _ in 0..MAX_CRITICAL_PATH_FRAMES {
            let (entity_id, subsegments) = match standing_on {
                None => (
                    Some(self.segments[at].id.as_str()),
                    &self.segments[at].subsegments,
                ),
                Some(subsegment) => (subsegment.id.as_deref(), &subsegment.subsegments),
            };
            let longest_subsegment = subsegments
                .iter()
                .filter_map(|subsegment| Some((subsegment, subsegment.duration()?)))
                .max_by(|left, right| left.1.total_cmp(&right.1));
            let longest_segment = entity_id
                .and_then(|entity_id| children_of.get(entity_id))
                .into_iter()
                .flatten()
                .filter_map(|child| Some((*child, self.segments[*child].duration()?)))
                .max_by(|left, right| left.1.total_cmp(&right.1));
            let subsegment_span = longest_subsegment.map_or(f64::NEG_INFINITY, |(_, span)| span);
            let segment_span = longest_segment.map_or(f64::NEG_INFINITY, |(_, span)| span);
            if subsegment_span >= segment_span {
                let Some((subsegment, span)) = longest_subsegment else {
                    break;
                };
                leaf_coverage = covers(Some(span)).unwrap_or_default();
                frames.push((at, subsegment.entity_covering(covers(Some(span)))));
                standing_on = Some(subsegment);
            } else {
                let Some((child, span)) = longest_segment else {
                    break;
                };
                leaf_coverage = covers(Some(span)).unwrap_or_default();
                frames.push((child, self.segments[child].entity_covering(covers(Some(span)))));
                at = child;
                standing_on = None;
            }
        }

        if frames.len() < 2 || leaf_coverage < RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE {
            return Vec::new();
        }

        // One service per segment the chain passed through, in the order it
        // passed through them, carrying that segment's frames.
        let mut services: Vec<RootCauseServiceView> = Vec::new();
        let mut current: Option<(usize, Vec<RootCauseEntityView>)> = None;
        for (segment_at, frame) in frames {
            match &mut current {
                Some((holder, path)) if *holder == segment_at => path.push(frame),
                _ => {
                    if let Some((holder, path)) = current.take() {
                        services.push(self.segments[holder].service(path));
                    }
                    current = Some((segment_at, vec![frame]));
                }
            }
        }
        if let Some((holder, path)) = current {
            services.push(self.segments[holder].service(path));
        }

        vec![RootCauseView {
            services,
            // The chain explains the ROOT segment's own span, which is the time
            // the trace's client waited for, so the client saw it.
            client_impacting: true,
        }]
    }

    /// Every entity id in this trace — each segment's own, and each id in its
    /// `subsegments` tree — mapped to the segment that holds it.
    ///
    /// Built once per derivation, so resolving a `parent_id` to the service it
    /// belongs to is a lookup rather than a scan of every segment's tree. The
    /// filter language's `edge(…)` scope reads the same index for the same
    /// reason ([`filter`]).
    ///
    /// The tree walk is bounded by [`MAX_SUBSEGMENT_DEPTH`], the depth the
    /// ingest refuses a deeper document with, so no stored tree can cost this
    /// read more stack than this module allows.
    pub fn entity_owners(&self) -> BTreeMap<&str, usize> {
        fn walk<'a>(
            at: usize,
            subsegments: &'a [SubsegmentRecord],
            depth: usize,
            out: &mut BTreeMap<&'a str, usize>,
        ) {
            if depth > MAX_SUBSEGMENT_DEPTH {
                return;
            }
            for subsegment in subsegments {
                if let Some(id) = subsegment.id.as_deref() {
                    out.entry(id).or_insert(at);
                }
                walk(at, &subsegment.subsegments, depth + 1, out);
            }
        }

        let mut owners = BTreeMap::new();
        for (at, segment) in self.segments.iter().enumerate() {
            owners.entry(segment.id.as_str()).or_insert(at);
            walk(at, &segment.subsegments, 1, &mut owners);
        }
        owners
    }

    /// The services a root cause recorded at `index` travelled through, root
    /// first: each ancestor segment contributes the frames from itself down to
    /// the entity the next segment hangs off, and `path` is the frames inside the
    /// segment that recorded the outcome.
    fn service_chain(
        &self,
        index: usize,
        path: Vec<RootCauseEntityView>,
        owners: &BTreeMap<&str, usize>,
    ) -> Vec<RootCauseServiceView> {
        let mut chain = vec![self.segments[index].service(path)];
        let mut seen = BTreeSet::from([index]);
        let mut current = index;
        // One hop per segment at most: a hop that lands on a segment already
        // walked ends the chain, so a `parent_id` cycle cannot loop here, and
        // each segment's subsegment tree is walked at most once per chain.
        while let Some(parent_id) = self.segments[current].parent_id.as_deref() {
            let Some(&at) = owners.get(parent_id) else {
                break;
            };
            if !seen.insert(at) {
                break;
            }
            let Some(frames) = self.segments[at].frames_to(parent_id) else {
                break;
            };
            chain.push(self.segments[at].service(frames));
            current = at;
        }
        chain.reverse();
        chain
    }

    /// This trace with only the segments whose `start_time` falls in the
    /// INCLUSIVE interval `[start, end]`, with its id index and time bounds
    /// recomputed over what is left.
    ///
    /// The index is rebuilt rather than carried over, so a reader that resolves
    /// a `parent_id` through the trimmed trace — a derived graph does — can
    /// never reach a segment the window excluded.
    fn within(&self, start: f64, end: f64) -> Self {
        let segments: Vec<SegmentRecord> = self
            .segments
            .iter()
            .filter(|segment| segment.start_time >= start && segment.start_time <= end)
            .cloned()
            .collect();
        let segment_index = segments
            .iter()
            .enumerate()
            .map(|(at, segment)| (segment.id.clone(), at))
            .collect();
        let mut trimmed = Self {
            id: self.id.clone(),
            segments,
            segment_index,
            first_start: 0.0,
            last_end: 0.0,
            limit_exceeded: self.limit_exceeded,
            ingest_seq: self.ingest_seq,
            last_updated_at: self.last_updated_at,
        };
        trimmed.recompute_bounds();
        trimmed
    }
}

impl GroupRecord {
    /// This record as the modelled `Group`.
    pub fn to_group(&self) -> Group {
        Group {
            group_name: Some(self.group_name.clone()),
            group_arn: Some(self.group_arn.clone()),
            filter_expression: self.filter_expression.clone(),
            insights_configuration: self
                .insights_configuration
                .as_ref()
                .map(InsightsConfigurationRecord::to_configuration),
        }
    }

    /// This record as the modelled `GroupSummary`.
    pub fn to_summary(&self) -> GroupSummary {
        GroupSummary {
            group_name: Some(self.group_name.clone()),
            group_arn: Some(self.group_arn.clone()),
            filter_expression: self.filter_expression.clone(),
            insights_configuration: self
                .insights_configuration
                .as_ref()
                .map(InsightsConfigurationRecord::to_configuration),
        }
    }
}

impl InsightsConfigurationRecord {
    /// This record as the modelled `InsightsConfiguration`.
    pub fn to_configuration(&self) -> InsightsConfiguration {
        InsightsConfiguration {
            insights_enabled: self.insights_enabled,
            notifications_enabled: self.notifications_enabled,
        }
    }

    /// Store a submitted `InsightsConfiguration`.
    pub fn from_configuration(configuration: &InsightsConfiguration) -> Self {
        Self {
            insights_enabled: configuration.insights_enabled,
            notifications_enabled: configuration.notifications_enabled,
        }
    }
}

impl ComAmazonawsXrayAwsxrayState {
    /// Replace the encryption configuration and start its `UPDATING` span,
    /// returning the document to report — which is the stored one, as of `now`.
    pub fn put_encryption_config(
        &mut self,
        encryption_type: &str,
        key_id: Option<String>,
        now: Instant,
    ) -> EncryptionConfig {
        self.encryption = EncryptionConfigRecord {
            encryption_type: encryption_type.to_string(),
            key_id,
            active_due: Some(now + ENCRYPTION_SETTLE_AFTER),
        };
        self.encryption.to_config(now)
    }

    /// The stored encryption configuration as of `now`, materialising the
    /// `UPDATING` -> `ACTIVE` transition the clock already owed.
    pub fn encryption_config(&mut self, now: Instant) -> EncryptionConfig {
        self.encryption.settle(now);
        self.encryption.to_config(now)
    }

    /// Store one parsed segment under its trace, together with the segments
    /// X-Ray infers from it for the downstream services it called
    /// ([`inferred_segments`]).
    ///
    /// Bounded in both directions: a trace holds at most
    /// [`MAX_SEGMENTS_PER_TRACE`] segments (a further new one is refused, and
    /// marks the trace `limit_exceeded`), and the store holds at most
    /// [`MAX_TRACES`] traces (a new trace past that evicts the least recently
    /// ingested one).
    ///
    /// An inferred segment that does not fit marks the trace `limit_exceeded`
    /// like any other refused segment, but is not reported as an
    /// `UnprocessedTraceSegment`: the caller submitted no such document.
    pub fn ingest_segment(
        &mut self,
        trace_id: &str,
        segment: SegmentRecord,
    ) -> Result<(), SegmentRejection> {
        let inferred = inferred_segments(trace_id, &segment);
        self.store_segment(trace_id, segment)?;
        for extra in inferred {
            // The rejection is dropped deliberately: `store_segment` has
            // already recorded it on the trace as `limit_exceeded`.
            let _ = self.store_segment(trace_id, extra);
        }
        Ok(())
    }

    /// Store one segment under its trace, replacing a segment already held under
    /// the same id — which is how an in-progress segment is overwritten by the
    /// completed one, and how a re-submitted document replaces the inferred
    /// segment it already derived rather than adding a second one.
    fn store_segment(
        &mut self,
        trace_id: &str,
        segment: SegmentRecord,
    ) -> Result<(), SegmentRejection> {
        if !self.traces.contains_key(trace_id) && self.traces.len() >= MAX_TRACES {
            self.evict_least_recently_ingested_trace();
        }
        self.ingest_seq += 1;
        let seq = self.ingest_seq;
        let trace = self
            .traces
            .entry(trace_id.to_string())
            .or_insert_with(|| TraceRecord {
                id: trace_id.to_string(),
                ..TraceRecord::default()
            });
        trace.ingest_seq = seq;
        // The trace's update time: when THIS service accepted a document for
        // it, which is what the `Event` time range windows a trace by.
        trace.last_updated_at = now_epoch_seconds();
        match trace.segment_index.get(&segment.id).copied() {
            Some(at) => {
                trace.segments[at] = segment;
                trace.recompute_bounds();
            }
            None => {
                if trace.segments.len() >= MAX_SEGMENTS_PER_TRACE {
                    trace.limit_exceeded = true;
                    return Err(SegmentRejection::trace_full(&segment.id));
                }
                trace
                    .segment_index
                    .insert(segment.id.clone(), trace.segments.len());
                trace.extend_bounds(&segment);
                trace.segments.push(segment);
            }
        }
        Ok(())
    }

    /// Drop the trace that has gone longest without an ingest, to keep the store
    /// at [`MAX_TRACES`]. Walked only when the store is full.
    fn evict_least_recently_ingested_trace(&mut self) {
        let oldest = self
            .traces
            .iter()
            .min_by_key(|(_, trace)| trace.ingest_seq)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest {
            self.traces.remove(&id);
        }
    }

    /// Mint a group ARN for a newly created group, in the shape
    /// [`group_arn`] writes.
    ///
    /// The suffix is unique per minted group: it is seeded from the never-reset
    /// [`group_seq`](Self::group_seq), advanced before use, so two groups never
    /// share an ARN even when one replaces the other by name, and none can
    /// collide with the reserved group's seed of `0`.
    pub fn mint_group_arn(&mut self, region: &str, group_name: &str) -> String {
        self.group_seq += 1;
        group_arn(region, group_name, self.group_seq)
    }

    /// The [`GroupKey`] a `(region, GroupName)` pair makes. The ONE place a
    /// group key is built, so a lookup and the write that follows it cannot
    /// disagree about which record is addressed.
    fn key(region: &str, group_name: &str) -> GroupKey {
        (region.to_string(), group_name.to_string())
    }

    /// Insert a group and index its ARN, returning the stored record, or `None`
    /// when the store already holds [`MAX_GROUPS`] groups.
    ///
    /// Inserting over a name that already exists IN THAT REGION replaces that
    /// record, and drops its superseded ARN from the index in the same step — so
    /// the index never grows past `groups`, and a superseded ARN resolves to
    /// nothing rather than to whatever group later took the name. A group of the
    /// same name in another region is a different group and is untouched.
    pub fn insert_group(&mut self, region: &str, record: GroupRecordDraft) -> Option<&GroupRecord> {
        let key = Self::key(region, &record.group_name);
        match self.groups.remove(&key) {
            Some(superseded) => {
                self.group_arn_to_name.remove(&superseded.group_arn);
            }
            None if self.groups.len() >= MAX_GROUPS => return None,
            None => {}
        }
        let group_arn = self.mint_group_arn(region, &record.group_name);
        self.group_arn_to_name
            .insert(group_arn.clone(), key.clone());
        self.groups.insert(
            key.clone(),
            GroupRecord {
                region: region.to_string(),
                group_name: record.group_name,
                group_arn,
                filter_expression: record.filter_expression,
                insights_configuration: record.insights_configuration,
                tags: record.tags,
            },
        );
        Some(&self.groups[&key])
    }

    /// The group a `(GroupName, GroupARN)` pair selects, for a caller in
    /// `region`.
    ///
    /// Either selector resolves the same record, but a request supplies EXACTLY
    /// ONE: `GroupName` is "the case-sensitive name of the group" and `GroupARN`
    /// "the ARN that was generated upon creation"
    /// (`smithy_query(query="shape",shape="UpdateGroupRequest")`), two names for
    /// one identity with no operation that re-points either at another group.
    /// So a pair that supplies NEITHER names nothing
    /// ([`GroupLookupError::NoSelector`]) and a pair that supplies BOTH states
    /// two identities at once and is refused unresolved
    /// ([`GroupLookupError::ConflictingSelectors`]) — whether or not the two
    /// happen to agree — rather than one side being preferred silently.
    /// Otherwise the single selector resolves the record, by name or through the
    /// ARN index, and last the reserved [`DEFAULT_GROUP_NAME`] group, which is
    /// derived rather than stored (see [`default_group`]) and so is resolvable by
    /// name and by ARN without ever appearing in `groups`.
    ///
    /// Both selectors are scoped to `region`: a group lives in one region, so an
    /// ARN minted in another region resolves to nothing here, exactly as a name
    /// that was never used in this one does.
    pub fn find_group(
        &self,
        region: &str,
        group_name: Option<&str>,
        group_arn: Option<&str>,
    ) -> Result<GroupRecord, GroupLookupError> {
        if group_name.is_none() && group_arn.is_none() {
            return Err(GroupLookupError::NoSelector);
        }
        if group_name.is_some() && group_arn.is_some() {
            return Err(GroupLookupError::ConflictingSelectors);
        }
        // An ARN that is not of the form a group's ARN has could never have named
        // one, so it is a malformed parameter rather than a group that is
        // missing — judged before the stores are read, exactly as the
        // exactly-one-selector rule above is.
        if group_arn.is_some_and(|arn| !is_group_arn(arn)) {
            return Err(GroupLookupError::InvalidArn);
        }
        if let Some(record) = group_name
            .map(|name| Self::key(region, name))
            .and_then(|key| self.groups.get(&key))
        {
            return Ok(record.clone());
        }
        if let Some(record) = group_arn
            .and_then(|arn| self.group_arn_to_name.get(arn))
            .filter(|(arn_region, _)| arn_region == region)
            .and_then(|key| self.groups.get(key))
        {
            return Ok(record.clone());
        }
        let default = default_group(region);
        if group_name == Some(DEFAULT_GROUP_NAME) || group_arn == Some(default.group_arn.as_str()) {
            return Ok(default);
        }
        Err(GroupLookupError::NotFound)
    }

    /// The compiled `FilterExpression` a `(GroupName, GroupARN)` pair narrows a
    /// derived read by — the ONE resolution every read over a group makes, so
    /// `GetServiceGraph` and `GetTimeSeriesServiceStatistics` cannot drift into
    /// two readings of one rule.
    ///
    /// `Ok(None)` is "nothing narrows this read": the request named no group, or
    /// the group it named carries no expression (the reserved
    /// [`DEFAULT_GROUP_NAME`] group is the one such group, since `CreateGroup`
    /// requires an expression). `Ok(Some(expression))` selects a SUBSET of the
    /// stored traces, which is an answer over less data and never a refusal.
    ///
    /// `Err` carries the refusal message, which each caller lifts into its own
    /// modelled client-input error: a selector that resolves no single group, or
    /// an expression the evaluator cannot answer — refused naming it, because
    /// answering a filtered request with the UNFILTERED derivation would be a
    /// wrong answer rather than a missing one.
    ///
    /// A selector naming no group is worded [`GROUP_NOT_FOUND_MESSAGE`] here,
    /// which is how X-Ray words it on the reads that SCOPE a derivation to a
    /// group — a different sentence from the one the group resource's own reads
    /// answer ([`GroupLookupError::message`]), down to its closing full stop.
    pub fn group_filter(
        &self,
        region: &str,
        group_name: Option<&str>,
        group_arn: Option<&str>,
    ) -> Result<Option<filter::FilterExpression>, String> {
        if group_name.is_none() && group_arn.is_none() {
            return Ok(None);
        }
        let group = self
            .find_group(region, group_name, group_arn)
            .map_err(|failure| match failure {
                GroupLookupError::NotFound => GROUP_NOT_FOUND_MESSAGE.to_string(),
                other => other.message().to_string(),
            })?;
        let Some(source) = group
            .filter_expression
            .as_deref()
            .map(str::trim)
            .filter(|expression| !expression.is_empty())
        else {
            return Ok(None);
        };
        filter::parse(source).map(Some).map_err(|error| {
            format!(
                "the FilterExpression of group `{}` could not be evaluated: {}",
                group.group_name, error.message
            )
        })
    }

    /// Every group a caller in `region` sees, in `GroupName` order: the reserved
    /// [`DEFAULT_GROUP_NAME`] group plus every one stored IN THAT REGION.
    ///
    /// This is what `GetGroups` lists, so the reserved group is visible on a
    /// fresh emulator exactly as it is in a fresh account, and another region's
    /// groups are not. Bounded by [`MAX_GROUPS`] + 1.
    pub fn visible_groups(&self, region: &str) -> Vec<GroupRecord> {
        // `groups` is a `BTreeMap` keyed (region, name), so one region's records
        // are a contiguous run in `GroupName` order.
        let mut visible: Vec<GroupRecord> = self
            .groups
            .range(Self::key(region, "")..)
            .take_while(|((stored_region, _), _)| stored_region == region)
            .map(|(_, record)| record.clone())
            .collect();
        // The reserved group is stored only once an `UpdateGroup` has
        // materialised it, in which case it is already in `groups`; this guard
        // is what keeps the listing from carrying it twice.
        if !self.groups.contains_key(&Self::key(region, DEFAULT_GROUP_NAME)) {
            visible.push(default_group(region));
        }
        visible.sort_by(|left, right| left.group_name.cmp(&right.group_name));
        visible
    }

    /// Whether `group_name` already names a group in `region`, the reserved
    /// [`DEFAULT_GROUP_NAME`] one included — `CreateGroup`'s uniqueness check.
    pub fn name_is_taken(&self, region: &str, group_name: &str) -> bool {
        group_name == DEFAULT_GROUP_NAME
            || self.groups.contains_key(&Self::key(region, group_name))
    }

    /// The stored record for `group`, for an operation that MUTATES it.
    ///
    /// Addressed by the key `group` itself carries — the record a lookup
    /// resolved — never by a string from the request, so a write cannot reach a
    /// record the lookup did not resolve.
    ///
    /// The reserved [`DEFAULT_GROUP_NAME`] group is derived rather than stored
    /// ([`default_group`]), so the first write to it is what gives it a stored
    /// record — carrying the SAME derived ARN, so its ARN stays stable across
    /// that write and every selector that resolved it before still does. `None`
    /// means there is no record to write and no room to materialise one.
    fn group_mut(&mut self, group: GroupRecord) -> Option<&mut GroupRecord> {
        let key = Self::key(&group.region, &group.group_name);
        if !self.groups.contains_key(&key) {
            if group.group_name != DEFAULT_GROUP_NAME || self.groups.len() >= MAX_GROUPS {
                return None;
            }
            self.group_arn_to_name
                .insert(group.group_arn.clone(), key.clone());
            self.groups.insert(key.clone(), group);
        }
        self.groups.get_mut(&key)
    }

    /// Store one `PutTelemetryRecords` batch under the source that reported it.
    ///
    /// Appending, not replacing: a source's records accumulate across calls, and
    /// its [`TelemetryTotals`] are the sums over every record ever accepted from
    /// it. The call itself is counted too, so a batch that carried no records is
    /// still a report from a known source.
    ///
    /// Bounded in both directions: a source keeps at most
    /// [`MAX_TELEMETRY_RECORDS_PER_SOURCE`] records (the oldest fall out of the
    /// window first), and the store holds at most [`MAX_TELEMETRY_SOURCES`]
    /// sources (a report from a new source past that evicts the least recently
    /// reporting one).
    pub fn record_telemetry(&mut self, source: TelemetrySource, records: &[TelemetryRecord]) {
        if !self.telemetry.contains_key(&source) && self.telemetry.len() >= MAX_TELEMETRY_SOURCES {
            self.evict_least_recently_reporting_source();
        }
        self.telemetry_seq += 1;
        let seq = self.telemetry_seq;
        let reporter = self
            .telemetry
            .entry(source.clone())
            .or_insert_with(|| TelemetryReporter {
                source,
                ..TelemetryReporter::default()
            });
        reporter.last_report_seq = seq;
        reporter.reports += 1;
        for record in records {
            reporter.totals.add(record);
            reporter.records_accepted += 1;
            reporter.records.push(record.clone());
        }
        if reporter.records.len() > MAX_TELEMETRY_RECORDS_PER_SOURCE {
            let overflow = reporter.records.len() - MAX_TELEMETRY_RECORDS_PER_SOURCE;
            reporter.records.drain(..overflow);
        }
    }

    /// Drop the reporting source that has gone longest without a report, to keep
    /// the telemetry store at [`MAX_TELEMETRY_SOURCES`]. Walked only when the
    /// store is full.
    fn evict_least_recently_reporting_source(&mut self) {
        let oldest = self
            .telemetry
            .iter()
            .min_by_key(|(_, reporter)| reporter.last_report_seq)
            .map(|(source, _)| source.clone());
        if let Some(source) = oldest {
            self.telemetry.remove(&source);
        }
    }

    /// Apply an `UpdateGroup` to the group `group` names, returning the record
    /// as it now stands, or `None` when there is no record to write (see
    /// [`group_mut`](Self::group_mut)).
    ///
    /// Each member the update SUPPLIES replaces the stored one; each member it
    /// OMITS leaves the stored one as it is. The group's identity — its
    /// case-sensitive name and the ARN that "was generated upon creation"
    /// (`UpdateGroupRequest$GroupARN`) — and its tags are never touched by an
    /// update, so neither the keyed map nor the ARN index needs maintenance
    /// here and every selector that resolved the group before still does.
    pub fn apply_group_update(
        &mut self,
        group: GroupRecord,
        update: GroupUpdate,
    ) -> Option<&GroupRecord> {
        let record = self.group_mut(group)?;
        if let Some(filter_expression) = update.filter_expression {
            record.filter_expression = Some(filter_expression);
        }
        if let Some(insights_configuration) = update.insights_configuration {
            record.insights_configuration = Some(insights_configuration);
        }
        Some(record)
    }

    /// Remove the group `group` IS — the record a lookup resolved — and its ARN
    /// index entry.
    ///
    /// Takes the resolved record rather than a name from the request, and
    /// addresses it by the key that record carries, so the removal can only ever
    /// reach the record the lookup returned: a caller in one region cannot
    /// remove a same-named group in another. `None` means there was nothing
    /// under that key.
    pub fn remove_group(&mut self, group: &GroupRecord) -> Option<GroupRecord> {
        let record = self
            .groups
            .remove(&Self::key(&group.region, &group.group_name))?;
        self.group_arn_to_name.remove(&record.group_arn);
        Some(record)
    }

    /// The tags held by the resource `resource_arn` names, or `None` when it
    /// names no resource this store holds.
    ///
    /// X-Ray tags attach to a group or a sampling rule, so both branches are
    /// here: a group resolves through the ARN index (or, for the reserved
    /// [`DEFAULT_GROUP_NAME`] group, through the ARN it is read with), and a
    /// sampling rule through the name its ARN embeds
    /// ([`sampling_rule_name_in_arn`]), so the tags `CreateSamplingRule`
    /// validated and stored on a rule are the ones its ARN reports.
    ///
    /// No region argument: a group ARN carries the region it was minted in and a
    /// suffix unique to the group, so it names at most one record in the whole
    /// store — the ARN IS the key, and resolving one cannot reach a resource in
    /// another region by accident.
    ///
    /// Until something tags the reserved group there is no record to read from,
    /// and the group carries no tags, so the answer is the empty map.
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        /// The tags a resource that holds none reports. `static`, so the read
        /// path borrows it rather than materialising a record.
        static NO_TAGS: BTreeMap<String, String> = BTreeMap::new();

        if let Some(rule_name) = sampling_rule_name_in_arn(resource_arn) {
            // The service-managed rule exists whether or not a record has been
            // materialised for it ([`default_sampling_rule`]), and until
            // something tags it, it carries no tags.
            let derived_default = rule_name == DEFAULT_SAMPLING_RULE_NAME
                && !self.sampling_rules.contains_key(rule_name);
            if derived_default {
                return Some(&NO_TAGS);
            }
            return Some(&self.sampling_rules.get(rule_name)?.tags);
        }
        if let Some(key) = self.group_arn_to_name.get(resource_arn) {
            return Some(&self.groups.get(key)?.tags);
        }
        default_group_for_arn(resource_arn).map(|_| &NO_TAGS)
    }

    /// The same lookup, for a tagging operation that writes.
    ///
    /// Writing to the reserved group materialises its derived record first —
    /// with the ARN it is already read with — the same way `UpdateGroup` does,
    /// because a tag has to be stored somewhere to be read back.
    pub fn tags_for_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        if let Some(rule_name) = sampling_rule_name_in_arn(resource_arn) {
            let rule_name = rule_name.to_string();
            // Tagging the service-managed rule materialises its derived record
            // first — with the members and the ARN it is already read with —
            // the same way `UpdateSamplingRule` does, because a tag has to be
            // stored somewhere to be read back. The region is the one the ARN
            // itself names.
            let region = arn_fields(resource_arn).map_or_else(
                || DEFAULT_REGION.to_string(),
                |[_, _, _, region, _, _]| region.to_string(),
            );
            return Some(&mut self.sampling_rule_mut(&region, &rule_name)?.tags);
        }
        if let Some(key) = self.group_arn_to_name.get(resource_arn).cloned() {
            return Some(&mut self.groups.get_mut(&key)?.tags);
        }
        let reserved = default_group_for_arn(resource_arn)?;
        Some(&mut self.group_mut(reserved)?.tags)
    }

    /// Materialise the destination change the clock already owes at `now`.
    ///
    /// Called by the reporting operation as well as the mutating one: the move
    /// to `ACTIVE` was due at a fixed instant whether or not anybody called, so
    /// applying it on a read shows nothing a later caller would not have seen.
    pub fn settle_trace_segment_destination(&mut self, now: Instant) {
        if self
            .trace_segment_destination
            .active_at
            .is_some_and(|due| now >= due)
        {
            self.trace_segment_destination.active_at = None;
        }
    }

    /// Whether an accepted span is DELIVERED to CloudWatch Logs: the account's
    /// destination is [`CLOUD_WATCH_LOGS_DESTINATION`] and the switch to it is
    /// in effect.
    ///
    /// A switch still `PENDING` has not taken effect — the service is still
    /// sending where it was — so nothing is delivered until the clock makes it
    /// `ACTIVE` ([`Self::settle_trace_segment_destination`]), which is why the
    /// caller settles the record before asking.
    pub fn delivers_spans_to_cloud_watch_logs(&self) -> bool {
        self.trace_segment_destination.destination == CLOUD_WATCH_LOGS_DESTINATION
            && self.trace_segment_destination.status() == DESTINATION_STATUS_ACTIVE
    }

    /// Point the service at `destination`, and record `spans_log_group_kms_key_id`
    /// when one was supplied.
    ///
    /// A change becomes effective [`DESTINATION_ACTIVATION_SPAN`] after `now`, so
    /// it reports `PENDING` until then. A request that names the destination
    /// already configured changes nothing — there is no switch to apply — and so
    /// leaves the reported status as it stands. A destination other than
    /// [`CLOUD_WATCH_LOGS_DESTINATION`] takes no `CloudWatchLogsConfiguration`,
    /// so pointing the service there drops the stored key rather than keeping a
    /// key for a log group nothing writes to.
    pub fn set_trace_segment_destination(
        &mut self,
        destination: &str,
        spans_log_group_kms_key_id: Option<&str>,
        now: Instant,
    ) -> TraceSegmentDestinationRecord {
        let current = &mut self.trace_segment_destination;
        if current.destination != destination {
            current.destination = destination.to_string();
            current.active_at = Some(now + DESTINATION_ACTIVATION_SPAN);
        }
        if destination == CLOUD_WATCH_LOGS_DESTINATION {
            if let Some(key_id) = spans_log_group_kms_key_id {
                current.spans_log_group_kms_key_id = Some(key_id.to_string());
            }
        } else {
            current.spans_log_group_kms_key_id = None;
        }
        current.clone()
    }
}

/// The `RuleName` a sampling rule ARN embeds, when `value` is one this service
/// could have minted ([`sampling_rule_arn`]).
///
/// A tagging operation takes an ARN rather than a name, and the ARN is derived
/// from the region and the name, so reading the name back out of it is how a
/// sampling rule is resolved from one. The partition, service, account and
/// resource prefix all have to match.
///
/// The REGION the ARN names is not checked here, and that is deliberate: a
/// tagging operation has no other selector, so the ARN is the caller's whole
/// statement of which resource it means, and this one store serves every
/// regional endpoint. An operation that can also select by name
/// ([`ComAmazonawsXrayAwsxrayState::resolve_sampling_rule_name`], used by
/// `DeleteSamplingRule`) holds an ARN selector to the region of the endpoint it
/// was called on instead, which is the stricter reading of a request that names
/// a region explicitly.
pub fn sampling_rule_name_in_arn(value: &str) -> Option<&str> {
    let [_, partition, service, _region, account, resource] = arn_fields(value)?;
    if partition != "aws" || service != ARN_NAMESPACE || account != ::emulator_server::emulator_account_id() {
        return None;
    }
    let name = resource.strip_prefix("sampling-rule/")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// What `CreateGroup` supplies; the ARN is minted by [`ComAmazonawsXrayAwsxrayState::insert_group`].
#[derive(Debug, Clone, Default)]
pub struct GroupRecordDraft {
    /// The requested group name.
    pub group_name: String,
    /// The requested filter expression.
    pub filter_expression: Option<String>,
    /// The requested insights configuration.
    pub insights_configuration: Option<InsightsConfigurationRecord>,
    /// The requested tags, keyed by tag key.
    pub tags: BTreeMap<String, String>,
}

/// X-Ray's filter-expression language: the ONE place this crate reads it.
///
/// Every surface that takes a `FilterExpression` goes through [`parse`] — the
/// grammar, the catalog of fields X-Ray indexes, the bounds on an expression's
/// length and nesting, and the wording of every refusal that follows from them
/// live here and nowhere else. A surface that only STORES an expression
/// (`CreateGroup`, `UpdateGroup`) calls [`validate`], which is `parse` with the
/// tree thrown away; `GetTraceSummaries` hands the tree to
/// [`crate::state::filter`], which compiles it into what it can answer from the
/// stored segment documents and evaluates it. So an expression is parsed once,
/// by one parser, however it arrives.
///
/// The language is proprietary to X-Ray — no crates.io crate parses it — so the
/// grammar is written here as a tokenizer plus a recursive-descent parser. Both
/// the grammar and the keyword catalog come from AWS's "Using filter
/// expressions" reference for X-Ray; the model carries no field list.
pub mod filter_expression {
    /// The longest expression this parser accepts, in CHARACTERS — the unit the
    /// tokenizer walks, so the bound and the parsed unit agree whatever the
    /// encoding. `FilterExpression` carries no modelled length bound, so this is
    /// the emulator's own: it keeps one request from driving unbounded parsing.
    pub const MAX_LENGTH: usize = 4_096;

    /// The deepest nesting of `(…)` / `{…}` this parser accepts, for the same
    /// reason — a bound on caller-driven work, not a published limit.
    pub const MAX_DEPTH: usize = 32;

    /// The fields a filter expression may address, from AWS's X-Ray "Using
    /// filter expressions" keyword reference: the boolean request keywords
    /// (`ok`, `error`, `fault`, `throttle`, `partial`), the numeric ones
    /// (`duration`, `responsetime`, `http.status`, `coverage`), the string ones
    /// (the rest of `http.*`, `user`, `resource.arn`, `trace_id`), the complex
    /// keywords that take arguments (`service`, `edge`, `id`, `group`) and the
    /// members addressable inside one of their blocks (`name`, `type`,
    /// `message`, `account.id`, `service.*`, `edge.*`).
    ///
    /// A field NOT reachable from this catalog is not indexed by X-Ray, and an
    /// expression addressing one is refused — `metadata.<key>` is the ordinary
    /// case: a segment's metadata is stored but never indexed, so no filter can
    /// select on it.
    const KEYWORDS: &[&str] = &[
        "account.id",
        "coverage",
        "duration",
        "edge",
        "edge.destination",
        "edge.source",
        "error",
        "fault",
        "group",
        "http.clientip",
        "http.method",
        "http.status",
        "http.url",
        "http.useragent",
        "id",
        "message",
        "name",
        "ok",
        "partial",
        "resource.arn",
        "responsetime",
        "service",
        "service.account.id",
        "service.name",
        "service.type",
        "throttle",
        "trace_id",
        "type",
        "user",
    ];

    /// Keyword roots that take a caller-chosen suffix: an annotation key
    /// (`annotation.<key>`) and a root-cause path (`rootcause.<…>`).
    const KEYWORD_PREFIXES: &[&str] = &["annotation.", "rootcause."];

    /// The keyword whose caller-chosen key may also be written as a SUBSCRIPT —
    /// `annotation[<key>]` for `annotation.<key>` — which is the form AWS's
    /// filter-expression reference writes an annotation predicate in.
    const SUBSCRIPTED_KEYWORD: &str = "annotation";

    /// The comparison operators written as words, from the same reference's
    /// string operators. They sit beside `=`, `!=`, `<`, `<=`, `>` and `>=`.
    const WORD_COMPARISONS: &[(&str, Operator)] = &[
        ("contains", Operator::Contains),
        ("beginswith", Operator::BeginsWith),
        ("startswith", Operator::BeginsWith),
        ("endswith", Operator::EndsWith),
    ];

    /// The operator `word` names, when it names one of the word comparisons.
    fn word_comparison(word: &str) -> Option<Operator> {
        WORD_COMPARISONS
            .iter()
            .find(|(name, _)| word.eq_ignore_ascii_case(name))
            .map(|(_, operator)| *operator)
    }

    /// Whether `path` names a field a filter expression may address.
    #[must_use]
    pub fn is_indexed_field(path: &str) -> bool {
        KEYWORDS.contains(&path)
            || KEYWORD_PREFIXES
                .iter()
                .any(|prefix| path.len() > prefix.len() && path.starts_with(prefix))
    }

    /// Why a `FilterExpression` was refused.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        /// The expression was absent, empty or only whitespace. X-Ray treats an
        /// omitted `FilterExpression` and an empty one identically.
        Empty,
        /// The expression holds input this language does not admit, at the place
        /// the parse gave up: the line and the one-based column of that place,
        /// and the input read there.
        InvalidSymbol {
            /// The one-based line the parse gave up on.
            line: usize,
            /// The one-based column within that line.
            position: usize,
            /// The input read at that place and not admitted.
            symbol: String,
        },
        /// The expression broke a bound this emulator declares on how much
        /// parsing one request may drive ([`MAX_LENGTH`], [`MAX_DEPTH`]) —
        /// which is this emulator's limit rather than a refusal of the
        /// language, so it states the limit instead of a location in the input.
        Bound(String),
    }

    impl Error {
        /// What to tell the caller.
        ///
        /// A parse failure carries X-Ray's own template: the location the parse
        /// gave up at and the input symbol it could not read there, which is
        /// the actionable part of the refusal — it points at the characters that
        /// broke the expression. The empty case carries X-Ray's fixed wording,
        /// and a bound this emulator adds states that bound.
        #[must_use]
        pub fn message(&self) -> String {
            match self {
                Self::Empty => "Filter Expression is empty".to_string(),
                Self::InvalidSymbol {
                    line,
                    position,
                    symbol,
                } => format!(
                    "Error Location: line {line}, position {position} : Invalid input symbol: '{symbol}'"
                ),
                Self::Bound(detail) => format!("Invalid filter expression: {detail}"),
            }
        }
    }

    /// The refusal for input at character offset `at` of `expression` that this
    /// language does not admit: where that offset falls, and the `symbol` read
    /// there.
    fn invalid_symbol(expression: &[char], at: usize, symbol: String) -> Error {
        let before = expression.get(..at.min(expression.len())).unwrap_or_default();
        Error::InvalidSymbol {
            line: 1 + before.iter().filter(|character| **character == '\n').count(),
            position: before
                .iter()
                .rev()
                .take_while(|character| **character != '\n')
                .count()
                + 1,
            symbol,
        }
    }

    /// The punctuation a token may begin with, beside the two string quotes —
    /// the lexemes [`could_begin_a_token`] judges a run of input against.
    const PUNCTUATION: &[&str] = &[
        "\"", "'", "(", ")", "{", "}", "[", "]", ",", ":", "=", "!", "!=", "<", "<=", ">",
        ">=",
    ];

    /// The words that are not keyword paths: the boolean operators, the boolean
    /// literals, and the word comparisons [`WORD_COMPARISONS`] names.
    const WORD_TOKENS: &[&str] = &[
        "and", "or", "not", "true", "false", "contains", "beginswith", "startswith", "endswith",
    ];

    /// Whether `text` could still grow into a token of this language: some
    /// keyword, word token or punctuation mark begins with it, or it is a
    /// keyword root followed by a caller-chosen suffix (`annotation.<key>`).
    ///
    /// This is what decides how much input a refusal reports: a tokenizer reads
    /// as far as a token could still reach and names what it had read when the
    /// next character could no longer belong to one.
    fn could_begin_a_token(text: &str) -> bool {
        if text.is_empty() {
            return true;
        }
        let lowered = text.to_ascii_lowercase();
        KEYWORDS.iter().any(|keyword| keyword.starts_with(&lowered))
            || WORD_TOKENS.iter().any(|word| word.starts_with(&lowered))
            || KEYWORD_PREFIXES
                .iter()
                .any(|prefix| prefix.starts_with(&lowered) || lowered.starts_with(prefix))
            || PUNCTUATION.iter().any(|mark| mark.starts_with(text))
    }

    /// The input a tokenizer reports when it cannot read a token at `rest`: the
    /// longest run there that could still have become one
    /// ([`could_begin_a_token`]), plus the character that could not belong to
    /// it — which is what X-Ray reports for a word that begins a keyword and
    /// then leaves it (`met` for an expression addressing `metadata.<key>`, `si`
    /// for one beginning a word `si…`).
    ///
    /// A word that begins NO keyword of the language has no such run, and is
    /// reported by its first [`MAX_REPORTED_WORD_CHARACTERS`] characters, or by
    /// the whole word when it is shorter (`lat` for a word beginning `lat`),
    /// rather than by its first character alone. A character no token can begin
    /// with is reported on its own.
    fn unreadable_symbol(rest: &[char]) -> String {
        let mut read = 0;
        for take in 1..=rest.len() {
            let candidate: String = rest[..take].iter().collect();
            if !could_begin_a_token(&candidate) {
                break;
            }
            read = take;
        }
        if read == 0 {
            let word = rest
                .iter()
                .take_while(|character| is_word_character(**character))
                .count();
            if word > 0 {
                return rest[..word.min(MAX_REPORTED_WORD_CHARACTERS)]
                    .iter()
                    .collect();
            }
        }
        rest[..(read + 1).min(rest.len())].iter().collect()
    }

    /// How many characters of a word that begins no keyword a refusal reports.
    const MAX_REPORTED_WORD_CHARACTERS: usize = 3;

    /// The characters a keyword path is written with, which is also what decides
    /// where a word a refusal reports ends.
    fn is_word_character(character: char) -> bool {
        character.is_alphanumeric() || "_.-".contains(character)
    }

    /// One lexical item.
    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        /// A bare word: a keyword path, or a label inside a keyword's arguments.
        Field(String),
        /// A quoted string literal.
        Text(String),
        /// A numeric literal.
        Number(f64),
        /// `true` / `false`.
        Boolean(bool),
        And,
        Or,
        Not,
        /// One of `=`, `!=`, `<`, `<=`, `>`, `>=`, or one of the string
        /// operators the same reference lists (`CONTAINS`, `BEGINSWITH`,
        /// `STARTSWITH`, `ENDSWITH`). WHICH comparison it is does not change
        /// whether the expression parses, but it does change what the
        /// expression MEANS, so the operator travels on the token for whoever
        /// evaluates the parsed tree.
        Comparison(Operator),
        OpenParen,
        CloseParen,
        OpenBrace,
        CloseBrace,
        /// `[`, which opens a subscripted keyword's key.
        OpenBracket,
        /// `]`, which closes one.
        CloseBracket,
        Comma,
        Colon,
    }

    /// Which comparison a predicate makes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Operator {
        /// `=`
        Equal,
        /// `!=`
        NotEqual,
        /// `<`
        Less,
        /// `<=`
        LessOrEqual,
        /// `>`
        Greater,
        /// `>=`
        GreaterOrEqual,
        /// `CONTAINS`
        Contains,
        /// `BEGINSWITH` / `STARTSWITH` — the same operator under two names.
        BeginsWith,
        /// `ENDSWITH`
        EndsWith,
    }

    /// One value a comparison or an argument carries.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Operand {
        /// A quoted string.
        Text(String),
        /// A number.
        Number(f64),
        /// `true` / `false`.
        Boolean(bool),
        /// Another indexed field, as `id(name: …)`'s arguments may name.
        Field(String),
        /// An `id(…)` function naming ONE entity, which is the other way the
        /// complex keywords name theirs: AWS's filter-expression reference writes
        /// a service scope as `service("name")` and as
        /// `service(id(name: "name", type: "type"))`, and an edge scope as
        /// `edge("source", "destination")` and as `edge(id(…), id(…))`, so the
        /// two spellings of one scope parse to one argument list.
        Entity(EntityId),
    }

    /// One argument of a complex keyword, labelled as in `id(name: "svc")` or
    /// positional as in `service("svc", "AWS::EC2::Instance")`.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Argument {
        /// The member the argument names, when it was labelled.
        pub label: Option<String>,
        /// The value it carries.
        pub value: Operand,
    }

    /// A parsed expression.
    ///
    /// This is the ONE tree the crate parses this language into: `validate`
    /// answers from it, and `crate::state::filter` evaluates it. `AND` binds
    /// tighter than `OR`, and each associates left to right.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Node {
        /// `left AND right`.
        And(Box<Node>, Box<Node>),
        /// `left OR right`.
        Or(Box<Node>, Box<Node>),
        /// `NOT inner`, written either as the word or as `!`.
        Not(Box<Node>),
        /// `annotation[<key>]`: whether the field the subscript names is SET at
        /// all, whatever value it holds — the unary predicate form, which makes
        /// no comparison.
        Exists {
            /// The field's path, as the dotted form writes it.
            path: String,
        },
        /// `keyword [ ( arguments ) ] [ { block } | comparison operand ]`.
        Predicate {
            /// The keyword's path, which [`is_indexed_field`] accepted.
            path: String,
            /// Its arguments, when it is a complex keyword.
            arguments: Vec<Argument>,
            /// The expression its `{ … }` block scopes, when it carries one.
            block: Option<Box<Node>>,
            /// The comparison it makes, when it makes one.
            comparison: Option<(Operator, Operand)>,
        },
    }

    /// One lexical item and the span of the expression it occupies, in
    /// characters — which is what lets a refusal name the place the parse gave
    /// up and the input it had read there.
    #[derive(Debug, Clone, PartialEq)]
    struct Spanned {
        /// The item.
        token: Token,
        /// The character offset the item starts at.
        start: usize,
        /// The character offset just past the item.
        end: usize,
    }

    /// Split `expression` into tokens, refusing anything that is not one at the
    /// place it was read.
    fn tokenize(characters: &[char]) -> Result<Vec<Spanned>, Error> {
        let mut tokens: Vec<Spanned> = Vec::new();
        let mut at = 0;
        // Push one token, remembering the span it occupies. Called once the
        // walk has stepped past the item, so `at` is its end.
        macro_rules! push {
            ($start:expr, $token:expr) => {
                tokens.push(Spanned {
                    token: $token,
                    start: $start,
                    end: at,
                })
            };
        }
        while at < characters.len() {
            let character = characters[at];
            let start = at;
            match character {
                whitespace if whitespace.is_whitespace() => at += 1,
                '"' | '\'' => {
                    at += 1;
                    let mut text = String::new();
                    loop {
                        match characters.get(at) {
                            // A string the expression never closes runs to the
                            // end of the input, so that whole run is the input
                            // read and not admitted.
                            None => {
                                return Err(invalid_symbol(
                                    characters,
                                    start,
                                    characters[start..].iter().collect(),
                                ))
                            }
                            Some('\\') => {
                                let Some(escaped) = characters.get(at + 1) else {
                                    return Err(invalid_symbol(
                                        characters,
                                        start,
                                        characters[start..].iter().collect(),
                                    ));
                                };
                                text.push(*escaped);
                                at += 2;
                            }
                            Some(&closing) if closing == character => {
                                at += 1;
                                break;
                            }
                            Some(&other) => {
                                text.push(other);
                                at += 1;
                            }
                        }
                    }
                    push!(start, Token::Text(text));
                }
                '(' => {
                    at += 1;
                    push!(start, Token::OpenParen);
                }
                ')' => {
                    at += 1;
                    push!(start, Token::CloseParen);
                }
                '{' => {
                    at += 1;
                    push!(start, Token::OpenBrace);
                }
                '}' => {
                    at += 1;
                    push!(start, Token::CloseBrace);
                }
                '[' => {
                    at += 1;
                    push!(start, Token::OpenBracket);
                }
                ']' => {
                    at += 1;
                    push!(start, Token::CloseBracket);
                }
                ',' => {
                    at += 1;
                    push!(start, Token::Comma);
                }
                ':' => {
                    at += 1;
                    push!(start, Token::Colon);
                }
                '=' => {
                    at += 1;
                    push!(start, Token::Comparison(Operator::Equal));
                }
                // The boolean operators, written as the DOUBLED symbols: a
                // filter expression may join its terms with `&&` and `||`
                // exactly as with `AND` and `OR`, so an expression written that
                // way is read rather than refused. A single `&` or `|` joins
                // nothing and is not of the language.
                conjunction @ ('&' | '|') => {
                    if characters.get(at + 1) != Some(&conjunction) {
                        return Err(invalid_symbol(
                            characters,
                            start,
                            unreadable_symbol(&characters[start..]),
                        ));
                    }
                    at += 2;
                    push!(
                        start,
                        if conjunction == '&' {
                            Token::And
                        } else {
                            Token::Or
                        }
                    );
                }
                '<' | '>' => {
                    let inclusive = characters.get(at + 1) == Some(&'=');
                    at += usize::from(inclusive) + 1;
                    push!(
                        start,
                        Token::Comparison(match (character, inclusive) {
                            ('<', false) => Operator::Less,
                            ('<', true) => Operator::LessOrEqual,
                            (_, false) => Operator::Greater,
                            (_, true) => Operator::GreaterOrEqual,
                        })
                    );
                }
                // `!` is the language's NEGATION operator, the symbol spelling
                // of `NOT`, and `!=` is the inequality comparison: which of the
                // two was written is decided by the character after it, so the
                // longer lexeme wins and a bare `!` is read as the operator
                // rather than refused.
                '!' => {
                    if characters.get(at + 1) == Some(&'=') {
                        at += 2;
                        push!(start, Token::Comparison(Operator::NotEqual));
                    } else {
                        at += 1;
                        push!(start, Token::Not);
                    }
                }
                digit
                    if digit.is_ascii_digit()
                        || (digit == '-'
                            && characters.get(at + 1).is_some_and(char::is_ascii_digit)) =>
                {
                    at += 1;
                    while characters
                        .get(at)
                        .is_some_and(|next| next.is_ascii_digit() || *next == '.')
                    {
                        at += 1;
                    }
                    let literal: String = characters[start..at].iter().collect();
                    let Ok(number) = literal.parse::<f64>() else {
                        return Err(invalid_symbol(characters, start, literal));
                    };
                    push!(start, Token::Number(number));
                }
                word if word.is_alphabetic() || word == '_' => {
                    while characters
                        .get(at)
                        .is_some_and(|next| next.is_alphanumeric() || "_.-".contains(*next))
                    {
                        at += 1;
                    }
                    let word: String = characters[start..at].iter().collect();
                    let token = if word.eq_ignore_ascii_case("and") {
                        Token::And
                    } else if word.eq_ignore_ascii_case("or") {
                        Token::Or
                    } else if word.eq_ignore_ascii_case("not") {
                        Token::Not
                    } else if word.eq_ignore_ascii_case("true") {
                        Token::Boolean(true)
                    } else if word.eq_ignore_ascii_case("false") {
                        Token::Boolean(false)
                    } else if let Some(operator) = word_comparison(&word) {
                        Token::Comparison(operator)
                    } else {
                        Token::Field(word)
                    };
                    push!(start, token);
                }
                _ => {
                    return Err(invalid_symbol(
                        characters,
                        start,
                        unreadable_symbol(&characters[start..]),
                    ));
                }
            }
        }
        Ok(tokens)
    }

    /// The recursive-descent walk over the tokens.
    ///
    /// ```text
    /// expression := term (("AND" | "OR") term)*
    /// term       := ("NOT" | "!") term | "(" expression ")" | predicate | existence
    /// predicate  := keyword ["(" arguments ")"] ["{" expression "}" | comparison operand]
    /// existence  := "annotation" "[" key "]"
    /// arguments  := argument ("," argument)*
    /// argument   := [label ":"] operand | id
    /// operand    := text | number | boolean | keyword
    /// ```
    struct Parser<'a> {
        /// The expression's characters, so a refusal can name where in the
        /// input the walk gave up and what it was reading there.
        source: &'a [char],
        tokens: Vec<Spanned>,
        at: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<&Token> {
            self.tokens.get(self.at).map(|spanned| &spanned.token)
        }

        /// Where the item the walk is looking at begins — the end of the input
        /// when there is none left.
        fn peek_start(&self) -> usize {
            self.tokens
                .get(self.at)
                .map_or(self.source.len(), |spanned| spanned.start)
        }

        fn eat(&mut self, token: &Token) -> bool {
            if self.peek() == Some(token) {
                self.at += 1;
                return true;
            }
            false
        }

        /// The refusal for the item the walk cannot read: located where that
        /// item begins, and naming the item itself — the input symbol that
        /// could not stand there. When the expression ran OUT before the grammar
        /// did there is no symbol to name, so the refusal is located at the end
        /// of the input and names the whole expression.
        fn invalid(&self) -> Error {
            match self.tokens.get(self.at) {
                Some(spanned) => invalid_symbol(
                    self.source,
                    spanned.start,
                    self.source
                        .get(spanned.start..spanned.end)
                        .unwrap_or_default()
                        .iter()
                        .collect(),
                ),
                None => invalid_symbol(
                    self.source,
                    self.source.len(),
                    self.source.iter().collect(),
                ),
            }
        }

        /// The refusal for input at `start` that is not a token of this
        /// language at all, as the tokenizer states one: the run it could read
        /// there and the character that broke it.
        fn unreadable_at(&self, start: usize) -> Error {
            invalid_symbol(
                self.source,
                start,
                unreadable_symbol(self.source.get(start..).unwrap_or_default()),
            )
        }

        fn expect(&mut self, token: &Token) -> Result<(), Error> {
            if self.eat(token) {
                return Ok(());
            }
            Err(self.invalid())
        }

        /// ```text
        /// selector := "service" "(" id ")" | "edge" "(" id "," id ")" | id
        /// ```
        fn entity_selector(&mut self) -> Result<EntitySelector, Error> {
            let start = self.peek_start();
            let Some(Token::Field(function)) = self.peek().cloned() else {
                return Err(self.invalid());
            };
            match function.as_str() {
                ENTITY_ID_FUNCTION => Ok(EntitySelector::Service(self.entity_id()?)),
                "service" => {
                    self.at += 1;
                    self.expect(&Token::OpenParen)?;
                    let entity = self.entity_id()?;
                    self.expect(&Token::CloseParen)?;
                    Ok(EntitySelector::Service(entity))
                }
                "edge" => {
                    self.at += 1;
                    self.expect(&Token::OpenParen)?;
                    let source = self.entity_id()?;
                    self.expect(&Token::Comma)?;
                    let destination = self.entity_id()?;
                    self.expect(&Token::CloseParen)?;
                    Ok(EntitySelector::Edge {
                        source,
                        destination,
                    })
                }
                _ => Err(self.unreadable_at(start)),
            }
        }

        /// ```text
        /// id       := "id" "(" argument ("," argument)* ")"
        /// argument := label ":" text
        /// ```
        fn entity_id(&mut self) -> Result<EntityId, Error> {
            let start = self.peek_start();
            match self.peek() {
                Some(Token::Field(function)) if function == ENTITY_ID_FUNCTION => self.at += 1,
                Some(_) => return Err(self.unreadable_at(start)),
                None => return Err(self.invalid()),
            }
            self.expect(&Token::OpenParen)?;
            let mut entity = EntityId::default();
            loop {
                let label_at = self.peek_start();
                let Some(Token::Field(label)) = self.peek().cloned() else {
                    return Err(self.invalid());
                };
                self.at += 1;
                self.expect(&Token::Colon)?;
                let value_at = self.peek_start();
                let Operand::Text(value) = self.operand()? else {
                    return Err(self.unreadable_at(value_at));
                };
                match label.as_str() {
                    "name" => entity.name = Some(value),
                    "type" => entity.r#type = Some(value),
                    "account.id" => entity.account_id = Some(value),
                    _ => return Err(self.unreadable_at(label_at)),
                }
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::CloseParen)?;
            Ok(entity)
        }

        fn expression(&mut self, depth: usize) -> Result<Node, Error> {
            // The terms and the operators between them, folded below so `AND`
            // binds tighter than `OR` while the WALK stays exactly what it was:
            // which expressions parse does not depend on the fold.
            let mut terms = vec![self.term(depth)?];
            let mut operators: Vec<Token> = Vec::new();
            while let Some(operator @ (Token::And | Token::Or)) = self.peek().cloned() {
                self.at += 1;
                operators.push(operator);
                terms.push(self.term(depth)?);
            }
            Ok(fold(terms, &operators))
        }

        fn term(&mut self, depth: usize) -> Result<Node, Error> {
            if depth > MAX_DEPTH {
                return Err(Error::Bound(format!(
                    "nested more than {MAX_DEPTH} levels deep"
                )));
            }
            let start = self.peek_start();
            match self.peek().cloned() {
                Some(Token::Not) => {
                    self.at += 1;
                    Ok(Node::Not(Box::new(self.term(depth + 1)?)))
                }
                Some(Token::OpenParen) => {
                    self.at += 1;
                    let inner = self.expression(depth + 1)?;
                    self.expect(&Token::CloseParen)?;
                    Ok(inner)
                }
                Some(Token::Field(path)) => {
                    self.at += 1;
                    if path == SUBSCRIPTED_KEYWORD && self.peek() == Some(&Token::OpenBracket) {
                        return self.subscripted(path, start, depth);
                    }
                    self.predicate(path, start, depth)
                }
                _ => Err(self.invalid()),
            }
        }

        /// A keyword whose caller-chosen key is written as a SUBSCRIPT:
        /// `annotation[<key>]` names the same field as `annotation.<key>`, so
        /// the two forms are read as one path and whatever follows reads exactly
        /// as it does after the dotted form. The bare form — a subscript with no
        /// comparison after it — is the unary predicate: the field is SET.
        ///
        /// ```text
        /// existence := "annotation" "[" key "]"
        /// key       := word | text
        /// ```
        ///
        /// The key is the caller's own data rather than a name of this language,
        /// so it is read as whatever was WRITTEN between the brackets: the word
        /// there, whichever lexeme the tokenizer made of it — a key spelling
        /// `true`, `and` or `contains` is still that key — or, quoted, whatever
        /// the quotes hold, which is how a key no bare word can spell is written.
        fn subscripted(&mut self, root: String, start: usize, depth: usize) -> Result<Node, Error> {
            self.expect(&Token::OpenBracket)?;
            let Some(written) = self.tokens.get(self.at).cloned() else {
                return Err(self.invalid());
            };
            let key = match &written.token {
                Token::Text(text) => text.clone(),
                _ => {
                    let word: String = self
                        .source
                        .get(written.start..written.end)
                        .unwrap_or_default()
                        .iter()
                        .collect();
                    if !word.chars().all(is_word_character) {
                        return Err(self.invalid());
                    }
                    word
                }
            };
            self.at += 1;
            self.expect(&Token::CloseBracket)?;
            let path = format!("{root}.{key}");
            // A subscript that names NO key — an empty quoted one — addresses no
            // field, so it is refused where the keyword was read, exactly as a
            // word that names no field of the language is.
            if !is_indexed_field(&path) {
                return Err(self.unreadable_at(start));
            }
            if matches!(self.peek(), Some(Token::Comparison(_))) {
                return self.predicate(path, start, depth);
            }
            Ok(Node::Exists { path })
        }

        fn predicate(&mut self, path: String, start: usize, depth: usize) -> Result<Node, Error> {
            // A word that names no field of the language is not a keyword this
            // grammar has a token for at all, so it is reported where it was
            // read rather than as a field that merely is not indexed.
            if !is_indexed_field(&path) {
                return Err(self.unreadable_at(start));
            }
            let arguments = if self.eat(&Token::OpenParen) {
                self.arguments()?
            } else {
                Vec::new()
            };
            if self.eat(&Token::OpenBrace) {
                let block = self.expression(depth + 1)?;
                self.expect(&Token::CloseBrace)?;
                return Ok(Node::Predicate {
                    path,
                    arguments,
                    block: Some(Box::new(block)),
                    comparison: None,
                });
            }
            let comparison = match self.peek() {
                Some(Token::Comparison(operator)) => {
                    let operator = *operator;
                    self.at += 1;
                    Some((operator, self.operand()?))
                }
                _ => None,
            };
            Ok(Node::Predicate {
                path,
                arguments,
                block: None,
                comparison,
            })
        }

        fn arguments(&mut self) -> Result<Vec<Argument>, Error> {
            let mut arguments = Vec::new();
            loop {
                // An argument that is itself an `id(…)` function, as in
                // `service(id(name: "svc", type: "AWS::EC2::Instance"))`: the
                // entity it names is read by the one reading of that function
                // ([`Self::entity_id`]), which is what makes the two spellings of
                // one scope the same argument list.
                if self.names_an_entity_id() {
                    arguments.push(Argument {
                        label: None,
                        value: Operand::Entity(self.entity_id()?),
                    });
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                    continue;
                }
                // A labelled argument, as in `id(name: "svc", type: "AWS::EC2")`.
                // The label is a member of the keyword, not a field of its own,
                // so it is not looked up in the catalog.
                let mut label = None;
                if let (Some(Token::Field(name)), Some(Token::Colon)) = (
                    self.peek(),
                    self.tokens.get(self.at + 1).map(|spanned| &spanned.token),
                ) {
                    label = Some(name.clone());
                    self.at += 2;
                }
                arguments.push(Argument {
                    label,
                    value: self.operand()?,
                });
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::CloseParen)?;
            Ok(arguments)
        }

        /// Whether the walk is looking at an `id(…)` function: that keyword
        /// followed by its own opening parenthesis, which is what tells it apart
        /// from the bare `id` keyword an argument may also name.
        fn names_an_entity_id(&self) -> bool {
            matches!(self.peek(), Some(Token::Field(function)) if function == ENTITY_ID_FUNCTION)
                && self.tokens.get(self.at + 1).map(|spanned| &spanned.token)
                    == Some(&Token::OpenParen)
        }

        fn operand(&mut self) -> Result<Operand, Error> {
            let start = self.peek_start();
            let operand = match self.peek().cloned() {
                Some(Token::Text(text)) => Operand::Text(text),
                Some(Token::Number(number)) => Operand::Number(number),
                Some(Token::Boolean(value)) => Operand::Boolean(value),
                Some(Token::Field(path)) => {
                    if !is_indexed_field(&path) {
                        return Err(self.unreadable_at(start));
                    }
                    Operand::Field(path)
                }
                _ => return Err(self.invalid()),
            };
            self.at += 1;
            Ok(operand)
        }
    }

    /// Fold a flat `term (AND|OR term)*` sequence into a tree in which `AND`
    /// binds tighter than `OR`, each associating left to right.
    fn fold(terms: Vec<Node>, operators: &[Token]) -> Node {
        let mut disjuncts: Vec<Node> = Vec::new();
        let mut conjunct: Option<Node> = None;
        for (at, term) in terms.into_iter().enumerate() {
            match at.checked_sub(1).map(|before| &operators[before]) {
                // The first term opens the first conjunction.
                None => conjunct = Some(term),
                Some(Token::And) => {
                    conjunct = Some(match conjunct.take() {
                        Some(left) => Node::And(Box::new(left), Box::new(term)),
                        None => term,
                    });
                }
                // An `OR` closes the conjunction built so far.
                _ => {
                    if let Some(left) = conjunct.take() {
                        disjuncts.push(left);
                    }
                    conjunct = Some(term);
                }
            }
        }
        if let Some(last) = conjunct {
            disjuncts.push(last);
        }
        let mut walk = disjuncts.into_iter();
        let first = walk
            .next()
            .expect("a parsed expression holds at least one term");
        walk.fold(first, |left, right| {
            Node::Or(Box::new(left), Box::new(right))
        })
    }

    /// Parse `expression` into the one tree this crate reads it as, answering
    /// `Ok` only for an expression X-Ray would accept.
    ///
    /// An ABSENT expression is the same refusal as an empty one — X-Ray treats
    /// the two identically — so a handler passes `unwrap_or_default()` here
    /// rather than skipping the check when the member is `None`.
    pub fn parse(expression: &str) -> Result<Node, Error> {
        if expression.trim().is_empty() {
            return Err(Error::Empty);
        }
        if expression.chars().count() > MAX_LENGTH {
            return Err(Error::Bound(format!(
                "longer than the {MAX_LENGTH} characters this emulator parses"
            )));
        }
        let characters: Vec<char> = expression.chars().collect();
        let mut parser = Parser {
            source: &characters,
            tokens: tokenize(&characters)?,
            at: 0,
        };
        let node = parser.expression(0)?;
        if parser.peek().is_some() {
            return Err(parser.invalid());
        }
        Ok(node)
    }

    /// Whether `expression` is one X-Ray would accept, for a surface that only
    /// STORES it: [`parse`] with the tree thrown away.
    pub fn validate(expression: &str) -> Result<(), Error> {
        parse(expression).map(drop)
    }

    /// The keyword that names ONE entity by its labelled members, in both
    /// productions of this language: `id(name: "…", type: "…")`.
    const ENTITY_ID_FUNCTION: &str = "id";

    /// Which entities of the recorded traffic an aggregation covers.
    ///
    /// `GetTimeSeriesServiceStatistics$EntitySelectorExpression` is "a filter
    /// expression defining entities that will be aggregated for statistics.
    /// Supports ID, service, and edge functions" — a production of its own over
    /// the same lexis as the boolean expression above, so it is read here, by the
    /// same tokenizer, and refused in the same words.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum EntitySelector {
        /// `id(…)` on its own, or wrapped in `service(…)`: the segments of the
        /// service it names.
        Service(EntityId),
        /// `edge(id(…), id(…))`: the calls that reached the second entity from
        /// the first.
        Edge {
            /// The entity the calls came from.
            source: EntityId,
            /// The entity they reached, whose segments the aggregate counts.
            destination: EntityId,
        },
    }

    /// What one `id(…)` function names: each member it labels is one the entity
    /// has to match, and a member it omits narrows nothing.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct EntityId {
        /// `id(name: "…")`: the service's name.
        pub name: Option<String>,
        /// `id(type: "…")`: the service's type.
        pub r#type: Option<String>,
        /// `id(account.id: "…")`: the account the service runs in.
        pub account_id: Option<String>,
    }

    /// Parse an `EntitySelectorExpression` into the entities it names, answering
    /// `Ok` only for a selector X-Ray would accept.
    pub fn parse_entity_selector(expression: &str) -> Result<EntitySelector, Error> {
        if expression.trim().is_empty() {
            return Err(Error::Empty);
        }
        if expression.chars().count() > MAX_LENGTH {
            return Err(Error::Bound(format!(
                "longer than the {MAX_LENGTH} characters this emulator parses"
            )));
        }
        let characters: Vec<char> = expression.chars().collect();
        let mut parser = Parser {
            source: &characters,
            tokens: tokenize(&characters)?,
            at: 0,
        };
        let selector = parser.entity_selector()?;
        if parser.peek().is_some() {
            return Err(parser.invalid());
        }
        Ok(selector)
    }
}

/// What `UpdateGroup` supplies: the members of a group a caller may change.
///
/// A `None` member is one the request OMITTED and so leaves alone — which is
/// why this is not a [`GroupRecordDraft`]: a draft states a whole group, an
/// update states only the parts of one.
#[derive(Debug, Clone, Default)]
pub struct GroupUpdate {
    /// The updated filter expression, when the request carried one.
    pub filter_expression: Option<String>,
    /// The updated insights configuration, when the request carried one.
    pub insights_configuration: Option<InsightsConfigurationRecord>,
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// Now, in floating point seconds of epoch time — the unit every stored instant
/// in this service uses, so a handler never converts between two clocks.
pub fn now_epoch_seconds() -> f64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// A modelled timestamp as floating point seconds of epoch time.
pub fn epoch_seconds(timestamp: &::aws_smithy_types::DateTime) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let seconds = timestamp.secs() as f64;
    seconds + f64::from(timestamp.subsec_nanos()) / 1_000_000_000.0
}

// ---------------------------------------------------------------------------
// [StartTime, EndTime] windows
// ---------------------------------------------------------------------------

/// What X-Ray tells a caller whose `EndTime` is before its `StartTime`.
///
/// A FIXED service text, not a sentence computed from the request: the refusal
/// names neither bound, so the same words answer the same mistake on every
/// operation that takes a window, and nothing in it varies from run to run.
pub const INVERTED_WINDOW_MESSAGE: &str = "End time cannot be earlier than start time";

/// The preamble a refused request PARAMETER is stated under by the operations
/// that word it that way (`StartTraceRetrieval`), ahead of the sentence naming
/// what was wrong with it.
pub const INVALID_INPUT_PARAMETERS_PREAMBLE: &str = "Input parameters invalid for request.";

/// Why a `[StartTime, EndTime]` window names no interval of recorded traffic.
///
/// Lives here, beside [`validate_window`], so every operation that takes a
/// required window refuses the same windows with the same wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowError {
    /// `EndTime` is before `StartTime`, so the window contains no instant.
    Inverted,
    /// A bound lands further than [`MAX_WINDOW_FUTURE_SKEW_SECONDS`] past now,
    /// so it names no time any recorded segment could carry — which is what a
    /// millisecond-scale epoch value read as seconds looks like.
    Future,
}

impl WindowError {
    /// What to tell the caller: the service's own fixed text for an inverted
    /// window, and — for the bound this emulator adds — the members at fault
    /// and the values that broke the rule.
    pub fn message(self, start: f64, end: f64) -> String {
        match self {
            Self::Inverted => INVERTED_WINDOW_MESSAGE.to_string(),
            Self::Future => format!(
                "StartTime ({start}) and EndTime ({end}) must be within {MAX_WINDOW_FUTURE_SKEW_SECONDS} seconds of the present, in seconds of epoch time"
            ),
        }
    }
}

/// Refuse a `[StartTime, EndTime]` window that runs BACKWARDS: an `EndTime`
/// before its `StartTime` names no interval at all, whatever instants the two
/// bounds are.
///
/// Split out of [`validate_window`] so a reader that must serve any instant its
/// store holds — one whose stored timestamps may lie arbitrarily far out — still
/// refuses an inverted window with the one judgement and the one wording every
/// other window-taking operation uses.
pub fn validate_window_order(start: f64, end: f64) -> Result<(), WindowError> {
    if end < start {
        return Err(WindowError::Inverted);
    }
    Ok(())
}

/// Refuse a `[StartTime, EndTime]` window that names no interval of recorded
/// traffic: one running backwards, or one whose bounds are not times the data
/// could have.
///
/// Nothing is refused for being OLD: this emulator's trace store keeps whatever
/// was ingested until [`MAX_TRACES`] forces an eviction, so there is no
/// retention horizon a window could fall off the far side of.
pub fn validate_window(start: f64, end: f64, now: f64) -> Result<(), WindowError> {
    if !start.is_finite() || !end.is_finite() {
        return Err(WindowError::Future);
    }
    validate_window_order(start, end)?;
    if start > now + MAX_WINDOW_FUTURE_SKEW_SECONDS || end > now + MAX_WINDOW_FUTURE_SKEW_SECONDS {
        return Err(WindowError::Future);
    }
    Ok(())
}

/// When X-Ray became generally available, in seconds of epoch time
/// (2017-04-19T00:00:00Z).
///
/// The FLOOR a retrieval's window is judged against: no trace can have been
/// recorded before the service that records them existed, so a window ending
/// below it names no retrievable traffic at all.
pub const XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS: f64 = 1_492_560_000.0;

/// What X-Ray tells a caller who asks to retrieve a window that lies outside
/// the interval a retrieval can cover — below the floor the service's own
/// availability sets, or past a time any recorded trace could carry, which is
/// what a millisecond-scale epoch value read as seconds is.
///
/// A FIXED service text, its two sentences run together exactly as the service
/// states them: it names neither bound and no limit, so nothing in it varies
/// from run to run.
pub const RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE: &str = "Invalid input parameters for starting trace retrieval.End time should not be before the service was generally available.";

/// Why a retrieval's `[StartTime, EndTime]` window names no interval a
/// retrieval could cover.
///
/// Separate from [`WindowError`] because the refusals a window-READER states
/// are not the ones the retrieval STARTER states: each class here carries the
/// service's own fixed sentence for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalWindowError {
    /// `EndTime` is before `StartTime`, so the window contains no instant.
    Inverted,
    /// A bound lies outside the interval a retrieval can cover: below
    /// [`XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS`], or further than
    /// [`MAX_WINDOW_FUTURE_SKEW_SECONDS`] past now.
    OutOfRange,
}

impl RetrievalWindowError {
    /// What to tell the caller: the service's fixed text for this class,
    /// interpolating nothing from the request.
    pub fn message(self) -> String {
        match self {
            Self::Inverted => format!("{INVALID_INPUT_PARAMETERS_PREAMBLE} {INVERTED_WINDOW_MESSAGE}"),
            Self::OutOfRange => RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE.to_string(),
        }
    }
}

/// Refuse a window a retrieval could not cover: one running backwards, and one
/// whose bounds are not instants retrievable traffic could carry — below the
/// floor the service's own general availability sets, or past the present by
/// more than any clock skew.
pub fn validate_retrieval_window(
    start: f64,
    end: f64,
    now: f64,
) -> Result<(), RetrievalWindowError> {
    if !start.is_finite() || !end.is_finite() {
        return Err(RetrievalWindowError::OutOfRange);
    }
    if validate_window_order(start, end).is_err() {
        return Err(RetrievalWindowError::Inverted);
    }
    if end < XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS
        || start > now + MAX_WINDOW_FUTURE_SKEW_SECONDS
        || end > now + MAX_WINDOW_FUTURE_SKEW_SECONDS
    {
        return Err(RetrievalWindowError::OutOfRange);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The per-account request budget
// ---------------------------------------------------------------------------

/// A request budget: [`burst`](Self::burst) requests may arrive back-to-back,
/// and the budget refills at [`per_second`](Self::per_second) requests a second.
///
/// A token bucket rather than a counter per fixed window, so a caller that has
/// just exhausted the budget is not locked out for the rest of a window: the
/// next request is served as soon as the refill has earned one request back.
#[derive(Debug)]
pub struct RateLimiter {
    /// How many requests may arrive back-to-back.
    burst: f64,
    /// How fast the budget refills, in requests per second.
    per_second: f64,
    /// What is left of the budget.
    tokens: f64,
    /// When the budget was last refilled.
    last_refill: ::std::time::Instant,
}

impl RateLimiter {
    /// A full budget of `burst` requests, refilling at `per_second`.
    pub fn new(burst: u32, per_second: f64) -> Self {
        Self {
            burst: f64::from(burst),
            per_second,
            tokens: f64::from(burst),
            last_refill: ::std::time::Instant::now(),
        }
    }

    /// Spend one request of the budget, answering whether there was one to
    /// spend. `false` is what a `ThrottledException` reports.
    pub fn try_acquire(&mut self) -> bool {
        let now = ::std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// `StartTraceRetrieval` is the one operation in this service with a request
/// budget, so the default is its: see [`TRACE_RETRIEVAL_BURST`].
impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(TRACE_RETRIEVAL_BURST, TRACE_RETRIEVAL_RATE_PER_SECOND)
    }
}

/// The decoder clause a value that is not of the token form at all is refused
/// with ([`malformed_retrieval_token_message`]): fixed text, capitalised exactly
/// this way, naming no value, whatever the operation and whatever the value.
pub const MALFORMED_RETRIEVAL_TOKEN_CAUSE: &str = "Invalid Execution Id";

/// The decoder clause a value of the token form whose check does not verify is
/// refused with ([`malformed_retrieval_token_message`]): fixed text, sentence
/// cased, followed by the value the decoder could not decode.
pub const UNDECODABLE_EXECUTION_ID_CAUSE: &str = "Invalid execution id provided: ";

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// One stored instant as the modelled `Timestamp`.
///
/// Ingest refuses an instant that is not finite or further from the epoch than
/// [`MAX_SEGMENT_EPOCH_SECONDS`], so this conversion is always in range; the
/// bounding here is the second belt, so no stored value can make a READ end the
/// request instead of answering it.
pub fn timestamp_of(seconds: f64) -> ::aws_smithy_types::DateTime {
    let bounded = if seconds.is_nan() {
        0.0
    } else {
        seconds.clamp(-MAX_SEGMENT_EPOCH_SECONDS, MAX_SEGMENT_EPOCH_SECONDS)
    };
    ::aws_smithy_types::DateTime::from_secs_f64(bounded)
}

/// The region this request's SigV4 credential scope names, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
///
/// `None` means the service was mounted on an engine that holds no state, which
/// is a misconfiguration of the emulator rather than anything about the request.
/// X-Ray models no server-fault shape — every operation here declares only
/// `InvalidRequestException` (400) and `ThrottledException` (429) — so the
/// handlers answer `ThrottledException` with a message naming the
/// misconfiguration: of the two codes this service can speak, it is the one that
/// does not blame the caller's request. The production router always mounts this
/// service on the shared `InMemoryEngine`, so the branch is unreachable there.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsXrayAwsxrayState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsXrayAwsxrayState>)
}
