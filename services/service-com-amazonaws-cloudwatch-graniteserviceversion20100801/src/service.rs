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
//!   ([`query_compatible_errors`]) — through [`TheCborErrorEnvelope`], so a failure it answers
//!   on the `rpcv2Cbor` route travels in the envelope that route answers one in — the
//!   qualified shape id, the message alone beside it, and the status of the fault side
//!   ([`cbor_error_envelope`]) — and through [`ReportingTheRequestId`], so a failure it
//!   answers on the query wire names the request it answered ([`request_protocol`]);
//!   `PutMetricData` is served through
//!   [`ReportingDroppedEntities`], so an accepted call
//!   whose entity this service could not hold says so in its response header, and
//!   `GetMetricStatistics` through [`JudgingTheValuesThisServiceEnforces`], so a value outside
//!   an enumerated type is refused as one of the parameter faults ITS model declares and a
//!   metric namespace the service accepts is served past the constraint walk — as it is for the
//!   other three metric operations a namespace is submitted to — and
//!   `PutMetricStream` through [`RefusingAValueFirst`], so the two members CloudWatch judges
//!   as parameter VALUES are judged before the model's own shape checks, and every operation
//!   whose model declares none of this service's parameter faults and whose input carries a
//!   bounded COLLECTION through [`StatingTheSubmittedCollection`], so a collection over its
//!   modelled maximum is refused in the request-validation class, quoting the collection the
//!   request carried. Every read whose response can carry a FLOATING-POINT member is served
//!   through [`RenderingFloatsForTheWire`] as well, so such a member leaves this service in the
//!   form the protocol of the request writes one in ([`decimal_floats`]).
//!   The key itself is read through the protocol THIS REQUEST is served in, so a call in any
//!   of the three protocols this service's model declares is served by the same wrappers.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — EDITED HERE: how this service words a request that reached it
//!   but named no modeled operation. The envelope, the status and the sentence stay the
//!   protocol-correct default's; what this service states is the CLASS such a refusal names,
//!   which on the `rpcv2Cbor` route is a protocol-level class of the framework in front of the
//!   service and is named in full there ([`cbor_error_envelope`]). The same answer serves a
//!   request this file DECLINES to route — a CBOR-route call addressed to another Smithy
//!   service ([`request_protocol`]), which the generated table's own key would otherwise
//!   resolve.
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

pub mod authorization;
pub mod cbor_error_envelope;
pub mod decimal_floats;
pub mod published_pattern;
pub mod query_compatible_errors;
pub mod request_protocol;
pub mod submitted_collection_refusal;

use authorization::AuthorizedByIam;
use cbor_error_envelope::TheCborErrorEnvelope;
use decimal_floats::RenderingFloatsForTheWire;
use query_compatible_errors::{
    QueryCompatibleErrors, MISSING_PARAMETER, PARAMETER_VALUE, VALIDATION,
};
use request_protocol::{ReportingTheRequestId, TheProtocolOfTheRequest};
use submitted_collection_refusal::StatingTheSubmittedCollection;

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
    /// service enforces ([`JudgingTheValuesThisServiceEnforces`]), the metric-stream writer
    /// whose own parameter-value rules are judged ahead of the boundary walk
    /// ([`RefusingAValueFirst`]), and the widget renderer whose per-call PAYLOAD limit is judged
    /// on the body the client sent ([`RefusingAnOversizedPayload`]), and the operations whose
    /// model declares none of this service's parameter faults and whose input carries a bounded
    /// COLLECTION, which state the collection a refused request carried
    /// ([`StatingTheSubmittedCollection`]). Each is built from the SAME
    /// shared engine as the generated table's and keyed by the routing key the protocol resolves,
    /// so the operation served is the modelled one, and each is METERED ([`Metering`]) like every
    /// other operation of this service and answers a failure under the class this
    /// `awsQueryCompatible` service names it under ([`QueryCompatibleErrors`]).
    overridden: OperationRoutingTable,
    /// Every OTHER modelled operation, served through the generated table and metered
    /// ([`Metering`] over [`TheModelledOperation`]): the call count this service publishes
    /// about its own API is owed by every operation, so the seam that counts a call is the one
    /// every request passes through rather than a line in each handler. The whole chain sits
    /// inside [`QueryCompatibleErrors`], for the same reason: naming the class of a failure is
    /// owed by every answer, whichever layer built it.
    metered: TheWayEveryOperationIsServed<Eng, TheModelledOperation>,
    /// The answer to a request this service DECLINES to serve: one the generated table's own
    /// key would have resolved, addressed to another Smithy service on the CBOR route
    /// ([`TheUnroutedRequest`]). Unmetered and unauthorized, because a request that named no
    /// operation of this service is not a call it served.
    unrouted: TheUnroutedRequest,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `$table`, each served the way
