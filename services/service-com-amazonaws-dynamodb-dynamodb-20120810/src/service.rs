// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dynamodb-dynamodb-20120810` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — EDITED HERE. It serves the endpoint's own HTTP surface for a
//!   request no operation of this model serves (see "The endpoint's own HTTP surface"), and
//!   otherwise the operation the generated routing table names; what it adds to that is the
//!   SERVED OPERATION in scope for that operation's whole run, which is what tells
//!   [`Service::override_refusal`] which form a constraint clause takes (see "Which form a
//!   constraint clause takes"). The routing decision among the MODELLED operations is not
//!   overridden.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — EDITED HERE. A request that reached this service and named
//!   no modelled operation is a target this ENDPOINT binds nothing for, which its front
//!   door refuses in its own vocabulary (see "The endpoint's own HTTP surface").
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The endpoint's own HTTP surface
//!
//! A DynamoDB endpoint is an HTTP endpoint before it is an API: its root has a method
//! surface of its own, it serves its API under the JSON media types alone, and a target it
//! cannot bind to an operation is refused ahead of every operation in a vocabulary that is
//! the front door's rather than this model's. That whole layer lives in [`front_door`], and
//! is wired at the two seams a request can meet it through — [`Service::override_operation`]
//! (the surface itself) and [`Service::override_routing_error`] (a target naming no bound
//! operation).
//!
//! ## What every response reports about itself
//!
//! Two things every response of this service carries, whichever layer produced it, are
//! stamped by [`reported`]: the request it answered (below) and the CRC-32 of its own body
//! ([`body_checksum`]), which an SDK validates every DynamoDB response against.
//!
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header the
//! protocol reports that id in (`x-amzn-RequestId` for `awsJson1_0`) — on a success
//! exactly as on a failure, and freshly per HTTP ATTEMPT, so two deliveries of one
//! retried call are two responses naming two different requests. It is a property of the
//! RESPONSE, not of any modelled shape: no output and no error shape of this model
//! declares it, so no handler can set it, and the framework's `awsJson` renderers do not
//! write it.
//!
//! The rule is not restated here: [`service_common::query_error_envelope`] owns it for
//! every emulated service, and this file only chooses the report this protocol takes (the
//! header alone — `awsJson1_0` has no envelope element for the id), in the FORM this
//! SERVICE spells its ids in ([`request_id`], which is the one part of the rule that is
//! DynamoDB's own rather than the shared module's), and wires it at the two layers every
//! response of this service passes through:
//!
//! * [`OperationScopedClauseForm`], the dispatch [`Service::override_operation`] hands the
//!   router in place of each modelled operation, covers everything an operation answers —
//!   a success, a modelled error, and a constraint refusal raised while that operation was
//!   being served;
//! * [`RequestIdEnvelopeDefinition::reporting`], the definition wrapper
//!   [`Service::definition`] hands back, covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal. It is handed the SAME report as the dispatch above, so which
//!   layer answered cannot change how the request is named.
//!
//! Nothing else about a response changes: the status, the other headers and every byte of
//! the body stay as the renderer wrote them.
//!
//! ## Wording a request authentication refused
//!
//! The emulator verifies every signature and hands this service one structural refusal for
//! every way a credential can fail, carrying a generic code that is an AUTHORIZATION denial
//! — which is not what AWS answers a credential failure with. The rule, and the sentences,
//! live in [`service_common::authentication_refusal`]; this file names only the two codes
//! this service answers the two failures with ([`AUTHENTICATION_REFUSAL`]) and wires that
//! layer under the definition wrapper above, so the id is reported on its response too.
//!
//! It is the definition rather than [`Service::override_refusal`] because the seam that is
//! handed WHICH failure it was is the definition's `refusal_response`; the refusal hook sees
//! the structural refusal alone, which is why this service uses it for input constraints.

use service_common::authentication_refusal::{
    AuthenticationRefusalDefinition, StatedAuthenticationRefusal,
};
use request_id::with_request_id_header;
use service_common::query_error_envelope::RequestIdEnvelopeDefinition;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Refusal, Refused,
    Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The FORM of the id every response of this service names its request by, which is this
/// service's own; `service_common::query_error_envelope` owns everything else about the
/// rule (see "Naming the request a response answers").
mod request_id;

/// The checksum every response of this service reports its own body under
/// (`x-amz-crc32`), stamped at the two layers below every response passes through.
mod body_checksum;

/// The endpoint's own HTTP surface: the root's method surface, the media types the API is
/// served under, and what a target naming no bound operation is refused with.
mod front_door;

/// The `AWS/DynamoDB` `SuccessfulRequestLatency` series this service publishes for each
/// table request it serves, under the `{TableName, Operation}` dimension pair that family is
/// reported on.
mod request_latency_metrics;

/// Everything every response of this service reports about ITSELF, whichever layer
/// produced it: the request it answered ([`request_id`]) and the checksum of its own body
/// ([`body_checksum`]).
///
/// This is the [`service_common::query_error_envelope::Report`] both layers of this service
/// take, so a response an operation produced and a response no operation produced report
/// the same two things. A response the FRONT DOOR produced is not a document of this API
/// and takes the first of them alone ([`front_door::EndpointSurface`]).
fn reported(
    resp: aws_smithy_runtime_api::http::Response,
) -> aws_smithy_runtime_api::http::Response {
    body_checksum::with_body_checksum_header(with_request_id_header(resp))
}

/// The error code this service answers an input-constraint violation with: the
/// `ValidationException` (HTTP 400) every DynamoDB operation refuses a malformed
/// request with. Restated here because `override_refusal` replaces the code and
/// the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The path segment the framework contributes when it steps from a collection
/// into one of its elements (`RequestItems.<table>.member.Keys`). It is a fixed
/// marker rather than a model name, so it is never re-cased — and the segment
/// BEFORE it is the caller's own map key or the element's 1-based position, so
/// that one is never re-cased either.
const COLLECTION_STEP: &str = "member";

