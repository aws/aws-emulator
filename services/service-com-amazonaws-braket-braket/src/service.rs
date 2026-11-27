// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-braket-braket` service: how it claims requests and how it words the
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
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
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
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## This service's one edited seam: the front door
//!
//! Braket rejects a request BODY that breaks a modelled constraint at its front door,
//! before the request reaches the application code that raises the modelled
//! `ValidationException`: the answer is HTTP 400 with the error code
//! `BadRequestException`, a code this service's model does not declare
//! (`smithy_query(query="operation", operation="CreateSpendingLimit")` lists
//! `AccessDenied`/`DeviceRetired`/`InternalService`/`Throttling`/`ValidationException`
//! and nothing else). A constraint on a value bound to the request URI — a
//! `@httpLabel` path segment, a `@httpQuery` parameter — is NOT a body rejection and
//! keeps the modelled `ValidationException`, which is what the recordings show for a
//! malformed `spendingLimitArn` or `jobArn` label.
//!
//! The framework refuses every modelled constraint before a handler runs and words it
//! through [`Service::override_refusal`], which is handed the violations alone — no
//! operation, no request — so it cannot tell a body violation from a URI one (and
//! Braket binds `deviceArn` BOTH ways: in the creates' bodies and as `GetDevice`'s
//! label). [`service_common::front_door::FrontDoor`] is where that distinction lives:
//! each modelled operation is served wrapped in it, and the wrapper reads the HTTP
//! BINDING of the violated member off THAT operation's own input schema.
//!
//! ## And the refusal its operations word for themselves: an invalid ARN parameter
//!
//! One class of refusal Braket words itself: a parameter naming a resource by ARN whose
//! value is not a valid ARN is refused as the modelled `ValidationException`, in a
//! sentence the SERVING OPERATION chooses — `GetJob` says `"jobArn parameter is not
//! valid."` where `CancelJob`, handed the same member, states the rejected value and says
//! `"jobArn <value> is not a valid ARN."`, and the two spending-limit doors word it a
//! third way. Amazon Braket words a client error per operation, so no one sentence can
//! serve them all.
//!
//! That makes [`Service::override_refusal`] the wrong seam for it: the centralized walk
//! hands it the violations ALONE, with no operation, so two doors refusing the same
//! member name are indistinguishable there. The sentence therefore travels with the
//! operation — each one's wording lives in its own module (`crate::ops::<op>`), and the
//! door it is served behind carries it ([`FrontDoor::wording_uri`]). What stays here is
//! the RULE for recognising the class at all ([`invalid_arn_parameter`]) and the table of
//! which door words which sentence; the suite in
//! `tests/all/client_error_wording.rs` pins both over every ARN parameter the model gives
//! a `@pattern` outside the body (`jobArn`, `spendingLimitArn`).

//! ## And the one codec correction: a `date-time` timestamp
//!
//! Every `Timestamp` member of this model declares the wire form it travels in, and all
//! but two declare `@timestampFormat("date-time")` — a quantum task's and a hybrid job's
//! `createdAt`/`startedAt`/`endedAt`, a job event's `timeOfEvent`, a spending limit's
//! `createdAt`/`updatedAt`, a device's status reason instants
//! (`smithy_query(query="shape",
//! shape="GetQuantumTaskResponse,QuantumTaskSummary,GetJobResponse,JobSummary,JobEventDetails,SpendingLimitSummary,DeviceStatusReason,TimePeriod")`).
//! The vendored codegen emits no member-level format at all, so the JSON codec fell back
//! to the protocol default and put an epoch NUMBER on the wire where the model pins a
//! date-time STRING. [`service_common::date_time_json::DateTimeRestJson`] is the tree's one
//! correction for that class and this service takes it at [`Service::override_protocol`];
//! it is given the members whose model declares that form ([`DATE_TIME_MEMBERS`]), so the
//! two members whose declared form really is `epoch-seconds` — `TimePeriod$startAt` and
//! `TimePeriod$endAt` — keep the protocol's own number. The catalog is a transcription of
//! the model, and `tests/all/quantum_task_loop.rs` re-derives it from the model file.
//!
//! ## And how an answer that carries NO CONTENT is framed
//!
//! Braket's two tag mutations answer `204 No Content` (`SUCCESS_STATUS` in
//! `src/ops/tag_resource.rs` and `src/ops/untag_resource.rs`), their modelled output
//! structures declaring no member at all. Such a structure is not `smithy.api#Unit`, the one
//! shape the renderers treat as bodyless, so the protocol renders it as the empty JSON
//! object — content on an answer that states it has none.
//!
//! [`NoContentDateTimeRestJson`] is therefore what this service serves: the codec above, with
//! a response at that status handed through `service_common::no_content_responses`, the
//! tree's one reading of that framing. Every answer that DOES carry content — every reader's,
//! every create's and every modelled refusal's — is left exactly as the codec rendered it,
//! and the correction is made at the one place every response of this service passes through
//! rather than per operation.

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use service_common::date_time_json::DateTimeRestJson;
use service_common::front_door::FrontDoor;
use service_common::no_content_responses::without_content;
use service_common::validation::SubmittedEcho;
use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The wire protocol this service serves: [`DateTimeRestJson`] — the model's own `restJson1`
/// with every `date-time` timestamp in the form the model declares — with one framing
/// correction on the way out: an answer that carries no content carries no body.
///
/// See the module docs. The correction itself is `service_common::no_content_responses`, so
/// what lives here is only the composition — which codec this service serves, and that every
/// response it renders is framed.
#[derive(Debug)]
struct NoContentDateTimeRestJson {
    inner: DateTimeRestJson,
}

