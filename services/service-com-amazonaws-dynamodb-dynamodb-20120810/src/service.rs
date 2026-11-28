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
//! ## The Region gate
//!
//! A DynamoDB endpoint is REGIONAL, and a Region the account has not turned on serves it
//! nothing: every request addressed to such an endpoint is refused ahead of every operation,
//! rather than answered with an empty result, so a client can tell a Region it cannot use
//! from one that merely holds nothing of its own. Which Regions are on for the account is
//! the Account API's own state, read through [`crate::state::region_is_enabled_for_account`],
//! and the refusal is worded by [`region_refusal`] at the one layer every request this
//! service serves passes through ([`OperationScopedClauseForm`]).
//!
//! ## Reading the request before validating it
//!
//! A request's body is READ into the modelled input before any rule about its values is
//! checked, and three things fail that reading rather than a rule: bytes that are not ONE
//! complete JSON object, which is no body of this protocol at all; a value whose JSON class
//! its member cannot be read from; and an explicit `null` standing for a member the input
//! requires, where a `null` on any other member simply states that member's absence. Each is
//! the protocol's own marshalling failure, a different error class from a constraint
//! violation and worded in a different body member. That layer lives in [`request_layer`] and
//! runs at the same point the Region gate does, ahead of every operation
//! ([`OperationScopedClauseForm`]).
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
//!
//! What was at fault in such a request is not always the CREDENTIAL, though: a request that
//! presented no signature at all, one whose presentation is missing components, one that
//! presented it in both forms at once, and one whose signature does not verify are faults in
//! the REQUEST, and AWS's SigV4 front door answers each of them in its own class with the
//! status a request fault carries. That layer lives in [`request_signature`] and is wired at
//! the same definition seam, outside the one above (so the identity failures it declines fall
//! through to it), and — for the one fault a verifier that reads a single form cannot see —
//! ahead of every operation ([`OperationScopedClauseForm`]).
//!
//! ## The audit leg: what every served call is recorded as
//!
//! The definition this file hands back is also where this front door's AUDIT leg hangs: the
//! generated entry is reached through the shared management-event recorder
//! ([`management_events`]), so every CONTROL-PLANE call this service serves — accepted and
//! refused alike — is recorded in the account's event history where CloudTrail's
//! `LookupEvents` reports it, through one code path for every operation rather than a
//! recording written into each handler. The calls on the ITEMS a table holds are the other
//! CloudTrail class: they are DATA events, recorded in no event history and delivered only
//! into the bucket of a trail whose selectors admit them ([`data_events`]).

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

/// Which bulk-operation targets the table this service routes through binds — read back at
/// the layer that routes them, because a target missing from it is answered as an operation
/// this endpoint does not have.
#[cfg(test)]
mod bulk_operation_management;

/// How a request whose SigV4 signature PRESENTATION is at fault is answered — unsigned,
/// incomplete, presented twice, or carrying a signature that does not verify — which is a
/// fault in the request rather than in the identity it named.
mod request_signature;

/// The `AWS/DynamoDB` `SuccessfulRequestLatency` series this service publishes for each
/// table request it serves, under the `{TableName, Operation}` dimension pair that family is
/// reported on.
mod request_latency_metrics;

/// The `AWS/DynamoDB` `ThrottledRequests` series this service publishes for each request a
/// provisioned-throughput ceiling turned down, under the same `{TableName, Operation}` pair.
mod throttled_request_metrics;

/// The input constraints this service checks ITSELF — the ones the model declares no trait
/// for — read off the request body so their clauses join the walk's in one refusal.
mod request_constraints;

/// The datatype defect of an `AttributeValue` a request sends, for the members that state
/// that defect behind a sentence of their own — read off the body, because a value the codec
/// cannot read never reaches a handler that could name its member.
mod attribute_value_members;

/// The layer BEFORE every constraint: a body value whose JSON class the member it is bound
/// to cannot be read from, which is a marshalling failure rather than a constraint
/// violation and is answered as a different error class.
pub mod request_layer;

/// This front door's AUDIT leg: every control-plane call it serves is a management event of
/// this service, recorded in the account's event history where CloudTrail reads it.
pub mod management_events;

/// The other CloudTrail class: the calls on the ITEMS a table holds, which are data events
/// of that table rather than management events of the account.
pub mod data_events;

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
/// request with, named by the shape id the wire carries for that class
/// ([`crate::scaffolded::VALIDATION_SHAPE_ID`] — the same id an operation's own
/// refusal names, because it is the same class whichever layer decided it). Restated
/// here because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = crate::scaffolded::VALIDATION_SHAPE_ID;

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
/// constraint: Member must not be null` — the walk's clause inside the counting envelope,
/// naming the member by the lowerCamel path that walk reports — while every other
/// operation answers the reader's own sentence. Nothing about the model distinguishes the
/// two groups — every one of these operations declares the member `@required`
/// (`smithy_query(query="operation", operation="DescribeTable")` gives `TableName`
/// required, target `TableArn`, `@length` 1..1024, exactly as `DeleteTable` and `Scan`
/// declare it) — so this list is the only place the difference is recorded, and an
/// operation later found to state the clause needs its own entry here.
/// `tests/all/required_member_refusals.rs` pins both groups side by side, and
/// `tests/all/absent_table_name_refusal_wording.rs` drives the clause over the forms an
/// absence takes, so the difference is visible rather than silent.
///
/// Sorted, which [`states_absent_member_as_clause`] relies on for `binary_search` and
/// `validator_null_clause_operations_are_sorted` pins.
const VALIDATOR_NULL_CLAUSE_OPERATIONS: &[&str] = &["DeleteTable", "DescribeTable", "Scan"];

/// Whether `operation` states an absent such member as the walk's clause.
fn states_absent_member_as_clause(operation: &str) -> bool {
    VALIDATOR_NULL_CLAUSE_OPERATIONS
        .binary_search(&operation)
        .is_ok()
}

/// The operations whose request body is read by a reader that words an absent member as
/// its OWN failure to read one — the member it ran out of, and where in the body the read
/// stopped ([`crate::state::missing_field_message`]) — rather than as this service's
/// parameter-required sentence or the validating walk's clause.
///
/// The VECTOR-search surface is read that way: its refusal of a request that carries no
/// value for a member its input requires states neither the sentence
/// ([`SERVICE_REQUIRED_MEMBERS`]) nor the clause
/// ([`VALIDATOR_NULL_CLAUSE_OPERATIONS`]), but the reader's own sentence with the position
/// in the body it stopped at — a position only a reader of the body has, which is why that
/// position is read off the request while it is still in hand
/// ([`request_constraints::body_read_end`]).
///
/// Nothing about the model distinguishes this surface — the member is `@required` exactly
/// as every other operation's is (`smithy_query(query="operation",
/// operation="SearchVectors")` gives `TableName`, `IndexName`, `SearchVector` and `TopK`
/// required) — so this list is the only place the difference is recorded, and an operation
/// later found to be read that way needs its own entry here.
/// `tests/all/refusal_precedence_and_body_reader.rs` pins the three readings side by
/// side, so the difference is visible rather than silent.
///
/// Sorted, which [`reads_body_with_its_own_reader`] relies on for `binary_search` and
/// `body_reader_operations_are_sorted` pins.
const BODY_READER_OPERATIONS: &[&str] = &["SearchVectors"];

/// Whether `operation`'s body is read by such a reader.
fn reads_body_with_its_own_reader(operation: &str) -> bool {
    BODY_READER_OPERATIONS.binary_search(&operation).is_ok()
}