/// A member path as DynamoDB states it: the framework builds the path in
/// lowerCamel (its own convention, `path::member`), and this service names a
/// member the way its model spells it on the wire — every DynamoDB member name
/// is UpperCamel, so restoring the wire spelling is restoring the first
/// character of each MODEL segment. A segment the caller chose (a map key) and
/// the collection marker itself pass through untouched.
///
/// The path arrives FLATTENED (`RequestItems.<table>.member.Keys`), so a map key
/// that itself contains a `.` — a table name may — is indistinguishable from two
/// segments. The segment immediately before the marker is the one taken as the
/// caller's, which is exact for every key without a dot and leaves the earlier
/// part of a dotted key re-cased; the alternative (never re-casing a run) would
/// leave real model segments lowerCamel, which is the error a client reading the
/// path would actually trip over.
fn wire_member_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('.').collect();
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let is_collection_marker = *segment == COLLECTION_STEP;
            // A list index or a caller-chosen map key is the segment the
            // collection marker follows: `<key>.member`, `<position>.member`.
            let is_collection_selector =
                segments.get(index + 1).copied() == Some(COLLECTION_STEP);
            if is_collection_marker || is_collection_selector {
                (*segment).to_string()
            } else {
                upper_camel(segment)
            }
        })
        .collect::<Vec<String>>()
        .join(".")
}

/// Upper-case the first character of a member name, leaving the rest alone
/// (`requestItems` -> `RequestItems`, `sSESpecification` -> `SSESpecification`).
fn upper_camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The requirement a violation states, in AWS's fixed phrasing, for the clause
/// form the framework's own renderer cannot produce — the one that echoes NO
/// value. Every phrase comes from `crate::state`, which is where this service
/// spells each of them once (and says why they are not borrowed from the
/// framework).
///
/// `None` for a violation class this service has no wording for, which lets
/// [`validation_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MinLength(min) => {
            crate::state::min_length_requirement(usize::try_from(*min).unwrap_or(0))
        }
        ViolationKind::MaxLength(max) => {
            crate::state::max_length_requirement(usize::try_from(*max).unwrap_or(0))
        }
        ViolationKind::MinValue(min) => crate::state::min_value_requirement(min),
        ViolationKind::MaxValue(max) => crate::state::max_value_requirement(max),
        ViolationKind::Pattern(pattern) => crate::state::pattern_requirement(pattern),
        ViolationKind::UniqueItems => crate::state::unique_items_requirement(),
        // Everything else has no no-echo phrase here: the enum and the
        // absent-member classes render exactly as the framework renders them, so
        // `violation_clause` hands those to `Violation::message` instead, and a
        // class added upstream has no phrase at all, which declines the refusal
        // and keeps the framework's wording.
        _ => return None,
    })
}

/// The input members DynamoDB's own request code validates rather than leaving to the
/// generic request walk, each named by the path that walk reports it under and paired
/// with the value set that check states.
///
/// Their refusals are worded differently from every other enumerated member's: the
/// clause states the requirement ALONE — neither the rejected value nor the member path
/// — which is what a check by the service's own code looks like next to the generic
/// validator's `Value '<v>' at '<path>' failed to satisfy constraint: …`.
///
/// These are NAMED MEMBERS, not a class, because the service's refusals of them are
/// observed to take that shape while the refusals of the service's other enumerated
/// members (`ReturnConsumedCapacity`, `Select`, `StreamViewType`, `ProjectionType`, …)
/// are observed to take the generic one. Nothing about the model distinguishes them, so
/// there is nothing to derive the difference from: a member later found to be checked
/// the same way needs its own entry, and
/// `tests/all/refusal_contracts_and_expression_pre_images.rs` and
/// `tests/all/legacy_conditional_operator_refusals.rs` pin both shapes side by side so
/// the difference is visible rather than silent.
///
/// The value set each entry states is the SERVICE's own rather than the model's:
///
/// * `returnValues` states `ReturnValue`
///   (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ReturnValue")` gives
///   `NONE ALL_OLD UPDATED_OLD ALL_NEW UPDATED_NEW`) in a different order and spells
///   `UPDATED_NEW` as `UPDATE_NEW`;
/// * `conditionalOperator` states `[ALL, OR]` where the model's `ConditionalOperator`
///   (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ConditionalOperator")`
///   gives `AND OR`) declares `AND` for the value `ALL` stands in for.
///
/// Every difference is the service's and is reproduced rather than normalised, because a
/// client matching the message has to read the same text from the emulator and from
/// DynamoDB; deriving a set from the model would state a different sentence from the one
/// the service sends.
const SERVICE_CHECKED_ENUMS: &[(&str, &[&str])] = &[
    (
        "returnValues",
        &["ALL_OLD", "UPDATED_OLD", "ALL_NEW", "UPDATE_NEW", "NONE"],
    ),
    ("conditionalOperator", &["ALL", "OR"]),
];

/// The set the service's own check states for `path`, or `None` for a member the generic
/// request walk words.
fn service_checked_enum_set(path: &str) -> Option<&'static [&'static str]> {
    SERVICE_CHECKED_ENUMS
        .iter()
        .find(|(member, _)| *member == path)
        .map(|(_, stated)| *stated)
}

/// Whether every violation this refusal reports is about a member the service checks
/// itself ([`SERVICE_CHECKED_ENUMS`]) being outside its set — the refusals whose
/// envelope follows the operation that was served.
fn all_service_checked_enums(violations: &[Violation]) -> bool {
    !violations.is_empty()
        && violations.iter().all(|violation| {
            matches!(violation.kind, ViolationKind::EnumValue(_))
                && service_checked_enum_set(&violation.path).is_some()
        })
}

/// The operations that state a clause about a member they check themselves BARE, outside
/// the `<N> validation error(s) detected: ` envelope every other refusal of this service
/// carries.
///
/// It is the same split this service states everywhere else about the two generations of
/// condition members (`crate::state`'s enveloped-members rule): a refusal about the
/// members a WRITE states its intent through carries the envelope, and one about the
/// members a READ narrows its result with does not. The two readers below are the
/// operations that carry such a member — `ConditionalOperator`, beside the filter map it
/// combines — so their refusal of it is the sentence alone.
///
/// Sorted, which [`states_service_checked_clause_bare`] relies on for `binary_search`
/// and `bare_clause_operations_are_sorted` pins.
const BARE_CLAUSE_OPERATIONS: &[&str] = &["Query", "Scan"];

/// Whether `operation` states such a clause bare.
fn states_service_checked_clause_bare(operation: &str) -> bool {
    BARE_CLAUSE_OPERATIONS.binary_search(&operation).is_ok()
}

