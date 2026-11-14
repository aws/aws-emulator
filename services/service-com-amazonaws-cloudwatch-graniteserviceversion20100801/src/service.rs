// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudwatch-graniteserviceversion20100801` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: not a re-route. The key and the handler stay the
//!   generated routing table's, but every operation is served through [`Metering`], so each
//!   call this service answers is counted into the usage telemetry it publishes about its own
//!   API, through [`QueryCompatibleErrors`], so whatever failure it answers names its
//!   error class the way an `awsQueryCompatible` service names one — the qualified shape id in
//!   the body and the legacy query code in the `x-amzn-query-error` header
//!   ([`query_compatible_errors`]) — and through [`ReportingTheRequestId`], so a failure it
//!   answers on the query wire names the request it answered ([`request_protocol`]);
//!   `PutMetricData` is served through
//!   [`ReportingDroppedEntities`], so an accepted call
//!   whose entity this service could not hold says so in its response header, and
//!   `GetMetricStatistics` through [`JudgingTheValuesThisServiceEnforces`], so a value outside
//!   an enumerated type is refused as one of the parameter faults ITS model declares and a
//!   metric namespace the service accepts is served past the constraint walk — as it is for the
//!   other three metric operations a namespace is submitted to — and
//!   `PutMetricStream` through [`RefusingAValueFirst`], so the two members CloudWatch judges
//!   as parameter VALUES are judged before the model's own shape checks.
//!   The key itself is read through the protocol THIS REQUEST is served in, so a call in any
//!   of the three protocols this service's model declares is served by the same wrappers.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
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
//! do not write it; editing the entry would let this file drift from the model. What this
//! file DOES own there is a pair of wrappers round it ([`ServedDefinition`]): the protocol
//! each request is served in, which follows from the request's content type, and the
//! `<RequestId>` an `awsQuery` error envelope carries ([`request_protocol`]).

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, Operation,
    OperationContext, OperationError, OperationRoutingTable, Refusal, Refused, ServerHttpProtocol,
    Service,
};
use std::sync::Arc;

pub mod query_compatible_errors;
pub mod request_protocol;

use query_compatible_errors::{
    QueryCompatibleErrors, MISSING_PARAMETER, PARAMETER_VALUE, VALIDATION,
};
use request_protocol::{ReportingTheRequestId, TheProtocolOfTheRequest};

/// This service's model-derived half as it is SERVED: the generated entry, serving each
/// request in the protocol that request itself names, under the error envelope an `awsQuery`
/// failure carries ([`request_protocol`]).
///
/// Nothing model-derived is restated by either wrapper — identity, the routing table and the
/// declared protocols are the generated entry's, reached through them.
type ServedDefinition<Eng> = service_common::query_error_envelope::RequestIdEnvelopeDefinition<
    TheProtocolOfTheRequest<crate::ServiceDefinition<Eng>>,
>;

/// Run `work` without occupying an async worker.
///
/// The alarm-evaluation pass ([`crate::state::alarm_actions::settle_alarms`]) is CPU-bound
/// and holds this service's store lock for its whole span, so it must not run on the thread
/// the runtime serves other requests with. `block_in_place` moves the current task off that
/// worker, which keeps the runtime serving while the pass runs; it is available only on a
/// multi-thread runtime — the one the emulator boots — so on a current-thread runtime (a
/// `#[tokio::test]`, or a caller driving the router in-process) the work runs inline, where
/// there is no other worker to protect.
pub(crate) fn off_the_async_worker<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

/// The `com-amazonaws-cloudwatch-graniteserviceversion20100801` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: ServedDefinition<Eng>,
    /// The operations whose ANSWER this file owns a part of — the one whose accepted response
    /// carries something its body cannot ([`ReportingDroppedEntities`]), the four metric
    /// operations a namespace is submitted to, whose request values are judged by what this
    /// service enforces ([`JudgingTheValuesThisServiceEnforces`]), and the metric-stream writer
    /// whose own parameter-value rules are judged ahead of the boundary walk
    /// ([`RefusingAValueFirst`]). Each is built from the SAME shared engine
    /// as the generated table's and keyed by the routing key the protocol resolves, so the
    /// operation served is the modelled one, and each is METERED ([`Metering`]) like every
    /// other operation of this service and answers a failure under the class this
    /// `awsQueryCompatible` service names it under ([`QueryCompatibleErrors`]).
    overridden: OperationRoutingTable,
    /// Every OTHER modelled operation, served through the generated table and metered
    /// ([`Metering`] over [`TheModelledOperation`]): the call count this service publishes
    /// about its own API is owed by every operation, so the seam that counts a call is the one
    /// every request passes through rather than a line in each handler. The whole chain sits
    /// inside [`QueryCompatibleErrors`], for the same reason: naming the class of a failure is
    /// owed by every answer, whichever layer built it.
    metered: ReportingTheRequestId<QueryCompatibleErrors<Metering<Eng, TheModelledOperation>>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut overridden = OperationRoutingTable::new();
        // `PutMetricData` is served through [`JudgingTheValuesThisServiceEnforces`] for both
        // decisions that wrapper makes: a publish into a namespace the model's `@pattern` does
        // not admit is one the service SERVES ([`is_unenforced_constraint`]), and a value
        // outside an enumerated type is refused as one of the parameter faults its model
        // declares. The entity header its accepted answer carries is added outside it
        // ([`ReportingDroppedEntities`]), over whichever of the two layers answered.
        overridden.add_operation(
            "PutMetricData",
            served(
                &engine,
                ReportingDroppedEntities::new(JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::put_metric_data::PutMetricData::new(Arc::clone(&engine)),
                    parameter_fault_for,
                )),
            ),
        );
        // `GetMetricStatistics` validates the SAME enumerated types as the retrieving read and
        // answers a value outside one as one of the parameter faults its model declares, which
        // the refusal seam cannot do because it cannot see which operation was called
        // ([`JudgingTheValuesThisServiceEnforces`]); every other operation keeps this service's
        // own validation code ([`scalar_enum_refusal`]).
        overridden.add_operation(
            "GetMetricStatistics",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::get_metric_statistics::GetMetricStatistics::new(Arc::clone(
                        &engine,
                    )),
                    parameter_fault_for,
                ),
            ),
        );
        // The other two reads of the metric data plane take a namespace from the caller too —
        // the one a query names and the one a catalogue listing filters by — so they are served
        // through the same wrapper, with the service-wide wording for every refusal they do
        // make ([`the_service_wide_refusal`]): what the wrapper adds for them is that a
        // namespace the service accepts is READ BACK rather than refused, so a series published
        // into one can be retrieved and listed.
        overridden.add_operation(
            "GetMetricData",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::get_metric_data::GetMetricData::new(Arc::clone(&engine)),
                    the_service_wide_refusal,
                ),
            ),
        );
        overridden.add_operation(
            "ListMetrics",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::list_metrics::ListMetrics::new(Arc::clone(&engine)),
                    listing_value_fault,
                ),
            ),
        );
        // `PutMetricStream` judges the two members CloudWatch judges as parameter VALUES —
        // the IAM role the stream delivers through and the stream's own name — BEFORE the
        // framework's constraint walk, so one of those refusals wins over a modelled
        // constraint the walk would otherwise answer first ([`RefusingAValueFirst`]).
        overridden.add_operation(
            "PutMetricStream",
            served(
                &engine,
                RefusingAValueFirst::new(
                    crate::ops::put_metric_stream::PutMetricStream::new(Arc::clone(&engine)),
                    crate::ops::put_metric_stream::parameter_value_refusal,
                ),
            ),
        );
        // The Contributor Insights rule operations state a refusal about ONE request member in
        // CloudWatch's coded form, under the parameter fault the class belongs to: a required
        // member the request left out entirely ([`missing_required_member_refusal`]) and a page
        // size below the published minimum of the member that names it
        // ([`non_positive_page_size_refusal`]) — classes the service-wide seam cannot word,
        // because it cannot see which operation was called.
        overridden.add_operation(
            "DescribeInsightRules",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::describe_insight_rules::DescribeInsightRules::new(Arc::clone(
                        &engine,
                    )),
                    coded_member_fault,
                ),
            ),
        );
        overridden.add_operation(
            "ListManagedInsightRules",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::list_managed_insight_rules::ListManagedInsightRules::new(
                        Arc::clone(&engine),
                    ),
                    coded_member_fault,
                ),
            ),
        );
        overridden.add_operation(
            "PutManagedInsightRules",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::put_managed_insight_rules::PutManagedInsightRules::new(
                        Arc::clone(&engine),
                    ),
                    coded_member_fault,
                ),
            ),
        );
        // The two operations that MOVE a rule's state report one entry of `Failures` per rule
        // name they could not write the state on, so they are served through the same wrapper
        // for its OTHER decision: a `RuleNames` entry whose characters or length the member's
        // own constraints do not admit is a per-entry outcome of the batch
        // ([`is_unenforced_constraint`]), judged inside the handler's walk, so the request is
        // SERVED — the rest of the batch takes effect and the offending entry comes back
        // classified. Every refusal they do make keeps the service-wide wording
        // ([`the_service_wide_refusal`]), so an absent `RuleNames` reads exactly as before.
        overridden.add_operation(
            "EnableInsightRules",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::enable_insight_rules::EnableInsightRules::new(Arc::clone(&engine)),
                    the_service_wide_refusal,
                ),
            ),
        );
        overridden.add_operation(
            "DisableInsightRules",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::disable_insight_rules::DisableInsightRules::new(Arc::clone(
                        &engine,
                    )),
                    the_service_wide_refusal,
                ),
            ),
        );
        Self {
            metered: the_way_every_operation_is_served(Arc::clone(&engine), TheModelledOperation),
            entry: service_common::query_error_envelope::RequestIdEnvelopeDefinition::new(
                TheProtocolOfTheRequest::new(crate::ServiceDefinition::new(engine)),
            ),
            overridden,
        }
    }
}