impl NoContentDateTimeRestJson {
    /// Build the protocol once, at service construction: [`Service::override_protocol`] hands
    /// back a borrow of it and must never build one per request.
    fn new() -> Self {
        Self {
            inner: DateTimeRestJson::for_members(DATE_TIME_MEMBERS),
        }
    }
}

impl ServerProtocol for NoContentDateTimeRestJson {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        // The request side is the codec's own.
        self.inner.deserialize_input(request, input_schema)
    }

    /// The codec's own rendering of the modelled success, framed as an answer that carries no
    /// content when that is what its status says it is. A success whose output carries
    /// members is returned exactly as rendered.
    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        let rendered =
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?;
        Ok(without_content(rendered))
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // Every modelled failure of this service carries its error envelope at a 4xx or 5xx,
        // so nothing here is emptied; the same call is made on both paths rather than leaving
        // one of them to answer differently.
        let rendered = self
            .inner
            .serialize_error(error, error_schema, error_code, http_status)?;
        Ok(without_content(rendered))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}

/// The error code every Braket operation models for input that fails its constraints,
/// and the one the framework's own refusal channel names for this restJson1 service.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The suffix of an ARN-shaped PARAMETER's member name. Every member of this service
/// that names a resource by ARN spells it this way (`jobArn`, `quantumTaskArn`,
/// `spendingLimitArn`, `deviceArn`, `resourceArn`).
const ARN_MEMBER_SUFFIX: &str = "Arn";

/// Whether `refused` is the class of refusal an operation words itself — an ARN-shaped
/// PARAMETER whose value is not a valid ARN — worded by `wording`, the serving
/// operation's own sentence for it.
///
/// Narrow on purpose: ONE violation, of `@pattern`, on a TOP-LEVEL member named as an
/// ARN. A member that fails some other constraint (a label longer than its `@length`
/// bound, an absent member) failed something else and keeps the framework's wording,
/// which states the constraint it broke; a body member's violation never reaches this
/// seam, because [`FrontDoor`] answers that as a body rejection first.
///
/// The parameter NAME comes from the request, and so does the rejected VALUE where the
/// operation's sentence states one — passed as `None` when the walk could not state it
/// (a value past the bounds `service_common::validation` publishes), which only a wording
/// that quotes it has to decline for. So a sentence built here follows the class rather
/// than fitting one recorded call.
fn invalid_arn_parameter(
    refused: &[(Violation, SubmittedEcho)],
    wording: fn(&str, Option<&str>) -> Option<String>,
) -> Option<Refused> {
    let [(violation, echo)] = refused else {
        return None;
    };
    if !matches!(violation.kind, ViolationKind::Pattern(_)) {
        return None;
    }
    let parameter = violation.path.as_str();
    if parameter.contains('.') || !parameter.ends_with(ARN_MEMBER_SUFFIX) {
        return None;
    }
    let value = match echo {
        SubmittedEcho::Rendered(value) => Some(value.as_str()),
        SubmittedEcho::Absent | SubmittedEcho::Unrendered => None,
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: wording(parameter, value)?,
    })
}

/// The wording each door carries for [`invalid_arn_parameter`]: one function per
/// operation that takes an ARN-shaped parameter the model gives a `@pattern`, naming the
/// sentence that operation words the refusal in.
///
/// The sentences themselves are in the operations' own modules, so adding one is a change
/// to that operation's file; this is only the wiring, because a `FrontDoor`'s wording is a
/// function it holds.
macro_rules! arn_parameter_wordings {
    ($($door:ident => $module:ident),+ $(,)?) => {
        $(
            fn $door(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
                invalid_arn_parameter(refused, crate::ops::$module::invalid_arn_refusal)
            }
        )+
    };
}

arn_parameter_wordings!(
    cancel_job_wording => cancel_job,
    get_job_wording => get_job,
    delete_spending_limit_wording => delete_spending_limit,
    update_spending_limit_wording => update_spending_limit,
);