/// The operations whose refusal of an ABSENT member this service's own request code
/// requires is the validating WALK's clause rather than this service's
/// parameter-required sentence ([`crate::state::missing_parameter_message`]).
///
/// The table-name member is the one such member ([`SERVICE_REQUIRED_MEMBERS`]), and the
/// same absence of the same member is observed to be stated BOTH ways: the operations
/// below answer `1 validation error detected: Value null at 'tableName' failed to satisfy
/// constraint: Member must not be null` — the walk's clause inside the counting envelope —
/// while every other operation answers the reader's own sentence. Nothing about the model
/// distinguishes the two groups, so this list is the only place the difference is recorded,
/// and an operation later found to state the clause needs its own entry here.
/// `tests/all/required_member_refusals.rs` pins both groups side by side so the difference
/// is visible rather than silent.
///
/// Sorted, which [`states_absent_member_as_clause`] relies on for `binary_search` and
/// `validator_null_clause_operations_are_sorted` pins.
const VALIDATOR_NULL_CLAUSE_OPERATIONS: &[&str] = &["DeleteTable"];

/// Whether `operation` states an absent such member as the walk's clause.
fn states_absent_member_as_clause(operation: &str) -> bool {
    VALIDATOR_NULL_CLAUSE_OPERATIONS
        .binary_search(&operation)
        .is_ok()
}

/// The enum value sets this service STATES in an order of its own, each written in the
/// order it states them.
///
/// The bracketed set a constraint clause ends with is the admissible set as the SERVICE
/// words it, which is a property of the enum and not of the model: the generic renderer
/// iterates the model's declaration order, and for the enums below the service is
/// observed to state the same values in a different one. An enum this list does not name
/// keeps the model's order, because that is the order the service is observed to state it
/// in (`ReturnConsumedCapacity` as `INDEXES, TOTAL, NONE` and `Select` as
/// `ALL_ATTRIBUTES, ALL_PROJECTED_ATTRIBUTES, SPECIFIC_ATTRIBUTES, COUNT`, both the
/// declared order — `tests/all/validation_message_wording.rs` pins them).
///
/// Nothing about the model distinguishes the two groups — the same values, a different
/// sentence — so this is the only place the difference is recorded, and an enum later
/// found to be stated in an order of its own needs its own entry here.
/// `tests/all/enum_value_set_order.rs` reads both groups back over the operations and
/// members that carry them, so the difference is visible rather than silent.
const PUBLISHED_ENUM_ORDERS: &[&[&str]] = &[
    // `StreamSpecification$StreamViewType`, which the model declares as
    // `NEW_IMAGE OLD_IMAGE NEW_AND_OLD_IMAGES KEYS_ONLY`
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#StreamViewType")`).
    &["OLD_IMAGE", "KEYS_ONLY", "NEW_AND_OLD_IMAGES", "NEW_IMAGE"],
    // `IncrementalExportSpecification$ExportViewType`, which the model declares as
    // `NEW_IMAGE NEW_AND_OLD_IMAGES`
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ExportViewType")`).
    &["NEW_AND_OLD_IMAGES", "NEW_IMAGE"],
];

/// The admissible set `allowed` is STATED in: the order this service publishes for that
/// enum ([`PUBLISHED_ENUM_ORDERS`]) where it publishes one, and otherwise the order the
/// model declared and the walk reported.
///
/// An entry is matched on MEMBERSHIP and size, never on position, so a model that
/// reordered its own declaration of the same enum would still be stated in the published
/// order rather than silently falling back.
fn stated_enum_order(allowed: &[String]) -> Vec<String> {
    PUBLISHED_ENUM_ORDERS
        .iter()
        .find(|published| {
            published.len() == allowed.len()
                && published
                    .iter()
                    .all(|value| allowed.iter().any(|declared| declared == value))
        })
        .map(|published| published.iter().map(|value| (*value).to_string()).collect())
        .unwrap_or_else(|| allowed.to_vec())
}

// =============================================================================
// WHICH FORM A CONSTRAINT CLAUSE TAKES
//
// DynamoDB states a clause about a member's declared bound in one of three forms:
//
//   * the ITEM-REQUEST form, which names the member by its WIRE spelling and
//     echoes NO value — `Value at 'Limit' failed to satisfy constraint: Member
//     must have value greater than or equal to 1` for a `Query` page size below
//     its `@range` minimum, and `Value at 'RequestItems.<table>.member.Keys' …`
//     for a batched key list past its cap;
//   * the VALIDATOR-PATH form, which echoes no value either but names the member
//     by the lowerCamel path the request validator builds — `Value at
//     'requestItems' …` for a `BatchWriteItem` request map past either end of its
//     `@length` bound;
//   * the REQUEST-VALIDATOR form, which names the member by that same lowerCamel
//     path and DOES echo the rejected value — `Value '0' at
//     'limit' …` for a backup listing's page size, `Value '26' at 'maxResults'
//     …` for an export listing's, `Value '' at
//     'timeToLiveSpecification.attributeName' …` for an empty TTL attribute name.
//
// All three are observed for the SAME violation class on the SAME member name
// (`Limit`, `@range` min 1, on `Query` and on `ListBackups`; `RequestItems`,
// `@length` min 1, on `BatchGetItem` and on `BatchWriteItem`), so nothing about
// the model or about the violation distinguishes them: only the OPERATION being
// served and the MEMBER it is about do, which is why
// [`ITEM_REQUEST_FORM_OPERATIONS`] and [`VALIDATOR_PATH_FORM_OPERATIONS`] list
// the operations whose refusals are observed to take the first two forms and
// every other operation takes the last, and [`VALUE_ECHOING_MEMBERS`] names the
// members a listed operation is observed to state in the last form even so.
// [`OperationScopedClauseForm`] is how the served operation
// reaches [`Service::override_refusal`], which the framework hands no request.
//
// TWO CLASSES ARE NOT PART OF THAT CHOICE, because neither is found by a
// validating walk at all: a value outside an enumerated member's set and a
// required member that is absent are both found while READING the request, and
// both keep the reader's own rendering whichever operation was served (`Value
// 'NOPE' at 'returnConsumedCapacity' …` on `GetItem`, `Value null at
// 'streamArn' …`).
// =============================================================================

/// The operations whose refusal of a value outside a bound the model declares is
/// observed to take the item-request form above.
///
/// Nothing in the model distinguishes them from the rest — the same member name,
/// the same trait, a different sentence — so this list is the only place the
/// difference is recorded, and an operation later found to state that form needs
/// its own entry here. `tests/all/validation_message_fidelity.rs`,
/// `tests/all/validation_message_wording.rs` and
/// `tests/all/index_key_types_and_range_refusals.rs` pin both forms side by side so
/// the difference is visible rather than silent.
///
/// Sorted, which [`clause_form`] relies on for `binary_search` and
/// `item_request_form_operations_are_sorted` pins.
const ITEM_REQUEST_FORM_OPERATIONS: &[&str] = &[
    "BatchGetItem",
    "Query",
    "Scan",
    "TransactGetItems",
    "TransactWriteItems",
];