/// The operations whose own request code reads the members of an ARN-ADDRESSED TAGGING
/// call — the whole tagging surface — so the refusal of one of those members is the
/// sentence THAT check states and not the validating walk's clause about it.
///
/// Two members, each with its own check:
///
/// * `ResourceArn`, the surface's whole selector, resolved by the service's table-ARN check
///   ([`crate::state::resolve_resource_arn`]) rather than by the bounds the model declares
///   on the member. Whatever is wrong with the value, the refusal states the ARN rule it
///   broke: the class sentence ([`crate::state::INVALID_TABLE_ARN`]) alone when the request
///   carries no value to name, and otherwise the sentence that check words for the value
///   (`crate::state::invalid_resource_arn_message`) — observed for all three operations,
///   both for a request that supplies no value and for one whose value is outside the
///   member's declared `@length` bound;
/// * the required TAG LIST a write or a removal states its entries in
///   ([`REQUIRED_TAG_LISTS`]).
///
/// A violation the walk reports against either member is therefore RESTATED here instead of
/// being stated as that walk's clause: the walk refuses the request before the handler runs,
/// so this seam is the only place the service's own checks can answer for it.
///
/// The resource-POLICY surface shares the `ResourceArn` member and not the vocabulary: it
/// addresses "a table or stream" (the `PutResourcePolicy` doc) and words its own refusals of
/// a value that is neither (`tests/all/resource_policy_arn_scope_refusals.rs`), so it keeps
/// the walk's clause.
///
/// Sorted, which [`checks_its_own_members`] relies on for `binary_search` and
/// `self_checked_member_operations_are_sorted` pins.
const SELF_CHECKED_MEMBER_OPERATIONS: &[&str] =
    &["ListTagsOfResource", "TagResource", "UntagResource"];

/// Whether `operation` reads those members through its own checks.
fn checks_its_own_members(operation: &str) -> bool {
    SELF_CHECKED_MEMBER_OPERATIONS
        .binary_search(&operation)
        .is_ok()
}

/// The operations whose `TableName` NAMES the table the request is creating, so the value
/// is held to the table-name form itself and not to the bound the model declares on the
/// member.
///
/// `TableName` targets the shape that also carries a table's ARN — "You can also provide
/// the Amazon Resource Name (ARN) of the table in this parameter"
/// (`CreateTable$TableName`), which is why the member admits far longer and far shorter
/// values than a NAME does. A request CREATING a table states the name the table will be
/// addressed by from then on, and that name has its own form
/// ([`crate::state::table_name_form_clauses`]); so a bound the validating walk reports on
/// that member is RESTATED as every way the submitted name breaks that form, and states
/// nothing of any other member — the name is the whole selector the created resource will
/// be addressed by, exactly as the ARN-addressed surface's own selector is
/// ([`SELF_CHECKED_MEMBER_OPERATIONS`]).
///
/// The operations that RESOLVE a table the account already holds are not here: each of
/// them accepts the ARN spelling of an existing table, so the bound its member declares is
/// the one a caller is held to, and the walk's clause states it.
///
/// Sorted, which [`holds_table_name_to_its_form`] relies on for `binary_search` and
/// `table_name_form_operations_are_sorted` pins.
const TABLE_NAME_FORM_OPERATIONS: &[&str] = &["CreateTable"];

/// Whether `operation` holds its `TableName` to the table-name form.
fn holds_table_name_to_its_form(operation: &str) -> bool {
    TABLE_NAME_FORM_OPERATIONS.binary_search(&operation).is_ok()
}

/// The REQUIRED TAG LISTS of that same surface, each as the member path the validating walk
/// reports it under and the entry its own sentence names
/// (`crate::state::no_tag_entries_message`).
///
/// A tag write and a tag removal state the entries they ask for in such a list, so the
/// service reads the list itself: a request that states NO entry of it asks for nothing and
/// is refused by that check — observed to be the same sentence whether the request carried
/// an EMPTY list or no list at all, which is why the absence is restated here rather than
/// left to the walk's `Member must not be null` clause. The empty half is the handler's,
/// because a list the request did carry reaches it
/// (`crate::state::check_tag_entries_stated`).
///
/// Read only for the operations above, so the `Tags` member another operation carries as an
/// OPTIONAL list (`CreateTable`, where stating none of them is a table with no tags) cannot
/// reach it.
const REQUIRED_TAG_LISTS: &[(&str, &str)] = &[
    ("tagKeys", crate::state::TAG_KEY_LIST_ENTRY),
    ("tags", crate::state::TAG_LIST_ENTRY),
];

/// The enum value sets this service STATES in an order of its own, each written in the
/// order it states them.
///
/// The bracketed set a constraint clause ends with is the admissible set as the SERVICE
/// words it, which is a property of the enum and not of the model: the generic renderer
/// iterates the model's declaration order, and for the enums below the service is
/// observed to state the same values in a different one. An enum this list does not name
/// keeps the model's order, because that is the order the service is observed to state it
/// in (`ReturnConsumedCapacity` as `INDEXES, TOTAL, NONE`, the declared order —
/// `tests/all/validation_message_wording.rs` pins it).
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
    // `ExportTableToPointInTime$ExportFormat`, which the model declares as
    // `DYNAMODB_JSON ION`
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ExportFormat")`), while
    // the service's refusals state the two in this one.
    &["ION", "DYNAMODB_JSON"],
    // `AttributeDefinition$AttributeType`, the scalar type a key attribute is defined with,
    // which the model declares as `S N B`
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ScalarAttributeType")`),
    // while the service's refusals state the same three in this one.
    &["B", "N", "S"],
    // `ReturnValue`, whose set the service is observed to state in THIS order; the model's
    // own declaration order is a different one
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ReturnValue")`), and what
    // the literal below holds is the order the refusal STATES, not that declaration.
    &["ALL_NEW", "UPDATED_OLD", "ALL_OLD", "NONE", "UPDATED_NEW"],
    // `Select`, which a keyed read and a table walk both carry and the model declares as
    // `ALL_ATTRIBUTES ALL_PROJECTED_ATTRIBUTES SPECIFIC_ATTRIBUTES COUNT`
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#Select")`), while the
    // service's refusals state the same four in this one.
    &[
        "SPECIFIC_ATTRIBUTES",
        "COUNT",
        "ALL_ATTRIBUTES",
        "ALL_PROJECTED_ATTRIBUTES",
    ],
    // `Condition$ComparisonOperator`, the comparator a LEGACY filter or key condition states
    // its operator in, which the model declares as `EQ NE IN LE LT GE GT BETWEEN NOT_NULL
    // NULL CONTAINS NOT_CONTAINS BEGINS_WITH`
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ComparisonOperator")`),
    // while the service's refusals state the same thirteen in this one.
    &[
        "IN",
        "NULL",
        "BETWEEN",
        "LT",
        "NOT_CONTAINS",
        "EQ",
        "GT",
        "NOT_NULL",
        "NE",
        "LE",
        "BEGINS_WITH",
        "GE",
        "CONTAINS",
    ],
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
//     must have value greater than or equal to 1` for an item read's page size
//     below its `@range` minimum, `Value at 'TotalSegments' …` for a table walk's
//     parallel-scan bounds, and `Value at 'RequestItems.<table>.member.Keys' …`
//     for a batched key list past its cap;
//   * the VALIDATOR-PATH form, which echoes no value either but names the member
//     by the lowerCamel path the request validator builds — `Value at
//     'requestItems' …` for a `BatchWriteItem` request map past either end of its
//     `@length` bound;
//   * the REQUEST-VALIDATOR form, which names the member by that same lowerCamel
//     path and DOES echo the rejected value — `Value '0' at
//     'limit' …` for a control-plane listing's page size, `Value
//     '26' at 'maxResults' …` for an export listing's, `Value '' at
//     'timeToLiveSpecification.attributeName' …` for an empty TTL attribute name.
//
// The SAME violation class on the SAME member name is observed in two of them
// (`Limit`, `@range` min 1, in the item-request form on both item reads and in the
// request validator's form on the control-plane listings; `RequestItems`, `@length`
// min 1, on `BatchGetItem` and on `BatchWriteItem`), so
// nothing about the model or about the violation distinguishes them: the form is a
// fact of the served OPERATION and of the MEMBER the clause is about, which is why
// [`CLAUSE_FORMS`] is ONE table recording, per operation, the form its clauses take
// and the members of its own input recorded in another form — the only place that is
// recorded — and every operation the table does not name states the request
// validator's own form.
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

/// One row of [`CLAUSE_FORMS`]: the operation, the form its clauses take, and the members
/// of its own input it is recorded to state in ANOTHER form.
type ClauseFormRow = (
    &'static str,
    ClauseForm,
    &'static [(&'static str, ClauseForm)],
);

/// No member of this operation is stated in another form.
const EVERY_MEMBER_ALIKE: &[(&str, ClauseForm)] = &[];