/// Every member of this model that declares `@timestampFormat("date-time")`, by the
/// absolute member id its generated member schema carries (`"ns#Shape$member"`).
///
/// The JSON codec reads a timestamp's wire form from the member schema alone and the
/// generated schemas carry no member-level format, so the declared form is supplied at the
/// protocol seam for exactly these members. Every OTHER timestamp member of this model
/// declares `epoch-seconds` (`TimePeriod$startAt` / `TimePeriod$endAt`, a spending limit's
/// period) and keeps the number the protocol gives it — which is why the correction is
/// named per member here instead of being applied to every format-less timestamp.
///
/// A transcription of the model: `tests/all/quantum_task_loop.rs` re-derives this list from
/// the model file and fails if a member is missing from it or named wrongly.
pub const DATE_TIME_MEMBERS: &[&str] = &[
    "com.amazonaws.braket#DeviceStatusReason$estimatedEndAt",
    "com.amazonaws.braket#DeviceStatusReason$startedAt",
    "com.amazonaws.braket#DeviceStatusReason$updatedAt",
    "com.amazonaws.braket#GetJobResponse$createdAt",
    "com.amazonaws.braket#GetJobResponse$endedAt",
    "com.amazonaws.braket#GetJobResponse$startedAt",
    "com.amazonaws.braket#GetQuantumTaskResponse$createdAt",
    "com.amazonaws.braket#GetQuantumTaskResponse$endedAt",
    "com.amazonaws.braket#JobEventDetails$timeOfEvent",
    "com.amazonaws.braket#JobSummary$createdAt",
    "com.amazonaws.braket#JobSummary$endedAt",
    "com.amazonaws.braket#JobSummary$startedAt",
    "com.amazonaws.braket#QuantumTaskSummary$createdAt",
    "com.amazonaws.braket#QuantumTaskSummary$endedAt",
    "com.amazonaws.braket#SpendingLimitSummary$createdAt",
    "com.amazonaws.braket#SpendingLimitSummary$updatedAt",
];

/// The `com-amazonaws-braket-braket` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: NoContentDateTimeRestJson,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`FrontDoor`] so a request BODY that breaks a modelled constraint is refused
    /// with `BadRequestException` before the operation runs. Keyed by the routing key
    /// the protocol resolves — the key the generated table uses — and built from the
    /// SAME shared engine, so a wrapped operation reads and writes the one store.
    front_door: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`FrontDoor`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
///
/// A line carrying `=> <wording>` serves that operation behind a door that words a
/// refused ARN PARAMETER in that operation's own sentence
/// ([`arn_parameter_wordings`]); every other line leaves such a refusal to the
/// framework's wording, which states the constraint the value broke.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident $(=> $wording:path)?),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(
                    FrontDoor::new(
                        crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                    )
                    $(.wording_uri($wording))?
                ) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut front_door = OperationRoutingTable::new();
        // EVERY modelled operation, so the front door's rule is applied by one code
        // path rather than per operation: an operation that binds nothing to the body
        // (`GetDevice`, whose only member is its URI label) simply never has a body
        // violation to find, which the wrapper reads off its own input schema.
        wrap_operations!(
            front_door,
            engine,
            cancel_job::CancelJob => cancel_job_wording,
            cancel_quantum_task::CancelQuantumTask,
            create_job::CreateJob,
            create_quantum_task::CreateQuantumTask,
            create_spending_limit::CreateSpendingLimit,
            delete_spending_limit::DeleteSpendingLimit => delete_spending_limit_wording,
            get_device::GetDevice,
            get_job::GetJob => get_job_wording,
            get_quantum_task::GetQuantumTask,
            list_tags_for_resource::ListTagsForResource,
            search_devices::SearchDevices,
            search_jobs::SearchJobs,
            search_quantum_tasks::SearchQuantumTasks,
            search_spending_limits::SearchSpendingLimits,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_spending_limit::UpdateSpendingLimit => update_spending_limit_wording,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: NoContentDateTimeRestJson::new(),
            front_door,
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `FrontDoor` so a request BODY that breaks a modelled constraint is refused
        // with `BadRequestException` before the operation runs, the way Braket's front
        // door refuses it. A key the model does not name resolves to `None` here
        // exactly as it does in the generated table, so the router's routing-error
        // path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. Returning `None` keeps the framework's default wording, which for
        // this restJson1 service is 400 `ValidationException` — the error EVERY Braket
        // operation models for input that fails its constraints, and what the service
        // answers for a value bound to the request URI (`GET /job/{jobArn}` with a
        // malformed ARN, `DELETE /tags/{resourceArn}` with no `tagKeys`) and for a
        // body the codec cannot read at all. The recordings confirm it: a malformed
        // `spendingLimitArn` or `jobArn` URI LABEL is answered `ValidationException`.
        //
        // Nothing is reworded HERE, because nothing this service words differently can be
        // placed from the violations alone: a BODY that breaks a modelled constraint is
        // answered by `FrontDoor` before this walk is reached, and the sentence for an
        // ARN-shaped PARAMETER outside its pattern belongs to the operation that was
        // addressed (see the module docs), which this seam is not told. What reaches here
        // is every refusal neither of those words — a label past its `@length` bound, an
        // absent member — and the framework's own wording, which states the constraint
        // the value broke, is the right answer for it.
        let _ = refusal;
        None
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own `restJson1`, with every member this model declares
        // `@timestampFormat("date-time")` on ([`DATE_TIME_MEMBERS`]) written in that form
        // (see the module docs). The two that declare `epoch-seconds` are outside that
        // catalog, so they keep the protocol's own number. And an answer that carries no
        // content — a tag mutation's success — carries no body rather than the empty JSON
        // object the renderer writes for a members-less output structure.
        Some(&self.protocol)
    }
}