/// The operations that echo NO value either, and name the member by the VALIDATOR's
/// lowerCamel path rather than its wire spelling.
///
/// A third form, observed for `BatchWriteItem`: its `RequestItems` map past either end of
/// its declared bound is reported as `Value at 'requestItems' failed to satisfy
/// constraint: …`, where the SAME violation class on `BatchGetItem`'s map of the same
/// name is reported as `Value at 'RequestItems' …`. Nothing about the model or the
/// violation distinguishes the two — one member name, one trait, two spellings — so, like
/// the split above, only the OPERATION being served does, and this is the only place that
/// is recorded. `tests/all/collection_cap_constraint_reports.rs` pins the two operations'
/// spellings side by side.
///
/// The form is stated per OPERATION rather than per member because `requestItems` is the
/// only member of that operation a clause raised HERE can name. `BatchWriteItemInput`
/// declares that map and the two `ReturnConsumedCapacity`/`ReturnItemCollectionMetrics`
/// enums (`smithy_query(query="operation", operation="BatchWriteItem")`), and a value
/// outside an enumerated member's set keeps the reader's own rendering whichever form was
/// served; nothing NESTED under the map reaches this seam either — the write-request list
/// inside each entry is bounded by the HANDLER over the operations the whole map carries
/// (`crate::state::batch_write_requests_outside_bound`), and the map's KEY is the
/// service's own table-name check (`crate::state::batch_table_key`), neither of which is a
/// violation the validating walk reports.
/// `tests/all/collection_cap_constraint_reports.rs` pins both of those beside the
/// top-level member's spelling.
///
/// Sorted, for the reason the list above is.
const VALIDATOR_PATH_FORM_OPERATIONS: &[&str] = &["BatchWriteItem"];

/// Which of the forms above a clause names its member and its value by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClauseForm {
    /// The member as the model spells it on the wire, with no value echoed.
    WireMemberOnly,
    /// The validator's lowerCamel path, with no value echoed.
    ValidatorMemberOnly,
    /// The validator's lowerCamel path, with the rejected value echoed.
    PathAndValue,
}

impl ClauseForm {
    /// The path a clause in this form names `path` by.
    fn stated_path(self, path: &str) -> String {
        match self {
            Self::WireMemberOnly => wire_member_path(path),
            Self::ValidatorMemberOnly | Self::PathAndValue => path.to_string(),
        }
    }
}

/// The form `operation`'s refusals take, for every member that has no form of its own
/// ([`VALUE_ECHOING_MEMBERS`]).
fn clause_form(operation: &str) -> ClauseForm {
    if ITEM_REQUEST_FORM_OPERATIONS.binary_search(&operation).is_ok() {
        ClauseForm::WireMemberOnly
    } else if VALIDATOR_PATH_FORM_OPERATIONS
        .binary_search(&operation)
        .is_ok()
    {
        ClauseForm::ValidatorMemberOnly
    } else {
        ClauseForm::PathAndValue
    }
}

/// The members a listed operation states in the REQUEST-VALIDATOR form — the validator's
/// own path with the rejected value echoed — whatever form [`clause_form`] gives the rest
/// of that operation's clauses.
///
/// Which form a clause takes is a fact of the served operation AND of the member, not of
/// the violation class: the parallel-scan bounds of a table walk are stated in the
/// item-request form (`Value at 'TotalSegments' …`) while that same walk's page size is
/// stated with the value echoed (`Value '0' at 'limit' …`), and a KEYED read's page size —
/// the same member name, the same `@range` minimum on the same target shape
/// (`smithy_query(query="operation", operation="Query,Scan")` gives both `Limit` members
/// targeting `PositiveIntegerObject`) — is stated in the item-request form. Nothing about
/// the model or about the violation distinguishes them, so this list is the only place the
/// difference is recorded, and a member later found to be stated with its value echoed
/// needs its own entry here. Each member is named by the path the validating walk reports
/// it under, and `tests/all/page_size_clause_forms.rs` reads the forms back side by side
/// so the difference is visible rather than silent.
const VALUE_ECHOING_MEMBERS: &[(&str, &[&str])] = &[("Scan", &["limit"])];

/// The members `operation` states with the rejected value echoed, over the validator's own
/// path ([`VALUE_ECHOING_MEMBERS`]) — empty for every operation that states none.
fn value_echoing_members(operation: &str) -> &'static [&'static str] {
    VALUE_ECHOING_MEMBERS
        .iter()
        .find(|(named, _)| *named == operation)
        .map_or(&[][..], |(_, members)| *members)
}

tokio::task_local! {
    /// The form the operation this request is being served as states its clauses in,
    /// set by [`OperationScopedClauseForm::run`] around that operation's whole run and
    /// read back by [`EmulatedService::override_refusal`].
    static SERVED_CLAUSE_FORM: ClauseForm;

    /// The members that operation states with the rejected value echoed whatever form the
    /// rest of its clauses take ([`VALUE_ECHOING_MEMBERS`]), set and read back the same way.
    static SERVED_VALUE_ECHOING_MEMBERS: &'static [&'static str];

    /// Whether that operation states a clause about a member the service checks itself
    /// BARE ([`BARE_CLAUSE_OPERATIONS`]), set and read back the same way.
    static SERVED_BARE_CLAUSE: bool;

    /// Whether that operation states an ABSENT member this service's own request code
    /// requires as the walk's clause ([`VALIDATOR_NULL_CLAUSE_OPERATIONS`]), set and read
    /// back the same way.
    static SERVED_NULL_CLAUSE: bool;
}

/// The form the operation this task is serving states its clauses in.
///
/// Absent altogether means the refusal happened outside an operation's run, where
/// there is no served operation to read a form off; the request validator's form is
/// the general one, so that is what such a refusal states.
fn served_clause_form() -> ClauseForm {
    SERVED_CLAUSE_FORM
        .try_with(|form| *form)
        .unwrap_or(ClauseForm::PathAndValue)
}

