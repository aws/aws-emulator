// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudwatchlogs-logs-20140328` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. It still serves the operation the generated
//!   routing table names; what it adds is the OPERATION IDENTITY the refusal seam
//!   below needs — see "The code a refusal carries".
//! - `override_refusal` — EDITED HERE. A request refused for violating a modelled
//!   constraint is answered with the refusal error the OPERATION BEING SERVED
//!   declares, worded against the input position that failed — see "The code a refusal
//!   carries" and "How this service words a refusal".
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
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
//! # The code a refusal carries
//!
//! CloudWatch Logs does not answer a modelled-constraint violation with a generic
//! `ValidationException`. Its own error for "A parameter is specified incorrectly" is
//! [`INVALID_PARAMETER_EXCEPTION`] (HTTP 400), which all but a couple of this service's
//! operations declare, and which is the code the recordings show it raising for a value
//! outside a declared `@range`, `@pattern` or enum, and for an absent `@required`
//! member. The code is what an SDK's error mapper dispatches on, so answering
//! `ValidationException` where the operation does not even declare that shape leaves a
//! code-generated client with no branch to take.
//!
//! The code is a per-OPERATION fact, because the error SET is: the delivery,
//! scheduled-query and S3-table-source families declare `ValidationException` and no
//! `InvalidParameterException` ([`VALIDATION_EXCEPTION_OPERATIONS`]), and the two
//! `@deprecated` tag operations declare neither ([`NO_REFUSAL_ERROR_OPERATIONS`], where
//! this service has nothing of its own to say and the framework's wording stands). So
//! the refusal code is chosen by [`refusal_code`] from the operation's own modelled
//! error set — every set is listed in the crate root's `OPERATIONS.md`, which is
//! generated from the same Smithy model as the handlers.
//!
//! The framework refuses such a request BEFORE any handler runs — the centralized walk
//! deserializes, validates, and only then calls `handle` — so [`Service::override_refusal`]
//! is the only place this choice can be made, and that seam is handed the refusal alone,
//! with no request and no operation. [`OperationScopedRefusal`] is how the operation
//! reaches it: `override_operation` returns it for every request the routing table can
//! serve, and it runs the real operation inside a task-local scope holding that
//! operation's refusal code, which `override_refusal` reads back. The scope wraps the
//! whole of the operation's run, and a task-local travels with the task across every
//! await point, so the code read on the refusal path is always the one belonging to the
//! request being served.
//!
//! # How this service words a refusal
//!
//! The envelope is AWS's own — `N validation error(s) detected:` over one clause per
//! violation, joined by `"; "` — and each clause names the INPUT POSITION that failed and
//! the constraint it failed, and nothing of the value submitted there ([`SUBJECT`],
//! [`requirement`]). Two parts of that clause are rules this service owns, and both are
//! read off the recordings rather than copied from one of them:
//!
//! * the subject is `Value at '<member path>'` whether a value reached the member or not:
//!   the recorded refusal for an ABSENT `@required` member states
//!   `Value at 'storageTier' failed to satisfy constraint: Member must not be null`,
//!   with no `null` in the subject, exactly as a present-but-invalid value's clause reads;
//! * an enum's admissible set is listed in the order AWS PUBLISHES for that enum
//!   ([`PUBLISHED_ENUM_ORDERS`]) — the order its own documentation and its model
//!   declaration state the values in — and, for an enum this service publishes no order
//!   for, in ASCENDING ORDER of its values: the recorded refusals list `StorageTier` as
//!   `[INTELLIGENT_TIERING, STANDARD]` where the model declares
//!   `[STANDARD, INTELLIGENT_TIERING]`, and `ListAggregateLogGroupSummariesGroupBy` as
//!   `[DATA_SOURCE_NAME_AND_TYPE, DATA_SOURCE_NAME_TYPE_AND_FORMAT]` where the model
//!   declares the format value first — two enums whose declaration order differs from the
//!   recorded one, and in both the recorded order is the sorted set.
//!
//! The published order is a property of the ENUM rather than of one member referencing it,
//! so it is stated the same way wherever this service lists that enum's admissible
//! values: in a constraint clause, and in the `Valid values are: [...]` catalogue the
//! handler-side refusal in `crate::state` lists for a policy-document attribute
//! ([`enum_value_set`]).
//!
//! A `@pattern` is stated as AWS publishes the expression for that shape — the model's
//! expression, except for the shapes whose published form drops its `^`…`$` anchors
//! ([`PUBLISHED_PATTERNS`]); the reasoning for that one is on [`requirement`], which is
//! also where the handler-side refusal in `crate::state` reads its wording from
//! ([`one_violation_message`]), so both refusal paths read alike.
//!
//! One violation class is not worded as a clause at all: a LOG GROUP or LOG STREAM NAME
//! longer than the 512 characters the model allows either of them is refused with the
//! service's own sentence for that pair of names ([`log_name_length_message`]), which the
//! service answers in place of the envelope because it holds both names to that one bound
//! itself rather than reporting the member the name arrived on.
//!
//! The subject rule is the half `services/common/src/validation.rs` names as "still to
//! come", and ~26 service crates word it inline today. Migrating them onto one shared
//! helper is a change of its own rather than a side effect of this service's issues, so it
//! is worded here — as that module asks, and as it records that "which values an enum's
//! value set lists and in which order" stays with the service.
//!
//! # Who is calling, and what they may do
//!
//! Every call of this API is signed, and AWS decides a signed call against the calling
//! principal's identity-based IAM policies BEFORE it is dispatched. That decision is taken
//! in front of every operation this service serves, by
//! [`identity_authorization::IamAuthorized`] — the wrapper `override_operation` hands the
//! router in place of each dispatch below — so a caller whose policies do not allow the
//! action it asked for is refused with the AWS-common `AccessDeniedException` and no
//! handler of this service is reached. The rule and the resource a request is judged
//! against live in that module; the decision itself is
//! [`crate::state::identity_authorization`], the one place this service reads a principal's
//! documents.
//!
//! # Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header this
//! protocol reports that id in (`x-amzn-RequestId`) — on a success exactly as on a failure.
//! It is a property of the RESPONSE, not of any modelled shape: no output and no error shape
//! of this model declares it, so no handler can set it, and the framework's `awsJson1_1`
//! renderers do not write it.
//!
//! The rule is not restated here: `service_common::query_error_envelope` owns it for every
//! emulated service, and this file only chooses the report this protocol takes (the header
//! alone — `awsJson1_1` has no envelope element for the id) and wires it at the two layers
//! every response of this service passes through:
//!
//! * [`RequestIdReportingOperation::header`], the outermost layer of each dispatch
//!   `override_operation` hands back, which covers a success, a modelled error, the
//!   authorization refusal and a constraint refusal raised while an operation was being
//!   served;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, which covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal.