/// `operation` served the way EVERY operation of this service is: counted into the usage
/// telemetry this service publishes about its own API ([`Metering`]), naming the class of
/// whatever failure it answers in full ([`QueryCompatibleErrors`]), and naming the request it
/// answered in the `awsQuery` error envelope ([`ReportingTheRequestId`]).
///
/// The three are owed by every answer, whichever layer built it, so the chain is stated here
/// once rather than at each of the operations whose answer this file owns a part of.
fn the_way_every_operation_is_served<Eng: Send + Sync + 'static, O: ErasedOperation>(
    engine: Arc<Eng>,
    operation: O,
) -> ReportingTheRequestId<QueryCompatibleErrors<Metering<Eng, O>>> {
    ReportingTheRequestId::new(QueryCompatibleErrors::new(Metering::new(engine, operation)))
}

/// The same chain as a table entry: [`the_way_every_operation_is_served`] boxed for
/// [`OperationRoutingTable::add_operation`].
fn served<Eng: Send + Sync + 'static, O: ErasedOperation + 'static>(
    engine: &Arc<Eng>,
    operation: O,
) -> Box<dyn ErasedOperation> {
    Box::new(the_way_every_operation_is_served(
        Arc::clone(engine),
        operation,
    ))
}

/// The refusal classes CloudWatch answers as one of its own parameter faults, and the whole
/// rewording this service does.
///
/// A constraint the boundary validator catches is refused before any handler runs, so the
/// framework answers it under the protocol's generic `ValidationException`. CloudWatch's
/// metric operations model the parameter faults instead — `InvalidParameterValueException` is
/// the only client error `ListMetrics` declares, and `GetMetricStatistics` declares it beside
/// the combination and missing-parameter faults — so these two classes are stated under that
/// code, in this service's own parameter wording ([`crate::state`]'s shared sentences, the
/// same ones its handlers refuse with), and a client branching on the code reaches its
/// handler:
///
/// * a `Dimensions` collection longer than its cap, named with the cap it broke
///   ([`dimension_cap_refusal`]);
/// * a required value submitted EMPTY, which is a parameter the request supplied no value for
///   ([`empty_required_value_refusal`]);
/// * a value inside a LIST that is outside the enumerated type the list's member names — a
///   percentile passed in `Statistics`, whose place is `ExtendedStatistics` — named as the
///   request member path it arrived on and the value set the type declares;
/// * a SCALAR member outside the enumerated type it is modelled as, which keeps the
///   framework's own clause and carries the request-validation class
///   ([`scalar_enum_refusal`]).
///
/// Every other violation keeps the framework's code and wording: a scalar outside its own
/// LENGTH or range bound is what this service is recorded answering `ValidationException`
/// for, and a code a client branches on is not changed without a recording that shows it.
///
/// This seam cannot see WHICH operation was called, and which validator answers a value
/// outside an enumerated type is an operation's property: the read whose model declares the
/// parameter faults answers one of THEM rather than the code above, so it is served through
/// [`JudgingTheValuesThisServiceEnforces`] and never reaches here with such a violation.
fn parameter_fault(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    violations
        .iter()
        .find_map(dimension_cap_refusal)
        .or_else(|| violations.iter().find_map(empty_required_value_refusal))
        .or_else(|| violations.iter().find_map(omitted_required_member_refusal))
        .or_else(|| violations.iter().find_map(list_element_enum_refusal))
        .or_else(|| scalar_enum_refusal(violations))
}

/// The refusal of a value submitted EMPTY where the model admits no empty one, or `None` when
/// `violation` is not one.
///
/// A parameter carrying no characters is a parameter the request supplied no VALUE for, and
/// that is the fault CloudWatch answers: the missing-parameter class ([`MISSING_PARAMETER`],
/// "An input parameter that is required is missing"), naming the request member path it was
/// looked for at ([`crate::state::parameter_required_message`]) — not the length clause the
/// model's own minimum would state, which tells a caller that supplied nothing to supply
/// something longer.
///
/// It is keyed on the CLASS of violation and nothing else — a minimum length the submitted
/// value broke by being empty — so it holds for every member of every operation whose model
/// declares such a minimum, at any depth: the dimension name and value a metric is published
/// with are the recorded case.
fn empty_required_value_refusal(
    violation: &aws_smithy_schema_server::Violation,
) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MinLength(_) = violation.kind else {
        return None;
    };
    if !violation.value.as_deref().is_some_and(str::is_empty) {
        return None;
    }
    Some(Refused {
        code: MISSING_PARAMETER.shape_name.to_string(),
        message: crate::state::parameter_required_message(&crate::state::query_parameter_path(
            &violation.path,
        )),
    })
}