/// The form the operation this task is serving states a clause about the member at `path`
/// in: the member's own form where that operation gives it one
/// ([`VALUE_ECHOING_MEMBERS`]), and the operation's own form for every other member.
fn served_clause_form_for(path: &str) -> ClauseForm {
    if SERVED_VALUE_ECHOING_MEMBERS
        .try_with(|members| members.contains(&path))
        .unwrap_or(false)
    {
        return ClauseForm::PathAndValue;
    }
    served_clause_form()
}

/// Whether the operation this task is serving states such a clause bare. Absent
/// altogether is a refusal raised outside an operation's run, which carries the
/// envelope every other refusal of this service carries.
fn served_states_clause_bare() -> bool {
    SERVED_BARE_CLAUSE.try_with(|bare| *bare).unwrap_or(false)
}

/// Whether the operation this task is serving states an absent member this service's own
/// request code requires as the walk's clause. Absent altogether is a refusal raised
/// outside an operation's run, which keeps this service's parameter-required sentence — the
/// answer every operation this list does not name gives.
fn served_states_absent_member_as_clause() -> bool {
    SERVED_NULL_CLAUSE.try_with(|clause| *clause).unwrap_or(false)
}

/// Serves a request as the operation the generated routing table names, with that
/// operation's clause form ([`clause_form`]) in scope for the whole run, and names the
/// request in the response's header (see "Naming the request a response answers").
///
/// It adds nothing else: the request, the service and the context are handed to the
/// modelled operation unchanged, so deserialization, validation, the handler and the
/// response are the framework's and the operation's own. The routing table holds only
/// generated operations, so the inner lookup can never resolve back to this type.
struct OperationScopedClauseForm<Eng: Send + Sync + 'static> {
    /// The SAME shared engine the generated entry threads into every operation, so the
    /// throughput each request charged is published into the one CloudWatch store this
    /// process holds.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for OperationScopedClauseForm<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // `override_operation` returns this type only for a request the routing
            // table can serve, so the lookup below resolves; a request that cannot be
            // routed keeps the framework's routing-error answer either way.
            let routed = svc
                .definition()
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| Some((key, svc.definition().routing_table().get(key)?)));
            let Some((operation, op)) = routed else {
                return reported(front_door::unknown_operation_response(req));
            };
            // What serving this request TAKES is measured around the operation's own run,
            // which is the elapsed time its latency series reports
            // ([`request_latency_metrics`]).
            let started = ::std::time::Instant::now();
            let answered = SERVED_CLAUSE_FORM
                .scope(
                    clause_form(operation),
                    SERVED_VALUE_ECHOING_MEMBERS.scope(
                        value_echoing_members(operation),
                        SERVED_BARE_CLAUSE.scope(
                            states_service_checked_clause_bare(operation),
                            SERVED_NULL_CLAUSE.scope(
                                states_absent_member_as_clause(operation),
                                op.run(svc, req, ctx),
                            ),
                        ),
                    ),
                )
                .await;
            let took = started.elapsed();
            // This is the ONE place every response an operation of this service produces
            // passes through, so no operation can answer without reporting the request it
            // answered and the checksum of its own body — and each attempt gets its own id,
            // because each attempt is served here once. It is also where the response
            // announces the media type the REQUEST named this endpoint's API under (see
            // `front_door`), which is the one layer that has both the request and the
            // rendered response in hand. It is also where the throughput the request
            // consumed is published: what it charged the table is taken under the store's
            // lock, and the series it belongs to is CloudWatch's own, so the publish happens
            // here, with that lock released and the request already served.
            crate::state::publish_pending_consumed(&*self.engine);
            // And where the capacity a provisioned table HOLDS is sampled into its own
            // series: a level the schedule has made a sample of due, taken with the store's
            // lock released ([`crate::state::provisioned_capacity_metrics`]).
            crate::state::publish_due_provisioned_capacity(&*self.engine);
            // And where the request itself is counted: a served table request is an
            // observation of its operation's latency on that table, published into the same
            // CloudWatch store.
            request_latency_metrics::publish_served_request(
                &*self.engine,
                operation,
                req,
                ctx.parts(),
                &answered,
                took,
            );
            front_door::announced(req, reported(answered))
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the
        // operation this dispatch delegates to — so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}

/// One clause for one violated member, in the `form` the served operation states
/// its clauses in (see "Which form a constraint clause takes").
///
/// Two classes are rendered the same way whichever operation was served, because
/// neither is found by the validating walk:
///
/// * a string outside an enumerated member's value set is reported the way the
///   reader reports it — the member's lowerCamel path with the rejected value
///   echoed, so a caller can read out of the message which value of which member
///   was refused. That is exactly the framework's own rendering, so the framework
///   renders it (`Violation::message`) rather than this service restating the
///   sentence; the one thing this service decides is the ORDER the admissible set
///   is stated in ([`stated_enum_order`]). [`SERVICE_CHECKED_ENUMS`] names the
///   members that are not reported that way, because the service checks them itself;
/// * an absent member states the `null` it was, which is a fact of the request's
///   shape — the framework's rendering again, over the member path the validating
///   walk reports it under (`Value null at 'streamArn' …`), which is the path the
///   recorded refusals of this service state, whichever member it is;
///
/// A COLLECTION member past its cap is a clause like any other: in the item-request
/// form the member and the bound, with nothing echoed. Some recorded AWS refusals of a
/// capped collection — the transactional `TransactItems` lists — instead echo the
/// rejected list as the service's OWN object rendering of it
/// (`[TransactGetItem(get=Get(key={pk=AttributeValue(s=k0, n=null, …,
/// iNT=null, dECIMALSET=null, …)}, …, workloadProfileName=null)), …]`), over
/// fields this service's model does not declare:
/// `smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`
/// gives `S N B SS NS BS M L NULL BOOL` and no `iNT`/`dECIMALSET`, and `Get`
/// and `Put` carry no `workloadProfileName`. Those clauses are NOT rendered here, for a
/// reason about this seam rather than about the rendering: the framework hands it a count
/// (`<n> items`) rather than the collection, so there is nothing to render FROM — while
/// the one member whose refusal this service does quote is reported by the HANDLER, which
/// has the request in hand (`crate::state::rendered_batch_write_request_items`, the frame
/// those fields belong to). Two recorded refusals of the SAME violation class (a
/// `BatchGetItem` key list past the same 100 cap) state the no-value form this function
/// builds, so a clause raised here states what it has: the member and the bound it broke.
/// `tests/all/collection_cap_constraint_reports.rs` pins that over the capped
/// collections of the batched and transactional operations.
fn violation_clause(violation: &Violation, form: ClauseForm) -> Option<String> {
    match &violation.kind {
        // A member the service checks itself: the requirement with nothing echoed, over
        // the set that check states.
        ViolationKind::EnumValue(_) if service_checked_enum_set(&violation.path).is_some() => {
            Some(format!(
                "Failed to satisfy constraint: {}",
                crate::state::enum_value_requirement(service_checked_enum_set(&violation.path)?)
            ))
        }
        ViolationKind::MissingRequiredMember | ViolationKind::EnumValue(_) => Some(
            Violation {
                kind: match &violation.kind {
                    // The admissible set is stated in the order this service states it
                    // ([`stated_enum_order`]), which the framework cannot know because
                    // it reads the model. Nothing else about the clause changes: the
                    // envelope, the path and the echoed value stay the renderer's own.
                    ViolationKind::EnumValue(allowed) => {
                        ViolationKind::EnumValue(stated_enum_order(allowed))
                    }
                    other => other.clone(),
                },
                ..violation.clone()
            }
            .message(),
        ),
        // Every bound the validating walk checks — a `@range`, a `@length`, a
        // `@pattern`, `@uniqueItems` — in the served operation's form.
        _ => match form {
            ClauseForm::PathAndValue => Some(as_stated(violation).message()),
            ClauseForm::WireMemberOnly | ClauseForm::ValidatorMemberOnly => {
                Some(crate::state::constraint_clause(
                    &form.stated_path(&violation.path),
                    None,
                    &requirement(&violation.kind)?,
                ))
            }
        },
    }
}

