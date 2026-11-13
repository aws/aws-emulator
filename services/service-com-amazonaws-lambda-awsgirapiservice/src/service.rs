// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lambda-awsgirapiservice` service: how it claims requests and how it words the
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
//! ## The download plane
//!
//! `GetFunction` hands out `Code$Location`, "A presigned URL that you can use to download
//! the deployment package", and `GetLayerVersion` hands out `Content$Location`, "A link to
//! the layer archive in Amazon S3"; a URL this service announces is a promise that a
//! caller can follow it, so each one is presigned against the base URL this emulator is
//! reached on and carries the object's S3 address — the per-region bucket and the key
//! under it — in its PATH (`crate::state::download_object_path`), a path space no modelled
//! operation binds. Two seams carry them, the same pair any emulated data plane uses:
//!
//! - [`Service::can_handle`] also claims a request addressed to an object of those
//!   published stores, so a URL this service handed out is not answered as a request
//!   naming no operation;
//! - [`Service::override_operation`] serves it, through [`Downloads`], which the router
//!   reaches BEFORE authentication — which is what lets a presigned URL be followed
//!   without a signature, as a presigned URL is.
//!
//! What it serves is the bytes the store HOLDS for the object the URL names: the
//! deployment package of the function revision the location was reported for (a published
//! version froze its own, so its location keeps serving those after the unpublished
//! version's code is replaced), or one layer version's archive. A download is a pure
//! READ: it answers the same bytes twice and changes nothing a later caller sees. A URL
//! whose object this emulator holds no bytes for — an S3-sourced package it does not
//! resolve — is a `404`, because nothing may be served that was not stored.
//!
//! ## The function URL plane
//!
//! `CreateFunctionUrlConfig` hands out "a dedicated HTTP(S) endpoint that you can use to
//! invoke your function", and a request to that endpoint names no modelled operation either,
//! so it is carried by the same two seams: [`Service::can_handle`] claims a request
//! addressed to a function URL this store HOLDS, and [`Service::override_operation`] serves
//! it through [`function_url::FunctionUrlPlane`], which runs the function the URL is attached
//! to and answers what the run produced. Unlike a download, it is mounted TWICE — once per
//! `AuthType` — because which callers a URL admits is carried by the auth schemes the serving
//! operation declares, so the emulator still verifies every signature and this service checks
//! none. See that module's own header.
//!
//! ## How a broken constraint is stated
//!
//! An AWS constraint message is built from the REQUEST — `1 validation error detected:
//! Value '<value>' at '<member path>' failed to satisfy constraint: <requirement>` — and
//! the value between the quotes is the one the caller SUBMITTED, whole. One part of that
//! sentence the framework cannot produce from the violations alone is exactly that value:
//! its constraint walk stores the echo ELIDED (a byte-bounded prefix plus
//! `... [<n> bytes]`, a marker AWS never emits), because the clause interpolates whatever
//! is stored and a caller controls the length, and the unelided value is recoverable only
//! from the built input — which [`Service::override_refusal`] is never handed.
//!
//! [`StatedPerOperation`] is where it comes from: every modelled operation is served
//! wrapped in it, the wrapper asks `service_common::validation` for the framework's OWN
//! violations against that operation's input schema with each echo restored to the bytes
//! the caller sent (up to [`LARGEST_QUOTED_VALUE_BYTES`]), and [`stated_violations`] —
//! which `override_refusal` goes through as well — words them, so the two refusal paths
//! cannot drift.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use service_common::event_stream::EventStreamRoute;
use service_common::shared_path::SharedPathArn;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The class of fault every error body of this service names in its `Type` member, which
/// no raise site carries because it is a property of the failure rather than of the call.
/// Applied to every response [`StatedPerOperation`] answers; see the module's own header.
pub mod fault_class;

/// The FUNCTION URL data plane: how a request to the endpoint `CreateFunctionUrlConfig`
/// handed out is served by the function that URL is attached to; see the module's own
/// header.
pub mod function_url;

/// How a request to a function URL that the emulator's authentication layer turned away is
/// worded; see the module's own header.
mod function_url_refusal;

/// The media type an INVOCATION's response announces, which is the media type of the
/// document the function produced rather than of the binding it travels in; see the
/// module's own header.
mod invoke_media_type;

/// The members of this model whose broken constraint is stated as AWS's own fixed sentence
/// rather than as the constraint envelope, because AWS keeps the value opaque; see the
/// module's own header.
mod opaque_members;

/// WHERE this service answers for a resource-based policy that is too large, and why that
/// answer is not stated at this layer; see the module's own header.
mod policy_length;

/// Whether the resource-based policy a policy READ reports grants public access, which that
/// read states beside the document; see the module's own header.
pub mod public_access;