/// The refusal of a required member the request did not supply AT ALL, or `None` when
/// `violation` is not one.
///
/// The same class and the same sentence as a required member supplied EMPTY
/// ([`empty_required_value_refusal`]): CloudWatch answers the missing-parameter fault naming the
/// request member path the value was looked for at, and a member the request left out and one it
/// carried with no characters are both parameters it supplied no value for. Keyed on the CLASS of
/// violation alone, so it holds for every required member of every operation whose refusals this
/// service words, at any depth — the `Name` of a dimension FILTER a metric listing is narrowed
/// by is the recorded case.
fn omitted_required_member_refusal(
    violation: &aws_smithy_schema_server::Violation,
) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MissingRequiredMember = violation.kind else {
        return None;
    };
    Some(Refused {
        code: MISSING_PARAMETER.shape_name.to_string(),
        message: crate::state::parameter_required_message(&crate::state::query_parameter_path(
            &violation.path,
        )),
    })
}

/// The refusal policy of the metric LISTING: a filter value outside the enumerated type its
/// member names, in the fixed sentence CloudWatch refuses one with, and nothing else — every
/// other class keeps the service-wide wording behind it ([`the_service_wide_wording`]).
///
/// The listing's filters are judged as VALUES rather than as the shape of the request: a window
/// to filter recent activity by that this service does not offer is a parameter value it cannot
/// use ([`crate::state::request_parameter_invalid_message`], the parameter named as the wire
/// spells it), under the parameter-value fault the operation's model declares — which is the
/// only client error it declares at all
/// (`smithy_query(query="operation",operation="ListMetrics")`).
fn listing_value_fault(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        let aws_smithy_schema_server::ViolationKind::EnumValue(_) = violation.kind else {
            return None;
        };
        Some(Refused {
            code: PARAMETER_VALUE.shape_name.to_string(),
            message: crate::state::request_parameter_invalid_message(&violation.path),
        })
    })
}

/// The refusal policy of the Contributor Insights rule operations: a page size below the
/// published minimum of the member that asks for one ([`non_positive_page_size_refusal`]) and a
/// required member the request did not supply at all, worded the way those operations word a
/// refusal about one request member ([`missing_required_member_refusal`]) — and nothing else:
/// every other class keeps the service-wide wording behind it ([`the_service_wide_wording`]).
fn coded_member_fault(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    violations
        .iter()
        .find_map(non_positive_page_size_refusal)
        .or_else(|| violations.iter().find_map(missing_required_member_refusal))
}

/// The refusal of a page size below the published minimum of the member that asks for one, or
/// `None` when `violation` is not one.
///
/// The class is the parameter-value fault ([`PARAMETER_VALUE`], "The value of an input parameter
/// is bad or out-of-range"), and the sentence is the coded form the Contributor Insights
/// operations state a refusal about one member in
/// ([`crate::state::NON_POSITIVE_MAX_RESULTS`]): a listing's smallest page is one result, so a
/// value under that asks for a page that could report nothing, which is a VALUE the service will
/// not use rather than a request whose shape it could not read.
///
/// Keyed on the CLASS of violation and the member — a page-size member below its modelled range
/// minimum — so it holds for every listing of these operations that takes one, at any depth
/// ([`is_page_size`]): the Contributor Insights rule listing is the recorded case. The bound the
/// sentence states is that member's own: both listings' page size targets `InsightRuleMaxResults`,
/// whose range minimum is one result
/// (`smithy_query(query="operation",operation="DescribeInsightRules,ListManagedInsightRules")`),
/// so a value below it is a value at or under zero.
fn non_positive_page_size_refusal(
    violation: &aws_smithy_schema_server::Violation,
) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MinValue(_) = violation.kind else {
        return None;
    };
    if !is_page_size(&violation.path) {
        return None;
    }
    Some(Refused {
        code: PARAMETER_VALUE.shape_name.to_string(),
        message: crate::state::NON_POSITIVE_MAX_RESULTS.to_string(),
    })
}

/// Whether `path` points at the member a listing of these operations asks for a page size with,
/// in the spelling the validating walk reports — at whatever depth the walk reports it at, which
/// is the last segment of the path it builds.
fn is_page_size(path: &str) -> bool {
    path.split('.').next_back() == Some("maxResults")
}

/// The refusal of a required member the request did not supply AT ALL, or `None` when
/// `violation` is not one.
///
/// The class is the missing-parameter fault ([`MISSING_PARAMETER`], "An input parameter that is
/// required is missing") — the same one a required member supplied EMPTY is answered under
/// ([`empty_required_value_refusal`]), because a member the request omitted is a parameter it
/// supplied no value for — and the sentence is the coded form the Contributor Insights
/// operations state a refusal about one member in
/// ([`crate::state::missing_parameter_message`]), naming the member in the spelling this
/// service's refusals name a request member by.
///
/// Keyed on the CLASS of violation and nothing else — a member the model declares required that
/// the request carried no value for — so it holds for every required member of the operations
/// mounted with this policy, at any depth: the resource a managed-rule listing is asked about
/// is the recorded case.
fn missing_required_member_refusal(
    violation: &aws_smithy_schema_server::Violation,
) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MissingRequiredMember = violation.kind else {
        return None;
    };
    Some(Refused {
        code: MISSING_PARAMETER.shape_name.to_string(),
        message: crate::state::missing_parameter_message(&crate::state::query_parameter_path(
            &violation.path,
        )),
    })
}

/// The refusal of a value against a modelled `@pattern`, quoting the expression the way AWS
/// quotes it — or `None` when nothing in `refusal` broke a pattern.
///
/// A `@pattern` is written in the model as a WHOLE-VALUE match: the `^` and the `$` are how
/// Smithy turns a find into a total match, not part of the expression the service advertises.
/// AWS states the requirement as that expression alone, so a client that lifts it out of the
/// message and re-validates locally gets the one the service publishes
/// ([`service_common::validation::unanchored`], the shared rendering). Everything else about
/// the refusal — the envelope, the clause, the member path, the class — is the framework's and
/// the shared renderer's, so this and the walk behind it cannot drift.
///
/// It holds for every member of this service whose model declares a `@pattern`, whatever the
/// operation: the dataset identifier and the KMS key ARN the dataset operations take are the
/// recorded cases, and a member modelled with a pattern later is stated the same way without
/// another line here.
fn unanchored_pattern_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if !violations.iter().any(|violation| {
        matches!(
            violation.kind,
            aws_smithy_schema_server::ViolationKind::Pattern(_)
        )
    }) {
        return None;
    }
    service_common::validation_message::aws_validation_refusal_with(
        refusal,
        service_common::validation::unanchored,
    )
}