/// `violation` with the one requirement this service states differently from the
/// framework's renderer: a `@pattern`, whose expression is stated WITHOUT the anchoring
/// the model spells it with (`crate::state::pattern_requirement` states the rule and is
/// the other half of it — the clause form that echoes no value already goes through it).
/// Everything else about the violation, and every other requirement, is the renderer's
/// own.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            kind: ViolationKind::Pattern(
                ::service_common::validation::unanchored(pattern).to_string(),
            ),
            ..violation.clone()
        },
        _ => violation.clone(),
    }
}

/// The member whose violation is reported ALONE, masking every other violation the same
/// request carries, named by the path the validating walk reports it under.
///
/// A request's IDEMPOTENCY TOKEN is read before the request it stands for is: the token
/// is what tells one attempt from a retry of it, so a request whose token is outside its
/// declared bound cannot be placed against the attempts that came before it at all, and
/// the refusal states that one violation rather than listing the other members beside
/// it. The member is the one the model marks as the token (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#TransactWriteItemsInput")` gives `ClientRequestToken`
/// with `idempotencyToken: true` and `@length` 1..36); no other member of this service
/// carries that trait, so this is the only entry.
///
/// Every other refusal states one clause per violated member inside the envelope that
/// counts them ([`validation_envelope`]).
const MASKING_MEMBERS: &[&str] = &["clientRequestToken"];

/// The violation that masks the rest, when this refusal carries one.
fn masking_violation(violations: &[Violation]) -> Option<&Violation> {
    violations
        .iter()
        .find(|violation| MASKING_MEMBERS.contains(&violation.path.as_str()))
}

/// The `ValidationException` message for a framework refusal: the same envelope
/// `crate::state::validation_message` builds for the bounds this service checks
/// itself, over one clause per violated member.
///
/// A member [`SERVICE_REQUIRED_MEMBERS`] names takes precedence over every clause on the
/// operations that state it as their own sentence, because the check that finds it absent
/// is not the validating walk; on the operations that state the walk's clause for it
/// ([`VALIDATOR_NULL_CLAUSE_OPERATIONS`]) it is a clause like any other.
///
/// `None` declines the whole refusal — see [`requirement`].
fn validation_envelope(violations: &[Violation]) -> Option<String> {
    if let Some(absent) = missing_service_required_member(violations) {
        if !served_states_absent_member_as_clause() {
            return Some(crate::state::missing_parameter_message(&absent));
        }
    }
    // A violated idempotency token is stated alone ([`MASKING_MEMBERS`]).
    if let Some(masking) = masking_violation(violations) {
        return crate::state::validation_message([violation_clause(
            masking,
            served_clause_form_for(&masking.path),
        )?]);
    }
    let clauses: Vec<String> = in_stated_member_order(violations)
        .into_iter()
        .map(|violation| violation_clause(violation, served_clause_form_for(&violation.path)))
        .collect::<Option<Vec<_>>>()?;
    // A refusal about nothing but the members this service checks itself is stated bare
    // by the operations that state such a clause bare ([`BARE_CLAUSE_OPERATIONS`]), and
    // inside the envelope by every other operation. Anything else — a bound the
    // validating walk found, or several members at once — is stated inside the envelope
    // whichever operation was served.
    if let [clause] = clauses.as_slice() {
        if all_service_checked_enums(violations) && served_states_clause_bare() {
            return Some(clause.clone());
        }
    }
    crate::state::validation_message(clauses)
}

/// The inputs whose refusal states its members' clauses in an order of its OWN, each
/// written as the paths the validating walk reports, in the order the service states
/// them.
///
/// The order several clauses appear in is the order the service validated the input's
/// members, which is a property of that input and not of the model: the walk reports
/// them in the order the model declares them, and for the inputs below the service is
/// observed to state the same members in a different one — `ListTables` states its page
/// SIZE before its page CURSOR, while the model declares the cursor first. An input this
/// list does not name keeps the order the walk reported, because that is the order the
/// service is observed to state it in.
///
/// Nothing about the model distinguishes the two groups — the same members, a different
/// order — so this is the only place the difference is recorded, and an input later found
/// to be stated in an order of its own needs its own entry here.
/// `tests/all/required_member_refusals.rs` reads it back over the operation that carries
/// it, so the difference is visible rather than silent.
const STATED_MEMBER_ORDERS: &[&[&str]] = &[
    // `ListTablesInput`, which the model declares as `ExclusiveStartTableName` then
    // `Limit` (`smithy_query(query="shape",
    // shape="com.amazonaws.dynamodb#ListTablesInput")`).
    &["limit", "exclusiveStartTableName"],
    // The capacity pair a table declaration or an update carries, which the model declares
    // as `ReadCapacityUnits` then `WriteCapacityUnits` (`smithy_query(query="shape",
    // shape="com.amazonaws.dynamodb#ProvisionedThroughput")`): the service states the
    // WRITE dimension's clause first.
    &[
        "provisionedThroughput.writeCapacityUnits",
        "provisionedThroughput.readCapacityUnits",
    ],
];