/// The size a request to one operation of this service may reach, and the `413` refusal for
/// one that passes it; see the module's own header.
mod request_quota;

/// The header every answer of this service names the request it answered in, which is where
/// an SDK reads its response metadata's `RequestId` from; see the module's own header.
pub mod request_id_header;

/// WHERE a constraint broken by a COLLECTION's elements is stated, and what that clause says
/// an element has to satisfy; see the module's own header.
mod validation;

/// The largest single value ONE clause of this service's validation refusal quotes back.
///
/// An AWS constraint message always quotes the value it refused in full, and the shared
/// restorer's own ceiling (`service_common::validation`, 16 KiB) is sized for the members
/// most models declare — so a member this model declares LONGER than that could never have
/// its refusal quoted in full. The ceiling is therefore stated from the model: twice the
/// largest `@length` maximum this model puts on a string member, `ResourcePolicy`'s 20480
/// (`smithy_query(query="shape", shape="ResourcePolicy")`), so a document up to twice the
/// size that member admits is quoted whole. It is a MODEL constant times a fixed factor,
/// not a request-sized one; past it the framework's own shortened rendering stands, which
/// is a deliberate bound on the size of a response and leaves that one divergence where it
/// belongs — the frozen crate stores only the elided value, which
/// `services/common/src/validation.rs` records as the core seam this whole re-read exists
/// until.
pub(crate) const LARGEST_QUOTED_VALUE_BYTES: usize = 2 * 20_480;

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// The order this service states one enum's value set in, one row per enum.
///
/// The value SET is the model's and the CHECK is the framework's; only the order the
/// sentence lists the set in is chosen here, because no model records it: AWS states a
/// value set in neither the model's declaration order nor ascending order, so each row
/// is RECORDED from an AWS answer rather than computed, and each row below names the
/// corpus units its order was recorded from.
///
/// A row is matched by its value SET rather than by the member the clause names (see
/// [`reported_enum_order`]), because the order is a property of the enum SHAPE: every
/// member targeting that enum, in any operation, is stated alike. An enum with no row —
/// every other enum this model declares — keeps the model's declaration order, which is
/// the only order there is evidence for.
const REPORTED_ENUM_ORDERS: &[&[&str]] = &[
    // com.amazonaws.lambda#UpdateRuntimeOn (`Auto | Manual | FunctionUpdate` in the
    // model), reported by AWS as below. Source: the answer the corpus unit
    // `lambda.put-runtime-management-config-invalid-mode-rejected` carries for an
    // `UpdateRuntimeOn` outside its value set.
    &["FunctionUpdate", "Manual", "Auto"],
    // com.amazonaws.lambda#InvocationType (`Event | RequestResponse | DryRun` in the
    // model), reported by AWS as below. Source: the answers the corpus units
    // `lambda-batch-diag-submit-wrapper-function-handle-mistakes`,
    // `lambda-function-invocation-paths` and `lambda-lsrno-traj-invoke-reproduces-timeout`
    // carry for an `InvocationType` outside its value set.
    &["DryRun", "RequestResponse", "Event"],
];

/// `allowed` in the order this service reports that enum in, or in the order the model
/// declares it (the order it arrived in) when there is no recorded order for it — the
/// only order there is evidence for.
fn reported_enum_order(allowed: &[String]) -> Vec<String> {
    match REPORTED_ENUM_ORDERS
        .iter()
        .find(|reported| is_same_set(reported, allowed))
    {
        Some(reported) => reported.iter().map(|value| (*value).to_string()).collect(),
        None => allowed.to_vec(),
    }
}

/// Whether two value sets hold the same values. An enum's values are unique by
/// definition, so equal lengths plus containment is set equality.
fn is_same_set(reported: &[&str], allowed: &[String]) -> bool {
    reported.len() == allowed.len()
        && reported
            .iter()
            .all(|value| allowed.iter().any(|candidate| candidate == value))
}

/// The literal first URI path segments that LEAD to the `{Resource}` ARN label of the
/// one path space this model and a peer signing as `lambda` bind IDENTICALLY:
/// `ListTags` (`GET`), `TagResource` (`POST`) and `UntagResource` (`DELETE`) are all
/// bound to `/2017-03-31/tags/{Resource}` here AND in the Lambda MicroVMs model, so the
/// model-derived path discriminator alone cannot separate the two there. Source:
/// `smithy_query(query="digest", …)` on both models.
const SHARED_TAGGING_PATH: &[&str] = &["2017-03-31", "tags"];

/// The `aws.api#service$arnNamespace` every service signing as `lambda` shares
/// (`smithy_query(query="service_info")`: `arnNamespace: lambda` for all four), and so
/// the only service field an ARN any of their tagging operations addresses can carry.
const SHARED_ARN_NAMESPACE: &str = "lambda";