use service_common::event_stream::EventStreamRoute;
use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdReportingOperation,
};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Refusal, Refused,
    Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// Whether IAM lets the principal that signed a request perform the action it asked for,
/// which is decided in front of every operation this service serves
/// ([`identity_authorization::IamAuthorized`]).
///
/// Public: its catalog of the operations that take the decision themselves, and the resource
/// a request is judged against, are checked by this crate's tests.
pub mod identity_authorization;

/// The operations the generated routing table cannot serve: each one's output member
/// targets a `@streaming` union (`StartLiveTail$responseStream`,
/// `GetLogObject$fieldStream`), so the generated shim declines the request and
/// [`EmulatedService::override_operation`] — which the router consults FIRST — hands it to
/// the hand-authored operation instead.
const START_LIVE_TAIL_OPERATION: &str = "StartLiveTail";
/// See [`START_LIVE_TAIL_OPERATION`].
const GET_LOG_OBJECT_OPERATION: &str = "GetLogObject";

/// CloudWatch Logs' error for a parameter it will not accept: "A parameter is specified
/// incorrectly" (HTTP 400). Declared by all but the operations named in
/// [`VALIDATION_EXCEPTION_OPERATIONS`] and [`NO_REFUSAL_ERROR_OPERATIONS`], and the code
/// this service answers for a modelled-constraint violation wherever it is declared.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The refusal error of the operations that declare no `InvalidParameterException`: the
/// modeled `ValidationException` (`com.amazonaws.cloudwatchlogs#ValidationException`,
/// `@error("client")`, HTTP 400), "One of the parameters for the request is not valid".
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The operations whose modelled error set declares [`VALIDATION_EXCEPTION`] and NOT
/// [`INVALID_PARAMETER_EXCEPTION`] — the delivery, scheduled-query and S3-table-source
/// families, which were added to this service with the newer error vocabulary. Each
/// one's error set is listed in the crate root's `OPERATIONS.md`.
///
/// Sorted, which [`refusal_code`] relies on for `binary_search` and
/// `operation_tables_are_sorted_and_disjoint` pins.
const VALIDATION_EXCEPTION_OPERATIONS: &[&str] = &[
    "AssociateSourceToS3TableIntegration",
    "CreateDelivery",
    "CreateScheduledQuery",
    "DeleteDelivery",
    "DeleteDeliveryDestination",
    "DeleteDeliveryDestinationPolicy",
    "DeleteDeliverySource",
    "DeleteScheduledQuery",
    "DescribeConfigurationTemplates",
    "DescribeDeliveries",
    "DescribeDeliveryDestinations",
    "DescribeDeliverySources",
    "DisassociateSourceFromS3TableIntegration",
    "GetDelivery",
    "GetDeliveryDestination",
    "GetDeliveryDestinationPolicy",
    "GetDeliverySource",
    "GetScheduledQuery",
    "GetScheduledQueryHistory",
    "ListScheduledQueries",
    "ListSourcesForS3TableIntegration",
    "PutDeliveryDestination",
    "PutDeliveryDestinationPolicy",
    "PutDeliverySource",
    "UpdateDeliveryConfiguration",
    "UpdateScheduledQuery",
];

/// The operations that declare NEITHER refusal error: the two `@deprecated` log-group
/// tag operations, whose modelled errors are `ResourceNotFoundException` (and, for
/// `ListTagsLogGroup`, `ServiceUnavailableException`). Naming either refusal shape on
/// them would answer with an error the operation cannot return, so this service declines
/// to choose and the framework's own wording stands.
///
/// Sorted, as [`VALIDATION_EXCEPTION_OPERATIONS`] is.
const NO_REFUSAL_ERROR_OPERATIONS: &[&str] = &["ListTagsLogGroup", "UntagLogGroup"];