/// `violations` in the order their clauses are stated: the order
/// [`STATED_MEMBER_ORDERS`] publishes for the input they are all members of, and
/// otherwise the order the walk reported.
///
/// An entry applies only when it names EVERY violated member, so a refusal that mixes a
/// member the entry knows with one it does not keeps the walk's order rather than
/// guessing where the unknown member belongs.
fn in_stated_member_order(violations: &[Violation]) -> Vec<&Violation> {
    let mut stated: Vec<&Violation> = violations.iter().collect();
    let Some(order) = STATED_MEMBER_ORDERS.iter().find(|order| {
        violations
            .iter()
            .all(|violation| order.contains(&violation.path.as_str()))
    }) else {
        return stated;
    };
    stated.sort_by_key(|violation| {
        order
            .iter()
            .position(|member| *member == violation.path.as_str())
            .unwrap_or(order.len())
    });
    stated
}

/// The required members this service's OWN request code demands, named by the path the
/// framework's validating walk reports them under.
///
/// Such a member is not left to that walk: this service resolves and bounds the value
/// itself (the table-name member is checked against the 3..=255 `[a-zA-Z0-9_.-]+` form
/// the public model does not carry — `crate::state::resolve_table_name`), so a request
/// that carries no value for it is refused by that check, with the service's own
/// parameter-required sentence instead of the walk's `Member must not be null` clause —
/// except on the operations that are observed to state that clause for it anyway
/// ([`VALIDATOR_NULL_CLAUSE_OPERATIONS`]). Every other required member keeps the walk's
/// clause, because the walk is what finds it absent (`Value null at 'streamArn' …` for an
/// absent `EnableKinesisStreamingDestination$StreamArn`).
///
/// Nothing about the model distinguishes the groups — every member is `@required` — so this
/// list is the only place the difference is recorded, and a member later found to be
/// refused the same way needs its own entry here.
/// `tests/all/required_member_refusals.rs` pins the groups side by side so the
/// difference is visible rather than silent.
const SERVICE_REQUIRED_MEMBERS: &[&str] = &[crate::state::TABLE_NAME_MEMBER_PATH];

/// The member of [`SERVICE_REQUIRED_MEMBERS`] this refusal found absent, named as the
/// model spells it on the wire, or `None` when the refusal is about anything else.
///
/// One request can omit several members; the one reported is the first in the order the
/// refusal states them, which is the order the input's members were read.
fn missing_service_required_member(violations: &[Violation]) -> Option<String> {
    violations
        .iter()
        .find(|violation| {
            matches!(violation.kind, ViolationKind::MissingRequiredMember)
                && SERVICE_REQUIRED_MEMBERS.contains(&violation.path.as_str())
        })
        .map(|violation| wire_member_path(&violation.path))
}

/// The service key `::emulator_server::service_from_target` reads out of a
/// DynamoDB STREAMS request's `X-Amz-Target` header. That service signs as
/// `dynamodb` too, so this key is what tells its traffic apart from ours: the
/// shared reader takes the segment before the `.`, strips the `_<version>`
/// suffix and lower-cases it, so `DynamoDBStreams_20120810.<Op>` normalises to
/// this key while `DynamoDB_20120810.<Op>` normalises to `dynamodb`. The streams
/// crate's own `can_handle` compares against the same key, so one discriminator
/// is read one way on both sides.
const STREAMS_TARGET_SERVICE_KEY: &str = "dynamodbstreams";

/// The two codes this service answers a request the emulator's authentication layer refused
/// with (see [`service_common::authentication_refusal`], which owns the rule and the
/// sentences): the `awsJson` unknown-credential code for an access key id that resolves to
/// no credential, and the `awsJson` signature code for one that resolves and whose signature
/// does not verify. Neither is `AccessDenied`, which is this service's answer to a KNOWN
/// principal whose policies do not permit the action.
const AUTHENTICATION_REFUSAL: StatedAuthenticationRefusal = StatedAuthenticationRefusal {
    unresolved_credential: "UnrecognizedClientException",
    signature_mismatch: "InvalidSignatureException",
};