/// The ARN resource types reachable on [`SHARED_TAGGING_PATH`] that belong to a PEER
/// service rather than to this one.
///
/// A resource belongs to the service whose own operations MINT it: `microvm-image` is
/// minted by `CreateMicrovmImage` in the Lambda MicroVMs service, whose model binds the
/// same tagging URIs and whose `TaggableResource` pattern accepts it (this model's does
/// not). Every other type that pattern accepts here — `function`, `layer`,
/// `code-signing-config`, `event-source-mapping`, `capacity-provider`,
/// `network-connector` — is minted by this
/// service or has no other claimant, so it stays with this endpoint, whose own identity
/// IS the shared signing name (`sdkId: Lambda`, `endpointPrefix: lambda`). Source:
/// `smithy_query(query="shape", shape="TaggableResource")` on both models.
const PEER_TAGGING_RESOURCE_TYPES: &[&str] = &["microvm-image"];

/// Whether this service claims a request on the `/2017-03-31/tags/{Resource}` path space
/// it shares with the Lambda MicroVMs service.
///
/// The ARN in the path names the resource, and a resource belongs to exactly one of the
/// two endpoints, so the resource type decides: this endpoint claims every tagging
/// request EXCEPT one whose ARN names a peer's resource. Everything else on the shared
/// API-date segment stays here — a label that names no recognisable `lambda` resource, and
/// a path under that segment neither model binds — because this is the service the shared
/// signing name belongs to, so exactly one service claims every request on it.
fn claims_shared_tagging_path(parts: &http::request::Parts) -> bool {
    match service_common::shared_path::shared_path_arn(
        parts.uri.path(),
        SHARED_TAGGING_PATH,
        SHARED_ARN_NAMESPACE,
    ) {
        SharedPathArn::NotShared | SharedPathArn::Unrecognised => true,
        SharedPathArn::Resource(kind) => !PEER_TAGGING_RESOURCE_TYPES.contains(&kind.as_str()),
    }
}