/// every operation of this service is ([`served`]) and each wrapped in
/// [`StatingTheSubmittedCollection`], keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
///
/// A macro rather than a loop because each operation is its own TYPE: the wrapper reads the
/// request through the wrapped operation's own input schema, which only a type can name.
macro_rules! stating_the_submitted_collection {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                served(
                    &$engine,
                    StatingTheSubmittedCollection::new(
                        crate::ops::$module::$operation::new(Arc::clone(&$engine)),
                    ),
                ),
            );
        )+
    };
}

/// The same, for an operation whose response can ALSO carry a FLOATING-POINT member: the
/// renderer that writes one in the form the protocol of the request asks for
/// ([`decimal_floats`]) sits inside the collection wording, because it decides only how the
/// answer to a served request is written.
macro_rules! stating_the_submitted_collection_and_rendering_floats {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                served(
                    &$engine,
                    StatingTheSubmittedCollection::new(RenderingFloatsForTheWire::new(
                        crate::ops::$module::$operation::new(Arc::clone(&$engine)),
                    )),
                ),
            );
        )+
    };
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
                    RenderingFloatsForTheWire::new(
                        crate::ops::get_metric_statistics::GetMetricStatistics::new(Arc::clone(
                            &engine,
                        )),
                    ),
                    parameter_fault_for,
                ),
            ),
        );
        // The other two reads of the metric data plane take a namespace from the caller too —
        // the one a query names and the one a catalogue listing filters by — so they are served
        // through the same wrapper: what it adds for them is that a namespace the service
        // accepts is READ BACK rather than refused, so a series published into one can be
        // retrieved and listed. The metric DATA read words its own refusals as the
        // request-validation layer in front of the service words them
        // ([`the_request_validation_fault`]), which is the class its recordings carry.
        overridden.add_operation(
            "GetMetricData",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    RenderingFloatsForTheWire::new(
                        crate::ops::get_metric_data::GetMetricData::new(Arc::clone(&engine)),
                    ),
                    the_request_validation_fault,
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
        // The two metric-stream SWITCHES word a `Names` entry outside the bounds the list's
        // element shape declares the way AWS words one — at the list member, quoting the list
        // and naming the element shape's whole set — which the framework's own walk cannot do,
        // so the refusal is taken on the way IN ([`RefusingAValueFirst`]) from the one judgement
        // both of them share (`crate::state::metric_stream_names`).
        overridden.add_operation(
            "StartMetricStreams",
            served(
                &engine,
                RefusingAValueFirst::new(
                    crate::ops::start_metric_streams::StartMetricStreams::new(Arc::clone(&engine)),
                    crate::ops::start_metric_streams::names_constraint_refusal,
                ),
            ),
        );
        overridden.add_operation(
            "StopMetricStreams",
            served(
                &engine,
                RefusingAValueFirst::new(
                    crate::ops::stop_metric_streams::StopMetricStreams::new(Arc::clone(&engine)),
                    crate::ops::stop_metric_streams::names_constraint_refusal,
                ),
            ),
        );
        // `GetMetricWidgetImage` publishes a per-call PAYLOAD limit ("Up to 100 KB uncompressed
        // payload"), which is a bound on the request's own body rather than on any member's
        // value, so it is judged on the way in and answered in the transport layer's own class
        // ([`RefusingAnOversizedPayload`]).
        overridden.add_operation(
            "GetMetricWidgetImage",
            served(
                &engine,
                RefusingAnOversizedPayload::new(
                    crate::ops::get_metric_widget_image::GetMetricWidgetImage::new(Arc::clone(
                        &engine,
                    )),
                    crate::state::metric_widget::MAX_PAYLOAD_BYTES,
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
        overridden.add_operation(
            "GetInsightRuleReport",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    RenderingFloatsForTheWire::new(
                        crate::ops::get_insight_rule_report::GetInsightRuleReport::new(Arc::clone(
                            &engine,
                        )),
                    ),
                    report_member_fault,
                ),
            ),
        );
        // The three BATCH rule operations report one entry of `Failures` per rule name they
        // could not act on, so they are served through the same wrapper for its OTHER
        // decision: a `RuleNames` entry whose characters or length the member's own constraints
        // do not admit is a per-entry outcome of the batch ([`is_unenforced_constraint`]),
        // judged inside the handler's walk, so the request is SERVED — the rest of the batch
        // takes effect and the offending entry comes back classified. The refusal they DO make
        // about one request member is the Contributor Insights coded form every other rule
        // operation states one in ([`coded_member_fault`]).
        overridden.add_operation(
            "EnableInsightRules",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::enable_insight_rules::EnableInsightRules::new(Arc::clone(&engine)),
                    coded_member_fault,
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
                    coded_member_fault,
                ),
            ),
        );
        overridden.add_operation(
            "DeleteInsightRules",
            served(
                &engine,
                JudgingTheValuesThisServiceEnforces::new(
                    crate::ops::delete_insight_rules::DeleteInsightRules::new(Arc::clone(&engine)),
                    coded_member_fault,
                ),
            ),
        );
        // Every operation whose model declares NONE of this service's parameter faults and
        // whose input carries a collection the model bounds or whose ELEMENTS it constrains:
        // such an operation refuses a collection over that maximum, and a constraint one of a
        // collection's elements broke, in the request-validation class — quoting the collection
        // the request carried and stating the element shape's whole set at the collection
        // member ([`submitted_collection_refusal`], which holds that rule). The operations left
        // out of this list are the ones it cannot reach — either their model declares a
        // parameter fault, so a collection over its cap is one of THOSE
        // ([`collection_cap_refusal`], which the seam behind this list words), or their input
        // declares no such collection at all, so there is no collection to refuse.
        stating_the_submitted_collection!(
            overridden,
            engine,
            delete_alarms::DeleteAlarms,
            describe_alarm_history::DescribeAlarmHistory,
            disable_alarm_actions::DisableAlarmActions,
            enable_alarm_actions::EnableAlarmActions,
            put_alarm_mute_rule::PutAlarmMuteRule,
            put_composite_alarm::PutCompositeAlarm,
            put_log_alarm::PutLogAlarm,
            put_metric_alarm::PutMetricAlarm,
            start_o_tel_enrichment::StartOTelEnrichment,
            update_o_tel_enrichment::UpdateOTelEnrichment,
            create_resource_metrics_configuration::CreateResourceMetricsConfiguration,
            update_resource_metrics_configuration::UpdateResourceMetricsConfiguration,
        );
        // The two of that set whose RESPONSE reports an alarm, and so can carry a
        // floating-point member — a reported alarm's `Threshold` — are served through the
        // renderer that writes one in the form the request's own protocol asks for as well
        // ([`decimal_floats`]).
        stating_the_submitted_collection_and_rendering_floats!(
            overridden,
            engine,
            describe_alarms::DescribeAlarms,
            describe_alarms_for_metric::DescribeAlarmsForMetric,
        );
        Self {
            metered: the_way_every_operation_is_served(Arc::clone(&engine), TheModelledOperation),
            entry: service_common::query_error_envelope::RequestIdEnvelopeDefinition::reporting(
                TheProtocolOfTheRequest::new(crate::ServiceDefinition::new(engine)),
                request_protocol::naming_the_request,
            ),
            overridden,
            unrouted: TheUnroutedRequest,
        }
    }

    /// The NAME of this service's own Smithy service shape, as the CBOR route's path addresses
    /// a service by: the local part of the shape id the generated entry carries
    /// (`com.amazonaws.cloudwatch#GraniteServiceVersion20100801` ->
    /// `GraniteServiceVersion20100801`), read off the model rather than restated here.
    fn service_shape_name(&self) -> &str {
        let shape_id = self.entry.identity().shape_id();
        shape_id.rsplit('#').next().unwrap_or(shape_id)
    }
}

