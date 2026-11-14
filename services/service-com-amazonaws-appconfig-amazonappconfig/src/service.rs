// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appconfig-amazonappconfig` service: how it claims requests and how it words the
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

mod immutable_profile_type;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

/// The body member every error shape in this service's model reports its detail text in
/// (`smithy_query(query="shape",
/// shape="BadRequestException,ResourceNotFoundException,ConflictException,InternalServerException,ServiceQuotaExceededException,PayloadTooLargeException")`
/// — each declares `Message`, none with a `@jsonName`). This is a restJson1 service, so a
/// member's wire name IS its Smithy member name, and that is the one member an AppConfig
/// client reads an error's text out of.
const DETAIL_MEMBER: &str = "Message";

/// The member the FRAMEWORK's own error renderers write that text in: the member name of
/// the synthetic `smithy.framework#ValidationException` / `NotImplemented` shapes the
/// pre-handler refusal and the scaffold 501 are built from.
const RENDERED_DETAIL_MEMBER: &str = "message";

/// Every error shape this service's model declares, by the wire code (`__type`) it renders
/// under — the same `smithy_query` call [`DETAIL_MEMBER`] cites, which is where this list
/// comes from and the only thing that may extend it.
///
/// The rename below is keyed on THIS set rather than on the status, so a failure whose
/// class AppConfig does not model keeps the member the framework wrote its text in: the
/// scaffold's own 501 `NotImplemented` (the emulator saying the operation is not written
/// yet) and an authentication refusal, which is the emulator's and which
/// [`EmulatedService::override_refusal`] deliberately leaves in the framework's words.
const MODELLED_ERROR_SHAPES: [&str; 6] = [
    "BadRequestException",
    "ConflictException",
    "InternalServerException",
    "PayloadTooLargeException",
    "ResourceNotFoundException",
    "ServiceQuotaExceededException",
];

/// The unconstrained run a quoted `@pattern` spells for an end the model leaves OPEN.
const ANY_RUN: &str = ".*";

/// A `@pattern` requirement as this service quotes it: the expression the WHOLE value has
/// to satisfy.
///
/// A Smithy `@pattern` is a FIND — the expression has to match SOMEWHERE in the value — so
/// the expression a member declares and the expression its whole value must satisfy are two
/// different things, and AppConfig's refusals state the second. The two ends are decided
/// independently, because a model may anchor one and leave the other open:
///
/// * an expression the model's two anchors ENCLOSE which already anchors ITS OWN ENDS —
///   every branch of `DeploymentStrategyId`'s
///   `^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$` pins both of its own ends — demands
///   the whole value on its own, so that pair is the model's wrapping of it and goes with
///   both ends: the expression advertised is the enclosed one,
///   `(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)`, interior anchors and all;
/// * an end the model ANCHORS around an expression that does not pin its own ends keeps
///   the model's own text at that end, except the trailing `$`, which goes: demanding the
///   expression over the whole value IS what that anchor said, so it is not part of what the
///   refusal advertises (`FlagKey` is modelled `^[a-z][a-zA-Z0-9_-]{1,64}$` and quoted
///   `^[a-z][a-zA-Z0-9_-]{1,64}`, and an anchor that pins neither end is quoted where it
///   stands);
/// * an end the model leaves OPEN admits any run of characters there, which is spelled as
///   [`ANY_RUN`] unless the expression already leaves that end open with one — so
///   `VersionLabel`, modelled `[^0-9]` and anchored at neither end, is quoted
///   `.*[^0-9].*`: "must contain a non-digit", which is what that `@pattern` demands,
///   rather than the bare expression, which reads as "must be one non-digit character".
///
/// A trailing `$` preceded by an ODD number of backslashes is an escaped literal dollar
/// rather than the anchor, so that end is an open one; whether the enclosed expression pins
/// its own ends is read the same way, through
/// `service_common::validation::anchors_its_own_ends`, which is called rather than restated
/// here.
///
/// Only the RENDERING decides the ends. The CHECK runs the model's pattern as written, so a
/// value that satisfies the quoted expression over a PREFIX of itself is still refused.
///
/// This is NOT the fleet rule `service_common::validation::whole_value_expression` states
/// (which drops the anchor at BOTH ends, the rendering those services' own recordings
/// show): AppConfig's refusals keep the leading `^` of an expression that does not pin its
/// own ends, so quoting a pattern is part of this service's validation-message dialect and
/// is stated here, for every operation at once.
fn quoted_pattern(pattern: &str) -> String {
    let (body, anchored_at_start, anchored_at_end) = split_outer_anchors(pattern);
    // An enclosed expression that pins its own ends is already the whole-value one, so the
    // pair around it is the model's wrapping and neither end of it is advertised.
    if anchored_at_start
        && anchored_at_end
        && service_common::validation::anchors_its_own_ends(body)
    {
        return body.to_string();
    }
    let mut quoted = String::with_capacity(pattern.len() + 2 * ANY_RUN.len());
    if anchored_at_start {
        quoted.push('^');
    } else if !body.starts_with(ANY_RUN) {
        quoted.push_str(ANY_RUN);
    }
    quoted.push_str(body);
    if !anchored_at_end && !body.ends_with(ANY_RUN) {
        quoted.push_str(ANY_RUN);
    }
    quoted
}