/// The `com-amazonaws-lambda-awsgirapiservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: function_url_refusal::FunctionUrlRefusals<Eng>,
    /// Serves the download URLs `GetFunction` and `GetLayerVersion` announce, from the
    /// SAME shared engine the generated entry threads into every operation — not a
    /// second copy of the store.
    downloads: Downloads<Eng>,
    /// Serves `InvokeWithResponseStream`, whose `@streaming`-union output the generated
    /// routing-table entry declines; from the same shared engine, for the same reason.
    ///
    /// It is served NAMING its request, as every other answer of this service is: a
    /// response-streaming invocation is an invocation, so its answer carries the id of the
    /// invocation it ran (see [`request_id_header`]).
    response_stream: NamingItsRequest<
        EventStreamRoute<crate::ops::invoke_with_response_stream::ResponseStreamEntry<Eng>>,
    >,
    /// Serves the FUNCTION URL endpoints `CreateFunctionUrlConfig` hands out, from the same
    /// shared engine, for the same reason as [`Downloads`]: a request to one names no
    /// modelled operation at all. Mounted once per `AuthType`, because which callers a URL
    /// admits is carried by the auth schemes the serving operation declares (see
    /// [`function_url`]).
    function_urls_anonymous: function_url::FunctionUrlPlane<Eng>,
    /// See [`Self::function_urls_anonymous`]: the mounting that serves the URLs whose
    /// `AuthType` is `AWS_IAM`.
    function_urls_aws_iam: function_url::FunctionUrlPlane<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request QUOTES the value it refused rather
    /// than the framework's shortened rendering of it. Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    stated: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut stated = OperationRoutingTable::new();
        // EVERY operation the generated table holds, because the rule is the SERVICE's and
        // not one operation's: an operation whose input declares no constraint simply never
        // has a violation to state, which the wrapper reads off that operation's own input
        // schema. An operation added later is added here too, or its refusal quotes the
        // shortened value again; `every_modelled_operation_is_wrapped` below fails when one
        // is missing. `InvokeWithResponseStream` is the one exception, and not an omission:
        // its output binds a `@streaming` union, so it implements no
        // `smithy_server_runtime::Operation` at all and is served by `response_stream`
        // below, which reads and checks its own input against the model.
        wrap_operations!(
            stated,
            engine,
            add_layer_version_permission::AddLayerVersionPermission,
            add_permission::AddPermission,
            checkpoint_durable_execution::CheckpointDurableExecution,
            create_alias::CreateAlias,
            create_capacity_provider::CreateCapacityProvider,
            create_code_signing_config::CreateCodeSigningConfig,
            create_event_source_mapping::CreateEventSourceMapping,
            create_function::CreateFunction,
            create_function_url_config::CreateFunctionUrlConfig,
            delete_alias::DeleteAlias,
            delete_capacity_provider::DeleteCapacityProvider,
            delete_code_signing_config::DeleteCodeSigningConfig,
            delete_event_source_mapping::DeleteEventSourceMapping,
            delete_function::DeleteFunction,
            delete_function_code_signing_config::DeleteFunctionCodeSigningConfig,
            delete_function_concurrency::DeleteFunctionConcurrency,
            delete_function_event_invoke_config::DeleteFunctionEventInvokeConfig,
            delete_function_url_config::DeleteFunctionUrlConfig,
            delete_layer_version::DeleteLayerVersion,
            delete_provisioned_concurrency_config::DeleteProvisionedConcurrencyConfig,
            delete_resource_policy::DeleteResourcePolicy,
            get_account_settings::GetAccountSettings,
            get_alias::GetAlias,
            get_capacity_provider::GetCapacityProvider,
            get_code_signing_config::GetCodeSigningConfig,
            get_durable_execution::GetDurableExecution,
            get_durable_execution_history::GetDurableExecutionHistory,
            get_durable_execution_state::GetDurableExecutionState,
            get_event_source_mapping::GetEventSourceMapping,
            get_function::GetFunction,
            get_function_code_signing_config::GetFunctionCodeSigningConfig,
            get_function_concurrency::GetFunctionConcurrency,
            get_function_configuration::GetFunctionConfiguration,
            get_function_event_invoke_config::GetFunctionEventInvokeConfig,
            get_function_recursion_config::GetFunctionRecursionConfig,
            get_function_scaling_config::GetFunctionScalingConfig,
            get_function_url_config::GetFunctionUrlConfig,
            get_layer_version::GetLayerVersion,
            get_layer_version_by_arn::GetLayerVersionByArn,
            get_layer_version_policy::GetLayerVersionPolicy,
            get_policy::GetPolicy,
            get_provisioned_concurrency_config::GetProvisionedConcurrencyConfig,
            get_resource_policy::GetResourcePolicy,
            get_runtime_management_config::GetRuntimeManagementConfig,
            invoke::Invoke,
            invoke_async::InvokeAsync,
            list_aliases::ListAliases,
            list_capacity_providers::ListCapacityProviders,
            list_code_signing_configs::ListCodeSigningConfigs,
            list_durable_executions_by_function::ListDurableExecutionsByFunction,
            list_event_source_mappings::ListEventSourceMappings,
            list_function_event_invoke_configs::ListFunctionEventInvokeConfigs,
            list_function_url_configs::ListFunctionUrlConfigs,
            list_function_versions_by_capacity_provider::ListFunctionVersionsByCapacityProvider,
            list_functions::ListFunctions,
            list_functions_by_code_signing_config::ListFunctionsByCodeSigningConfig,
            list_layer_versions::ListLayerVersions,
            list_layers::ListLayers,
            list_provisioned_concurrency_configs::ListProvisionedConcurrencyConfigs,
            list_tags::ListTags,
            list_versions_by_function::ListVersionsByFunction,
            publish_layer_version::PublishLayerVersion,
            publish_version::PublishVersion,
            put_function_code_signing_config::PutFunctionCodeSigningConfig,
            put_function_concurrency::PutFunctionConcurrency,
            put_function_event_invoke_config::PutFunctionEventInvokeConfig,
            put_function_recursion_config::PutFunctionRecursionConfig,
            put_function_scaling_config::PutFunctionScalingConfig,
            put_provisioned_concurrency_config::PutProvisionedConcurrencyConfig,
            put_resource_policy::PutResourcePolicy,
            put_runtime_management_config::PutRuntimeManagementConfig,
            remove_layer_version_permission::RemoveLayerVersionPermission,
            remove_permission::RemovePermission,
            send_durable_execution_callback_failure::SendDurableExecutionCallbackFailure,
            send_durable_execution_callback_heartbeat::SendDurableExecutionCallbackHeartbeat,
            send_durable_execution_callback_success::SendDurableExecutionCallbackSuccess,
            stop_durable_execution::StopDurableExecution,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_alias::UpdateAlias,
            update_capacity_provider::UpdateCapacityProvider,
            update_code_signing_config::UpdateCodeSigningConfig,
            update_event_source_mapping::UpdateEventSourceMapping,
            update_function_code::UpdateFunctionCode,
            update_function_configuration::UpdateFunctionConfiguration,
            update_function_event_invoke_config::UpdateFunctionEventInvokeConfig,
            update_function_url_config::UpdateFunctionUrlConfig,
        );
        Self {
            entry: function_url_refusal::FunctionUrlRefusals::new(Arc::clone(&engine)),
            response_stream: NamingItsRequest(EventStreamRoute(
                crate::ops::invoke_with_response_stream::ResponseStreamEntry::new(Arc::clone(
                    &engine,
                )),
            )),
            downloads: Downloads {
                engine: Arc::clone(&engine),
            },
            function_urls_anonymous: function_url::FunctionUrlPlane::anonymous(Arc::clone(&engine)),
            function_urls_aws_iam: function_url::FunctionUrlPlane::aws_iam(Arc::clone(&engine)),
            stated,
        }
    }
}