/// The error code `operation` answers for a value outside a modelled constraint, or
/// `None` when it declares no refusal error and the framework's wording stands.
fn refusal_code(operation: &str) -> Option<&'static str> {
    if NO_REFUSAL_ERROR_OPERATIONS.binary_search(&operation).is_ok() {
        return None;
    }
    if VALIDATION_EXCEPTION_OPERATIONS
        .binary_search(&operation)
        .is_ok()
    {
        return Some(VALIDATION_EXCEPTION);
    }
    Some(INVALID_PARAMETER_EXCEPTION)
}

tokio::task_local! {
    /// The refusal code of the operation the current request is being served as, set by
    /// [`OperationScopedRefusal::run`] around that operation's whole run and read back by
    /// [`EmulatedService::override_refusal`], which the framework hands no request.
    ///
    /// `None` inside the scope means the operation declares no refusal error; absent
    /// altogether means the refusal happened outside an operation's run (an
    /// authentication refusal, which the framework words), and both leave the framework's
    /// wording in place.
    static SERVED_REFUSAL_CODE: Option<&'static str>;
}

/// The refusal code of the operation this task is serving, if any.
fn served_refusal_code() -> Option<&'static str> {
    SERVED_REFUSAL_CODE.try_with(|code| *code).ok().flatten()
}

/// Serves a request as the operation the generated routing table names, with that
/// operation's refusal code ([`refusal_code`]) in scope for the whole run.
///
/// It adds nothing else: the request, the service and the context are handed to the
/// modelled operation unchanged, so deserialization, validation, the handler and the
/// response are the framework's and the operation's own. The routing table holds only
/// generated operations, so the inner lookup can never resolve back to this type.
struct OperationScopedRefusal;

impl ErasedOperation for OperationScopedRefusal {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // `override_operation` returns this type only for a request the routing table
            // can serve, so the two lookups below resolve; a request that cannot be routed
            // keeps the framework's routing-error answer either way.
            let routed = svc
                .definition()
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| Some((key, svc.definition().routing_table().get(key)?)));
            let Some((operation, op)) = routed else {
                return svc.definition().routing_error_response(req);
            };
            SERVED_REFUSAL_CODE
                .scope(refusal_code(operation), op.run(svc, req, ctx))
                .await
        })
    }
}

/// How CloudWatch Logs names the subject of a constraint clause: the POSITION in the input
/// that failed, and never the value sitting there — nor, for a member the request did not
/// carry at all, the framework's `null` (see "How this service words a refusal").
///
/// A caller's own data — a delivery id, a delivery-source name, an integration-source
/// identifier, a page size — is not reflected back into an error body a client may log or
/// match on.
const SUBJECT: &str = "Value at";