/// The refusal of a SCALAR member carrying a value outside the enumerated type it is modelled
/// as, or `None` when no violation of the refusal is one.
///
/// CloudWatch judges a request against its model before any handler runs and answers a value
/// outside an enumerated type as the REQUEST-VALIDATION class ([`VALIDATION`], the class
/// [`crate::scaffolded::Scaffolded::validation_error`] already answers this service's
/// handler-side validation refusals under), so a client branches on one class whichever layer
/// refused the request.
///
/// Only the code and two RENDERINGS are this service's; the envelope, the clauses and the
/// member paths are the shared ones, so this and the walk behind it cannot drift:
///
/// * the value set is stated in the order the SET ITSELF is walked
///   ([`service_common::validation_message::in_hashed_enum_order`]), which is the order this
///   service is recorded listing an enumerated type's values in — neither the model's
///   declaration order nor a sorted one;
/// * the clause quotes the SUBMITTED value for every enumerated type but the ones this service
///   is recorded naming the member alone for ([`VALUE_WITHHELD_ENUM_TYPES`]).
fn scalar_enum_refusal(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    if !violations.iter().any(is_scalar_enum_violation) {
        return None;
    }
    let stated = Refusal::Invalid(
        violations
            .iter()
            .map(service_common::validation_message::in_hashed_enum_order)
            .collect(),
    );
    // The whole envelope is the shared renderer's; only the CODE and which clauses carry a
    // value half are this service's.
    let message =
        service_common::validation_message::aws_validation_refusal_withholding_violations(
            &stated,
            withholds_the_submitted_value,
        )?
        .message;
    Some(Refused {
        code: VALIDATION.shape_name.to_string(),
        message,
    })
}

/// The enumerated TYPES whose refusal states NO submitted value, each row holding the values
/// its type declares.
///
/// WHICH HALVES a clause carries — the member path alone (`Value at '<path>'`) or the member
/// path and the value the request sent (`Value '<value>' at '<path>'`) — is a property of the
/// enumerated TYPE, exactly as the ORDER that type's values are listed in is
/// ([`service_common::validation_message::in_hashed_enum_order`]): it is read off what this
/// service is recorded answering for the type and derives from nothing the model carries, so
/// one type is stated alike wherever it is carried — whichever operation was called, whichever
/// member the value arrived on and at whatever depth. One type so far: the SCAN DIRECTION a
/// read is ordered by, whose refusal names the member alone. A type with no row keeps the value
/// half the framework's clause carries, which is what tells a client which of the values it
/// sent was rejected — a near miss of a declared value (a mis-cased one, say) cannot be told
/// from an unsupported one without it.
const VALUE_WITHHELD_ENUM_TYPES: &[&[&str]] = &[crate::ops::types::shared_types::SCAN_BY_VALUES];

/// Whether the clause for `violation` states no submitted value: a value outside one of the
/// enumerated types stated without one ([`VALUE_WITHHELD_ENUM_TYPES`]), and nothing else.
///
/// The type is recognised by the values the violation carries, matched as a SET, so a row holds
/// however the schema presented those values and wherever the value sat in the request — the
/// member path is not consulted at all.
fn withholds_the_submitted_value(violation: &aws_smithy_schema_server::Violation) -> bool {
    let aws_smithy_schema_server::ViolationKind::EnumValue(ref carried) = violation.kind else {
        return false;
    };
    let admissible: std::collections::BTreeSet<&str> =
        carried.iter().map(String::as_str).collect();
    VALUE_WITHHELD_ENUM_TYPES.iter().any(|withheld| {
        withheld.len() == admissible.len()
            && withheld.iter().all(|value| admissible.contains(value))
    })
}

/// Whether `violation` is a SCALAR member outside the enumerated type it is modelled as — the
/// one class this service restates ([`scalar_enum_refusal`]); a value inside a LIST is the
/// class [`list_element_enum_refusal`] answers instead.
fn is_scalar_enum_violation(violation: &aws_smithy_schema_server::Violation) -> bool {
    matches!(
        violation.kind,
        aws_smithy_schema_server::ViolationKind::EnumValue(_)
    ) && !is_list_element(&violation.path)
}

/// The refusal of a `Dimensions` collection longer than the dimensions it may carry, or
/// `None` when `violation` is not one.
///
/// The cap CloudWatch enforces on a dimension collection is the per-metric dimension maximum
/// ([`crate::state::MAX_METRIC_DIMENSIONS`]), whatever the operation; `ListMetricsInput`'s own
/// filter list is modelled BELOW it (`length 0..10`), and a collection inside the per-metric
/// maximum but outside that member's own bound is refused naming THAT bound — the refusal
/// names the cap the collection actually broke, and this seam chooses the words of a refusal
/// rather than whether there is one.
fn dimension_cap_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MaxLength(declared) = violation.kind else {
        return None;
    };
    if !is_dimension_collection(&violation.path) {
        return None;
    }
    let submitted =
        service_common::validation_message::list_item_count(violation.value.as_deref()?)?;
    let cap = if submitted > crate::state::MAX_METRIC_DIMENSIONS {
        crate::state::MAX_METRIC_DIMENSIONS
    } else {
        usize::try_from(declared).ok()?
    };
    Some(Refused {
        // The class's shape, as every refusal this file words names it — see
        // [`enum_value_refusal`].
        code: PARAMETER_VALUE.shape_name.to_string(),
        message: crate::state::collection_too_large_message(
            &crate::state::query_parameter_path(&violation.path),
            cap,
        ),
    })
}

/// The refusal of an element of a LIST holding a value outside the enumerated type the list's
/// member names, or `None` when `violation` is not one.
///
/// Stated the way CloudWatch states a value outside the set a member accepts
/// ([`crate::state::value_in_set_message`]): the request member path the element arrived on as
/// a query-protocol refusal names it, down to the list's own target shape
/// (`Statistics.member.1.<list element>`), and the whole value set the enumerated type
/// declares — which is what lets a client repair the request. The submitted value is NOT
/// echoed; the set it had to come from is what locates the repair.
fn list_element_enum_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    is_list_element(&violation.path)
        .then(|| enum_value_refusal(violation))
        .flatten()
}

/// The refusal of a value outside the enumerated type its member targets, as an operation that
/// MODELS this service's parameter faults states it, or `None` when `violation` is not one.
///
/// One sentence for every such value ([`crate::state::value_in_set_message`]): the request
/// member path it arrived on and the whole value set the type declares, and nothing of the
/// submitted value — the set is what lets a client repair the request. The path is the one the
/// validating walk found, spelled as this service's refusals spell a request member
/// ([`crate::state::query_parameter_path`]), with the step into a list's own target shape added
/// for an ELEMENT of a list, which has no member name of its own.
fn enum_value_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::EnumValue(ref allowed) = violation.kind else {
        return None;
    };
    let member = crate::state::query_parameter_path(&violation.path);
    let parameter = if is_list_element(&violation.path) {
        crate::state::list_target_path(&member)
    } else {
        member
    };
    Some(Refused {
        // The SHAPE of the class, which is what an awsJson error body names; the legacy query
        // code a CloudWatch client branches on is reported in the header, by the wrapper every
        // operation of this service is served through ([`QueryCompatibleErrors`]), so this
        // refusal and the same class answered from a handler read alike.
        code: PARAMETER_VALUE.shape_name.to_string(),
        message: crate::state::value_in_set_message(&parameter, allowed),
    })
}

/// Whether `path` points at a request's `Dimensions` collection — of any operation, and at
/// any depth, since a metric's dimension set is one shape wherever it is submitted.
fn is_dimension_collection(path: &str) -> bool {
    path.split('.').next_back() == Some("dimensions")
}

/// Whether `path` points at an ELEMENT of a list rather than at a member of its own: the
/// validating walk ends such a path with the step from the collection into the element.
fn is_list_element(path: &str) -> bool {
    path.split('.').next_back() == Some("member")
}

/// The status a refused request value carries: a value the model does not admit is the
/// caller's to repair, so every class below is a client error.
const REFUSAL_STATUS: u16 = 400;