/// The operations whose refusal of a value outside a bound the model declares is observed to
/// take a form other than the request validator's own: one row per operation, naming the form
/// that operation's clauses take and the members of its input recorded in another form.
///
/// This is the ONE place the choice is recorded. Nothing in the model distinguishes these
/// sentences — the same member name, the same trait, a different sentence — and the choice is
/// not derivable from the violation either: an ITEM READ's page size and a control-plane
/// listing's are the same kind of scalar `@range` member
/// (`smithy_query(query="operation", operation="Query,Scan")`: both `Limit` target
/// `PositiveIntegerObject` `@range{min:1}`, while `ListBackups$Limit` targets
/// `BackupsInputLimit`) and the two are nevertheless stated in different forms — the item
/// reads naming the member with nothing echoed — while `BatchGetItem`'s and
/// `BatchWriteItem`'s
/// `RequestItems` maps are the same non-scalar member of the same `@length` bound and are
/// stated in two different forms. So the form is read off the operation being served, with
/// the members that operation is recorded to state otherwise named beside it — an operation
/// or a member later observed to state another form is a row, or an entry in a row's own
/// list. The suites that pin these forms side by side, so a difference is visible rather
/// than silent, are
/// `tests/all/clause_form_per_member.rs`, `tests/all/page_size_clause_forms.rs`,
/// `tests/all/item_request_clause_form.rs`, `tests/all/collection_cap_constraint_reports.rs`,
/// `tests/all/validation_message_fidelity.rs`, `tests/all/validation_message_wording.rs` and
/// `tests/all/index_key_types_and_range_refusals.rs`.
///
/// Every operation this table does not name states [`ClauseForm::PathAndValue`], the request
/// validator's own form, which is also what a refusal raised outside any operation's run
/// states.
const CLAUSE_FORMS: &[ClauseFormRow] = &[
    // A batched key list past its cap is `Value at 'RequestItems.<table>.member.Keys' …`.
    ("BatchGetItem", ClauseForm::WireMemberOnly, EVERY_MEMBER_ALIKE),
    // Its request map past either end of its `@length` bound is `Value at 'requestItems' …`,
    // where `BatchGetItem`'s map of the same name is `Value at 'RequestItems' …`. Nothing
    // NESTED under the map reaches this seam — the write-request list inside each entry is
    // bounded by the HANDLER over the operations the whole map carries
    // (`crate::state::batch_write_requests_outside_bound`) and the map's KEY is the service's
    // own table-name check (`crate::state::batch_table_key`), neither of which is a violation
    // the validating walk reports.
    (
        "BatchWriteItem",
        ClauseForm::ValidatorMemberOnly,
        EVERY_MEMBER_ALIKE,
    ),
    // An item read's page size is `Value at 'Limit' …`, with no value echoed — the keyed
    // read's and the table walk's alike, which is the one form the whole input of either is
    // recorded in. The table walk states its parallel-scan bounds that way too (`Value at
    // 'TotalSegments' …`, `Value at 'Segment' …`), so nothing of an item read is stated in
    // the request validator's form, while a control-plane listing's page size — the same
    // kind of member under the same trait — is.
    ("Query", ClauseForm::WireMemberOnly, EVERY_MEMBER_ALIKE),
    ("Scan", ClauseForm::WireMemberOnly, EVERY_MEMBER_ALIKE),
    // The VECTOR search states its members the same way — `Value at 'TopK' …`, with no
    // value echoed — which is the one form its whole input is recorded in; it is also the
    // surface whose body reader words an absent member itself
    // ([`BODY_READER_OPERATIONS`]).
    ("SearchVectors", ClauseForm::WireMemberOnly, EVERY_MEMBER_ALIKE),
    (
        "TransactGetItems",
        ClauseForm::WireMemberOnly,
        EVERY_MEMBER_ALIKE,
    ),
    (
        "TransactWriteItems",
        ClauseForm::WireMemberOnly,
        EVERY_MEMBER_ALIKE,
    ),
];

/// The forms the clauses of one served operation take: the form its members are stated in,
/// and the members it states otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClauseForms {
    /// The form every member of the operation is stated in unless [`Self::otherwise`]
    /// names it.
    stated: ClauseForm,
    /// The members recorded in another form, by the validator's path.
    otherwise: &'static [(&'static str, ClauseForm)],
}

impl ClauseForms {
    /// The form a clause about the member at `path` takes.
    fn of(self, path: &str) -> ClauseForm {
        self.otherwise
            .iter()
            .find(|(member, _)| *member == path)
            .map_or(self.stated, |(_, form)| *form)
    }
}

/// The forms the refusals of `operation` take ([`CLAUSE_FORMS`]).
fn clause_forms(operation: &str) -> ClauseForms {
    CLAUSE_FORMS
        .iter()
        .find(|(named, _, _)| *named == operation)
        .map_or(
            ClauseForms {
                stated: ClauseForm::PathAndValue,
                otherwise: EVERY_MEMBER_ALIKE,
            },
            |(_, stated, otherwise)| ClauseForms {
                stated: *stated,
                otherwise,
            },
        )
}

tokio::task_local! {
    /// The forms the operation this request is being served as states its clauses in,
    /// set by [`OperationScopedClauseForm::run`] around that operation's whole run and
    /// read back by [`EmulatedService::override_refusal`].
    static SERVED_CLAUSE_FORM: ClauseForms;

    /// Whether that operation states a clause about a member the service checks itself
    /// BARE ([`BARE_CLAUSE_OPERATIONS`]), set and read back the same way.
    static SERVED_BARE_CLAUSE: bool;

    /// Whether that operation states an ABSENT member this service's own request code
    /// requires as the walk's clause ([`VALIDATOR_NULL_CLAUSE_OPERATIONS`]), set and read
    /// back the same way.
    static SERVED_NULL_CLAUSE: bool;

    /// Whether that operation reads an ARN-addressed tagging call's members through its own
    /// checks ([`SELF_CHECKED_MEMBER_OPERATIONS`]), set and read back the same way.
    static SERVED_MEMBER_CHECK: bool;

    /// The clauses the `TableName` of the request this task is serving earns for breaking
    /// the table-name form, for an operation that holds it to that form
    /// ([`TABLE_NAME_FORM_OPERATIONS`]), set and read back the same way.
    static SERVED_TABLE_NAME_CLAUSES: Vec<String>;

    /// The clauses this service's OWN reading of the request found
    /// ([`request_constraints`]), set around the same run and read back by
    /// [`validation_envelope`] so they are counted in the one envelope beside the
    /// violations the validating walk reports.
    static SERVED_SERVICE_CHECKED_CLAUSES: Vec<String>;

    /// The body of the request this task is serving, beside the schema its declared bounds
    /// live in ([`request_constraints::readable_body`]), set around the same run and read
    /// back by [`validation_envelope`]: they are the bounds the validating walk never got
    /// to, and they are worded and counted exactly as that walk's own violations are.
    static SERVED_READABLE_BODY: Option<request_constraints::ReadableBody>;

    /// The clause the capped LIST of the request this task is serving earns for carrying
    /// more elements than the model admits, with the rejected list rendered from the
    /// request ([`request_constraints::capped_list_clause`]), set around the same run and
    /// read back by [`validation_envelope`] in place of the walk's own clause about that
    /// member.
    static SERVED_CAPPED_LIST_CLAUSE: Option<request_constraints::CappedListClause>;

    /// Where the reader of the request this task is serving stops reading its body
    /// ([`request_constraints::body_read_end`]), for an operation whose reader words an
    /// absent member as its own failure to read one ([`BODY_READER_OPERATIONS`]), set
    /// around the same run and read back by [`validation_envelope`]: the position is a
    /// fact of the BODY, which no refusal seam is handed.
    static SERVED_BODY_READ_END: Option<(usize, usize)>;
}