/// The requirement one violation states: the modeled constraint the value failed.
///
/// Read off the violation's structural [`ViolationKind`] — the bound, the pattern, the
/// value set the MODEL declares — so a clause states the constraint rather than anything
/// about the request beyond which member it names.
///
/// THE PATTERN RENDERING RULE, stated once for the whole service (the handler-side
/// refusal in `crate::state` words its clause through [`one_violation_message`], so both
/// refusal paths read alike): a `@pattern` is stated as AWS PUBLISHES the expression for
/// that shape — the model's trait string, except for the shapes whose published expression
/// drops the trait's surrounding `^`…`$` anchors ([`PUBLISHED_PATTERNS`]). The anchoring is
/// a property of the SHAPE rather than of a member or an operation, and the recorded
/// refusals of this service differ on it — the three recorded `DeliveryId` refusals print
/// the model's `^[0-9A-Za-z]+$` anchored, while the recorded `DeliverySourceName` one prints
/// the model's `^[\w-]*$` as `[\w-]*` and the recorded `logGroupName` ones print the model's
/// `^[\.\-_/#A-Za-z0-9]+$` as `[\.\-_/#A-Za-z0-9]+` — so the shapes whose published form
/// differs from the model's are named in that table and every other shape's expression is
/// rendered as the model spells it.
///
/// THE ENUM ORDERING RULE: an enum's admissible set is listed in the order AWS publishes
/// for that enum where there is one ([`PUBLISHED_ENUM_ORDERS`]), and otherwise in ascending
/// order of its VALUES rather than in the model's declaration order, which is what the
/// recordings show for the two enums whose orders differ (see "How this service words a
/// refusal"). Both halves are keyed on the enum's own values rather than on the member or
/// the operation carrying it, so they hold for every enum this service declares rather
/// than for the ones the recordings happen to cover ([`enum_value_set`]).
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => {
            format!(
                "Member must satisfy regular expression pattern: {}",
                published_pattern(pattern)
            )
        }
        // A "value set" is just that: the requirement is WHICH values the model allows,
        // listed in the one order this service states that enum's values in (the enum
        // ordering rule above).
        ViolationKind::EnumValue(allowed) => {
            let values = enum_value_set(allowed.iter().map(String::as_str));
            format!("Member must satisfy enum value set: [{}]", values.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// The shapes whose PUBLISHED `@pattern` expression is not the trait string the model
/// carries: one entry per shape, as `(the model's expression, the expression AWS states)`.
///
/// AWS states the expression its own request validator holds, which for these shapes is the
/// model's expression WITHOUT the surrounding `^`…`$` anchors — the recorded
/// `DeliverySourceName` refusal states `[\w-]*` for the model's `^[\w-]*$`. The entry is
/// keyed on the model's expression rather than on a member or an operation, so it holds for
/// every member of every operation that targets the shape (`DeliverySourceName` is reached by
/// `name` of the four delivery-source operations and by `deliverySourceName` of
/// `CreateDelivery`), and a shape not named here is stated as the model spells it — which is
/// what the recorded `DeliveryId` refusals show for `^[0-9A-Za-z]+$`.
///
/// Each entry's published form is the declared one unanchored, which
/// `a_published_pattern_is_the_declared_expression_unanchored` pins.
const PUBLISHED_PATTERNS: &[(&str, &str)] = &[
    // `DeliverySourceName` (`@length` 1..60, `@pattern ^[\w-]*$`).
    (r"^[\w-]*$", r"[\w-]*"),
    // `LogGroupName` (`@length` 1..512, `@pattern ^[\.\-_/#A-Za-z0-9]+$`), reached by the
    // `logGroupName` member of every operation that names a log group by name. AWS publishes
    // this shape's expression UNANCHORED, which the model's own documentation of a log group
    // name states ("Pattern: [\.\-_/#A-Za-z0-9]+", in the `logGroupIdentifier` docs of
    // `PutLogGroupDeletionProtection` and `PutBearerTokenAuthentication`).
    (r"^[\.\-_/#A-Za-z0-9]+$", r"[\.\-_/#A-Za-z0-9]+"),
    // `LogStreamName` (`@length` 1..512, `@pattern ^[^:*]*$`), reached by the
    // `logStreamName` member of every operation that names a log stream, by the
    // `logStreamNamePrefix` of the reads and the export, and by the `logStreamNames` list of
    // the filtering reads. Published UNANCHORED like the log group name it sits beside.
    (r"^[^:*]*$", r"[^:*]*"),
    // `LogGroupIdentifier` (`@length` 1..2048, `@pattern ^[\w#+=/:,.@-]*$`), reached by the
    // `logGroupIdentifier` member of every operation that names a log group by name or ARN,
    // and by the ARN shapes the model gives the same expression (`LogGroupArn`,
    // `AnomalyDetectorArn`). Published UNANCHORED, as the recorded `logGroupIdentifier`
    // refusal states it — the same way the log group name beside it is published.
    (r"^[\w#+=/:,.@-]*$", r"[\w#+=/:,.@-]*"),
];

/// The expression this service states for the `@pattern` the model spells `pattern`: the
/// published form where AWS publishes another one ([`PUBLISHED_PATTERNS`]), and otherwise the
/// model's own expression.
fn published_pattern(pattern: &str) -> &str {
    PUBLISHED_PATTERNS
        .iter()
        .find(|(declared, _published)| *declared == pattern)
        .map_or(pattern, |(_declared, published)| *published)
}

/// The order this service STATES an enum's admissible set in, for an enum AWS publishes an
/// order for that is not the ascending order of its values ([`enum_value_set`]'s default).
///
/// One entry per enum, each a permutation of that enum's declared values — pinned against
/// the generated value-set constants by `a_published_order_is_the_models_set_reordered`.
const PUBLISHED_ENUM_ORDERS: &[&[&str]] = &[
    // The grouping a subscription filter delivers log data under
    // (`cloudwatchlogs#Distribution`), published with the random grouping first: the
    // model's declaration and the documentation of the enum itself — "which can be either
    // random or grouped by log stream" — state the values in that order.
    &["Random", "ByLogStream"],
    // How a log-stream listing is ordered (`cloudwatchlogs#OrderBy`), published with the
    // stream-name value first: the model's declaration and the documentation of
    // `DescribeLogStreams$orderBy` — "If the value is LogStreamName, the results are
    // ordered by log stream name. If the value is LastEventTime, …" — state the values in
    // that order.
    &["LogStreamName", "LastEventTime"],
];

/// The order this service states the admissible set `allowed` in, or `None` when AWS
/// publishes no order for that enum and the ascending order of the values stands.
///
/// The match is on membership and size, never on position, so a model that reordered its
/// own declaration of the same enum would still be stated in the published order.
fn published_enum_order<'a>(
    allowed: impl IntoIterator<Item = &'a str>,
) -> Option<&'static [&'static str]> {
    let allowed: Vec<&str> = allowed.into_iter().collect();
    PUBLISHED_ENUM_ORDERS.iter().copied().find(|published| {
        published.len() == allowed.len() && published.iter().all(|value| allowed.contains(value))
    })
}

/// An enum's admissible values in the one order this service lists them in: the order AWS
/// publishes for that enum ([`published_enum_order`]), and otherwise the ascending order
/// of the values.
///
/// Shared with the handler-side refusals in `crate::state`, which list a policy-document
/// attribute's admissible values in a sentence of their own, so one enum reads the same
/// way whichever refusal states it.
pub(crate) fn enum_value_set<'a>(declared: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut values: Vec<&str> = declared.into_iter().collect();
    if let Some(published) = published_enum_order(values.iter().copied()) {
        return published.to_vec();
    }
    values.sort_unstable();
    values
}