/// `operation` served the way EVERY operation of this service is: decided against the
/// calling principal's own IAM policies before anything is read or written
/// ([`authorization`]), counted into the usage telemetry this service publishes about its own
/// API ([`Metering`]), naming the class of whatever failure it answers in full
/// ([`QueryCompatibleErrors`] on the two text wires, [`TheCborErrorEnvelope`] on the CBOR one,
/// which also carries that route's own status and member projection), and naming the request
/// it answered — in the `awsQuery` error envelope and in the header every protocol reports it
/// in ([`ReportingTheRequestId`]).
///
/// All of them are owed by every answer, whichever layer built it, so the chain is stated here
/// once rather than at each of the operations whose answer this file owns a part of. The
/// authorization decision sits OUTSIDE the metering, because a call refused before it is
/// dispatched is not a call this service served; it sits INSIDE the request-id writer, so its
/// refusal names the request it answered like every other failure. The CBOR envelope sits
/// outside the class naming of the text wires, because each reads only the answers written in
/// its own protocol: a failure is in exactly one envelope, so only one of the two completes it.
fn the_way_every_operation_is_served<Eng: Send + Sync + 'static, O: ErasedOperation>(
    engine: Arc<Eng>,
    operation: O,
) -> TheWayEveryOperationIsServed<Eng, O> {
    let metered = TheCborErrorEnvelope::new(QueryCompatibleErrors::new(Metering::new(
        Arc::clone(&engine),
        operation,
    )));
    ReportingTheRequestId::new(AuthorizedByIam::new(engine, metered))
}

/// The chain [`the_way_every_operation_is_served`] builds, named so the field holding it and
/// that function cannot state it differently.
type TheWayEveryOperationIsServed<Eng, O> = ReportingTheRequestId<
    AuthorizedByIam<Eng, TheCborErrorEnvelope<QueryCompatibleErrors<Metering<Eng, O>>>>,