/// How an operation whose model DECLARES this service's parameter faults states a boundary
/// refusal, or `None` when nothing in `violations` is one of the classes it words.
///
/// `GetMetricStatistics` declares `InvalidParameterValueException` ("The value of an input
/// parameter is bad or out-of-range", the class [`PARAMETER_VALUE`]) beside
/// the combination and missing-parameter faults, so a client of it branches on that code and
/// the sentence is this service's own: the dimension cap first, then a required value supplied
/// empty, then a value outside an
/// enumerated type, in the SAME order as the service-wide seam ([`parameter_fault`]), so one
/// operation cannot answer a request that breaks two of them differently from every other. An
/// operation that declares none of those faults keeps the request-validation class
/// instead ([`scalar_enum_refusal`]).
fn parameter_fault_for(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    violations
        .iter()
        .find_map(dimension_cap_refusal)
        .or_else(|| violations.iter().find_map(empty_required_value_refusal))
        .or_else(|| violations.iter().find_map(omitted_required_member_refusal))
        .or_else(|| violations.iter().find_map(enum_value_refusal))
}

/// Whether `violation` broke a constraint this service does NOT enforce on the wire.
///
/// Three such members, all of them values the service reads or applies rather than values it
/// refuses:
///
/// * a metric NAMESPACE: `Namespace` is modelled `@pattern "^[^:]"`, and the service accepts a
///   namespace whatever character it begins with — the only namespace space it refuses a
///   publish into is the one AWS reserves for its own services' metrics
///   (`crate::ops::put_metric_data`'s own check, which no model constraint expresses). A client
///   publishing into a leading-colon namespace is SERVED by AWS and can then read the series
///   back, so refusing the request here would make the data plane unusable for a namespace AWS
///   holds data in.
/// * a managed rule's TEMPLATE NAME: `TemplateName` is modelled `@pattern`, and
///   `PutManagedInsightRules` resolves the name against the catalog instead of refusing the
///   request for its SHAPE — a name no template answers to is reported back in that call's
///   `Failures`, per entry, on the HTTP 200 ("If the operation could not process one of the
///   rules, the following data is returned for each of those rules.", `PartialFailure`), which
///   is what keeps the other entries of the same batch enabled.
/// * a DIMENSION FILTER list longer than that member's own modelled maximum
///   ([`is_applicable_dimension_filter`]): what bounds a dimension collection of this service is
///   the per-metric dimension maximum, so a filter list inside it names dimensions a metric can
///   actually carry and is APPLIED — a listing filtered by more of them simply matches fewer
///   metrics, and the recorded answer to one is an empty listing rather than a refusal.
/// * a `RuleNames` ENTRY of a batch Contributor Insights rule operation
///   ([`is_batch_rule_name_entry`]): such an operation reports per-ENTRY outcomes
///   ("If the operation could not process one of the rules, the following data is returned for
///   each of those rules.", `PartialFailure`), so the length and the pattern the entry's member
///   declares are judged inside the handler's per-entry walk — the entry at fault comes back as
///   one `Failures` entry on the HTTP 200 and every other name in the batch is still applied,
///   which refusing the request for its shape would make unreachable.
///
/// Each is keyed on the member and the CLASS of violation alone — a pattern broken by the
/// namespace of a METRIC ([`is_metric_namespace`]) or by the template name of a MANAGED RULE
/// ([`is_managed_rule_template_name`]), a maximum broken by a dimension COLLECTION
/// ([`is_dimension_collection`]), a length or pattern broken by one ENTRY of a batch's rule-name
/// list — so each holds wherever that member is submitted: for
/// the namespace, inside a `MetricDataQuery` as well as at the top of a publish; for the
/// template name, at any position of the `ManagedRules` array; for the rule name, at any
/// position of the `RuleNames` array of either operation that moves a rule's state.
fn is_unenforced_constraint(violation: &aws_smithy_schema_server::Violation) -> bool {
    (matches!(
        violation.kind,
        aws_smithy_schema_server::ViolationKind::Pattern(_)
    ) && (is_metric_namespace(&violation.path)
        || is_managed_rule_template_name(&violation.path)))
        || is_applicable_dimension_filter(violation)
        || is_per_entry_rule_name_constraint(violation)
}

/// Whether `violation` is a constraint of the rule NAME one entry of a batch rule operation
/// carries — the classes that member declares, which that operation reports per entry rather
/// than refusing the request for: the characters a name is built from and the number of them.
fn is_per_entry_rule_name_constraint(violation: &aws_smithy_schema_server::Violation) -> bool {
    matches!(
        violation.kind,
        aws_smithy_schema_server::ViolationKind::Pattern(_)
            | aws_smithy_schema_server::ViolationKind::MinLength(_)
            | aws_smithy_schema_server::ViolationKind::MaxLength(_)
    ) && is_batch_rule_name_entry(&violation.path)
}

/// Whether `path` points at one ENTRY of a batch rule operation's `RuleNames` array — at any
/// position of it, which is where the validating walk puts the element's index, and nowhere
/// else: the member that carries ONE rule name (`GetInsightRuleReportInput$RuleName`,
/// `PutInsightRuleInput$RuleName`) is not an entry of a batch and keeps its constraints.
fn is_batch_rule_name_entry(path: &str) -> bool {
    let mut segments = path.split('.');
    segments.next() == Some("ruleNames") && is_list_element(path)
}

/// Whether `violation` is a dimension collection longer than the maximum its own member
/// declares, but no longer than the dimensions one metric can carry
/// ([`crate::state::MAX_METRIC_DIMENSIONS`]) — a filter this service can still apply.
///
/// `ListMetricsInput$Dimensions` is modelled `length 0..10` while a metric's dimension set holds
/// up to thirty, so a listing may legitimately be narrowed by more filters than that member's
/// own maximum admits: the collection the service refuses is one bigger than any metric could
/// match, which [`dimension_cap_refusal`] states against the cap it broke.
fn is_applicable_dimension_filter(violation: &aws_smithy_schema_server::Violation) -> bool {
    let aws_smithy_schema_server::ViolationKind::MaxLength(_) = violation.kind else {
        return false;
    };
    if !is_dimension_collection(&violation.path) {
        return false;
    }
    violation
        .value
        .as_deref()
        .and_then(service_common::validation_message::list_item_count)
        .is_some_and(|submitted| submitted <= crate::state::MAX_METRIC_DIMENSIONS)
}

/// Whether `path` points at the `TemplateName` of a `ManagedRule` entry — at any position of
/// the array, which is where the validating walk puts the element's index.
fn is_managed_rule_template_name(path: &str) -> bool {
    let mut segments = path.split('.');
    segments.next() == Some("managedRules") && path.ends_with(".templateName")
}

/// Whether `path` points at the namespace of a METRIC, which is the only member this service
/// does not enforce a modelled constraint on.
///
/// The two paths such a member arrives on, in the spelling the validating walk reports: the
/// namespace a request states for the metric it publishes or reads (`PutMetricDataInput`,
/// `GetMetricStatisticsInput`, `ListMetricsInput`), and the one inside the `Metric` a query
/// names (`GetMetricDataInput`'s `MetricStat.Metric.Namespace`, at whatever depth its array
/// puts it). Matched as those two members rather than as any member whose name happens to end
/// in `namespace`, so the exemption cannot widen to a member that is not a metric's namespace.
fn is_metric_namespace(path: &str) -> bool {
    path == "namespace" || path.ends_with(".metric.namespace")
}