/// Render one constraint violation the way this service states it: the input position and
/// the constraint it failed, and NOTHING of the value that was rejected.
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    Some(clause_at(&violation.path, &requirement))
}

/// The clause a member at `path` is refused with: the position and the requirement, with
/// no value — the one subject form this service states, for a value it rejected and for a
/// member the request never carried alike.
fn clause_at(path: &str, requirement: &str) -> String {
    format!("{SUBJECT} '{path}' failed to satisfy constraint: {requirement}")
}

/// The envelope one clause is wrapped in.
fn one_error_envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The whole message for ONE refused member, for a violation a HANDLER found itself at a
/// member the framework's validating walk no longer sees (a member carrying
/// `skip_validation`, which `crate::state` judges).
///
/// Built from the same subject, the same envelope and the same pattern rendering as a
/// refusal the framework raised, so one violation reads as one message whichever of the
/// two refused the request, and the submitted value is withheld either way.
pub(crate) fn one_violation_message(member: &str, requirement: &str) -> String {
    one_error_envelope(&clause_at(member, requirement))
}

/// The `@length` maximum the model declares on BOTH `LogGroupName` and `LogStreamName`
/// (1..512 on each): the characters either name may be long.
const MAX_LOG_NAME_CHARS: i64 = 512;

/// The members whose value IS a log group name or a log stream name, by the name a
/// violation's member path carries. Source: the model — `logGroupName` and
/// `logGroupNamePrefix` target `LogGroupName`, and `logStreamName`, `logStreamNamePrefix`,
/// `logStreamNames` and `logStreamNamePrefixes` target `LogStreamName` — as the crate root's
/// `OPERATIONS.md` lists them per operation. A list member's path steps into its element, so
/// the match is on any segment of the path rather than on the whole of it.
const LOG_NAME_MEMBERS: &[&str] = &[
    "logGroupName",
    "logGroupNamePrefix",
    "logStreamName",
    "logStreamNamePrefix",
    "logStreamNames",
    "logStreamNamePrefixes",
];

/// The whole message CloudWatch Logs refuses a log group or log stream name longer than the
/// [`MAX_LOG_NAME_CHARS`] both are held to with.
///
/// The service checks the two names against that ONE shared bound itself, before the
/// modelled walk has a member to report, so the refusal states the PAIR and the bound rather
/// than a clause naming the member the name arrived on — which is why this message carries no
/// `N validation error(s) detected` envelope. Only the bound is interpolated, from the
/// model's own maximum; nothing of the submitted name is.
fn log_name_length_message() -> String {
    format!(
        "Invalid LogGroup or LogStream, both must have length less than or equal to \
         {MAX_LOG_NAME_CHARS}."
    )
}

/// Whether one violation is a log group or log stream name past the bound both share.
fn is_log_name_length_breach(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::MaxLength(max) if max == MAX_LOG_NAME_CHARS)
        && violation
            .path
            .split('.')
            .any(|segment| LOG_NAME_MEMBERS.contains(&segment))
}

/// The `@length` bounds the model declares on `QueryId` (1..256): the characters a query id
/// may be long, at either end.
const QUERY_ID_MIN_CHARS: i64 = 1;
const QUERY_ID_MAX_CHARS: i64 = 256;

/// The members whose value IS a query id, by the name a violation's member path carries.
/// Source: the model — every input member targeting `QueryId` is spelled `queryId`
/// (`GetQueryResults`, `StopQuery`, `ListLogGroupsForQuery`, `CreateLookupTable`,
/// `UpdateLookupTable`), as the crate root's `OPERATIONS.md` lists them per operation. The
/// match is on any segment of the path, like every other member rule here.
const QUERY_ID_MEMBERS: &[&str] = &["queryId"];

/// The whole message CloudWatch Logs refuses a `queryId` outside the length bounds
/// [`QUERY_ID_MIN_CHARS`]..[`QUERY_ID_MAX_CHARS`] the model declares for it with.
///
/// The service holds a query id to that ONE bound itself, so the refusal states the KIND of
/// identifier and the bound rather than a clause naming the member the id arrived on — which
/// is why this message carries no `N validation error(s) detected` envelope, the way the log
/// name bound beside it carries none ([`log_name_length_message`]). One sentence covers both
/// ends of the bound, because a value shorter than the minimum is a BLANK id. Only the bound
/// is interpolated, from the model's own maximum; nothing of the submitted id is.
fn query_id_length_message() -> String {
    format!("Query id is blank or exceeded max length {QUERY_ID_MAX_CHARS}")
}

/// Whether one violation is a query id outside the bounds the model declares for one.
fn is_query_id_length_breach(violation: &Violation) -> bool {
    let bound = match violation.kind {
        ViolationKind::MinLength(min) => min == QUERY_ID_MIN_CHARS,
        ViolationKind::MaxLength(max) => max == QUERY_ID_MAX_CHARS,
        _ => false,
    };
    bound
        && violation
            .path
            .split('.')
            .any(|segment| QUERY_ID_MEMBERS.contains(&segment))
}