/// The `com-amazonaws-dynamodb-dynamodb-20120810` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers, and inside that
    /// the layer that states an authentication refusal in this service's own codes
    /// ([`AUTHENTICATION_REFUSAL`]).
    entry: RequestIdEnvelopeDefinition<
        AuthenticationRefusalDefinition<crate::ServiceDefinition<Eng>>,
    >,
    /// The dispatch `override_operation` hands back, which carries the served
    /// operation's clause form into [`Service::override_refusal`] and names the request
    /// on the response. Held on the service because that seam returns a borrow of
    /// `self`.
    clause_form_scope: OperationScopedClauseForm<Eng>,
    /// The endpoint's own HTTP surface, served in place of an operation for a request no
    /// operation of this service serves (see [`front_door`]). Held here for the same
    /// reason: that seam returns a borrow of `self`.
    endpoint_surface: front_door::EndpointSurface,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::reporting(
                AuthenticationRefusalDefinition::new(
                    crate::ServiceDefinition::new(Arc::clone(&engine)),
                    AUTHENTICATION_REFUSAL,
                ),
                reported,
            ),
            clause_form_scope: OperationScopedClauseForm { engine },
            endpoint_surface: front_door::EndpointSurface,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // DynamoDB and DynamoDB Streams are two services behind ONE SigV4 signing
        // name (`dynamodb`), which the model cannot express, so the default
        // signing-name match makes BOTH claim every DynamoDB-signed request and the
        // router answers `500 AmbiguousService` for all of it. On awsJson1_0 the
        // request names its service in `X-Amz-Target`, so a
        // `DynamoDBStreams_20120810.*` target belongs to the streams service and is
        // declined here.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                ::emulator_server::service_from_target(&parts.headers).as_deref()
                    != Some(STREAMS_TARGET_SERVICE_KEY)
            }
            _ => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The endpoint's own HTTP surface comes first, because it is what a request meets
        // before any operation of this model is named: the root's method surface and the
        // media types the API is served under are decided from the request line and the
        // headers alone (see [`front_door`]), and a request this endpoint routes nothing
        // for is answered there rather than by an operation.
        if front_door::unserved(req).is_some() {
            return Some(&self.endpoint_surface);
        }
        // Otherwise serve the operation the generated routing table names, through the
        // scope that carries its clause form into `override_refusal` (see "Which form a
        // constraint clause takes"). The routing decision itself is NOT overridden:
        // the key is resolved by the model's own protocol, and a request the table
        // cannot serve is declined here so the front door's refusal below answers it.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        Some(&self.clause_form_scope)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // DynamoDB words such a refusal as a `ValidationException` (HTTP 400 —
        // the status and code the framework would also have answered, so only
        // the wording changes) carrying the `<N> validation error(s) detected: `
        // envelope over one clause per violated member, each clause naming the
        // member and the constraint it failed in the form the SERVED OPERATION
        // states its clauses in (see "Which form a constraint clause takes"). An
        // authentication refusal is declined here — which failure it was is not in the
        // structural refusal this seam is handed — and worded by the definition layer
        // instead (see "Wording a request authentication refused").
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: validation_envelope(violations)?,
            }),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no modeled
        // operation — neither `override_operation` nor the generated routing table
        // resolved it. For this endpoint that is one thing: an `x-amz-target` naming no
        // operation the front door binds, including one that does not parse as
        // `Service.Operation` at all. The front door refuses it in its OWN vocabulary
        // ([`front_door::unknown_operation_response`]) rather than in the framework's
        // "modelled but not registered" wording, because no operation was identified and
        // this endpoint has no notion of an operation it declines to serve.
        //
        // A request the endpoint's own surface owns never reaches this seam:
        // `override_operation` is asked first and claims it there.
        Some(reported(front_door::unknown_operation_response(req)))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clause_form, stated_enum_order, states_absent_member_as_clause,
        states_service_checked_clause_bare, ClauseForm, BARE_CLAUSE_OPERATIONS,
        ITEM_REQUEST_FORM_OPERATIONS, PUBLISHED_ENUM_ORDERS, VALIDATOR_NULL_CLAUSE_OPERATIONS,
        VALIDATOR_PATH_FORM_OPERATIONS,
    };

    /// The admissible sets as the walk hands them over.
    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn a_published_order_is_matched_on_membership_not_position() {
        for published in PUBLISHED_ENUM_ORDERS {
            // Whatever order the model declared the same values in — here the
            // published order reversed, and the published order itself — the set is
            // stated in the one this service publishes.
            let mut reversed = owned(published);
            reversed.reverse();
            for declared in [owned(published), reversed] {
                let stated = stated_enum_order(&declared);
                assert_eq!(
                    stated,
                    owned(published),
                    "case {declared:?}: expected the published order {published:?}, got {stated:?}"
                );
            }
        }
    }

    #[test]
    fn a_set_with_no_published_order_keeps_the_order_it_arrived_in() {
        // `ReturnConsumedCapacity` and `Select`, whose recorded refusals state the
        // model's own declaration order, plus a subset of a published set (same values,
        // one missing), which is a different set and not that enum.
        for declared in [
            owned(&["INDEXES", "TOTAL", "NONE"]),
            owned(&[
                "ALL_ATTRIBUTES",
                "ALL_PROJECTED_ATTRIBUTES",
                "SPECIFIC_ATTRIBUTES",
                "COUNT",
            ]),
            owned(&["OLD_IMAGE", "KEYS_ONLY", "NEW_IMAGE"]),
        ] {
            let stated = stated_enum_order(&declared);
            assert_eq!(
                stated, declared,
                "case {declared:?}: expected the order it arrived in, got {stated:?}"
            );
        }
    }

    #[test]
    fn every_published_order_states_each_value_once() {
        for published in PUBLISHED_ENUM_ORDERS {
            let mut sorted = published.to_vec();
            sorted.sort_unstable();
            let before = sorted.len();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                before,
                "case {published:?}: expected each value stated once, got {sorted:?}"
            );
        }
    }

    #[test]
    fn bare_clause_operations_are_sorted() {
        let mut sorted = BARE_CLAUSE_OPERATIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            BARE_CLAUSE_OPERATIONS,
            sorted.as_slice(),
            "case the bare-clause list: expected it sorted for `binary_search`, got \
             {BARE_CLAUSE_OPERATIONS:?}"
        );
        for operation in BARE_CLAUSE_OPERATIONS {
            assert!(
                states_service_checked_clause_bare(operation),
                "case {operation:?}: expected the bare form, got the envelope"
            );
        }
        for operation in ["PutItem", "UpdateItem", "DeleteItem", "GetItem", "CreateTable"] {
            assert!(
                !states_service_checked_clause_bare(operation),
                "case {operation:?}: expected the envelope, got the bare form"
            );
        }
    }

    #[test]
    fn validator_null_clause_operations_are_sorted() {
        let mut sorted = VALIDATOR_NULL_CLAUSE_OPERATIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            VALIDATOR_NULL_CLAUSE_OPERATIONS,
            sorted.as_slice(),
            "case the null-clause list: expected it sorted for `binary_search`, got \
             {VALIDATOR_NULL_CLAUSE_OPERATIONS:?}"
        );
        for operation in VALIDATOR_NULL_CLAUSE_OPERATIONS {
            assert!(
                states_absent_member_as_clause(operation),
                "case {operation:?}: expected the walk's clause, got the service's sentence"
            );
        }
    }

    #[test]
    fn item_request_form_operations_are_sorted() {
        for list in [ITEM_REQUEST_FORM_OPERATIONS, VALIDATOR_PATH_FORM_OPERATIONS] {
            let mut sorted = list.to_vec();
            sorted.sort_unstable();
            assert_eq!(
                list,
                sorted.as_slice(),
                "case the clause-form list {list:?}: expected it sorted for `binary_search`"
            );
        }
    }

    #[test]
    fn the_served_operation_chooses_the_clause_form() {
        for operation in ITEM_REQUEST_FORM_OPERATIONS {
            let form = clause_form(operation);
            assert_eq!(
                form,
                ClauseForm::WireMemberOnly,
                "case {operation:?}: expected the item-request form, got {form:?}"
            );
        }
        for operation in VALIDATOR_PATH_FORM_OPERATIONS {
            let form = clause_form(operation);
            assert_eq!(
                form,
                ClauseForm::ValidatorMemberOnly,
                "case {operation:?}: expected the validator-path form, got {form:?}"
            );
        }
        for operation in [
            "ListBackups",
            "ListExports",
            "UpdateTimeToLive",
            "CreateTable",
            "DescribeTable",
        ] {
            let form = clause_form(operation);
            assert_eq!(
                form,
                ClauseForm::PathAndValue,
                "case {operation:?}: expected the request-validator form, got {form:?}"
            );
        }
    }
}