/// What this wrapper decided about one request, before anything is awaited.
///
/// Its own type rather than an `Option`, because there are THREE outcomes and only one of them
/// is an answer this wrapper built.
enum Judgement {
    /// Refused here, in the words this operation refuses it in. BOXED, because a response is
    /// far larger than the two decisions that carry nothing at all.
    Refused(Box<aws_smithy_runtime_api::http::Response>),
    /// Left to the framework's own deserialize -> validate -> handle -> serialize sequence.
    TheFrameworksSequence,
    /// Served here, past the constraint walk: everything the walk found is a constraint this
    /// service does not enforce ([`is_unenforced_constraint`]).
    ServedPastTheWalk,
}

/// One modelled operation, served so that the values of a request are judged by what this
/// service ENFORCES rather than only by what its model declares.
///
/// Two decisions the framework's model-driven walk cannot make on its own:
///
/// * A constraint this service does not enforce on the wire ([`is_unenforced_constraint`]) must
///   not refuse the request: a request that breaks nothing else is SERVED — read, handled and
///   serialized here, past the walk — because the service it emulates serves it. The walk is
///   driven by the model alone and cannot know which of its constraints the service applies.
/// * Which CODE a refusal carries is a property of the OPERATION — an operation that models
///   this service's parameter faults answers one of them, where an operation that models none
///   keeps this service's own validation code ([`scalar_enum_refusal`]) — and
///   `Service::override_refusal` is handed the violations alone, with no way to tell which
///   operation was called. So the choice is made here, where the operation is known, by the
///   `refusal` policy this wrapper is mounted with ([`parameter_fault_for`], or
///   [`the_service_wide_refusal`] for an operation that keeps the service-wide wording).
///
/// Nothing else is re-implemented: a request with nothing to decide is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize and
/// every modelled error, and the violations both decisions read come from the framework's OWN
/// deserializer and constraint walk over the same input schema, so this and the walk behind it
/// cannot find a different set.
struct JudgingTheValuesThisServiceEnforces<O> {
    operation: O,
    /// How this operation states the violations this service enforces, or `None` to leave the
    /// request to the service-wide wording behind it.
    refusal: fn(&[aws_smithy_schema_server::Violation]) -> Option<Refused>,
}

impl<O> JudgingTheValuesThisServiceEnforces<O> {
    /// Wrap the operation, already constructed from the shared engine, with the policy that
    /// words its boundary refusal.
    fn new(
        operation: O,
        refusal: fn(&[aws_smithy_schema_server::Violation]) -> Option<Refused>,
    ) -> Self {
        Self { operation, refusal }
    }
}

impl<O> JudgingTheValuesThisServiceEnforces<O>
where
    O: Operation + ErasedOperation,
{
    /// Serve the request HERE, past the constraint walk that would have refused it for a
    /// constraint this service does not enforce.
    ///
    /// The SAME sequence the framework's own runs — read the input through this service's
    /// protocol, call the operation's `handle`, serialize its output or its modelled error —
    /// and nothing else, so the behaviour is still the operation's own. A body no codec can
    /// read names no values to judge and is left to the framework's own refusal. Unlike the
    /// generated walk it records no per-operation telemetry; the CALL is still counted, by the
    /// wrapper every operation of this service is served through ([`Metering`]).
    async fn served_past_the_walk(
        &self,
        svc: &dyn Service,
        req: &aws_smithy_runtime_api::http::Request,
        ctx: &OperationContext<'_>,
    ) -> aws_smithy_runtime_api::http::Response {
        // The protocol is read, used and dropped inside each statement below:
        // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await between
        // them would make this future non-`Send`. It is the protocol THIS REQUEST is served in
        // ([`request_protocol`]), so a query-protocol call is read and answered in the query
        // protocol here exactly as it is in the framework's own sequence.
        let read = {
            let protocol = svc.definition().wire_protocol(req).protocol();
            read_input::<O>(protocol, req)
        };
        let Some(input) = read else {
            return self.operation.run(svc, req, ctx).await;
        };
        let answered = self.operation.handle(input, ctx).await;
        let protocol = svc.definition().wire_protocol(req).protocol();
        match answered {
            Ok(output) => protocol
                .serialize_output(&output, O::OUTPUT_SCHEMA, O::NAME, O::SUCCESS_STATUS)
                .unwrap_or_else(|error| svc.definition().framework_error_response(error, req)),
            // The modelled error as this service renders one, header and all
            // ([`modelled_error_response`]), so a failure answered here and the same failure
            // answered from the framework's sequence are one response.
            Err(error) => modelled_error_response(protocol, req, &error),
        }
    }
}