/// The refusal message for a framework-level refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the order the
/// validating walk found them.
///
/// `None` declines the whole refusal — see [`requirement`].
///
/// Shared with the hand-authored event-stream operation in
/// `crate::ops::start_live_tail`, which the framework's centralized walk does not
/// deserialize or validate for (an event-stream operation serves the whole
/// request-to-response step itself), so it words the violations it finds through this
/// one function and reads alike.
pub(crate) fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    // The TWO violation classes this service words as a sentence of its own rather than as a
    // clause. First: a log group or log stream name past the bound the service holds both to
    // ([`log_name_length_message`]), which it answers whatever else the request also broke.
    if violations.iter().any(is_log_name_length_breach) {
        return Some(log_name_length_message());
    }
    // Second: a query id outside the bounds the model declares for one, which the service
    // states as the one sentence it words that bound with ([`query_id_length_message`]), at
    // either end.
    if violations.iter().any(is_query_id_length_breach) {
        return Some(query_id_length_message());
    }
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(one_error_envelope(&clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-cloudwatchlogs-logs-20140328` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers (see "Naming the
    /// request a response answers"). Identity, the protocol and the operations are the
    /// generated entry's untouched.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    /// The dispatch `override_operation` hands back, which carries the served
    /// operation's refusal code into [`Service::override_refusal`] — behind the IAM
    /// decision, and under the layer that names the request its response answers. Held on
    /// the service because that seam returns a borrow of `self`.
    refusal_scope: Dispatch<Eng>,
    /// One of this service's two event-stream operations, `StartLiveTail`. It is NOT in the
    /// entry's routing table in a servable form: the generated `ErasedOperation` shim of
    /// an operation whose output is a `@streaming` union declines the request (it cannot
    /// drive `serialize_as_event_stream<P>` through an object-safe `run`), so it is held
    /// here behind an [`EventStreamRoute`] and routed by
    /// [`EmulatedService::override_operation`] — behind the same two layers as the dispatch
    /// above.
    start_live_tail: Dispatch<Eng>,
    /// This service's other event-stream operation, `GetLogObject`, held and routed for
    /// the same reason as `start_live_tail` above: its output member `fieldStream` is a
    /// `@streaming` union, so the generated shim in the entry's routing table declines the
    /// request.
    get_log_object: Dispatch<Eng>,
}

/// One dispatch this service hands the router: whatever serves the request, behind the IAM
/// decision taken in front of every operation
/// ([`identity_authorization::IamAuthorized`]) and under the layer that names the request
/// its response answers ([`RequestIdReportingOperation::header`]) — so neither can be
/// forgotten for one route and taken for another.
type Dispatch<Eng> = RequestIdReportingOperation<identity_authorization::IamAuthorized<Eng>>;

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Each dispatch is built from the SAME shared engine, so the policies the decision
        // reads and the state the operation serves from are the one store.
        let authorized = |served: Box<dyn ErasedOperation>| {
            RequestIdReportingOperation::header(identity_authorization::IamAuthorized::new(
                Arc::clone(&engine),
                served,
            ))
        };
        Self {
            entry: RequestIdEnvelopeDefinition::header_only(crate::ServiceDefinition::new(
                Arc::clone(&engine),
            )),
            refusal_scope: authorized(Box::new(OperationScopedRefusal)),
            start_live_tail: authorized(Box::new(EventStreamRoute(
                crate::ops::start_live_tail::StartLiveTail::new(Arc::clone(&engine)),
            ))),
            get_log_object: authorized(Box::new(EventStreamRoute(
                crate::ops::get_log_object::GetLogObject::new(Arc::clone(&engine)),
            ))),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what is
        // handed back is the shared `RequestIdEnvelopeDefinition`, which reads identity,
        // protocol and the routing table straight off that entry and adds the request-id
        // header to the three error renderers (see "Naming the request a response answers").
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve the operation the generated routing table names, through the scope that
        // carries its refusal code into `override_refusal` (see "The code a refusal
        // carries") — behind the IAM decision taken in front of every operation, and under
        // the layer that names the request each response answers. The routing decision
        // itself is NOT overridden: the key is resolved by the model's own protocol, and a
        // request the table cannot serve is declined here so the framework's routing-error
        // path stays exactly as it was.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        // The exceptions are the event-stream operations, whose generated shims in that
        // table decline the request: each is served by its hand-authored operation, which
        // owns the whole request-to-response step and therefore words its own refusals
        // (`crate::ops::start_live_tail`, `crate::ops::get_log_object`) rather than
        // reaching `override_refusal`.
        if operation == START_LIVE_TAIL_OPERATION {
            return Some(&self.start_live_tail as &dyn ErasedOperation);
        }
        if operation == GET_LOG_OBJECT_OPERATION {
            return Some(&self.get_log_object as &dyn ErasedOperation);
        }
        Some(&self.refusal_scope)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // THE CODE is the refusal error the SERVED OPERATION declares —
        // `InvalidParameterException` on all but the delivery, scheduled-query and
        // S3-table-source families — which is what an SDK's error mapper binds for this
        // failure; see "The code a refusal carries".
        //
        // THE MESSAGE states a member-constraint failure against the INPUT POSITION that
        // failed and the constraint it failed — never the value that was submitted there,
        // so a delivery id, a source name or a page size a caller sent is not reflected
        // back into a 400 body. A `@pattern` is stated as AWS publishes the expression for
        // that shape and an enum's value set is listed in ascending order — both rules
        // stated once on `requirement` above, and shared with the handler-side refusal
        // through `one_violation_message`.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure, and it is raised before any operation is served.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: served_refusal_code()?.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
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
        constraint_clause, constraint_envelope, enum_value_set, published_enum_order,
        published_pattern, refusal_code, served_refusal_code, INVALID_PARAMETER_EXCEPTION,
        NO_REFUSAL_ERROR_OPERATIONS, PUBLISHED_ENUM_ORDERS, PUBLISHED_PATTERNS,
        SERVED_REFUSAL_CODE, VALIDATION_EXCEPTION, VALIDATION_EXCEPTION_OPERATIONS,
    };
    use smithy_server_runtime::{Violation, ViolationKind};

    /// Every published pattern is the DECLARED expression with its anchors dropped — a table
    /// entry cannot rewrite the expression itself — and the lookup is keyed on the declared
    /// string, so a shape the table does not name keeps the model's spelling.
    #[test]
    fn a_published_pattern_is_the_declared_expression_unanchored() {
        for (declared, published) in PUBLISHED_PATTERNS {
            let unanchored = declared
                .strip_prefix('^')
                .and_then(|pattern| pattern.strip_suffix('$'));
            assert_eq!(
                unanchored,
                Some(*published),
                "case {declared:?}: the published form is the declared expression unanchored, \
                 got {published:?}"
            );
            let stated = published_pattern(declared);
            assert_eq!(
                stated, *published,
                "case {declared:?}: expected {published:?}, got {stated:?}"
            );
        }

        // A shape the table does not name: the model's expression stands, anchors included.
        // Source: the model's `DeliveryId` shape (`@pattern ^[0-9A-Za-z]+$`), whose recorded
        // refusals state it anchored.
        let declared = "^[0-9A-Za-z]+$";
        let stated = published_pattern(declared);
        assert_eq!(
            stated, declared,
            "case {declared:?}: an unnamed shape keeps the model's expression, got {stated:?}"
        );
    }

    /// Both tables are searched with `binary_search`, so both must be sorted; and an
    /// operation cannot be in both, or its refusal code would depend on which table was
    /// consulted first.
    #[test]
    fn operation_tables_are_sorted_and_disjoint() {
        for table in [VALIDATION_EXCEPTION_OPERATIONS, NO_REFUSAL_ERROR_OPERATIONS] {
            assert!(
                table.windows(2).all(|pair| pair[0] < pair[1]),
                "case {table:?}: the table must be sorted and hold no duplicate"
            );
        }
        for operation in NO_REFUSAL_ERROR_OPERATIONS {
            assert!(
                !VALIDATION_EXCEPTION_OPERATIONS.contains(operation),
                "case {operation}: an operation belongs to at most one table"
            );
        }
    }

    /// The code is the operation's own: `InvalidParameterException` by default, the
    /// newer families' `ValidationException` where that is all they declare, and nothing
    /// at all for an operation that declares neither.
    #[test]
    fn the_refusal_code_is_the_served_operations_own() {
        let cases = [
            ("GetLogEvents", Some(INVALID_PARAMETER_EXCEPTION)),
            ("DescribeLogStreams", Some(INVALID_PARAMETER_EXCEPTION)),
            ("PutStorageTierPolicy", Some(INVALID_PARAMETER_EXCEPTION)),
            ("CreateLookupTable", Some(INVALID_PARAMETER_EXCEPTION)),
            ("PutIntegration", Some(INVALID_PARAMETER_EXCEPTION)),
            (
                "ListAggregateLogGroupSummaries",
                Some(INVALID_PARAMETER_EXCEPTION),
            ),
            ("GetDelivery", Some(VALIDATION_EXCEPTION)),
            ("UpdateScheduledQuery", Some(VALIDATION_EXCEPTION)),
            (
                "AssociateSourceToS3TableIntegration",
                Some(VALIDATION_EXCEPTION),
            ),
            ("ListTagsLogGroup", None),
            ("UntagLogGroup", None),
        ];
        for (operation, expected) in cases {
            let actual = refusal_code(operation);
            assert_eq!(
                actual, expected,
                "case {operation}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Outside an operation's run there is no served operation, so nothing is reworded;
    /// inside one, the scoped code is what the refusal seam reads.
    #[tokio::test]
    async fn the_served_code_is_read_back_only_inside_the_scope() {
        assert_eq!(
            served_refusal_code(),
            None,
            "case outside a scope: no operation is being served"
        );
        let inside = SERVED_REFUSAL_CODE
            .scope(refusal_code("GetLogEvents"), async {
                // An await point inside the scope: a task-local survives it.
                tokio::task::yield_now().await;
                served_refusal_code()
            })
            .await;
        assert_eq!(
            inside,
            Some(INVALID_PARAMETER_EXCEPTION),
            "case inside GetLogEvents' scope: the served operation's code is read back"
        );
        let declined = SERVED_REFUSAL_CODE
            .scope(refusal_code("UntagLogGroup"), async {
                served_refusal_code()
            })
            .await;
        assert_eq!(
            declined, None,
            "case inside UntagLogGroup's scope: the operation declares no refusal error"
        );
    }

    /// Every published order is the SAME SET the model declares, only reordered — a value
    /// the model does not declare would otherwise be stated as admissible — and it is the
    /// order the set is stated in whatever order the values reach the renderer in, while an
    /// enum with no published order is stated in the ascending order of its values.
    #[test]
    fn a_published_order_is_the_models_set_reordered() {
        // The value sets the model declares, as the generated constants hold them (each
        // sorted by the generator): `crate::ops::types::<ENUM>_VALUES`.
        let declared: &[&[&str]] = &[
            crate::ops::types::DISTRIBUTION_VALUES,
            crate::ops::types::LOG_GROUP_CLASS_VALUES,
            crate::ops::types::ORDER_BY_VALUES,
            crate::ops::types::STORAGE_TIER_VALUES,
        ];
        let as_set = |values: &[&'static str]| {
            let mut sorted = values.to_vec();
            sorted.sort_unstable();
            sorted
        };
        for published in PUBLISHED_ENUM_ORDERS {
            let set = as_set(published);
            assert!(
                declared.iter().any(|values| as_set(values) == set),
                "case {published:?}: a published order names a value set the model declares"
            );
            let reversed: Vec<&str> = published.iter().rev().copied().collect();
            for arrival in [published.to_vec(), reversed] {
                let stated = enum_value_set(arrival.iter().copied());
                assert_eq!(
                    stated,
                    published.to_vec(),
                    "case {arrival:?}: expected the published order {published:?}, got {stated:?}"
                );
            }
        }
        for values in declared {
            let stated = enum_value_set(values.iter().copied());
            let expected = published_enum_order(values.iter().copied())
                .map_or_else(|| as_set(values), <[&str]>::to_vec);
            assert_eq!(
                stated, expected,
                "case {values:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// Every constraint class states `Value at '<member>'` and no value — an enum stated in
    /// the one order this service lists its values in, a `@pattern` with the anchors the
    /// model writes, a bound, and an absent member with no `null` in the subject — and
    /// several violations take the plural envelope, counted and joined the way AWS counts
    /// and joins them.
    #[test]
    fn a_clause_states_the_member_and_the_requirement_without_a_value() {
        let outside_enum = Violation {
            path: "storageTier".to_string(),
            // The model's declaration order, which is not the order a refusal lists.
            kind: ViolationKind::EnumValue(vec![
                "STANDARD".to_string(),
                "INTELLIGENT_TIERING".to_string(),
            ]),
            value: Some("GLACIER".to_string()),
            sensitive: false,
        };
        let outside_published_enum = Violation {
            path: "distribution".to_string(),
            // An enum this service publishes an order for, handed to the renderer in the
            // model's declaration order.
            kind: ViolationKind::EnumValue(vec![
                "Random".to_string(),
                "ByLogStream".to_string(),
            ]),
            value: Some("ByShard".to_string()),
            sensitive: false,
        };
        let outside_pattern = Violation {
            path: "lookupTableName".to_string(),
            kind: ViolationKind::Pattern("^[a-zA-Z0-9_]+$".to_string()),
            value: Some("bad-name-with-hyphen".to_string()),
            sensitive: false,
        };
        let above_bound = Violation {
            path: "limit".to_string(),
            kind: ViolationKind::MaxValue("50".to_string()),
            value: Some("51".to_string()),
            sensitive: false,
        };
        let absent = Violation {
            path: "storageTier".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let cases = [
            (
                "a value outside an enum",
                &outside_enum,
                "Value at 'storageTier' failed to satisfy constraint: Member must satisfy \
                 enum value set: [INTELLIGENT_TIERING, STANDARD]",
            ),
            (
                "a value outside an enum with a published order",
                &outside_published_enum,
                "Value at 'distribution' failed to satisfy constraint: Member must satisfy \
                 enum value set: [Random, ByLogStream]",
            ),
            (
                "a value outside a pattern",
                &outside_pattern,
                "Value at 'lookupTableName' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: ^[a-zA-Z0-9_]+$",
            ),
            (
                "a value above a bound",
                &above_bound,
                "Value at 'limit' failed to satisfy constraint: Member must have value \
                 less than or equal to 50",
            ),
            (
                "an absent member",
                &absent,
                "Value at 'storageTier' failed to satisfy constraint: Member must not be \
                 null",
            ),
        ];
        for (case, violation, expected) in cases {
            let actual = constraint_clause(violation);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case}: expected {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(constraint_envelope(&[]), None, "nothing to state");
        assert_eq!(
            constraint_envelope(std::slice::from_ref(&above_bound)).as_deref(),
            Some(
                "1 validation error detected: Value at 'limit' failed to satisfy \
                 constraint: Member must have value less than or equal to 50"
            ),
            "one violation takes the singular envelope"
        );
        assert_eq!(
            constraint_envelope(&[above_bound, absent]).as_deref(),
            Some(
                "2 validation errors detected: Value at 'limit' failed to satisfy \
                 constraint: Member must have value less than or equal to 50; Value at \
                 'storageTier' failed to satisfy constraint: Member must not be null"
            ),
            "several violations are counted and joined"
        );
    }
}