>;

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
/// * a COLLECTION longer than its cap, named with the cap it broke
///   ([`collection_cap_refusal`]) — a collection past its maximum is a value the service
///   cannot use, whichever member carries it;
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
/// A required member the request OMITTED is one of those others, and deliberately so: the
/// request-validation layer in front of the service judges the SHAPE of a request before the
/// service reads any of its values, so a request that names a member no value at all is
/// refused there, under the request-validation class — where a member the request DID carry,
/// with no characters in it, is a value the service read and found unusable. Only an
/// operation whose own refusals are recorded naming the missing-parameter fault states that
/// class for an omitted member, and it does so where the operation is known
/// ([`parameter_fault_for`], [`coded_member_fault`]).
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
        .find_map(collection_cap_refusal)
        .or_else(|| violations.iter().find_map(empty_required_value_refusal))
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
/// violation alone, so it holds for every required member of the operations mounted with a policy
/// that reaches it, at any depth — the `Name` of a dimension FILTER a metric listing is narrowed
/// by is the recorded case.
///
/// WHICH operations those are is the point: an operation recorded answering the
/// request-validation class for a member it was not given reaches no policy that words this, and
/// its refusal stays the one the layer in front of the service makes ([`parameter_fault`]).
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
/// member names, and a filter value supplied with no characters at all, each in the fixed
/// sentence CloudWatch refuses one with — every other class keeps the service-wide wording
/// behind it ([`the_service_wide_wording`]).
///
/// The listing's filters are judged as VALUES rather than as the shape of the request: a window
/// to filter recent activity by that this service does not offer is a parameter value it cannot
/// use ([`crate::state::request_parameter_invalid_message`], the parameter named as the wire
/// spells it), and so is a filter the request supplied EMPTY
/// ([`unusable_filter_value_refusal`]) — both under the parameter-value fault the operation's
/// model declares, which is the only client error it declares at all
/// (`smithy_query(query="operation",operation="ListMetrics")`).
fn listing_value_fault(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    violations
        .iter()
        .find_map(unusable_filter_value_refusal)
        .or_else(|| violations.iter().find_map(omitted_required_member_refusal))
        .or_else(|| {
            violations.iter().find_map(|violation| {
                let aws_smithy_schema_server::ViolationKind::EnumValue(_) = violation.kind else {
                    return None;
                };
                Some(Refused {
                    code: PARAMETER_VALUE.shape_name.to_string(),
                    message: crate::state::request_parameter_invalid_message(&violation.path),
                })
            })
        })
}

/// The refusal of a value the metric LISTING is narrowed by that the request supplied with no
/// characters, or `None` when `violation` is not one.
///
/// Every member of the listing's request is a filter it narrows the catalogue by, and a filter
/// carrying no characters narrows it by nothing: the listing judges that as the VALUE it is —
/// the parameter-value fault its model declares, in the sentence this service states an unusable
/// value in ([`crate::state::value_is_invalid_message`]), naming the request member path the
/// value arrived on — rather than as the length its member's own minimum declares, which is what
/// the request-validation layer in front of the service would have stated.
///
/// The value half of that sentence is the one the template leaves UNSUBSTITUTED
/// ([`crate::state::NO_SUBMITTED_VALUE`]): there is no value to state, which is the whole
/// condition the refusal is about.
///
/// Keyed on the CLASS of violation alone — a minimum length the submitted value broke by being
/// empty — so it holds for every filter of the listing at any depth: the VALUE of one dimension
/// filter is the recorded case, and the filter's name, the metric name and the namespace are
/// stated the same way.
fn unusable_filter_value_refusal(
    violation: &aws_smithy_schema_server::Violation,
) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MinLength(_) = violation.kind else {
        return None;
    };
    if !violation.value.as_deref().is_some_and(str::is_empty) {
        return None;
    }
    Some(Refused {
        code: PARAMETER_VALUE.shape_name.to_string(),
        message: crate::state::value_is_invalid_message(
            crate::state::NO_SUBMITTED_VALUE,
            &crate::state::query_parameter_path(&violation.path),
        ),
    })
}