/// Serves one operation that is NOT wrapped in [`StatedPerOperation`] — the
/// response-streaming invocation, which implements no `smithy_server_runtime::Operation` to
/// wrap — and names the request on its answer.
///
/// It adds nothing else: the request, the service and the context are handed to the inner
/// entry unchanged, so its input check, its handler and its event-stream body are its own.
/// The naming is the service's one rule for every answer it sends, including the id of an
/// invocation the call ran (see [`request_id_header`]).
struct NamingItsRequest<O>(O);

impl<O: ErasedOperation + 'static> ErasedOperation for NamingItsRequest<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(request_id_header::naming_its_request(
            self.0.run(svc, req, ctx),
        ))
    }

    /// The inner entry's modelled auth schemes: naming a response must not change which
    /// schemes sign the request.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.0.auth_schemes()
    }
}

/// One modelled operation, served so that a refused request is stated the way this service
/// states it: with the value as SUBMITTED between the quotes.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse, and the WORDING is the same
/// [`stated_violations`] that [`Service::override_refusal`] uses — so the two refusal paths
/// cannot drift. The cost is that a request this wrapper does not refuse is read twice,
/// once here and once in the operation's own sequence, because the framework hands no
/// already-built input to an operation — including on the data-plane operations, where a
/// `Payload` blob makes that second read the larger one. What the second read KEEPS is
/// bounded whatever the request's size (`service_common::validation` collects at most 256
/// leaves and 64 KiB of them, and a blob contributes no leaf at all), so only the parse is
/// paid twice; removing even that needs the core seam `services/common/src/validation.rs`
/// records — handing a handler, or `override_refusal`, the input the framework already
/// built.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// This service's answer to every modelled constraint `req` breaks, for the operation `O`:
/// the violations the framework's own walk finds — each value echo restored to the bytes
/// the caller sent, up to [`LARGEST_QUOTED_VALUE_BYTES`] — and the wording
/// [`stated_violations`] gives them. `None` when the request breaks none, and for a failure
/// that is NOT a refusal (a body the codec cannot read at all, a member of the wrong type),
/// which the framework's own walk answers in its own envelope.
///
/// The same ONE read also yields the request's own LISTS, which a clause stated at a list
/// member quotes and no violation carries (see [`validation`]): both halves come out of the
/// walk that already built the input, so the request is not read a third time for them.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists_quoting_up_to::<O>(
            protocol,
            req,
            LARGEST_QUOTED_VALUE_BYTES,
        );
    let stated = stated_violations(&violations, &lists)?;
    Some((violations, stated))
}

/// How this service words a constraint refusal, from the violations alone: the code, the
/// envelope and the `@pattern` rendering are `service-common`'s, and the ORDER an enum's
/// value set is listed in is this service's ([`reported_enum_order`]).
///
/// The ONE renderer both refusal paths go through — [`StatedPerOperation`] for a request a
/// wrapped operation refuses, and [`Service::override_refusal`] for one the centralized
/// walk refuses — so a caller cannot be told the same broken constraint two ways.
///
/// A member this service refuses OPAQUELY is answered first, with the whole fixed sentence
/// AWS gives it instead of the envelope over its clause (see [`opaque_members`]): the
/// envelope would quote back a token the caller cannot compose and the form a minted one
/// has, neither of which AWS discloses.
///
/// A constraint one of a COLLECTION's elements broke is stated at the COLLECTION member,
/// over the whole set of requirements an element of it has ([`validation`]) and quoting what
/// that member held — which is `lists`, keyed by the member path the clause is stated at.
/// The refusal path that is handed the violations ALONE passes no lists, so such a clause
/// states the member and the set with no value, exactly as it does for a collection whose
/// own `@length` was broken.
fn stated_violations(
    violations: &[Violation],
    lists: &std::collections::HashMap<String, String>,
) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    if let Some(opaque) = opaque_members::opaque_refusal(violations) {
        return Some(opaque);
    }
    let restated: Vec<Violation> = violations
        .iter()
        .map(|violation| match &violation.kind {
            ViolationKind::EnumValue(allowed) => Violation {
                path: violation.path.clone(),
                kind: ViolationKind::EnumValue(reported_enum_order(allowed)),
                value: violation.value.clone(),
                sensitive: violation.sensitive,
            },
            _ => violation.clone(),
        })
        .collect();
    service_common::validation_message::aws_validation_refusal_over_elements_stating(
        &Refusal::Invalid(restated),
        service_common::validation::unanchored,
        |collection| lists.get(collection).cloned(),
        validation::element_set,
    )
}