impl<O> ErasedOperation for JudgingTheValuesThisServiceEnforces<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement, for the reason
            // [`Self::served_past_the_walk`] states.
            let judgement = {
                let protocol = svc.definition().wire_protocol(req).protocol();
                // The violations come from the framework's OWN deserializer and constraint
                // walk, collected by the one shared helper every service that words its own
                // pre-handler refusal uses, so this and the walk that follows it cannot find
                // a different set. A request the codec cannot read at all yields none, and so
                // falls through to the framework's own sequence.
                let (unenforced, enforced): (Vec<_>, Vec<_>) =
                    service_common::validation::refusing_violations::<O>(protocol, req)
                        .into_iter()
                        .partition(is_unenforced_constraint);
                judgement(protocol, &self.refusal, unenforced, enforced)
            };
            match judgement {
                Judgement::Refused(response) => *response,
                Judgement::TheFrameworksSequence => self.operation.run(svc, req, ctx).await,
                Judgement::ServedPastTheWalk => self.served_past_the_walk(svc, req, ctx).await,
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// What to do with a request whose constraint walk found `enforced` violations this service
/// enforces beside `unenforced` ones it does not.
///
/// A request with nothing enforceable broken is served past the walk when the walk DID find
/// something — the walk would otherwise refuse it — and is left to the framework's sequence
/// when it found nothing at all, which is every ordinary request.
///
/// Otherwise the request is refused, and the words are this operation's own `refusal` policy's.
/// When that policy words none of the violations, a request the walk would have answered on
/// its own is left to it — keeping the service-wide wording ([`parameter_fault`], reached
/// through `Service::override_refusal`) — while one carrying an unenforced violation beside
/// them is worded HERE over the enforced violations ALONE, so a refusal never states a
/// requirement this service does not apply.
fn judgement(
    protocol: &dyn ServerHttpProtocol,
    refusal: &fn(&[aws_smithy_schema_server::Violation]) -> Option<Refused>,
    unenforced: Vec<aws_smithy_schema_server::Violation>,
    enforced: Vec<aws_smithy_schema_server::Violation>,
) -> Judgement {
    if enforced.is_empty() {
        // A walk that filled its collecting bound may have left a violation this service DOES
        // enforce out of the set, so a request at the bound is not one this file can read as
        // carrying nothing enforceable: it goes to the framework's sequence, which refuses it.
        let at_the_collecting_bound =
            unenforced.len() >= service_common::validation::MAX_VIOLATIONS;
        return if unenforced.is_empty() || at_the_collecting_bound {
            Judgement::TheFrameworksSequence
        } else {
            Judgement::ServedPastTheWalk
        };
    }
    let stated = refusal(&enforced).or_else(|| {
        if unenforced.is_empty() {
            return None;
        }
        let refused = Refusal::Invalid(enforced);
        // The one chain this service words a boundary refusal by, and then the shared
        // renderer's own envelope: unlike the framework's sequence, this layer cannot hand the
        // request back to the walk for the violations that remain — the walk would refuse it
        // for the unenforced one as well — so it needs an answer for every class.
        the_service_wide_wording(&refused)
            .or_else(|| service_common::validation_message::aws_validation_refusal(&refused))
    });
    stated
        .and_then(|refused| refusal_response(protocol, &refused))
        .map_or(Judgement::TheFrameworksSequence, |response| {
            Judgement::Refused(Box::new(response))
        })
}

/// How this service words a boundary refusal when no OPERATION states one of its own: the two
/// classes it restates, in order — the ones CloudWatch answers as one of its own PARAMETER
/// faults ([`parameter_fault`]), then a value that broke a modelled `@pattern`, whose
/// requirement AWS quotes without the anchors the model wraps it in
/// ([`unanchored_pattern_refusal`], which keeps the framework's own class). Everything else
/// keeps the framework's code and wording.
///
/// ONE chain, so the refusal `Service::override_refusal` answers with and the one a wrapper in
/// this file words ahead of the framework's sequence cannot drift apart.
fn the_service_wide_wording(refusal: &Refusal) -> Option<Refused> {
    parameter_fault(refusal).or_else(|| unanchored_pattern_refusal(refusal))
}

/// The refusal policy of an operation that keeps this service's SERVICE-WIDE wording for a
/// boundary refusal ([`the_service_wide_wording`]): it words nothing of its own.
///
/// Mounted on an operation that is served through [`JudgingTheValuesThisServiceEnforces`] for
/// the other decision that wrapper makes — serving a request past a constraint this service
/// does not enforce — so that wrapping it changes nothing about how its refusals read.
fn the_service_wide_refusal(_: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    None
}

/// One modelled operation, served so that this service's own PARAMETER-VALUE rules are
/// judged BEFORE the framework's constraint walk.
///
/// The framework's per-request sequence is deserialize -> validate -> handle, so a handler
/// never sees a request that breaks a modelled constraint — and a request that breaks BOTH a
/// modelled constraint and one of the rules the service judges values by is answered for the
/// constraint, under the generic validation code, when CloudWatch answers the parameter-value
/// fault. A caller repairing what it was told about is then refused again for the fault it was
/// never shown. This wrapper takes the value decision on the way IN and answers it with the
/// operation's OWN modelled error.
///
/// Nothing else is re-implemented: a request the policy does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize and every modelled error, and the refusal comes from the operation's own file
/// (`crate::ops::put_metric_stream::parameter_value_refusal`, which its handler calls too), so
/// the answer here and the answer behind it are one decision. The cost is DELIBERATE: such a
/// request is read twice, because the framework hands no already-built input to an operation.
struct RefusingAValueFirst<O: Operation, E> {
    operation: O,
    /// The modelled error this operation owes for the values the request carries, or `None` to
    /// leave the request to the operation itself.
    refusal: fn(&O::Input) -> Option<E>,
}

impl<O: Operation, E> RefusingAValueFirst<O, E> {
    /// Wrap the operation, already constructed from the shared engine, with the policy that
    /// judges the values its input carries.
    fn new(operation: O, refusal: fn(&O::Input) -> Option<E>) -> Self {
        Self { operation, refusal }
    }
}

impl<O, E> ErasedOperation for RefusingAValueFirst<O, E>
where
    O: Operation + ErasedOperation,
    E: Into<O::Error> + Send + Sync + 'static,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let protocol = svc.definition().wire_protocol(req).protocol();
                read_input::<O>(protocol, req)
                    .and_then(|input| (self.refusal)(&input))
                    .map(|refusal| modelled_error_response(protocol, req, &refusal.into()))
            };
            match refused {
                Some(response) => response,
                None => self.operation.run(svc, req, ctx).await,
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `req` read into `O`'s input shape through the service's own protocol, or `None` when it
/// does not READ as that input at all.
///
/// Deliberately NOT validated: the whole point of the wrapper above is to judge a value the
/// constraint walk would have refused the request for first. A body no codec can read, or one
/// missing a required member, yields `None` and is left to the framework's own refusal, which
/// is the only answer there is for a request that names no values to judge.
fn read_input<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<O::Input> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    read.ok()
}

/// The header an `awsQueryCompatible` service reports the legacy query error code in, for a
/// client that asked for it.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The header a query-compatible SDK opts into that report with.
const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";

/// `error` rendered as this service answers a modelled failure: the error shape through the
/// service's own protocol, under its wire code and its modelled status
/// (`service_common::event_stream::error_response`, the shared renderer), plus the
/// `awsQueryCompatible` header when the client asked for the legacy code.
///
/// Rendered from the SAME facts the per-operation sequence renders a modelled error from
/// ([`OperationError`]), so a refusal answered ahead of that sequence and the same refusal
/// answered from the handler are one response. The header is added HERE for the same reason
/// the framework adds it inside the sequence this refusal precedes: a MODELLED error's legacy
/// code is read off the error value itself, which only the layer holding that value can do.
/// The body half of the class's name is completed on the way out, for every answer of this
/// service alike ([`query_compatible_errors`]).
fn modelled_error_response<E: OperationError>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    error: &E,
) -> aws_smithy_runtime_api::http::Response {
    let mut response = service_common::event_stream::error_response(protocol, error);
    // A query-compat SDK opts in with `x-amzn-query-mode: true`, and only then is the legacy
    // query error code and its fault reported in the header; a native awsJson client gets the
    // unadorned awsJson error.
    let query_mode = req
        .headers()
        .get(QUERY_MODE_HEADER)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    if query_mode {
        if let Some(code) = error.query_error_code() {
            let fault = if response.status().as_u16() >= 500 {
                "Receiver"
            } else {
                "Sender"
            };
            response
                .headers_mut()
                .insert(QUERY_ERROR_HEADER, format!("{code};{fault}"));
        }
    }
    response
}

/// `refused` rendered in this service's own error envelope: its code and its one `message`,
/// with the status every refused request value carries ([`REFUSAL_STATUS`]).
///
/// The body is the single-`message` shape every refusal of this service that names no modelled
/// error renders through (`crate::scaffolded`), which is also all the refusal SEAM can answer
/// with — a code and a message and nothing else — so a refusal worded here and one the seam
/// words read alike; `None` when the envelope cannot be built, which leaves the request to the
/// operation and so to the framework's own refusal.
fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    refused: &Refused,
) -> Option<aws_smithy_runtime_api::http::Response> {
    protocol
        .serialize_error(
            &crate::scaffolded::ValidationException::new(refused.message.clone()),
            &crate::scaffolded::VALIDATION_EXCEPTION_SCHEMA,
            &refused.code,
            REFUSAL_STATUS,
        )
        .ok()
}