/// `pattern` without the anchors at its two ends, and whether each end carried one.
///
/// The ends are read independently, and a `$` preceded by an odd number of backslashes is
/// an escaped literal dollar rather than the anchor.
fn split_outer_anchors(pattern: &str) -> (&str, bool, bool) {
    let (body, anchored_at_start) = match pattern.strip_prefix('^') {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };
    match body.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) => (inner, anchored_at_start, true),
        _ => (body, anchored_at_start, false),
    }
}

/// Whether `body` ends in a backslash that escapes whatever followed it.
fn ends_with_escape(body: &str) -> bool {
    (body.len() - body.trim_end_matches('\\').len()) % 2 == 1
}

/// The path segment a collection step is spelled with in [`floating_point_member_paths`]:
/// the framework reports a list element under its 1-based POSITION
/// (`treatments.1.member.weight`), while which members are floating point is a property of
/// the model rather than of one request's element count, so one entry stands for every
/// position.
const ANY_POSITION: &str = "*";

/// The member paths of this service's FLOATING-POINT members, as a constraint clause names
/// them, with every collection step spelled [`ANY_POSITION`].
///
/// Read off the MODEL rather than named by hand:
/// `service_common::validation_message::floating_point_member_paths` collects the members
/// each shape's own compiled schema types as `float` or `double`. A member schema carries
/// no members of its own, so a NESTED shape is passed as its own schema under the path that
/// binds it — `TreatmentInput` under the two members of an experiment-definition request
/// that target it. The shapes listed are the ones whose float members declare a `@range`
/// (`smithy_query(query="shape",
/// shape="GrowthFactor,Weight,NullablePercentage,TreatmentInput")`: `GrowthFactor` 1.0-100.0
/// on the two deployment-strategy requests, `Weight` min 0.0 on `TreatmentInput`,
/// `NullablePercentage` 0.0-100.0 on the two experiment-run requests), since a float member
/// under no constraint can never be the subject of a clause;
/// `every_floating_point_member_under_a_range_is_covered` pins that against the model.
fn floating_point_member_paths() -> &'static BTreeSet<String> {
    /// The member of an experiment-definition request that targets `TreatmentInput`
    /// directly …
    const CONTROL: &str = "control";
    /// … and the one that targets a LIST of it, whose elements the walk reports one
    /// collection step deeper.
    const TREATMENTS: &str = "treatments";

    static PATHS: OnceLock<BTreeSet<String>> = OnceLock::new();
    PATHS.get_or_init(|| {
        let element = format!("{TREATMENTS}.{ANY_POSITION}.member");
        service_common::validation_message::floating_point_member_paths(&[
            (
                "",
                crate::ops::types::CREATE_DEPLOYMENT_STRATEGY_INPUT_SCHEMA,
            ),
            (
                "",
                crate::ops::types::UPDATE_DEPLOYMENT_STRATEGY_INPUT_SCHEMA,
            ),
            ("", crate::ops::types::START_EXPERIMENT_RUN_INPUT_SCHEMA),
            ("", crate::ops::types::UPDATE_EXPERIMENT_RUN_INPUT_SCHEMA),
            (CONTROL, crate::ops::types::TREATMENT_INPUT_SCHEMA),
            (&element, crate::ops::types::TREATMENT_INPUT_SCHEMA),
        ])
    })
}

/// Whether the member a clause names at `path` is a floating-point one.
fn is_floating_point_member(path: &str) -> bool {
    floating_point_member_paths().contains(&at_any_position(path))
}

/// `path` with every collection position replaced by [`ANY_POSITION`].
fn at_any_position(path: &str) -> String {
    path.split('.')
        .map(|segment| {
            if segment.parse::<usize>().is_ok() {
                ANY_POSITION
            } else {
                segment
            }
        })
        .collect::<Vec<&str>>()
        .join(".")
}

/// A `@range` bound as a FLOATING-POINT member's clause states it: the decimal text a
/// floating-point value is written with.
///
/// The framework renders an integral bound without a fractional part (`100`, not `100.0`),
/// which is what an INTEGRAL member's bound reads as; a float member's bound is a
/// floating-point value and is written as one, through the one function that writes a
/// floating-point value into a clause
/// (`service_common::validation_message::float_value_text`, which this service also quotes
/// the submitted VALUE through). A bound that is not a number at all is left as the
/// framework rendered it.
fn decimal_bound(bound: &str) -> String {
    match bound.parse::<f64>() {
        Ok(value) => service_common::validation_message::float_value_text(value),
        Err(_) => bound.to_string(),
    }
}

/// The same violation with the two things this service words differently restated: how a
/// `@pattern` requirement is quoted ([`quoted_pattern`]), and how a FLOATING-POINT member's
/// value and `@range` bound are written ([`decimal_bound`]).
///
/// Every other requirement class — a null member, a length, an enum value set, a uniqueness
/// demand — and every INTEGRAL member's bound is handed back untouched, so its wording, the
/// member path and the echoed value stay the framework's and cannot drift per operation.
pub(crate) fn as_stated(violation: &Violation) -> Violation {
    // The VALUE half first, since a float member's value is written as a decimal whichever
    // requirement that value broke.
    let stated = service_common::validation_message::in_float_value_rendering(
        violation,
        is_floating_point_member,
    );
    let restated = match &stated.kind {
        ViolationKind::Pattern(pattern) => Some(ViolationKind::Pattern(quoted_pattern(pattern))),
        ViolationKind::MinValue(bound) if is_floating_point_member(&stated.path) => {
            Some(ViolationKind::MinValue(decimal_bound(bound)))
        }
        ViolationKind::MaxValue(bound) if is_floating_point_member(&stated.path) => {
            Some(ViolationKind::MaxValue(decimal_bound(bound)))
        }
        _ => None,
    };
    match restated {
        Some(kind) => Violation { kind, ..stated },
        None => stated,
    }
}