impl<O> ErasedOperation for StatedPerOperation<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // Every answer names the request it answered, and an answer that started an
        // INVOCATION names that invocation (see [`request_id_header`]): the whole run is
        // served inside that scope, so the operation can state the invocation it started at
        // the moment it mints it.
        Box::pin(request_id_header::naming_its_request(async move {
            // A request whose BODY is past the quota published for this operation is
            // refused on its size alone, before it is read as an input at all: that is
            // what makes the refusal a 413 about the request rather than a 400 about what
            // the request would have stored (see [`request_quota`]).
            if let Some(refusal) = request_quota::refusing_body(O::NAME, req) {
                return fault_class::with_fault_class(O::NAME, refusal);
            }
            // The definition is read, used and dropped inside this statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                // An OPAQUE refusal is answered as the message alone: the body AWS
                // records for it carries no other member, so it does not name the
                // class of its fault either (see [`opaque_members`]).
                let names_no_fault_class = opaque_members::names_no_fault_class(&violations);
                let response = svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(stated), req);
                return if names_no_fault_class {
                    response
                } else {
                    fault_class::with_fault_class(O::NAME, response)
                };
            }
            // A policy read also states whether the policy it reports grants public
            // access (see [`public_access`]); every other answer passes through.
            let answered = public_access::with_public_access_allowed(
                O::NAME,
                self.operation.run(svc, req, ctx).await,
            );
            // An INVOCATION's response body is the document the function produced, which
            // is announced as the JSON it is (see [`invoke_media_type`]).
            let answered = invoke_media_type::announced(O::NAME, answered);
            fault_class::with_fault_class(O::NAME, answered)
        }))
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Serves a deployment package or a layer archive from the bytes [`crate::state`] holds.
///
/// This is the data plane behind the URLs `GetFunction` and `GetLayerVersion` announce.
/// It is NOT a modelled operation — AWS serves those objects from a presigned S3 URL, and
/// this emulator serves them at the path it published in that URL, on its OWN endpoint,
/// because a URL it announces has to be one a caller can reach. It only READS.
struct Downloads<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for Downloads<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // An object is READ, so `GET` carries it and `HEAD` reports its metadata
            // without the bytes; anything else is refused as the method it is.
            if !matches!(req.method(), "GET" | "HEAD") {
                return download_error(405, "MethodNotAllowed", METHOD_NOT_ALLOWED_MESSAGE);
            }
            let Some(target) = request_target(req) else {
                return download_error(404, "NoSuchKey", NO_SUCH_KEY_MESSAGE);
            };
            let Some(state) = crate::state::service_state(&*self.engine) else {
                return download_error(
                    500,
                    "ServiceException",
                    "Lambda state is not reachable on this engine",
                );
            };
            let store = state.lock();
            let held = match &target {
                crate::state::DownloadTarget::FunctionPackage { name, revision_id } => store
                    .functions
                    .get(name)
                    .and_then(|function| function.package_at_revision(revision_id))
                    .and_then(crate::state::held_package_bytes),
                crate::state::DownloadTarget::LayerArchive {
                    region,
                    name,
                    version,
                } => store
                    .layer(region, name)
                    .and_then(|layer| layer.versions.get(version))
                    .and_then(|held| held.archive.zip_bytes.as_deref()),
            };
            // A URL whose object this store no longer holds — a deleted function or layer
            // version, a revision that has been replaced — names no object, and so does
            // one whose bytes were never held here at all.
            let Some(bytes) = held else {
                return download_error(404, "NoSuchKey", NO_SUCH_KEY_MESSAGE);
            };
            download_response(bytes, req.method() == "GET")
        })
    }

    /// Served UNSIGNED. A download URL is handed to a caller to fetch directly, so the
    /// fetch carries no SigV4 `Authorization` header. The modelled operations are
    /// unaffected — the router reads their schemes from the model, which this answer
    /// cannot weaken.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

/// What this service's download message says for an object it does not hold.
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";

/// What it says for a method a download does not answer.
const METHOD_NOT_ALLOWED_MESSAGE: &str = "The specified method is not allowed against this resource.";