/// The response header an ACCEPTED `PutMetricData` reports a dropped entity in.
///
/// `PutMetricDataInput$StrictEntityValidation` documents that such an answer carries "an
/// additional response header [which] will contain information about the validation errors"
/// and enumerates the values that information is drawn from, but it does not name the header
/// itself: this name, and carrying the bare failure name as its value, are INFERRED from what
/// a lenient call is recorded answering, not quoted from the model.
const ENTITY_FAILURE_HEADER: &str = "x-amz-failure-message";

/// `PutMetricData`, served so that an accepted call which DROPPED an entity says so.
///
/// "In the case of an invalid entity, the operation will return a 200 status, but an
/// additional response header will contain information about the validation errors."
/// (`PutMetricDataInput$StrictEntityValidation`) — the accepted answer's shape is `Unit`, so
/// it binds no member the handler could put that on, and the header is added here, the one
/// place a response can still be touched. Nothing else is re-implemented: the request is
/// handed to the generated [`ErasedOperation::run`], which owns deserialize -> validate ->
/// handle -> serialize and every modelled error, and the failure the header names comes from
/// the operation's OWN entity validation
/// (`crate::ops::put_metric_data::dropped_entity_failure`), so the two cannot disagree.
struct ReportingDroppedEntities<O> {
    operation: O,
}

impl<O> ReportingDroppedEntities<O> {
    /// Wrap the operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for ReportingDroppedEntities<O>
where
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`. The request is read through the
            // service's OWN protocol into the operation's OWN input shape, and judged by the
            // operation's OWN entity validation, so what the header reports and what the
            // handler did cannot disagree. The cost is DELIBERATE: a request is read twice,
            // once here and once in the operation's own sequence, because the framework
            // hands no already-built input to an operation.
            let dropped = service_common::event_stream::deserialize_request::<
                crate::ops::types::PutMetricDataInput,
            >(
                svc.definition().wire_protocol(req).protocol(),
                req,
                crate::ops::types::PUT_METRIC_DATA_INPUT_SCHEMA,
            )
            .ok()
            .and_then(|input| crate::ops::put_metric_data::dropped_entity_failure(&input));
            let mut response = self.operation.run(svc, req, ctx).await;
            // Only an ACCEPTED call drops an entity: a refused one ingested nothing, so it
            // has no accepted metric data to report an unassociated entity for.
            //
            // The failure is one of a FIXED set of names, never text from the request, so it
            // is always a representable header value; the fallible insert is used all the
            // same, so that a header which could not be built could only cost the answer its
            // header and never the answer itself.
            if response.status().is_success() {
                if let Some(failure) = dropped {
                    let _ = response
                        .headers_mut()
                        .try_insert(ENTITY_FAILURE_HEADER, failure);
                }
            }
            response
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One operation, served so that the call this service answers is COUNTED into the usage
/// telemetry it publishes about its own API ([`crate::state::usage`]).
///
/// Every operation of this service is served through one of these, so a call is counted
/// wherever it was answered from — the handler, a modelled refusal, or the boundary refusal
/// one of the wrappers above words. The count is taken AFTER the answer is built and is
/// written off it: the response a caller sees is the wrapped operation's own, and a count that
/// could not be stored costs the telemetry a datapoint and never the answer.
///
/// The operation counted is named by the routing key the service's OWN protocol resolves —
/// the modelled operation's shape name, the same key the generated routing table is read with
/// — so the series a reader asks for by operation name is the series the call was counted in.
struct Metering<Eng, O> {
    /// The shared engine, so the count reaches the same store every handler of this service
    /// writes ([`crate::state::service_state`]).
    engine: Arc<Eng>,
    /// The operation whose answer this one is.
    operation: O,
}

impl<Eng, O> Metering<Eng, O> {
    /// Meter the operation, already constructed from the shared engine.
    fn new(engine: Arc<Eng>, operation: O) -> Self {
        Self { engine, operation }
    }
}

impl<Eng, O> ErasedOperation for Metering<Eng, O>
where
    Eng: Send + Sync + 'static,
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let operation = svc
                .definition()
                .wire_protocol(req)
                .protocol()
                .routing_key(req)
                .ok()
                .map(ToString::to_string);
            let response = self.operation.run(svc, req, ctx).await;
            // A served call is counted whatever it answered: "every request" includes the
            // refused ones, which are requests the API answered.
            if let Some(operation) = operation {
                crate::state::usage::count_api_call(&*self.engine, &operation, ctx).await;
            }
            response
        })
    }

    /// The wrapped operation's modelled auth schemes: metering a call must not change which
    /// schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The modelled operation this service's generated routing table holds for the request's own
/// routing key: what the request would have been served by had this file claimed nothing.
///
/// It re-implements NOTHING — it resolves the same key the router would have resolved and runs
/// the generated table's own operation — and exists only so that the metering above can wrap
/// every operation of this service without this file naming all 55 of them.
struct TheModelledOperation;

impl ErasedOperation for TheModelledOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The definition is read and dropped inside this one statement, as above: the
            // operation it hands back is `Send`, the definition reference is not.
            let modelled = {
                let definition = svc.definition();
                definition
                    .wire_protocol(req)
                    .protocol()
                    .routing_key(req)
                    .ok()
                    .and_then(|key| definition.routing_table().get(key))
            };
            match modelled {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable through `override_operation`, which hands a request here only
                // once the generated table has answered for its key; a request whose key the
                // table does not name is the framework's own routing failure, worded by this
                // service's entry exactly as it would be had nothing claimed the key.
                None => svc.definition().routing_error_response(req),
            }
        })
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves, and
        // the operation it names is the one served — `PutMetricData` wrapped in
        // [`ReportingDroppedEntities`] so an accepted call reports the entity it dropped, the
        // four metric operations a namespace is submitted to wrapped in
        // [`JudgingTheValuesThisServiceEnforces`] so a namespace this service accepts is served
        // past the constraint walk and, for the two whose model declares the parameter faults,
        // so a value outside an enumerated type is refused as one of THEM — where a refusal of
        // such a value by any OTHER operation keeps this service's own validation code
        // ([`scalar_enum_refusal`]) — and `PutMetricStream` wrapped in
        // [`RefusingAValueFirst`] so the values its model does not constrain to the form the
        // service requires are judged before the boundary walk.
        //
        // Every modelled key resolves here, because every call this service answers is counted
        // into the usage telemetry it publishes about its own API ([`Metering`]) and names the
        // class of whatever failure it answers in full ([`QueryCompatibleErrors`]): a key the
        // wrappers above do not claim is served by the generated table's own operation
        // ([`TheModelledOperation`]), metered. A key the table does not name at all resolves to
        // `None`, exactly as it would in an unedited service, and falls through to the
        // framework's routing failure.
        //
        // The key is read through the protocol THIS REQUEST is served in ([`request_protocol`]),
        // which for a query-protocol call is the `Action` field of its form body: a request in
        // one of this service's protocols is served by the same wrappers as one in another.
        let key = self.entry.wire_protocol(req).protocol().routing_key(req).ok()?;
        if let Some(operation) = self.overridden.get(key) {
            return Some(operation);
        }
        self.entry
            .routing_table()
            .get(key)
            .map(|_| &self.metered as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent required
        // member, or a value outside a declared bound) to let this service reword it. Two
        // classes are restated, by the one chain this service words such a refusal with
        // ([`the_service_wide_wording`], which the wrapper that words a refusal ahead of the
        // framework's sequence reaches too). Everything else keeps the framework's code and
        // wording.
        the_service_wide_wording(refusal)
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