/// The forms the operation this task is serving states its clauses in.
///
/// Absent altogether means the refusal happened outside an operation's run, where
/// there is no served operation to read a form off; the request validator's form is
/// the general one, so that is what such a refusal states.
fn served_clause_form() -> ClauseForms {
    SERVED_CLAUSE_FORM.try_with(|form| *form).unwrap_or(ClauseForms {
        stated: ClauseForm::PathAndValue,
        otherwise: EVERY_MEMBER_ALIKE,
    })
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

/// Whether the operation this task is serving reads an ARN-addressed tagging call's members
/// through its own checks. Absent altogether is a refusal raised outside an operation's run,
/// where there is no served operation to read that off — and the walk's clause is what such
/// a refusal states.
fn served_checks_its_own_members() -> bool {
    SERVED_MEMBER_CHECK
        .try_with(|checked| *checked)
        .unwrap_or(false)
}

/// The clauses the `TableName` of the request this task is serving earns for breaking the
/// table-name form. None at all is a refusal raised outside an operation's run, where there
/// is no request to have read a name off — and the walk's clause is what such a refusal
/// states.
fn served_table_name_clauses() -> Vec<String> {
    SERVED_TABLE_NAME_CLAUSES
        .try_with(Clone::clone)
        .unwrap_or_default()
}

/// The clauses this service's own reading of the request it is serving found. None at all
/// is a refusal raised outside an operation's run, where there is no request to have read.
fn served_service_checked_clauses() -> Vec<String> {
    SERVED_SERVICE_CHECKED_CLAUSES
        .try_with(Clone::clone)
        .unwrap_or_default()
}

/// The violations of the served input's declared bounds this service can still read off the
/// request it is serving. None at all is a refusal raised outside an operation's run, where
/// there is no request to have read. The body is read HERE rather than when it arrived:
/// only a refusal whose deserialization aborted needs it.
fn served_body_violations() -> Vec<Violation> {
    SERVED_READABLE_BODY
        .try_with(|held| {
            held.as_ref()
                .map(request_constraints::ReadableBody::violations)
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// The clause the capped list of the request this task is serving earns for carrying more
/// elements than the model admits. None at all is a refusal raised outside an operation's
/// run, where there is no request to have read a list off — and the walk's own clause is
/// what such a refusal states.
fn served_capped_list_clause() -> Option<request_constraints::CappedListClause> {
    SERVED_CAPPED_LIST_CLAUSE
        .try_with(Clone::clone)
        .unwrap_or_default()
}

/// Where the reader of the request this task is serving stops reading its body. None at
/// all is a refusal raised outside an operation's run, where there is no request to have
/// read a position off — and the answer every operation whose reader states no position
/// gives is the one such a refusal states.
fn served_body_read_end() -> Option<(usize, usize)> {
    SERVED_BODY_READ_END
        .try_with(|held| *held)
        .unwrap_or_default()
}

/// Serves a request as the operation the generated routing table names, with that
/// operation's clause form ([`clause_form`]) in scope for the whole run, and names the
/// request in the response's header (see "Naming the request a response answers").
///
/// It adds two things to that: the endpoint's own gates ahead of every operation — the
/// Region gate and the deserialization layer ([`request_layer`]) — and the reporting every
/// response of this service carries. Past them the request, the service and the context are
/// handed to the modelled operation unchanged, so validation, the handler and the response
/// are the framework's and the operation's own. The routing table holds only generated
/// operations, so the inner lookup can never resolve back to this type.
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
            // The SIGNATURE gate, ahead of everything: a request that presented its
            // signature in BOTH of SigV4's forms names two auth mechanisms where exactly one
            // may be used, and AWS's front door refuses it before the request is served.
            // It is read here because it is the one such fault a verifier cannot raise: the
            // verifier reads ONE form, and a request whose header verifies is served with
            // its query-string presentation never looked at ([`request_signature`], which
            // words every other signature fault at the refusal seam instead).
            if let Some(refusal) = request_signature::two_forms_refusal(req) {
                return front_door::announced(req, reported(refusal));
            }
            // The REGION gate, ahead of every operation: DynamoDB is reached through a
            // REGIONAL endpoint, and a Region the account has not turned on serves it
            // nothing at all — every request to that endpoint is refused rather than
            // answered with an empty result, which is what lets a client tell "this Region
            // is not available to me" from "this Region holds nothing of mine". It belongs
            // here rather than in any one handler because it is a fact of the ENDPOINT the
            // request reached, not of the operation it named
            // ([`crate::state::region_is_enabled_for_account`]).
            if let Some(refusal) = region_refusal(&*self.engine, ctx) {
                return front_door::announced(req, reported(refusal));
            }
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
            // DESERIALIZATION precedes every later layer, and the BODY is where it starts:
            // bytes that are not one complete JSON object are no body of this protocol at
            // all; a value the protocol READS in another form than the one the model declares
            // is read in that form, and the operation is served the request carrying it as
            // read; a member the body carries as an explicit `null` is a member it does not
            // state, so that member is absent from the request served; and a value whose JSON
            // class the modelled member it is bound to cannot be read from never becomes a
            // value at all, so the request is refused as the PROTOCOL's own marshalling
            // failure before the operation's validating walk reaches a constraint and before
            // its handler reads or writes anything. The framework would otherwise render that
            // codec failure in the constraint-violation envelope, which is a different error
            // class, under a different member name, and a different remediation for the
            // caller ([`request_layer`]). The cost is deliberate: a request's body is read
            // twice, once here and once by the dispatch that follows.
            let reading = {
                let protocol = svc.definition().wire_protocol(req);
                request_layer::body_reading::<Eng>(operation, protocol.protocol(), req)
            };
            let req = match &reading {
                request_layer::BodyReading::AsSent => req,
                // The request as this layer READ it — every stated absence absent and every
                // value in the form the protocol reads it in — lives for the call it is read
                // by, which is all the layers below borrow it for.
                request_layer::BodyReading::Absences(read) => read,
                // Which members the refusal's body carries is a fact of the DEFECT: a value
                // no conversion was attempted for is refused with the error class alone.
                request_layer::BodyReading::Unreadable(prose) => {
                    if let Some(refusal) =
                        request_layer::serialization_refusal(svc, req, prose.as_ref())
                    {
                        return front_door::announced(req, reported(refusal));
                    }
                    req
                }
                // A body that breaks the model's own rules before it can be built into an
                // input: a member the input REQUIRES stated `null`, beside every other bound
                // the same body breaks. They are this request's violations, so they are
                // worded and counted exactly as the validating walk's own are
                // ([`validation_envelope`]) — in the form the operation being served states
                // its clauses in, which the scope below is not entered for yet.
                request_layer::BodyReading::Invalid(violations) => {
                    if let Some(message) = body_violation_refusal(operation, req, violations) {
                        return front_door::announced(req, reported(validation_refusal(message)));
                    }
                    req
                }
            };
            // And the DATATYPE of one value the body carries, for the members that state a
            // defect of it behind a sentence of their own: such a value never becomes an
            // input, so the member it stood in is known only while the body is still in hand
            // ([`attribute_value_members`]). Every other value keeps the codec's own refusal,
            // which the dispatch below raises.
            if let Some(message) = attribute_value_members::enveloped_refusal(operation, req) {
                return front_door::announced(req, reported(validation_refusal(message)));
            }
            // And the same rules over every OTHER position an attribute value stands in,
            // which state the fact alone rather than an envelope of a member's own
            // ([`attribute_value_members::value_refusal`]): the union's arity, and a `null`
            // listed as a member of a set. Such a value never becomes an input either \u2014 the
            // codec names the union rather than the rule, or reads one of several datatypes
            // and serves a request AWS refuses \u2014 so it is answered while the body is in hand.
            if let Some(message) = attribute_value_members::value_refusal(operation, req) {
                return front_door::announced(req, reported(validation_refusal(message)));
            }
            // What serving this request TAKES is measured around the operation's own run,
            // which is the elapsed time its latency series reports
            // ([`request_latency_metrics`]).
            let started = ::std::time::Instant::now();
            // The clause a capped list past its maximum earns, read off the request
            // while it is still in hand ([`request_constraints::capped_list_clause`]).
            let capped_list = {
                let protocol = svc.definition().wire_protocol(req);
                request_constraints::capped_list_clause(operation, protocol.protocol(), req)
            };
            let answered = SERVED_CLAUSE_FORM
                .scope(
                    clause_forms(operation),
                    SERVED_BARE_CLAUSE.scope(
                        states_service_checked_clause_bare(operation),
                        SERVED_NULL_CLAUSE.scope(
                            states_absent_member_as_clause(operation),
                            SERVED_MEMBER_CHECK.scope(
                                checks_its_own_members(operation),
                                SERVED_TABLE_NAME_CLAUSES.scope(
                                    request_constraints::table_name_form_clauses(operation, req),
                                    SERVED_SERVICE_CHECKED_CLAUSES.scope(
                                        request_constraints::service_checked_clauses(
                                            operation, req,
                                        ),
                                        SERVED_CAPPED_LIST_CLAUSE.scope(
                                            capped_list,
                                            SERVED_BODY_READ_END.scope(
                                                request_constraints::body_read_end(
                                                    operation, req,
                                                ),
                                                SERVED_READABLE_BODY.scope(
                                                    request_constraints::readable_body(
                                                        operation, req,
                                                    ),
                                                    op.run(svc, req, ctx),
                                                ),
                                            ),
                                        ),
                                    ),
                                ),
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
            // And where a request a provisioned-throughput ceiling turned down — refused
            // outright, or served with part of its work handed back — is counted into the
            // table's `ThrottledRequests` series
            // ([`throttled_request_metrics`]).
            throttled_request_metrics::publish_throttled_request(
                &*self.engine,
                operation,
                req,
                ctx.parts(),
                &answered,
            );
            // And where what the request's outcome WAS is counted: the items a read returned
            // and the writes a condition turned down, settled under the store's lock and
            // published with it released ([`crate::state::request_outcome_metrics`]).
            crate::state::publish_pending_request_outcomes(&*self.engine);
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

/// The `ValidationException` message the violations of a body read BEFORE any operation ran
/// earn ([`request_layer::BodyReading::Invalid`]), worded exactly as the operation being
/// served words the validating WALK's own violations.
///
/// The scopes entered here are the ones that walk's refusal is answered inside
/// ([`OperationScopedClauseForm::run`] sets the same ones around the operation's run), so a
/// body refused before it could be built into an input is refused in the same words as one
/// refused after — the clause form, the members the service checks itself, and the sentences
/// those checks state for them.
///
/// [`SERVED_READABLE_BODY`] is deliberately left unset: what it carries is the bounds read off
/// a body whose walk never ran ([`request_constraints::readable_body`]), and these violations
/// already ARE that whole reading, over every member of the body rather than its top-level
/// strings.
fn body_violation_refusal(
    operation: &str,
    req: &aws_smithy_runtime_api::http::Request,
    violations: &[Violation],
) -> Option<String> {
    SERVED_CLAUSE_FORM.sync_scope(clause_forms(operation), || {
        SERVED_BARE_CLAUSE.sync_scope(states_service_checked_clause_bare(operation), || {
            SERVED_NULL_CLAUSE.sync_scope(states_absent_member_as_clause(operation), || {
                SERVED_MEMBER_CHECK.sync_scope(checks_its_own_members(operation), || {
                    SERVED_TABLE_NAME_CLAUSES.sync_scope(
                        request_constraints::table_name_form_clauses(operation, req),
                        || {
                            SERVED_SERVICE_CHECKED_CLAUSES.sync_scope(
                                request_constraints::service_checked_clauses(operation, req),
                                || {
                                    SERVED_BODY_READ_END.sync_scope(
                                        request_constraints::body_read_end(operation, req),
                                        || validation_envelope(violations),
                                    )
                                },
                            )
                        },
                    )
                })
            })
        })
    })
}

/// `message` as this service's own `ValidationException` (HTTP 400), rendered in the
/// protocol's error envelope — what a request refused AHEAD of any operation is answered
/// with, for a refusal this service words itself rather than one the framework described
/// structurally ([`EmulatedService::override_refusal`] answers those).
///
/// The status and the envelope stay with the protocol, which owns them
/// ([`management_events::JSON_ENVELOPE`]); only the class and the sentence are this
/// service's. No violation is stated, because the chosen wording replaces what the
/// framework would have rendered from one.
fn validation_refusal(message: String) -> aws_smithy_runtime_api::http::Response {
    smithy_server_runtime::protocol_error_response_with(
        management_events::JSON_ENVELOPE,
        ::aws_smithy_schema_server::ProtocolError::Invalid(Vec::new()),
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message,
        }),
    )
}

/// The refusal a request addressed to a Region this account has not turned on is answered
/// with, or `None` when the Region it addresses serves the account
/// ([`crate::state::region_is_enabled_for_account`]).
///
/// The code and the sentence are the ones this service answers a credential that does not
/// resolve with ([`AUTHENTICATION_REFUSAL`] and the sentence
/// `service_common::authentication_refusal` owns): a Region the account has not turned on
/// has no principal to speak for a request addressed to it, so what a caller is answered is
/// that its credential is not valid there rather than an empty result or a denial of the
/// action. The status and the envelope are the protocol's, taken from the framework's own
/// `awsJson` renderer ([`management_events::JSON_ENVELOPE`]) so this refusal is rendered
/// exactly like every other pre-dispatch one rather than by a second hand-written envelope.
///
/// The refusal names no member of the request, because it is about the ENDPOINT the request
/// reached: nothing of the request, and nothing of this account's state, is echoed in it.
fn region_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
) -> Option<aws_smithy_runtime_api::http::Response> {
    let region = crate::state::request_region(ctx.parts());
    if crate::state::region_is_enabled_for_account(
        engine,
        &region,
        ::std::time::SystemTime::now(),
    ) {
        return None;
    }
    Some(smithy_server_runtime::protocol_error_response_with(
        management_events::JSON_ENVELOPE,
        ::aws_smithy_schema_server::ProtocolError::Unauthenticated {
            code: AUTHENTICATION_REFUSAL.unresolved_credential,
            message: ::service_common::authentication_refusal::UNRESOLVED_CREDENTIAL_MESSAGE,
        },
        None,
    ))
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
/// form the member and the bound, with nothing echoed — which is what the transactional
/// `TransactItems` lists past their 100-action maximum state, exactly as the same members'
/// EMPTY case does. ONE recorded AWS refusal of a capped collection — the PartiQL
/// transaction's `TransactStatements` past its maximum — instead
/// echoes the rejected list as the service's OWN object rendering of it
/// (`[ParameterizedStatement(statement=…, parameters=null,
/// returnValuesOnConditionCheckFailure=null)]`). That clause is not rendered BY THIS
/// FUNCTION, for a reason about this seam rather than about the rendering: the framework
/// hands it a count (`<n> items`) rather than the collection, so there is nothing here to
/// render FROM. It is rendered where the REQUEST still is and substituted for this
/// function's clause about that member
/// (`request_constraints::capped_list_clause`), exactly as the one member whose
/// refusal the HANDLER quotes is rendered where that handler has the request
/// (`crate::state::rendered_batch_write_request_items`, the frame that rendering belongs
/// to).
/// Three recorded refusals of the SAME violation class (a `BatchGetItem` key list past the
/// same 100 cap, a transactional list past its maximum, and one below its minimum) state the
/// no-value form this function builds, so a clause raised here states what it has: the member
/// and the bound it broke. `tests/all/collection_cap_constraint_reports.rs` pins that over the
/// capped collections of the batched and transactional operations.
fn violation_clause(violation: &Violation, forms: ClauseForms, alone: bool) -> Option<String> {
    match &violation.kind {
        // A member the service checks itself: the requirement with nothing echoed, over
        // the set that check states. Only where the refusal is about that member alone —
        // a request several of whose members break a declared bound is refused by the
        // validator before any check of the service's own reads it, so every clause of
        // such a refusal is the validator's.
        ViolationKind::EnumValue(_)
            if alone && service_checked_enum_set(&violation.path).is_some() =>
        {
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
        // `@pattern`, `@uniqueItems` — in the form the served operation states THAT member
        // in ([`ClauseForms::of`]).
        _ => match forms.of(&violation.path) {
            ClauseForm::PathAndValue => Some(as_stated(violation).message()),
            ClauseForm::WireMemberOnly | ClauseForm::ValidatorMemberOnly => {
                Some(crate::state::constraint_clause(
                    &forms.of(&violation.path).stated_path(&violation.path),
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
        _ => quoting_an_empty_collection(violation),
    }
}

/// `violation` with the walk's SUMMARY of an empty collection replaced by the collection
/// itself, where the clause echoes a value.
///
/// A clause states the value the request carried, and the validating walk hands this seam
/// its own summary of a collection it length-checked (`<n> items` for a list, `<n>
/// entries` for a map) rather than the collection — neither of which any caller sent. For
/// a collection that HOLDS something there is nothing here to render from (see the
/// paragraph above [`violation_clause`]), but a collection the walk counted as EMPTY has
/// exactly one rendering whatever it held nothing of, so that one is restored: an empty
/// list is quoted as the empty list and an empty map as the empty map, through the shared
/// renderings every service quotes a submitted collection with
/// (`service_common::validation_message::quoting_the_submitted_collection`, which is also
/// what holds the restoring to the two `@length` classes). The requirement half, the path
/// and the envelope stay the framework's.
fn quoting_an_empty_collection(violation: &Violation) -> Violation {
    let Some(stored) = violation.value.as_deref() else {
        return violation.clone();
    };
    let empty = if ::service_common::validation_message::list_item_count(stored) == Some(0) {
        ::service_common::validation::rendered_list(&[])
    } else if ::service_common::validation_message::map_entry_count(stored) == Some(0) {
        ::service_common::validation::rendered_map(&[])
    } else {
        return violation.clone();
    };
    ::service_common::validation_message::quoting_the_submitted_collection(
        violation.clone(),
        ::service_common::validation::SubmittedEcho::Rendered(empty),
    )
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
    // The tagging surface's `ResourceArn` is resolved by the service's own table-ARN check,
    // so a violation the walk reports against that member is restated as the ARN rule it
    // broke ([`ARN_CHECKED_OPERATIONS`]): the sentence that check words for the value, or
    // the class sentence alone when there is no value to name. It comes first, and states
    // nothing of any other member, because the member it is about is the whole selector the
    // request addresses a resource by.
    if served_checks_its_own_members() {
        if let Some(value) = resource_arn_violation(violations) {
            return Some(match value {
                Some(value) => crate::state::invalid_resource_arn_message(value),
                None => crate::state::INVALID_TABLE_ARN.to_string(),
            });
        }
        // The same surface reads its required tag LIST itself, so a request that states no
        // entry of it is refused by that check too ([`REQUIRED_TAG_LISTS`]).
        if let Some(entry) = absent_required_tag_list(violations) {
            return Some(crate::state::no_tag_entries_message(entry));
        }
    }
    // A `TableName` that NAMES the table the request creates is held to the table-name form
    // rather than to the bound the ARN-accepting member declares, so a request whose name
    // breaks that form is refused by every way it breaks it, and states nothing of any
    // other member — the name is the whole selector the created resource will be addressed
    // by ([`TABLE_NAME_FORM_OPERATIONS`]).
    if let Some(refusal) = crate::state::validation_message(served_table_name_clauses()) {
        return Some(refusal);
    }
    // The surface whose body reader words an absent member itself answers that reader's
    // own sentence, naming the member it ran out of and the position the read stopped at
    // ([`BODY_READER_OPERATIONS`]). It comes before the two readings a member's absence is
    // otherwise given, because on this surface neither of them is what found it.
    if let Some((line, column)) = served_body_read_end() {
        if let Some(absent) = absent_required_member(violations) {
            return Some(crate::state::missing_field_message(&absent, line, column));
        }
    }
    if let Some(absent) = missing_service_required_member(violations) {
        if !served_states_absent_member_as_clause() {
            return Some(crate::state::missing_parameter_message(&absent));
        }
    }
    // A request whose table-name member states no name at all names no table to address,
    // and that one clause is the whole refusal ([`unusable_table_name_clause`]).
    if let Some(clause) = unusable_table_name_clause(violations) {
        return crate::state::validation_message([clause]);
    }
    // A violated idempotency token is stated alone ([`MASKING_MEMBERS`]).
    if let Some(masking) = masking_violation(violations) {
        return crate::state::validation_message([violation_clause(
            masking,
            served_clause_form(),
            true,
        )?]);
    }
    // The bounds the validating WALK never got to, read off the body: a member the input
    // REQUIRES and the request does not carry stops the deserializer, so the walk that
    // would have found every other violated bound of the same request never ran, and
    // nothing but that one absence would be reported. Those violations are this request's
    // too, so they are clauses of the same envelope ([`request_constraints`]) — and they
    // are merged only where that is what happened, over the members the walk did not
    // itself report, so a refusal the walk DID raise states the walk's own findings and
    // nothing beside them.
    let read_off_the_body: Vec<Violation> = if violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::MissingRequiredMember))
    {
        served_body_violations()
            .into_iter()
            .filter(|found| {
                !violations
                    .iter()
                    .any(|reported| reported.path == found.path)
            })
            .collect()
    } else {
        Vec::new()
    };
    let reported: Vec<&Violation> = read_off_the_body.iter().chain(violations.iter()).collect();
    // A member the service checks ITSELF is stated in that check's own words only when the
    // refusal is about that member ALONE: such a check is reached by a request whose shape
    // the validator already admitted, so a request breaking several of its input's
    // declared bounds never reaches one — every clause of it is the validator's, in the
    // validator's own form ([`SERVICE_CHECKED_ENUMS`]).
    let alone = reported.len() == 1 && served_service_checked_clauses().is_empty();
    // The clause about a capped list past its maximum is the one the SERVICE words, with
    // the rejected list rendered from the request: the walk reports the violation but carries
    // only its own summary of the collection, so the clause is substituted for that member's
    // and every other clause of the same refusal is the walk's
    // ([`request_constraints::capped_list_clause`]).
    let capped_list = served_capped_list_clause();
    let clauses: Vec<String> = in_stated_member_order(&reported)
        .into_iter()
        .map(|violation| match capped_list.as_ref() {
            Some(capped) if violation.path == capped.path => Some(capped.clause.clone()),
            _ => violation_clause(violation, served_clause_form(), alone),
        })
        .collect::<Option<Vec<_>>>()?;
    // The violations this service found by READING the request are violations of the same
    // input, so they are clauses of the same envelope and are counted with the walk's
    // ([`request_constraints`]). They come first because the members they are about are
    // declared first — the one order the refusal states its members in. A refusal the
    // walk raised for another request cannot pick them up: the scope they are read from
    // is the run of the request they were read off.
    let clauses: Vec<String> = served_service_checked_clauses()
        .into_iter()
        .chain(clauses)
        .collect();
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
    // The two secondary-index lists of a table declaration, which the model declares as
    // `LocalSecondaryIndexes` then `GlobalSecondaryIndexes`
    // (`smithy_query(query="operation", operation="CreateTable")`): the service states the
    // GLOBAL list's clauses first. Named by the LIST members, so a clause about one of
    // their elements is ordered by the list it belongs to whichever position and member
    // inside the element it is about.
    &["globalSecondaryIndexes", "localSecondaryIndexes"],
    // The members of a KEYED ITEM request, which the model declares as `TableName`, `Key`,
    // then (far later) `ReturnValues` and `ReturnConsumedCapacity`
    // (`smithy_query(query="operation", operation="UpdateItem")`): the service states the
    // capacity member's clause first, then the table name's, then the returned-values
    // member's, and an absent member's last.
    &[
        "returnConsumedCapacity",
        "tableName",
        "returnValues",
        "key",
    ],
    // The capacity MODE and the key schema of a table declaration, which the model declares
    // as `KeySchema` then `BillingMode` (`smithy_query(query="operation",
    // operation="CreateTable")`): the service states the MODE's clause first. Named by the
    // schema's LIST member, so a clause about one of its elements is ordered by the list it
    // belongs to whichever position and member inside the element it is about.
    &["billingMode", "keySchema"],
    // The key schema of a table declaration and the attribute definitions it draws its
    // elements from, which the model declares as `AttributeDefinitions` then `KeySchema`
    // (`smithy_query(query="operation", operation="CreateTable")`): the service states the
    // SCHEMA's clauses first — so one over-long attribute name, which breaks a bound in both
    // lists, is reported under the key schema before the definitions. Named by the two LIST
    // members, so a clause about one of their elements is ordered by the list it belongs to
    // whichever position and member inside the element it is about.
    &["keySchema", "attributeDefinitions"],
];

/// `violations` in the order their clauses are stated: the order
/// [`STATED_MEMBER_ORDERS`] publishes for the input they are all members of, and
/// otherwise the order the walk reported — and then, among the clauses about members of ONE
/// structure, the order [`PUBLISHED_SIBLING_ORDERS`] publishes for that structure.
///
/// An entry applies only when it names EVERY violated member, so a refusal that mixes a
/// member the entry knows with one it does not keeps the walk's order rather than
/// guessing where the unknown member belongs.
fn in_stated_member_order<'v>(violations: &[&'v Violation]) -> Vec<&'v Violation> {
    let mut stated: Vec<&Violation> = violations.to_vec();
    let Some(order) = STATED_MEMBER_ORDERS
        .iter()
        .find(|order| violations.iter().all(|violation| at_in(order, violation).is_some()))
    else {
        return in_published_sibling_order(stated);
    };
    stated.sort_by_key(|violation| at_in(order, violation).unwrap_or(order.len()));
    in_published_sibling_order(stated)
}

/// The members of ONE STRUCTURE whose clauses this service states in an order of its own,
/// each written in the order it states them, by the member names the validating walk reports
/// them under.
///
/// The capacity pair is the one such structure: `ProvisionedThroughput` declares
/// `ReadCapacityUnits` then `WriteCapacityUnits` (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#ProvisionedThroughput")`), while the service is observed to
/// state the WRITE dimension's clause first. It is a property of that STRUCTURE rather than
/// of the input that carries it, so it holds wherever the structure stands — the table's own
/// capacity, and the capacity of an index inside a list of index updates.
///
/// Nothing about the model distinguishes such a structure from one stated in the declared
/// order, so this is the only place the difference is recorded.
/// `tests/all/required_null_member_reports.rs` reads it back over a nested capacity pair.
const PUBLISHED_SIBLING_ORDERS: &[&[&str]] = &[&["writeCapacityUnits", "readCapacityUnits"]];

/// `stated` with the clauses about members of one structure in the order
/// [`PUBLISHED_SIBLING_ORDERS`] publishes for it.
///
/// The clauses keep the POSITIONS they already hold and are permuted among themselves, so
/// nothing about where a structure's clauses sit relative to any other member's changes: the
/// only thing decided here is which of the structure's own members is stated first. A
/// structure only one of whose members is reported has nothing to reorder.
fn in_published_sibling_order(mut stated: Vec<&Violation>) -> Vec<&Violation> {
    for order in PUBLISHED_SIBLING_ORDERS {
        let mut parents: Vec<String> = Vec::new();
        for violation in &stated {
            if let Some((parent, _)) = sibling_in(order, violation) {
                if !parents.contains(&parent) {
                    parents.push(parent);
                }
            }
        }
        for parent in parents {
            let slots: Vec<usize> = (0..stated.len())
                .filter(|at| {
                    sibling_in(order, stated[*at])
                        .is_some_and(|(named, _)| named == parent)
                })
                .collect();
            if slots.len() < 2 {
                continue;
            }
            let mut held: Vec<&Violation> = slots.iter().map(|at| stated[*at]).collect();
            held.sort_by_key(|violation| {
                sibling_in(order, violation).map_or(order.len(), |(_, at)| at)
            });
            for (slot, violation) in slots.iter().zip(held) {
                stated[*slot] = violation;
            }
        }
    }
    stated
}

/// The structure `violation`'s member belongs to and where that member sits in `order`, or
/// `None` for a clause about a member the order does not name.
fn sibling_in(order: &[&str], violation: &Violation) -> Option<(String, usize)> {
    let (parent, member) = violation.path.rsplit_once('.')?;
    let at = order.iter().position(|named| *named == member)?;
    Some((parent.to_string(), at))
}

/// Where `violation`'s member sits in `order`, or `None` for a member it does not name.
///
/// A clause's member is its whole PATH where the order names paths, and otherwise the
/// top-level member the path starts at: an order over the input's own members decides
/// where a clause about one of their elements goes too, because the element is reported
/// under that member (`globalSecondaryIndexes.1.member.indexName`).
fn at_in(order: &[&str], violation: &Violation) -> Option<usize> {
    let path = violation.path.as_str();
    order
        .iter()
        .position(|member| *member == path)
        .or_else(|| {
            let top = path.split('.').next()?;
            order.iter().position(|member| *member == top)
        })
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

/// The clause a request that NAMES NO TABLE is refused with, and the only clause such a
/// refusal states — or `None` when the request's table-name member carries a value long
/// enough to name one, carries no value at all, or stands in a request the reader could not
/// build.
///
/// The table name is the whole SELECTOR a request addresses a table by, so a value too short
/// for the member to name one leaves the request pointed at nothing: this service's own check
/// of that selector answers for it, with the member named as the model spells it on the wire
/// and NOTHING echoed — the words that check states, whichever operation was served, exactly
/// as the created table's own name form ([`TABLE_NAME_FORM_OPERATIONS`]) and the enums this
/// service checks itself ([`SERVICE_CHECKED_ENUMS`]) keep their own words. And no other
/// member's violation is reported beside it: the remaining field checks are not reached at
/// all once the request names no table.
///
/// A request the reader could not BUILD is a different refusal: a member the input requires
/// and the body omits stops that reading, and what is reported is the reading's own account
/// of every bound the body broke, the absent member among them (see the merge in
/// [`validation_envelope`]) — so this check answers only for a request whose every required
/// member carries a value.
fn unusable_table_name_clause(violations: &[Violation]) -> Option<String> {
    if violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::MissingRequiredMember))
    {
        return None;
    }
    let violation = violations.iter().find(|violation| {
        violation.path == crate::state::TABLE_NAME_MEMBER_PATH
            && matches!(violation.kind, ViolationKind::MinLength(_))
    })?;
    Some(crate::state::constraint_clause(
        &wire_member_path(&violation.path),
        None,
        &requirement(&violation.kind)?,
    ))
}

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

/// ANY TOP-LEVEL member this refusal found absent, named as the model spells it on the
/// wire, or `None` when the refusal is about anything else.
///
/// A member's absence is reported whichever member it is, because the refusal this answers
/// for is the body READER's own failure to read one ([`BODY_READER_OPERATIONS`]) rather
/// than a check of one named member. One request can omit several; the one reported is the
/// first in the order the refusal states them, which is the order the input's members were
/// read — the order that reader ran out of them in.
///
/// A member absent inside a NESTED structure is not reported here: the position such a
/// refusal states is the end of the object the read was building
/// ([`request_constraints::body_read_end`] reads the request's own), so a nested absence
/// would be stated at the wrong place and under a dotted path no reader spells a field by.
/// It keeps the reading every other operation gives it.
fn absent_required_member(violations: &[Violation]) -> Option<String> {
    violations
        .iter()
        .find(|violation| {
            matches!(violation.kind, ViolationKind::MissingRequiredMember)
                && !violation.path.contains('.')
        })
        .map(|violation| wire_member_path(&violation.path))
}

/// The entry the sentence names for the required tag list this refusal found absent
/// ([`REQUIRED_TAG_LISTS`]), or `None` when the refusal is about anything else.
fn absent_required_tag_list(violations: &[Violation]) -> Option<&'static str> {
    violations.iter().find_map(|violation| {
        REQUIRED_TAG_LISTS
            .iter()
            .find(|(member, _)| *member == violation.path.as_str())
            .map(|(_, entry)| *entry)
    })
}

/// Whether this refusal found a violation of the ARN-addressed selector
/// ([`crate::state::RESOURCE_ARN_MEMBER_PATH`]), and the value the member held — `None`
/// inside the `Some` for a member the request carried no value for.
///
/// One request can break several members' rules; the one reported is this member, because
/// the check that reads it is what resolves the resource the request addresses.
fn resource_arn_violation(violations: &[Violation]) -> Option<&Option<String>> {
    violations
        .iter()
        .find(|violation| violation.path == crate::state::RESOURCE_ARN_MEMBER_PATH)
        .map(|violation| &violation.value)
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

/// The codes this service answers a request the emulator's authentication layer refused
/// with (see [`service_common::authentication_refusal`], which owns the rule and the
/// sentences): the `awsJson` unknown-credential code for an access key id that resolves to
/// no credential, and the `awsJson` signature code for one that resolves and whose signature
/// does not verify. Neither is `AccessDenied`, which is this service's answer to a KNOWN
/// principal whose policies do not permit the action.
///
/// A request that presented NO credential at all is named by this endpoint's OWN front door
/// instead ([`request_signature`], whose class is namespaced and whose status is the one a
/// fault in the request carries), so none is named here.
const AUTHENTICATION_REFUSAL: StatedAuthenticationRefusal = StatedAuthenticationRefusal {
    unresolved_credential: "UnrecognizedClientException",
    signature_mismatch: "InvalidSignatureException",
    missing_credential: None,
};

/// The `com-amazonaws-dynamodb-dynamodb-20120810` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers, inside that the
    /// layer that states an authentication refusal in this service's own codes
    /// ([`AUTHENTICATION_REFUSAL`]), and INSIDE both the AUDITED routing table every
    /// request this service serves is resolved through ([`management_events`]).
    entry: RequestIdEnvelopeDefinition<
        request_signature::FaultStatedDefinition<
            AuthenticationRefusalDefinition<
                management_events::AuditedDefinition<crate::ServiceDefinition<Eng>>,
            >,
        >,
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
        // The AUDITED table, built before the entry it is nested inside: every modelled
        // operation, each constructed from this same shared engine and wrapped in the
        // management-event recorder (see [`management_events`]).
        let audited = management_events::audited_table(&engine);
        Self {
            entry: RequestIdEnvelopeDefinition::reporting(
                // The SIGNATURE-fault layer is outside the credential one, because it
                // answers the refusals that are about the REQUEST's signature material and
                // leaves the identity failures — a key id that resolves to no credential —
                // to the layer below it.
                request_signature::FaultStatedDefinition::new(AuthenticationRefusalDefinition::new(
                    // The audited table is the INNERMOST wrapper, so every layer that
                    // resolves an operation for a request — the router, this service's own
                    // dispatch, the envelope wrappers around it — reaches the audited
                    // operation, and no request can be served by a path that records
                    // nothing.
                    management_events::AuditedDefinition::new(
                        crate::ServiceDefinition::new(Arc::clone(&engine)),
                        audited,
                    ),
                    AUTHENTICATION_REFUSAL,
                )),
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
        checks_its_own_members, clause_forms, holds_table_name_to_its_form,
        reads_body_with_its_own_reader, served_clause_form, stated_enum_order,
        states_absent_member_as_clause, states_service_checked_clause_bare, ClauseForm,
        BARE_CLAUSE_OPERATIONS, BODY_READER_OPERATIONS, CLAUSE_FORMS, PUBLISHED_ENUM_ORDERS,
        SELF_CHECKED_MEMBER_OPERATIONS, TABLE_NAME_FORM_OPERATIONS,
        VALIDATOR_NULL_CLAUSE_OPERATIONS,
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
        // `ReturnConsumedCapacity`, whose recorded refusals state the model's own
        // declaration order, plus a subset of a published set (same values, one missing),
        // which is a different set and not that enum.
        for declared in [
            owned(&["INDEXES", "TOTAL", "NONE"]),
            owned(&["OLD_IMAGE", "KEYS_ONLY", "NEW_IMAGE"]),
            owned(&["SPECIFIC_ATTRIBUTES", "COUNT", "ALL_ATTRIBUTES"]),
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
        // The operations observed to state the reader's own parameter-required sentence
        // for the same absence, which is what tells the two groups apart.
        for operation in [
            "CreateTable",
            "DescribeKinesisStreamingDestination",
            "GetItem",
            "PutItem",
            "Query",
        ] {
            assert!(
                !states_absent_member_as_clause(operation),
                "case {operation:?}: expected the service's sentence, got the walk's clause"
            );
        }
    }

    #[test]
    fn self_checked_member_operations_are_sorted() {
        let mut sorted = SELF_CHECKED_MEMBER_OPERATIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            SELF_CHECKED_MEMBER_OPERATIONS,
            sorted.as_slice(),
            "case the self-checked list: expected it sorted for `binary_search`, got \
             {SELF_CHECKED_MEMBER_OPERATIONS:?}"
        );
        for operation in SELF_CHECKED_MEMBER_OPERATIONS {
            assert!(
                checks_its_own_members(operation),
                "case {operation:?}: expected the service's own sentence, got the walk's clause"
            );
        }
        // The surface that shares the ARN member and not the vocabulary, and the operation
        // that carries the same `Tags` list as an OPTIONAL member.
        for operation in [
            "PutResourcePolicy",
            "GetResourcePolicy",
            "DeleteResourcePolicy",
            "CreateTable",
        ] {
            assert!(
                !checks_its_own_members(operation),
                "case {operation:?}: expected the walk's clause, got the service's sentence"
            );
        }
    }

    #[test]
    fn body_reader_operations_are_sorted() {
        let mut sorted = BODY_READER_OPERATIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            BODY_READER_OPERATIONS,
            sorted.as_slice(),
            "case the body-reader list: expected it sorted for `binary_search`, got \
             {BODY_READER_OPERATIONS:?}"
        );
        for operation in BODY_READER_OPERATIONS {
            assert!(
                reads_body_with_its_own_reader(operation),
                "case {operation:?}: expected the reader's own sentence, got another reading"
            );
        }
        // The operations whose absent member is stated by this service's parameter check or
        // by the validating walk instead, which is what tells the groups apart.
        for operation in ["CreateTable", "DescribeTable", "GetItem", "Query", "UpdateTable"] {
            assert!(
                !reads_body_with_its_own_reader(operation),
                "case {operation:?}: expected another reading, got the reader's own sentence"
            );
        }
    }

    #[test]
    fn table_name_form_operations_are_sorted() {
        let mut sorted = TABLE_NAME_FORM_OPERATIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            TABLE_NAME_FORM_OPERATIONS,
            sorted.as_slice(),
            "case the table-name-form list: expected it sorted for `binary_search`, got \
             {TABLE_NAME_FORM_OPERATIONS:?}"
        );
        for operation in TABLE_NAME_FORM_OPERATIONS {
            assert!(
                holds_table_name_to_its_form(operation),
                "case {operation:?}: expected the table-name form, got the walk's clause"
            );
        }
        // The operations that RESOLVE a table the account already holds, whose member
        // accepts that table's ARN as well as its name.
        for operation in ["DescribeTable", "GetItem", "PutItem", "UpdateTable", "DeleteTable"] {
            assert!(
                !holds_table_name_to_its_form(operation),
                "case {operation:?}: expected the walk's clause, got the table-name form"
            );
        }
    }

    #[test]
    fn the_clause_form_table_names_each_operation_once() {
        let mut named: Vec<&str> = CLAUSE_FORMS
            .iter()
            .map(|(operation, _, _)| *operation)
            .collect();
        let before = named.len();
        named.sort_unstable();
        named.dedup();
        assert_eq!(
            named.len(),
            before,
            "case the clause-form table: expected each operation named once, got {named:?}"
        );
    }

    #[test]
    fn the_served_operation_chooses_the_clause_form() {
        for (operation, expected, members) in CLAUSE_FORMS {
            // A member the row does not name states the operation's own form.
            let unnamed = "aMemberNoRowNames";
            assert!(
                !members.iter().any(|(member, _)| *member == unnamed),
                "case {operation:?}: the probe member must be one no row names"
            );
            let form = clause_forms(operation).of(unnamed);
            assert_eq!(
                form, *expected,
                "case {operation:?}: expected {expected:?}, got {form:?}"
            );
            // And each member it DOES name states the form recorded beside it.
            for (member, recorded) in members.iter() {
                let form = clause_forms(operation).of(member);
                assert_eq!(
                    form, *recorded,
                    "case {operation:?} member {member:?}: expected {recorded:?}, got {form:?}"
                );
            }
        }
        // Every operation the table does not name states the request validator's own form.
        for operation in [
            "ListBackups",
            "ListExports",
            "UpdateTimeToLive",
            "CreateTable",
            "DescribeTable",
        ] {
            let form = clause_forms(operation).of("limit");
            assert_eq!(
                form,
                ClauseForm::PathAndValue,
                "case {operation:?}: expected the request-validator form, got {form:?}"
            );
        }
    }

    #[test]
    fn each_member_states_the_form_its_operation_gives_it() {
        // The form is read off the served OPERATION and the member it is about: both ITEM
        // READS state the page size they share — and the table walk its two parallel-scan
        // bounds — in the item-request form, while a control-plane listing's page size, the
        // same member name under the same kind of bound, is stated in the request
        // validator's.
        for (operation, member, expected) in [
            ("Scan", "limit", ClauseForm::WireMemberOnly),
            ("Scan", "totalSegments", ClauseForm::WireMemberOnly),
            ("Scan", "segment", ClauseForm::WireMemberOnly),
            ("Query", "limit", ClauseForm::WireMemberOnly),
            (
                "BatchWriteItem",
                "requestItems",
                ClauseForm::ValidatorMemberOnly,
            ),
            ("ListBackups", "limit", ClauseForm::PathAndValue),
        ] {
            let form = clause_forms(operation).of(member);
            assert_eq!(
                form, expected,
                "case {operation:?} member {member:?}: expected {expected:?}, got {form:?}"
            );
        }
        // And a refusal raised outside any operation's run states the validator's form,
        // which is the general one.
        let outside = served_clause_form().of("limit");
        assert_eq!(
            outside,
            ClauseForm::PathAndValue,
            "case \"outside an operation's run\": expected the request-validator form, got \
             {outside:?}"
        );
    }
}