/// The object a download request names, or `None` when it names none.
///
/// The PATH is what says which object is addressed — the published store's bucket and the
/// key under it — and `crate::state` reads it back from the same two parts it builds the
/// announced URL out of, so the URL this service mints and the request it serves cannot
/// disagree about either. The path is taken by `service_common`'s one reader of a request
/// URI, so this plane and every other service's claim read a URI the same way.
fn request_target(req: &RtRequest) -> Option<crate::state::DownloadTarget> {
    crate::state::parse_download_target(service_common::request_path(req.uri()))
}

/// Whether the request addresses an object in one of the published download stores.
///
/// The PATH is what makes a request this plane's: a URL this service announced is addressed
/// to its own endpoint, which every modelled operation is addressed to as well, so the
/// published bucket the path opens with is what tells the two apart. The KEY is not part of
/// that reading, so a key naming no object this store holds is answered by the plane as the
/// object it does not hold, not by the router as a request naming no operation.
fn addresses_download_object(req: &RtRequest) -> bool {
    crate::state::addresses_download_store(service_common::request_path(req.uri()))
}

/// Serve one stored object: its own bytes, under its own length.
///
/// `with_body` is false for a `HEAD`, which reports the object's metadata without its
/// bytes.
fn download_response(bytes: &[u8], with_body: bool) -> RtResponse {
    let body = if with_body {
        SdkBody::from(bytes.to_vec())
    } else {
        SdkBody::empty()
    };
    let mut response = RtResponse::new(download_status(200), body);
    // A deployment package and a layer archive are both .zip archives, which is what the
    // store holds for them.
    response
        .headers_mut()
        .insert("content-type", "application/zip");
    response
        .headers_mut()
        .insert("content-length", bytes.len().to_string());
    response
}

/// One refusal of a download request, in this service's own JSON error envelope: the
/// path is not a modelled operation, so there is no modelled error to render.
fn download_error(code: u16, error_code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "__type": error_code, "message": message }).to_string();
    let mut response = RtResponse::new(download_status(code), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", "application/json");
    response
        .headers_mut()
        .insert("x-amzn-errortype", error_code.to_string());
    response
}

/// A status this plane answers with. Every call site passes a constant, so the conversion
/// cannot fail.
fn download_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("the download plane's statuses are valid constants")
}