/// The refusal policy of the metric DATA read: every class keeps the REQUEST-VALIDATION class
/// and the clause the layer in front of the service states it in, with NO value half.
///
/// Which class a refusal carries is a property of the OPERATION ([`parameter_fault_for`] states
/// the other side of it): this read's refusals are the request-validation layer's — a required
/// member of one of its queries that the request left out is recorded as that layer's not-null
/// clause rather than as the missing-parameter fault the service-wide wording states
/// ([`parameter_fault`], which is what the operations recorded answering a parameter fault
/// reach) — so the policy words them here, where the operation is known, instead of letting the
/// request fall through to a wording its recordings contradict.
///
/// WHICH HALVES the clause carries is a property of the operation too, and this read is the one
/// that states NO value: `Value at '<member path>' failed to satisfy constraint: <requirement>`,
/// whichever requirement was broken and whatever the request carried — where every other
/// operation's clause quotes the submitted value ([`scalar_enum_refusal`]). Only that choice and
/// the ORDER a value set is listed in are this service's: the envelope, the clauses, the
/// requirements and the member paths are the shared renderer's and the framework's, so this and
/// the walk behind it cannot drift.
fn the_request_validation_fault(
    violations: &[aws_smithy_schema_server::Violation],
) -> Option<Refused> {
    let stated = Refusal::Invalid(
        violations
            .iter()
            .map(service_common::validation_message::in_hashed_enum_order)
            .collect(),
    );
    let message =
        service_common::validation_message::aws_validation_refusal_withholding_violations(
            &stated,
            |_| true,
        )?
        .message;
    Some(Refused {
        code: VALIDATION.shape_name.to_string(),
        message,
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

/// The refusal policy of the Contributor Insights REPORT read: a page size below the published
/// minimum of the member that asks for one, exactly as the rule operations state it
/// ([`non_positive_page_size_refusal`]), and a required member the request did not supply at
/// all, in the coded form those operations state a refusal about one member in, closed by the
/// clause that names the request the parameter has to be present in
/// ([`crate::state::missing_report_parameter_message`]).
///
/// Which sentence a missing member earns is a property of the OPERATION: the rule operations name
/// the member alone, while this read names the member and the contributor query behind it, which
/// is what its own recorded refusal carries.
fn report_member_fault(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    violations
        .iter()
        .find_map(non_positive_page_size_refusal)
        .or_else(|| {
            violations.iter().find_map(|violation| {
                matches!(
                    violation.kind,
                    aws_smithy_schema_server::ViolationKind::MissingRequiredMember
                )
                .then(|| Refused {
                    code: MISSING_PARAMETER.shape_name.to_string(),
                    message: crate::state::missing_report_parameter_message(
                        &crate::state::query_parameter_path(&violation.path),
                    ),
                })
            })
        })
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
/// A Smithy `@pattern` is a FIND, so the expression a member DECLARES and the expression its
/// whole value has to satisfy are two different things, and the requirement this service
/// states is the second — decided END BY END, with what the anchors enclosed quoted as the
/// model wrote it ([`published_pattern::quoted_pattern`], this service's one reading of that
/// question). So a client that lifts the expression out of the message and re-validates
/// locally gets the one the service publishes, for a member anchored at both ends and for one
/// anchored at a single end alike. Everything else about the refusal — the envelope, the
/// clause, the member path, the class — is the framework's and the shared renderer's, so this
/// and the walk behind it cannot drift.
///
/// It holds for every member of this service whose model declares a `@pattern`, whatever the
/// operation: the statistic an anomaly detection model is trained on, the dataset identifier
/// and the KMS key ARN the dataset operations take (`^…$`, quoted as the expression between
/// the anchors) and the metric NAMESPACE a filter selects by (`^[^:]`, open at its close and
/// so quoted with the run that end admits) are the recorded cases, and a member modelled with
/// a pattern later is stated the same way without another line here. The CHECK always runs
/// the model's pattern as written; only this rendering decides the ends.
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
        published_pattern::quoted_pattern,
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
/// * the value set is the one this service PUBLISHES for the type, in the order the SET ITSELF
///   is walked ([`in_the_published_value_set`]), which is the order this service is recorded
///   listing an enumerated type's values in — neither the model's declaration order nor a
///   sorted one;
/// * the clause quotes the SUBMITTED value, which is what tells a client which of the values it
///   sent was rejected — a near miss of a declared value (a mis-cased one, say) cannot be told
///   from an unsupported one without it. WHETHER a clause carries that half is a property of the
///   OPERATION, not of the type: the one read whose clauses state no value at all words its own
///   ([`the_request_validation_fault`]).
fn scalar_enum_refusal(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    if !violations.iter().any(is_scalar_enum_violation) {
        return None;
    }
    let stated = Refusal::Invalid(violations.iter().map(in_the_published_value_set).collect());
    // The whole envelope is the shared renderer's; only the CODE and the value set a clause
    // enumerates are this service's.
    let message = service_common::validation_message::aws_validation_refusal(&stated)?.message;
    Some(Refused {
        code: VALIDATION.shape_name.to_string(),
        message,
    })
}

/// The value sets this service PUBLISHES for its own enumerated types, where what it
/// publishes is not the whole set its on-disk model declares: the values the model declares,
/// matched as a set, against the values a refusal of this service enumerates.
///
/// A refusal states the set a caller has to choose from, and that set is the one the service
/// ADVERTISES for the type — which a model revision may run ahead of, since a value the model
/// gains is in the shape before the published enumeration names it. The alarm TYPE is such a
/// type: the on-disk model declares a third kind of alarm beside the metric and composite ones
/// (`smithy_query(query="shape",shape="AlarmType")`), and this service's constraint refusals
/// enumerate the two the published enumeration carries. Which values a member ADMITS is not
/// decided here — that is the enum the member targets, so a filter naming the third kind is
/// still served ([`service_common::validation_message::stated_enum_value_set`], the shared
/// reading of such a row).
const PUBLISHED_ENUM_VALUE_SETS: &service_common::validation_message::StatedEnumValueSets<'static> = &[(
    &["CompositeAlarm", "MetricAlarm", "LogAlarm"],
    &["MetricAlarm", "CompositeAlarm"],
)];

/// `declared` as a refusal of this service enumerates it: the values it PUBLISHES for that
/// type ([`PUBLISHED_ENUM_VALUE_SETS`]), in the order the set's own hash table is walked
/// ([`service_common::validation_message::hashed_enum_order`], which is the order this service
/// is recorded listing an enumerated type's values in).
///
/// Both halves belong to the TYPE rather than to a member, an operation or a depth, so one
/// type is enumerated alike wherever a clause states it: a value set the table names no row
/// for is the model's own, in that same walked order.
fn published_enum_values(declared: &[String]) -> Vec<String> {
    let published: Vec<String> =
        match service_common::validation_message::stated_enum_value_set(
            declared,
            PUBLISHED_ENUM_VALUE_SETS,
        ) {
            Some(published) => published.iter().map(|value| (*value).to_string()).collect(),
            None => declared.to_vec(),
        };
    service_common::validation_message::hashed_enum_order(&published)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `violation` with an enumerated type's value set stated as this service enumerates one
/// ([`published_enum_values`]), and any other violation returned unchanged.
///
/// The CLAUSE stays the framework's: only the set that goes into it moves, so the member path,
/// the echoed value and the requirement's wording keep coming from the one place that writes
/// them.
fn in_the_published_value_set(
    violation: &aws_smithy_schema_server::Violation,
) -> aws_smithy_schema_server::Violation {
    let aws_smithy_schema_server::ViolationKind::EnumValue(ref declared) = violation.kind else {
        return violation.clone();
    };
    aws_smithy_schema_server::Violation {
        path: violation.path.clone(),
        kind: aws_smithy_schema_server::ViolationKind::EnumValue(published_enum_values(declared)),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
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

/// The refusal of a COLLECTION carrying more entries than its cap, or `None` when `violation`
/// is not one.
///
/// CloudWatch names the collection and the cap it broke and nothing of the entries themselves
/// ([`crate::state::collection_too_large_message`]), under the parameter-value fault: a
/// collection past its maximum is a VALUE the service cannot use, which is the class its
/// handlers refuse one with too (`crate::ops::put_metric_data`,
/// `crate::ops::put_metric_stream`), so a cap the boundary catches and one a handler catches
/// read alike. The cap stated is the one the collection ACTUALLY broke — the member's own
/// modelled maximum, except for a dimension collection, whose cap is the per-metric dimension
/// maximum this service enforces ([`crate::state::MAX_METRIC_DIMENSIONS`]) wherever such a
/// collection is submitted: `ListMetricsInput`'s own filter list is modelled BELOW it
/// (`length 0..10`), and a collection inside the per-metric maximum but outside that member's
/// own bound is one the service still APPLIES ([`is_applicable_dimension_filter`]).
///
/// Keyed on the CLASS of violation alone — a maximum length broken by a value the walk rendered
/// as a collection's SIZE, which is a rendering no caller can send for a scalar member — so it
/// holds for every collection, at any depth, of every operation that REACHES it: the statistics
/// a metric read asks for and the dimension set it names are the recorded cases. WHICH
/// operations those are is the point, because the class of a boundary refusal is a property of
/// the OPERATION: an operation whose model declares this service's parameter faults reaches
/// here, where one that declares none states the collection it was sent under the
/// request-validation class instead ([`submitted_collection_refusal`], which holds that half of
/// the rule).
fn collection_cap_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MaxLength(declared) = violation.kind else {
        return None;
    };
    let submitted =
        service_common::validation_message::list_item_count(violation.value.as_deref()?)?;
    let cap = if is_dimension_collection(&violation.path)
        && submitted > crate::state::MAX_METRIC_DIMENSIONS
    {
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
/// the sentence is this service's own: a collection over its cap first
/// ([`collection_cap_refusal`], the same rule the service-wide seam applies), then a required
/// value supplied
/// empty, then a required member the request left out, then a granularity that names no span of
/// time ([`non_positive_period_refusal`]), then a value outside an
/// enumerated type, in the SAME order as the service-wide seam ([`parameter_fault`]) for the
/// classes they share, so one operation cannot answer a request that breaks two of them
/// differently from every other. An operation that declares none of those faults keeps the
/// request-validation class instead ([`scalar_enum_refusal`],
/// [`the_request_validation_fault`]).
fn parameter_fault_for(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    violations
        .iter()
        .find_map(collection_cap_refusal)
        .or_else(|| violations.iter().find_map(empty_required_value_refusal))
        .or_else(|| violations.iter().find_map(omitted_required_member_refusal))
        .or_else(|| violations.iter().find_map(non_positive_period_refusal))
        .or_else(|| violations.iter().find_map(enum_value_refusal))
}

/// The refusal of a `Period` that names no span of time at all, or `None` when `violation` is
/// not one.
///
/// A period is a number of SECONDS, so the smallest one there is is a single second — the bound
/// the member itself declares (`Period`, `range min 1`) — and a value at or below it names no
/// granularity a series could be folded at. CloudWatch states that against the bound a span has
/// to be above rather than against the sub-minute set a short period has to come from, by the
/// one rule every operation that takes a period is judged by
/// ([`crate::state::period_error`], which the handlers behind this seam call too), under the
/// parameter-value fault ([`PARAMETER_VALUE`]): the value is one the service read and cannot
/// use.
///
/// Keyed on the CLASS of violation and the member — a range minimum broken by a period — so it
/// holds wherever that member is submitted, at any depth, and the bound the sentence states is
/// the member's own.
fn non_positive_period_refusal(
    violation: &aws_smithy_schema_server::Violation,
) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MinValue(_) = violation.kind else {
        return None;
    };
    if !is_period(&violation.path) {
        return None;
    }
    let submitted: i32 = violation.value.as_deref()?.trim().parse().ok()?;
    Some(Refused {
        code: PARAMETER_VALUE.shape_name.to_string(),
        message: crate::state::period_error(submitted)?,
    })
}

/// Whether `path` points at the member an operation states a metric's granularity on, in the
/// spelling the validating walk reports — at whatever depth it sits, which is the last segment
/// of the path the walk builds (`GetMetricStatisticsInput$Period`, `MetricStat$Period`,
/// `MetricDataQuery$Period`).
fn is_period(path: &str) -> bool {
    path.split('.').next_back() == Some("period")
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
/// position of the `RuleNames` array of every batch operation that acts on rules by name.
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
///   [`coded_member_fault`] for an operation that words a refusal about one request member in
///   the Contributor Insights coded form).
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
/// requirement AWS quotes as the expression the WHOLE value must satisfy
/// ([`unanchored_pattern_refusal`], which keeps the framework's own class). Everything else
/// keeps the framework's code and wording.
///
/// ONE chain, so the refusal `Service::override_refusal` answers with and the one a wrapper in
/// this file words ahead of the framework's sequence cannot drift apart.
fn the_service_wide_wording(refusal: &Refusal) -> Option<Refused> {
    parameter_fault(refusal).or_else(|| unanchored_pattern_refusal(refusal))
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

/// One modelled operation, served so that a request whose PAYLOAD is larger than that
/// operation's documented per-call limit is refused before anything reads it.
///
/// Such a limit is on the call's own payload rather than on any member's value — "Up to 100 KB
/// uncompressed payload" (`GetMetricWidgetImage`) — so it is decided where the request's body is
/// still the body the client sent, ahead of the deserialize -> validate -> handle sequence: a
/// caller that sent too much learns the size it sent and the size it may send, instead of a
/// refusal of whichever member happened to carry the bulk. The class is the transport's own
/// ([`ENTITY_TOO_LARGE_CODE`]) and so belongs to no shape of this service's model, which is why
/// the refusal is worded here rather than in the operation.
///
/// Nothing else is re-implemented: a request inside the limit is handed to the generated
/// [`ErasedOperation::run`], which owns the whole per-request sequence and every modelled error.
struct RefusingAnOversizedPayload<O> {
    operation: O,
    /// The most bytes of payload the wrapped operation documents for ONE call.
    limit: usize,
}

impl<O> RefusingAnOversizedPayload<O> {
    /// Wrap the operation, already constructed from the shared engine, with the payload limit
    /// its own documentation publishes.
    fn new(operation: O, limit: usize) -> Self {
        Self { operation, limit }
    }
}

impl<O> ErasedOperation for RefusingAnOversizedPayload<O>
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
            // below would make this future non-`Send`.
            let refused = payload_size(req)
                .filter(|sent| *sent > self.limit)
                .and_then(|sent| {
                    let protocol = svc.definition().wire_protocol(req).protocol();
                    entity_too_large_response(protocol, sent, self.limit)
                });
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

/// How many bytes of payload `req` carries, or `None` when this layer cannot tell.
///
/// The body the client sent, as it was sent: an error-bodied request is built in memory by the
/// transport, so its bytes are readable here, and a request whose body is not (a streamed one)
/// is measured by the length it advertised. A request that states neither is not judged against
/// the limit at all, which leaves it to the operation.
fn payload_size(req: &aws_smithy_runtime_api::http::Request) -> Option<usize> {
    if let Some(bytes) = req.body().bytes() {
        return Some(bytes.len());
    }
    req.headers()
        .get(CONTENT_LENGTH_HEADER)
        .and_then(|advertised| advertised.trim().parse().ok())
}

/// The header a client advertises the length of its body in.
const CONTENT_LENGTH_HEADER: &str = "content-length";

/// The wire error code a payload past the limit of the call it was sent to is refused with: the
/// class of the transport layer in front of the service, which no shape of this model declares.
const ENTITY_TOO_LARGE_CODE: &str = "RequestEntityTooLarge";

/// The status that class carries ("Content Too Large" in the HTTP semantics).
const ENTITY_TOO_LARGE_STATUS: u16 = 413;

/// A payload of `sent` bytes refused against the `limit` bytes one call carries, in this
/// service's own error envelope.
///
/// The sentence is built from the two sizes that decided the refusal — the one the client sent
/// and the one it may send — so a caller splitting its request knows by how much; `None` when
/// the envelope cannot be built, which leaves the request to the operation.
fn entity_too_large_response(
    protocol: &dyn ServerHttpProtocol,
    sent: usize,
    limit: usize,
) -> Option<aws_smithy_runtime_api::http::Response> {
    protocol
        .serialize_error(
            &crate::scaffolded::ValidationException::new(format!(
                "Request size {sent} exceeded {limit} bytes"
            )),
            &crate::scaffolded::VALIDATION_EXCEPTION_SCHEMA,
            ENTITY_TOO_LARGE_CODE,
            ENTITY_TOO_LARGE_STATUS,
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

    /// Which schemes sign the request: the MODELLED operation's own, which this answers by
    /// DEFERRING rather than by naming any — "`None` (an author-written operation) defers to
    /// the modeled operation the request's routing key names"
    /// ([`smithy_server_runtime::ErasedOperation::auth_schemes`]).
    ///
    /// Stated here rather than left to the trait's default so that what the whole table-routed
    /// surface reports is explicit: this operation IS the generated table's operation for the
    /// request's key ([`Self::run`]), and the key is a property of the request, which this
    /// method is not handed — so the one answer that resolves to each modelled operation's own
    /// schemes is the deferral, and naming a scheme list here would be this file asserting an
    /// answer for all 55 operations that the model states per operation. The wrapper around it
    /// ([`Metering::auth_schemes`]) forwards whatever this returns, so metering a call does not
    /// change which schemes sign it either.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// A request this service does not SERVE, answered as the framework answers a request that
/// named no operation of it: the routing failure this service's entry renders, with the class
/// of a protocol-level refusal named in full where the route states one
/// ([`cbor_error_envelope::naming_the_protocol_level_class`]).
///
/// It re-implements nothing and reads nothing of the request: the answer is the one the router
/// itself would have given had `override_operation` claimed nothing, which is the point —
/// `override_operation` is the only seam that can decline a request the generated table's own
/// key WOULD have resolved (a CBOR-route call addressed to another Smithy service,
/// [`request_protocol::names_another_service`]), and a declined request has to be answered
/// there rather than served.
struct TheUnroutedRequest;

impl ErasedOperation for TheUnroutedRequest {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move { the_routing_failure(svc.definition(), req) })
    }
}

/// How this service answers a request that named no operation of it: its entry's own
/// protocol-correct routing failure, with the class named as a protocol-level refusal of the
/// route it travels on is named ([`cbor_error_envelope::naming_the_protocol_level_class`]).
///
/// ONE answer, so the seam the router asks ([`Service::override_routing_error`]) and the one a
/// declined request reaches ([`TheUnroutedRequest`]) cannot word the same failure differently.
fn the_routing_failure(
    definition: &dyn ErasedServiceDefinition,
    req: &aws_smithy_runtime_api::http::Request,
) -> aws_smithy_runtime_api::http::Response {
    cbor_error_envelope::naming_the_protocol_level_class(definition.routing_error_response(req))
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
        // The CBOR route addresses the SERVICE in the path beside the operation, and the
        // framework's routing key is the operation step alone: a call addressed to another
        // Smithy service names no operation of THIS one, so it is declined here — the only
        // seam that can decline a key the generated table would otherwise resolve — and
        // answered as the unrouted request it is ([`TheUnroutedRequest`]).
        if request_protocol::names_another_service(req, self.service_shape_name()) {
            return Some(&self.unrouted as &dyn ErasedOperation);
        }
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
        // routing table resolved it. The envelope, the status and the sentence stay the
        // protocol-correct default's (query -> 400 InvalidAction text/xml, awsJson -> 404,
        // rpcv2Cbor -> 404); what this service states is the CLASS such a refusal names, which
        // on the CBOR route is a protocol-level class of the framework in front of the service
        // and is named in full there ([`the_routing_failure`]).
        Some(the_routing_failure(self.definition(), req))
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