/// The error body with its detail member renamed to [`DETAIL_MEMBER`], or `None` when
/// there is nothing to rename: the body is not a JSON object, its `__type` names no error
/// shape this service's model declares (see [`MODELLED_ERROR_SHAPES`]), or it carries no
/// rendered detail member.
///
/// Idempotent, and non-destructive: a body that already carries a `Message` member keeps
/// that value, so a member of the MODELLED error shape always wins over the renamed one.
fn with_detail_member(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    // The `__type` may be namespaced (`com.amazonaws.appconfig#BadRequestException`), so
    // the shape name is what is matched.
    let shape = object
        .get("__type")
        .and_then(Value::as_str)?
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    if !MODELLED_ERROR_SHAPES.contains(&shape.as_str()) {
        return None;
    }
    let detail = object.remove(RENDERED_DETAIL_MEMBER)?;
    object.entry(DETAIL_MEMBER).or_insert(detail);
    serde_json::to_vec(&document).ok()
}

/// The response as this service answers it: an error body whose detail text is in the
/// `Message` member its model declares for it.
///
/// A response that is not a failure, or whose body is not a modelled error body carrying a
/// rendered detail member (see [`with_detail_member`]), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them. A modelled
/// error a handler returns is serialized from the model's own shape, so it already
/// carries `Message` and nothing is rewritten.
fn with_service_detail_member(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_detail_member(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if
        // it ever did, the header is REMOVED rather than left describing the body before
        // the rename, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// The error code this service names for a request that broke a modelled constraint.
///
/// AppConfig's model declares NO `ValidationException` anywhere: every operation lists
/// `BadRequestException` (HTTP 400) for an input its constraints refuse, so that is the
/// code a client switches on, and it is stated once here for both paths that render such a
/// refusal ([`InServiceDialect`] and [`EmulatedService::override_refusal`]).
const CONSTRAINT_REFUSAL_CODE: &str = "BadRequestException";

/// This service's answer to the violations `req` broke, or `None` when it broke none.
///
/// The sentence is the framework's own `N validation error(s) detected: Value '<v>' at
/// '<member>' failed to satisfy constraint: …` envelope — already the text AWS states for a
/// constraint violation, and built from the request's own member path and value — with each
/// violation restated through [`as_stated`] for the one thing AppConfig words differently,
/// how a `@pattern` requirement is quoted. The status and the envelope stay the
/// framework's, through the shared renderer every service that words its own pre-handler
/// refusal answers from (`service_common::front_door::worded_rejection`): a constraint
/// failure is a 400 in every protocol.
///
/// The values it quotes are the bytes the CALLER SENT. `service_common::validation`
/// re-reads the same built input the constraint walk validated to restore each echo,
/// because the framework stores a long value already shortened to a prefix plus a
/// byte-count annotation — text no AWS service emits — and the echoed value is part of the
/// request-derived rule a caller reads the rejected input back out of.
fn constraint_refusal<O: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    // The protocol is read, used and dropped inside this one function, so nothing borrowed
    // from the service definition is held across the caller's await: `&dyn
    // ErasedServiceDefinition` is not `Sync`.
    let violations =
        service_common::validation::refusing_violations::<O>(svc.definition().protocol(), req);
    let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
    let message = envelope(&stated)?;
    Some(service_common::front_door::worded_rejection(
        svc,
        req,
        violations,
        Refused {
            code: CONSTRAINT_REFUSAL_CODE.to_string(),
            message,
        },
    ))
}

/// Serves one modelled operation, with every failure of it stated in this service's own
/// dialect: a pre-handler constraint refusal QUOTING the value the caller submitted, and
/// the detail text of whatever is answered reported in the `Message` member this service's
/// model declares for it.
///
/// WHY a wrapper and not [`Service::override_refusal`]: that seam is handed the violations
/// ALONE — no operation, no request — so it can neither recover a value the framework
/// already shortened as it BUILT the violation, nor name the operation whose input schema
/// the request would have to be re-read against. Nor can it choose the body member the
/// words are serialized into: the framework writes a framework-rendered error's text into
/// the synthetic shapes' own `message`. A per-operation wrapper has the request, the
/// operation and the whole response, so it can do all three.
///
/// WHY not `service_common::front_door::FrontDoor`, which is the same wrapping for the
/// services whose own front door refuses a body: that wrapper narrows its refusal to the
/// violations a member BOUND TO THE BODY broke (`binds_to_body`) and words them as that
/// layer's fixed sentence, and AppConfig does neither — it refuses a `@httpLabel`, an
/// `@httpQuery` and an `@httpHeader` outside its constraints in the same modelled
/// `BadRequestException` as a body member, which `pattern_refusal_quoting` pins per
/// binding. Everything that is NOT this service's own is taken from the shared module all
/// the same: the collecting walk ([`constraint_refusal`]), the rendering
/// (`front_door::worded_rejection`) and the key an operation is filed under ([`served`]).
/// What stays here is the two things that are AppConfig's: refusing over EVERY violation
/// rather than the body's, and the `Message` member its model declares.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery (the protocol's deserializer and its constraint walk over the built
/// input), so this cannot find a different set from the walk that follows it; the cost is
/// that a request it does not refuse is read twice, bounded by the body limits the
/// framework already enforces.
struct InServiceDialect<O> {
    operation: O,
}

impl<O> InServiceDialect<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InServiceDialect<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = constraint_refusal::<O>(svc, req) {
                return with_service_detail_member(refused);
            }
            with_service_detail_member(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality. That path conjunct is what separates two services sharing one
/// SigV4 signing name — the AppConfig control plane and the AppConfigData data plane
/// both sign as `appconfig`, so the key alone matches both and the router would reject
/// every such request as `500 AmbiguousService` — and their modelled path spaces are
/// disjoint (`applications`, `deploymentstrategies`, `extensions`, … here;
/// `configuration` and `configurationsessions` there).
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed
/// only the request's parts, so the two things it reads — the URI and the headers —
/// are re-presented to it as a bodiless request. It reads neither the body nor the
/// method, so nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-appconfig-amazonappconfig` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each served through
    /// [`InServiceDialect`]: a pre-handler constraint refusal quotes the value the caller
    /// submitted, and every error's detail text is reported in the `Message` member this
    /// service's model declares for it. Keyed by the routing key the protocol resolves —
    /// the key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
    in_service_dialect: OperationRoutingTable,
    /// The route each of those operations binds, read off its own input schema as it is
    /// filed: what [`EmulatedService::addressed_operation`] resolves a request the
    /// framework's own lookup named no operation for.
    modelled_routes: Vec<ModelledRoute>,
}

/// One modelled operation and the `@http` route its input schema binds.
///
/// The route is not written here: the method and the URI are the schema's own, so this
/// carries the schema and reads them through it.
pub(super) struct ModelledRoute {
    /// The operation's modelled name — the key it is filed under.
    pub(super) operation: &'static str,
    /// Its input schema, which carries the `@http` method and URI pattern.
    pub(super) input_schema: &'static ::aws_smithy_schema::Schema,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table` through [`served`].
macro_rules! wrap_operations {
    ($table:expr, $routes:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            served(
                &mut $table,
                &mut $routes,
                crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
            );
        )+
    };
}

/// Serve `operation` through [`InServiceDialect`], filed under its own modelled
/// [`Operation::NAME`] — the routing key the protocol resolves and the generated table is
/// keyed by, so the key cannot be spelled by hand. The same keying
/// `service_common::front_door::FrontDoorTable::serve` does for the shared wrapper.
///
/// The operation's own `@http` route is recorded in `routes` as it is filed, so the
/// fallback lookup ([`EmulatedService::addressed_operation`]) spans exactly the operations
/// this service serves.
fn served<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    routes: &mut Vec<ModelledRoute>,
    operation: O,
) {
    routes.push(ModelledRoute {
        operation: O::NAME,
        input_schema: O::INPUT_SCHEMA,
    });
    table.add_operation(O::NAME, Box::new(InServiceDialect::new(operation)));
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut in_service_dialect = OperationRoutingTable::new();
        let mut modelled_routes: Vec<ModelledRoute> = Vec::new();
        // EVERY modelled operation, so both rules are applied by ONE code path rather
        // than per operation: an operation whose input declares no constraint simply
        // never has a violation to find, which the wrapper reads off that operation's own
        // input schema. `every_modelled_operation_is_served_in_this_services_dialect`
        // pins the list against the model.
        wrap_operations!(
            in_service_dialect,
            modelled_routes,
            engine,
            create_application::CreateApplication,
            create_configuration_profile::CreateConfigurationProfile,
            create_deployment_strategy::CreateDeploymentStrategy,
            create_environment::CreateEnvironment,
            create_experiment_definition::CreateExperimentDefinition,
            create_extension::CreateExtension,
            create_extension_association::CreateExtensionAssociation,
            create_hosted_configuration_version::CreateHostedConfigurationVersion,
            delete_application::DeleteApplication,
            delete_configuration_profile::DeleteConfigurationProfile,
            delete_deployment_strategy::DeleteDeploymentStrategy,
            delete_environment::DeleteEnvironment,
            delete_experiment_definition::DeleteExperimentDefinition,
            delete_extension::DeleteExtension,
            delete_extension_association::DeleteExtensionAssociation,
            delete_hosted_configuration_version::DeleteHostedConfigurationVersion,
            get_account_settings::GetAccountSettings,
            get_application::GetApplication,
            get_configuration::GetConfiguration,
            get_configuration_profile::GetConfigurationProfile,
            get_deployment::GetDeployment,
            get_deployment_strategy::GetDeploymentStrategy,
            get_environment::GetEnvironment,
            get_experiment_definition::GetExperimentDefinition,
            get_experiment_run::GetExperimentRun,
            get_extension::GetExtension,
            get_extension_association::GetExtensionAssociation,
            get_hosted_configuration_version::GetHostedConfigurationVersion,
            list_applications::ListApplications,
            list_configuration_profiles::ListConfigurationProfiles,
            list_deployment_strategies::ListDeploymentStrategies,
            list_deployments::ListDeployments,
            list_environments::ListEnvironments,
            list_experiment_definitions::ListExperimentDefinitions,
            list_experiment_run_events::ListExperimentRunEvents,
            list_experiment_runs::ListExperimentRuns,
            list_extension_associations::ListExtensionAssociations,
            list_extensions::ListExtensions,
            list_hosted_configuration_versions::ListHostedConfigurationVersions,
            list_tags_for_resource::ListTagsForResource,
            start_deployment::StartDeployment,
            start_experiment_run::StartExperimentRun,
            stop_deployment::StopDeployment,
            stop_experiment_run::StopExperimentRun,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_account_settings::UpdateAccountSettings,
            update_application::UpdateApplication,
            update_deployment_strategy::UpdateDeploymentStrategy,
            update_environment::UpdateEnvironment,
            update_experiment_definition::UpdateExperimentDefinition,
            update_experiment_run::UpdateExperimentRun,
            update_extension::UpdateExtension,
            update_extension_association::UpdateExtensionAssociation,
            validate_configuration::ValidateConfiguration,
        );
        // `UpdateConfigurationProfile` is served through the same dialect as every
        // operation above, behind the one guard a handler cannot apply itself: a request
        // asking the profile to become a different `Type` names a member the model does
        // not declare, which is dropped before any handler sees it. See
        // [`immutable_profile_type`].
        immutable_profile_type::served(
            &mut in_service_dialect,
            &mut modelled_routes,
            Arc::clone(&engine),
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_service_dialect,
            modelled_routes,
        }
    }

    /// The operation whose modelled route `req` addresses, for a request the framework's
    /// own lookup named NO operation for.
    ///
    /// WHY a second lookup: the framework's route table REJECTS a route whose `@required`
    /// header or query bindings the request does not carry. That rejection is
    /// disambiguation for the services where several operations share one method and path
    /// and only those bindings tell them apart; AppConfig's model overlaps no such route —
    /// each of its operations owns its (method, URI) — so for this service the rejection
    /// turns an INCOMPLETE request into a request for an operation that does not exist. The
    /// operation it addressed is resolved here instead, and refuses the member it left out
    /// through its own modelled constraints, the way it refuses every other member outside
    /// them.
    ///
    /// Each candidate's method and URI pattern are the ones its own input schema carries,
    /// matched by `service_common::addresses_modeled_http_route`; a path TWO of them could
    /// claim resolves to neither, leaving the framework's own answer in place, since picking
    /// between them is what the bindings the request omitted are for.
    fn addressed_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&'static str> {
        let mut matching = self
            .modelled_routes
            .iter()
            .filter(|route| service_common::addresses_modeled_http_route(req, route.input_schema));
        let addressed = matching.next()?;
        matching.next().is_none().then_some(addressed.operation)
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
        // Delegated to the framework's own predicate — see [`can_handle_parts`]:
        // this service's signing name (`appconfig`) is shared with the AppConfigData
        // data plane, so the claim is the route key AND the model-derived first URI
        // path segment. Matching on the signing name alone would have both services
        // claim every `appconfig`-signed request, which the router answers with 500
        // `AmbiguousService` instead of serving either one.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // [`InServiceDialect`] so a request refused for breaking a modelled constraint
        // quotes the value the caller submitted, and so an error the FRAMEWORK renders
        // (that refusal, the scaffold's 501) reports its detail text in the `Message`
        // member every error shape in this service's model declares, rather than in the
        // synthetic shapes' own `message`. A key the model does not name resolves to
        // `None` here exactly as it does in the generated table, so the router's
        // routing-error path is unchanged.
        //
        // A request the protocol names no key for is given one more chance, on the route
        // its method and path address ([`Self::addressed_operation`]): a `@required`
        // header or query member a request leaves out is a request this service refuses,
        // not an operation it does not have.
        match self.entry.protocol().routing_key(req) {
            Ok(key) => self.in_service_dialect.get(key),
            Err(_) => self
                .in_service_dialect
                .get(self.addressed_operation(req)?),
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Every modelled operation is served through [`InServiceDialect`], which refuses
        // a constraint violation itself — in this code, in these words, with the value
        // the caller submitted quoted in full — so this seam is the FALLBACK for a
        // refusal that reaches the framework's own walk anyway. Wording it here from the
        // same violations keeps the two paths from disagreeing: a value outside a
        // declared bound is this service's `BadRequestException` either way, with the
        // same sentence.
        //
        // AppConfig's model declares NO `ValidationException` anywhere: a request that
        // violates a declared constraint is refused with the modelled
        // `BadRequestException` (HTTP 400), which every operation in this service
        // lists, so that is the code a client switches on. The framework's default
        // names `ValidationException` — a class this service does not model, which an
        // SDK cannot classify — so the code is replaced here.
        //
        // The one thing this seam cannot do is quote a long value as the caller sent it:
        // it is handed the violations alone, not the request they were collected from,
        // and the framework shortens such a value as it BUILDS the violation. That is
        // why the wrapper collects the refusal itself.
        //
        // `Unauthenticated` keeps the framework's wording: authentication is the
        // emulator's, not AppConfig's, and this service models no error for it.
        match refusal {
            Refusal::Invalid(violations) => {
                let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
                Some(Refused {
                    code: CONSTRAINT_REFUSAL_CODE.to_string(),
                    message: envelope(&stated)
                        .unwrap_or_else(|| "invalid request".to_string()),
                })
            }
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
        as_stated, floating_point_member_paths, quoted_pattern, with_detail_member, EmulatedService,
    };
    use aws_smithy_schema_server::{Violation, ViolationKind};
    use std::sync::Arc;

    /// RULE: a `@pattern` is quoted as the expression the WHOLE value must satisfy — an
    /// expression the model's two anchors enclose which already pins ITS OWN ENDS is quoted
    /// without that pair, an end the model anchors around an expression that does not keeps
    /// the model's text there except the trailing `$`, which goes, and an end the model
    /// leaves open is spelled with the unconstrained run `.*`. Nothing else about the
    /// expression is touched.
    ///
    /// The rows are every distinct SHAPE of `@pattern` this service's model carries
    /// (`smithy_query(query="shape", shape="FlagKey,Id,DeploymentStrategyId,VersionLabel,Arn")`
    /// and `smithy_query(query="operation", operation="CreateConfigurationProfile")` for the
    /// role-ARN and type members), plus the regex constructs that can hide an anchor: an
    /// escaped dollar, a dollar inside the expression, and each half-anchored form.
    #[test]
    fn a_pattern_is_quoted_as_the_whole_value_expression() {
        // (case, the modelled expression, the expression a clause quotes)
        let rows: [(&str, &str, &str); 12] = [
            (
                "the experiment flag key",
                r"^[a-z][a-zA-Z0-9_-]{1,64}$",
                r"^[a-z][a-zA-Z0-9_-]{1,64}",
            ),
            ("a minted resource id", r"^[a-z0-9]{4,7}$", r"^[a-z0-9]{4,7}"),
            // Branches that carry anchors of their own already demand the whole value, so
            // the pair the model wrote around them is its wrapping and goes with both
            // ends; the expression's own interior anchors are quoted as modelled.
            (
                "a strategy id, whose branches are anchored individually",
                r"^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$",
                r"(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)",
            ),
            ("a version label", r"[^0-9]", r".*[^0-9].*"),
            (
                "a resource ARN",
                r"^arn:(aws[a-zA-Z-]*)?:[a-z]+:(\d{12})?:[a-zA-Z0-9-_/:.]+$",
                r"^arn:(aws[a-zA-Z-]*)?:[a-z]+:(\d{12})?:[a-zA-Z0-9-_/:.]+",
            ),
            (
                "a configuration profile type",
                r"^[a-zA-Z0-9\.\-]+$",
                r"^[a-zA-Z0-9\.\-]+",
            ),
            // An escaped trailing dollar is a literal dollar sign, not the anchor, so the
            // expression leaves that end open and the open-end run is spelled there.
            ("an escaped trailing dollar", r"^costs\$", r"^costs\$.*"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            ("an escaped backslash then the anchor", r"^[a-z]\\$", r"^[a-z]\\"),
            // A dollar that is not at the end is not the end anchor either, and it pins
            // neither end of the expression, so the model's own leading `^` is quoted as
            // the `FlagKey` row's is.
            ("a dollar inside the expression", r"^a$b$", r"^a$b"),
            // Already open at the end with a run of its own: nothing is added to it.
            ("anchored at the start only", r"^arn:aws.*", r"^arn:aws.*"),
            // Anchored at the END only: that anchor goes with the end it anchored, and
            // the open start is spelled with the run it admits.
            ("anchored at the end only", r"[0-9]{12}$", r".*[0-9]{12}"),
            ("no anchor at all", r"\d{3}", r".*\d{3}.*"),
        ];
        for (case, modelled, expected) in rows {
            let actual = quoted_pattern(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {modelled:?} to be quoted as {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    /// RULE: a `@pattern` requirement and a FLOATING-POINT member's value and `@range`
    /// bound are restated — every other class, and every integral member's bound, keeps
    /// the framework's own wording, member path and echoed value.
    #[test]
    fn only_a_pattern_and_a_float_members_bound_are_restated() {
        let pattern = Violation {
            path: "flagKey".to_string(),
            kind: ViolationKind::Pattern(r"^[a-z][a-zA-Z0-9_-]{1,64}$".to_string()),
            value: Some("1invalid".to_string()),
            sensitive: false,
        };
        let stated = as_stated(&pattern);
        assert_eq!(
            stated.kind,
            ViolationKind::Pattern(r"^[a-z][a-zA-Z0-9_-]{1,64}".to_string()),
            "expected the pattern quoted without its end anchor, got {:?}",
            stated.kind
        );
        assert_eq!(
            (stated.path.as_str(), stated.value.as_deref()),
            ("flagKey", Some("1invalid")),
            "expected the framework's own path and echoed value, got {:?}",
            (stated.path.as_str(), stated.value.as_deref())
        );

        // Every float member of this service that declares a `@range`, each with the bound
        // the framework rendered as an integral one and the decimal text its clause states.
        // (case, the member path a clause names, the framework's violation, the bound and
        // value the clause must state)
        let floats: [(&str, &str, ViolationKind, &str, &str); 4] = [
            (
                "a deployment strategy's growth factor over its maximum",
                "growthFactor",
                ViolationKind::MaxValue("100".to_string()),
                "100.0",
                "150.0",
            ),
            (
                "an experiment run's exposure over its maximum",
                "exposurePercentage",
                ViolationKind::MaxValue("100".to_string()),
                "100.0",
                "150.0",
            ),
            (
                "a treatment list element's weight under its minimum",
                "treatments.1.member.weight",
                ViolationKind::MinValue("0".to_string()),
                "0.0",
                "150.0",
            ),
            (
                "the control treatment's weight under its minimum",
                "control.weight",
                ViolationKind::MinValue("0".to_string()),
                "0.0",
                "150.0",
            ),
        ];
        for (case, path, kind, bound, value) in floats {
            let violation = Violation {
                path: path.to_string(),
                kind,
                value: Some("150".to_string()),
                sensitive: false,
            };
            let stated = as_stated(&violation);
            let requirement = stated.message();
            assert!(
                requirement.ends_with(bound),
                "case {case:?}: expected the bound stated as {bound:?}, got {requirement:?}"
            );
            assert_eq!(
                stated.value.as_deref(),
                Some(value),
                "case {case:?}: expected the value quoted as {value:?}, got {:?}",
                stated.value
            );
        }

        // An INTEGRAL member's bound and value are the framework's: `protectionPeriodInMinutes`
        // and `deploymentDurationInMinutes` are integers, so nothing is requoted for them.
        for (case, path) in [
            ("an account setting's protection period", "protectionPeriodInMinutes"),
            (
                "a deployment strategy's duration",
                "deploymentDurationInMinutes",
            ),
        ] {
            let violation = Violation {
                path: path.to_string(),
                kind: ViolationKind::MaxValue("1440".to_string()),
                value: Some("1441".to_string()),
                sensitive: false,
            };
            let stated = as_stated(&violation);
            assert_eq!(
                stated, violation,
                "case {case:?}: expected the framework's own violation, got {stated:?}"
            );
        }

        for other in [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(64),
            ViolationKind::MinValue("0".to_string()),
            ViolationKind::MaxValue("1440".to_string()),
            ViolationKind::EnumValue(vec!["NONE".to_string()]),
            ViolationKind::UniqueItems,
        ] {
            let violation = Violation {
                path: "name".to_string(),
                kind: other.clone(),
                value: Some("x".to_string()),
                sensitive: false,
            };
            let stated = as_stated(&violation);
            assert_eq!(
                stated, violation,
                "case {other:?}: expected the framework's own violation, got {stated:?}"
            );
        }
    }

    /// RULE applied to the whole surface: every FLOATING-POINT member a constraint clause
    /// can be stated about — one reachable from an operation's input and declaring a
    /// `@range` — is covered by [`floating_point_member_paths`], so its value and its bound
    /// are written as decimals.
    ///
    /// The members come from the MODEL rather than from a second hand-written list: a float
    /// member added to an input shape (or a `@range` put on one that had none) fails here
    /// instead of quietly having its clause state an integer.
    #[test]
    fn every_floating_point_member_under_a_range_is_covered() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");

        // Walk out from every operation's INPUT, since a clause names a member of a
        // request. A member's own traits or its target shape's may carry the `@range`.
        let mut pending: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(_, shape)| shape["input"]["target"].as_str())
            .collect();
        let mut visited: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut bounded_floats: Vec<String> = Vec::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(shape) = shapes.get(id) else {
                continue;
            };
            match shape["type"].as_str() {
                Some("structure" | "union") => {
                    let Some(members) = shape["members"].as_object() else {
                        continue;
                    };
                    for (name, member) in members {
                        let Some(target) = member["target"].as_str() else {
                            continue;
                        };
                        pending.push(target);
                        let Some(targeted) = shapes.get(target) else {
                            continue;
                        };
                        let is_float = matches!(targeted["type"].as_str(), Some("float" | "double"));
                        let ranged = !member["traits"]["smithy.api#range"].is_null()
                            || !targeted["traits"]["smithy.api#range"].is_null();
                        if is_float && ranged {
                            // The casing of a path segment is the framework's; a member
                            // names itself in lowerCamel.
                            let mut segment = name.chars();
                            let segment: String = match segment.next() {
                                Some(first) => first.to_lowercase().chain(segment).collect(),
                                None => continue,
                            };
                            bounded_floats.push(segment);
                        }
                    }
                }
                Some("list" | "set") => {
                    if let Some(target) = shape["member"]["target"].as_str() {
                        pending.push(target);
                    }
                }
                Some("map") => {
                    for slot in ["key", "value"] {
                        if let Some(target) = shape[slot]["target"].as_str() {
                            pending.push(target);
                        }
                    }
                }
                _ => {}
            }
        }

        assert!(
            !bounded_floats.is_empty(),
            "case the model's surface: this service's inputs declare at least one float \
             member under a @range, so finding none means this walk reads the model wrongly"
        );
        let covered = floating_point_member_paths();
        for member in &bounded_floats {
            assert!(
                covered
                    .iter()
                    .any(|path| path == member || path.ends_with(&format!(".{member}"))),
                "case {member:?}: the model declares this float member under a @range, so a \
                 clause about it must state its value and bound as decimals; no path in \
                 {covered:?} names it"
            );
        }
    }

    /// RULE: a framework-rendered error body reports its detail text in the `Message`
    /// member this service's error shapes declare — and ONLY a body whose class the model
    /// declares, so the scaffold's 501 signal and any other framework class keep the
    /// member the framework wrote it in. Nothing else about the body changes, and the
    /// rename is idempotent.
    #[test]
    fn an_error_bodys_detail_text_moves_to_the_modelled_member() {
        // (case, the body a renderer wrote, the body this service answers — `None` for a
        // body there is nothing to rename in, which is returned untouched)
        let rows: [(&str, &str, Option<&str>); 9] = [
            (
                "the pre-handler refusal",
                r#"{"__type":"BadRequestException","message":"1 validation error detected"}"#,
                Some(r#"{"__type":"BadRequestException","Message":"1 validation error detected"}"#),
            ),
            (
                // The `__type` an error shape renders under may be namespaced.
                "a namespaced modelled error",
                r#"{"__type":"com.amazonaws.appconfig#ConflictException","message":"in flight"}"#,
                Some(
                    r#"{"__type":"com.amazonaws.appconfig#ConflictException","Message":"in flight"}"#,
                ),
            ),
            (
                "the scaffold's 501 signal, which is not a modelled error",
                r#"{"__type":"NotImplemented","message":"operation X is scaffolded"}"#,
                None,
            ),
            (
                // The class the framework names for a refusal this service does not word
                // itself, and which its model does not declare either.
                "a framework error class this model does not declare",
                r#"{"__type":"ValidationException","message":"not a modelled class"}"#,
                None,
            ),
            (
                "a body with no error class at all",
                r#"{"message":"no __type"}"#,
                None,
            ),
            (
                "a modelled error, already in the modelled member",
                r#"{"__type":"ResourceNotFoundException","Message":"no such thing"}"#,
                None,
            ),
            (
                // Both members present: the modelled one wins and the rendered one goes,
                // so one body never states two texts.
                "both members present",
                r#"{"__type":"BadRequestException","Message":"modelled","message":"rendered"}"#,
                Some(r#"{"__type":"BadRequestException","Message":"modelled"}"#),
            ),
            (
                "another member of the error shape, which is left alone",
                r#"{"__type":"BadRequestException","Reason":"InvalidConfiguration","message":"no"}"#,
                Some(
                    r#"{"__type":"BadRequestException","Reason":"InvalidConfiguration","Message":"no"}"#,
                ),
            ),
            ("a body that is not a JSON object", "not json at all", None),
        ];
        for (case, rendered, expected) in rows {
            let answered = with_detail_member(rendered.as_bytes());
            match expected {
                Some(expected) => {
                    let answered = answered.unwrap_or_else(|| {
                        panic!("case {case:?}: expected {expected:?}, got no rewrite")
                    });
                    let answered: serde_json::Value = serde_json::from_slice(&answered)
                        .unwrap_or_else(|error| {
                            panic!("case {case:?}: the answer must be JSON, got {error}")
                        });
                    let expected: serde_json::Value =
                        serde_json::from_str(expected).expect("the expected body is JSON");
                    assert_eq!(
                        answered, expected,
                        "case {case:?}: expected {expected:?}, got {answered:?}"
                    );
                    // Idempotent: answering the same body twice changes nothing.
                    let again = with_detail_member(
                        serde_json::to_vec(&answered)
                            .expect("the answer serializes")
                            .as_slice(),
                    );
                    assert!(
                        again.is_none(),
                        "case {case:?}: the rename must be idempotent, got {again:?}"
                    );
                }
                None => assert!(
                    answered.is_none(),
                    "case {case:?}: expected the body untouched, got {:?}",
                    answered.as_deref().map(String::from_utf8_lossy)
                ),
            }
        }
    }

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-appconfig-amazonappconfig.json"
    );

    /// How many operations this service's model declares, as a second, independent
    /// statement of its surface: a model that grows or shrinks fails here rather than
    /// quietly leaving the new operation out of the wrapper.
    const MODELLED_OPERATIONS: usize = 56;

    /// RULE applied to the whole surface: every operation the model declares is served
    /// through [`InServiceDialect`], under the same key the generated routing table is
    /// keyed by — so a constraint refusal quotes the submitted value, and an error's
    /// detail text lands in the modelled member, for EVERY operation rather than for the
    /// ones someone remembered.
    ///
    /// The names come from the MODEL rather than from a second hand-written list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering in the framework's wording — and a key spelled differently from
    /// the generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_in_this_services_dialect() {
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
            .collect();
        assert_eq!(
            operations.len(),
            MODELLED_OPERATIONS,
            "case the model's surface: expected this service's {MODELLED_OPERATIONS} \
             operations, got {}",
            operations.len()
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.in_service_dialect.get(operation).is_some(),
                "case {operation}: every modelled operation is served through \
                 InServiceDialect; this one is missing from wrap_operations!, so a \
                 constraint refusal of it quotes a shortened value and its detail text \
                 lands in the framework's own member"
            );
        }
    }
}