/// [`addresses_download_object`] for the buffered request `can_handle` is handed, read by
/// the same rule, so the claim and the serving seam cannot disagree about which requests
/// belong to this plane.
fn claims_download_object(parts: &http::request::Parts) -> bool {
    crate::state::addresses_download_store(parts.uri.path())
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `lambda` — a signing name it SHARES with every other
        // emulated service in the Lambda family (Lambda Core, Lambda MicroVMs and
        // Lambda Web; `smithy_query(query="service_info")` gives all four
        // `sigv4Name: lambda`). The router resolves a request's routing key from the
        // SigV4 credential scope first, so all four see the key `lambda`: claiming on
        // the signing name alone makes each one claim the others' traffic, which the
        // router answers `500 AmbiguousService` instead of serving any of them —
        // leaving every Lambda operation unreachable.
        //
        // So the claim is narrowed to this service's own request space, by the one rule
        // `service-common` states for a shared signing name: this model is `restJson1`
        // (a REST protocol, so the `rpc_service_shape_id` argument is `None`), and a
        // REST service's own surface is the modelled URI path space its generated
        // `ROUTE_PATH_SEGMENTS` publishes — here the API-date segments this model binds
        // (`2015-03-31`, `2018-10-31`, …), which are disjoint from the peers' own
        // (`2026-04-04`, `2025-09-09`, `2025-03-07`) except for the shared tagging path
        // `/2017-03-31/tags/{Resource}`, where the ARN decides (see
        // [`claims_shared_tagging_path`]).
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        ) && claims_shared_tagging_path(parts)
            // And a request addressed to an object in one of the DOWNLOAD stores this
            // service publishes its presigned URLs into, which is a path space of its own
            // that no modelled operation binds: without this the router answers a URL this
            // service handed out as a request naming no operation. See the module docs.
            || claims_download_object(parts)
            // And a request addressed to a FUNCTION URL this service holds, for the same
            // reason: the endpoint `CreateFunctionUrlConfig` handed out names no service
            // in its first label either, and a client that cannot resolve that host
            // reaches the emulator's own endpoint carrying the url id in the path. The
            // claim is narrowed to a URL this store HOLDS (see
            // [`function_url::FunctionUrlPlane::claims`]), so no control-plane request
            // and no other service's traffic is claimed by it.
            || self.function_urls_anonymous.claims(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // The download plane is the one such request: a fetch of a URL this service
        // announced names no modelled operation at all, so there is no routing key to
        // resolve for it (see the module docs). Every method addressed to a published
        // download object reaches the plane, which decides the method itself: a read is
        // served and anything else is refused as a method, rather than falling through to
        // the router's answer for an unrouted request.
        if addresses_download_object(req) {
            return Some(&self.downloads as &dyn ErasedOperation);
        }
        // And a request to a FUNCTION URL this service holds, which names no modelled
        // operation either: the url id is read off the request's address and the serving
        // mounting is the one the stored `AuthType` names, so a URL restricted to
        // authenticated callers is served only to a request the emulator verified a
        // signature for (see [`function_url`]).
        //
        // Every such request is NAMED, including a CORS preflight the addressed URL's own
        // configuration answers: naming it is what keeps the front door's own checks in
        // front of this plane — the refusal of a request whose browser-set headers say a
        // browser sent it from another site, and the signature verification of an `AWS_IAM`
        // URL — so nothing this service serves steps around them.
        if let Some(target) = self.function_urls_anonymous.target(req) {
            return Some(
                if self.function_urls_aws_iam.serves_auth_type(&target.auth_type) {
                    &self.function_urls_aws_iam as &dyn ErasedOperation
                } else {
                    &self.function_urls_anonymous as &dyn ErasedOperation
                },
            );
        }
        // And `InvokeWithResponseStream`: its output binds a `@streaming` union, which
        // the generated routing-table entry declines (that entry is still what the router
        // reads the model's auth schemes off, so the operation stays signed exactly as the
        // model declares). The match is the operation's own route, applied by the function
        // that lives beside the handler, so routing and handling cannot disagree about
        // which requests it claims.
        if crate::ops::invoke_with_response_stream::addresses_response_streaming_invocations(req) {
            return Some(&self.response_stream as &dyn ErasedOperation);
        }
        // Every other modelled operation is served WRAPPED. This is NOT a re-route: the
        // key is the one the generated table's own protocol resolves, and the operation it
        // names is the one served — wrapped in `StatedPerOperation` so a refused request
        // quotes the value it refused (see that type). A key the model does not name
        // resolves to `None` here exactly as it does in the generated table, so the
        // router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. Everything about the envelope — the `N validation error(s)
        // detected:` prefix, the member paths, the echoed value and every requirement
        // sentence — is the framework's, so the two refusal paths cannot drift.
        //
        // TWO renderings are stated here, neither of which a model records:
        //
        // * the ORDER of an enum's value set (see [`reported_enum_order`]);
        // * how a `@pattern` requirement quotes its expression. AWS states it WITHOUT
        //   the `^`/`$` anchors a whole-value Smithy pattern wraps it in, so a
        //   `TaggableResource` the tagging URIs refuse is reported as
        //   `Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*):lambda:…`
        //   rather than `^arn:…$`. That wording is shared by every service on a JSON
        //   protocol, so it lives in `service-common` and is delegated to rather than
        //   restated — the CHECK still uses the model's pattern as written, and only
        //   the rendering drops the pair.
        //
        // Both live in [`stated_violations`], which is also what every wrapped operation
        // words its refusal through. What this path cannot do is quote the value as
        // SUBMITTED: a refusal reaches here with the violations alone, and the unelided
        // value is only recoverable from the built input — which is why the operations are
        // wrapped. A refusal that reaches here is one no wrapped operation saw.
        let Refusal::Invalid(violations) = refusal else {
            // An unauthenticated caller keeps the framework's 403 code and wording.
            return None;
        };
        stated_violations(violations, &std::collections::HashMap::new())
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
        //
        // The envelope stays the framework's; what is added is the one header every answer
        // of this service names the request it answered in (see [`request_id_header`]), so a
        // client correlating on it has an id for a refused route as much as for a served
        // call.
        Some(request_id_header::with_request_id_header(
            self.entry.routing_error_response(req),
        ))
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
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-lambda-awsgirapiservice.json"
    );

    /// The one modelled operation `wrap_operations!` does not carry, because it
    /// implements no `smithy_server_runtime::Operation` to wrap: its output binds a
    /// `@streaming` union, so it is served by its own hand-written entry, which reads and
    /// checks its own input against the model (`crate::ops::invoke_with_response_stream`).
    const SERVED_BY_ITS_OWN_ENTRY: &str = "InvokeWithResponseStream";

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedPerOperation`], under the same key the generated routing table is
    /// keyed by — so every one of them quotes a refused value as submitted.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// answering a refusal with the value shortened, and a key spelled differently from
    /// the generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_is_wrapped() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .filter(|name| *name != SERVED_BY_ITS_OWN_ENTRY)
            .collect();
        assert!(
            operations.len() > 80,
            "case the model's surface: expected this service's whole operation surface, \
             got {} operations",
            operations.len()
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so a value \
                 its refusal quotes is the framework's shortened rendering instead of the \
                 bytes the caller sent"
            );
        }
    }
}
