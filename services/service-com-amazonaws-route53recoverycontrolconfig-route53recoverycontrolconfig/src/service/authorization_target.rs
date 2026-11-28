// SPDX-License-Identifier: CC0-1.0

//! The stage this API's front door takes once it knows WHO is calling: which RESOURCE the
//! call is authorized against — and what it answers a request that names none.
//!
//! # The rule
//!
//! Route 53 ARC's recovery-control API is reached through a REST front door that decides
//! whether the caller may perform the action it asked for ON THE RESOURCE THE REQUEST
//! NAMES, before anything behind that layer reads the rest of the call. Every operation of
//! this model names that resource in a value the request has to carry: an ARN bound to the
//! URI (`/cluster/{ClusterArn}`, `/controlpanel/{ControlPanelArn}`,
//! `/safetyrule/{SafetyRuleArn}`, `/tags/{ResourceArn}`, …) or a body member the model
//! declares `@required` (`UpdateCluster$ClusterArn`, `CreateControlPanel$ClusterArn`,
//! `UpdateRoutingControl$RoutingControlArn`, …) — with exactly two exceptions, the two
//! safety-rule WRITES. Their target ARN travels only INSIDE one of two members the model
//! leaves optional, and is required only within whichever of the two a request carries
//! (`CreateSafetyRule$AssertionRule` / `$GatingRule`, each carrying a required
//! `ControlPanelArn`; `UpdateSafetyRule$AssertionRuleUpdate` / `$GatingRuleUpdate`, each
//! carrying a required `SafetyRuleArn` —
//! `smithy_query(query="operation",operation="CreateSafetyRule,UpdateSafetyRule")`).
//!
//! So those two are the only requests of this API that can satisfy the model and still name
//! no resource at all — by carrying neither member, or by carrying one without the ARN
//! inside it — and a request that names none gives that layer nothing to authorize the
//! caller against. It is refused there, with that layer's own answer and never with one of
//! this service's modelled errors: HTTP 401, the wire error code `UnauthorizedException`
//! in `x-amzn-errortype`, and the one fixed sentence `Unauthorized` under a lowercase
//! `message`, with nothing else in the body — the same refusal, stated in the same one
//! place ([`::service_common::front_door`]), as the one this endpoint gives a caller it
//! could not verify at all ([`super::front_door_stage`]). The model declares no such shape,
//! because the layer that answers is not the service.
//!
//! # Where it lands
//!
//! Ahead of everything the service itself would say about such a request: ahead of the
//! front door's own body-schema validation (whose refusal is a 400 `BadRequestException`)
//! and ahead of the operation's modelled `ValidationException`, because authorizing the
//! caller for a resource comes before reading what the call says about it. It is taken in
//! [`::smithy_server_runtime::Service::override_operation`], the one seam that sees the
//! whole request ahead of the modelled walk — a refused request reaches no handler, so it
//! mints nothing, consumes no idempotency token and leaves the store as it was.
//!
//! A request that DOES name a resource passes untouched and keeps every answer this service
//! gives it: the `Invalid Rule provided.` refusal of a write carrying BOTH rule members
//! (which names two resources, so the caller is authorized and the operation refuses the
//! choice), the reference refusals for an ARN that holds nothing, and the created rule.
//!
//! Nothing here reads a credential or any state, and neither whether the named resource
//! EXISTS nor whether the ARN naming it is even well formed is decided here: that a
//! resource is named at all is all this stage reads. Those questions are the service's own,
//! answered behind the door — the malformed-ARN refusal every handler raises
//! ([`crate::state::read_arn_format`]), `ResourceNotFoundException` (404) for a read of an
//! ARN holding nothing, and `AccessDeniedException` (403) for another account's resource
//! ([`crate::state::authorize_resource_arn`]).

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// One operation whose target can be absent: the modelled operation name the protocol
/// routes a request by, the two optional members that can carry the ARN of the resource
/// the call acts on, and the member INSIDE either of them that is that ARN.
///
/// Both carriers of each pair are optional, which is what makes a request that names no
/// resource possible at all; the ARN member is `@required` within whichever carrier a
/// request does send, so a carrier without it names no resource either
/// (`smithy_query(query="shape",shape="NewAssertionRule,NewGatingRule,AssertionRuleUpdate,GatingRuleUpdate")`).
/// Every other operation of this model carries its target in a URI label or in a member
/// the model requires (see the module docs), so none of them is listed here.
struct TargetCarriers {
    /// The operation, as the model names it and the routing key spells it.
    operation: &'static str,
    /// The two optional members either of which can carry the resource.
    carriers: [&'static str; 2],
    /// The ARN member inside a carrier: the value that names the resource.
    arn: &'static str,
}

/// The two operations of this model whose target can be absent from a request the model
/// otherwise admits.
const TARGET_CARRIERS: [TargetCarriers; 2] = [
    TargetCarriers {
        operation: "CreateSafetyRule",
        carriers: ["AssertionRule", "GatingRule"],
        arn: "ControlPanelArn",
    },
    TargetCarriers {
        operation: "UpdateSafetyRule",
        carriers: ["AssertionRuleUpdate", "GatingRuleUpdate"],
        arn: "SafetyRuleArn",
    },
];

/// Whether the front door can read the resource that the request `req` to `operation` acts
/// on.
///
/// True for every operation whose target the request carries outside such a pair of
/// optional members: there the model itself guarantees a resource is named, so this stage
/// has nothing to decide and the layers behind it answer.
///
/// For the two that do carry it that way, it is whether the body sends either carrier with
/// an ARN in it: a carrier the request omits, sends null, sends as something other than a
/// structure, or sends without the ARN member the model requires inside it, names no
/// resource. The ARN is read for its PRESENCE as a non-empty string and not for its form —
/// a value in that slot names what the caller asked to act on, so a malformed one is left
/// to the layers behind the door, which refuse it as the input it is.
///
/// A body no reader can take members out of at all — absent, or not a JSON object — names
/// no resource either, and is refused the same way rather than left to the body-schema
/// layer behind the door: the caller is authorized for a resource BEFORE the body is read
/// as the operation's input, so there is no reading of it that could answer first. The
/// bytes are always there to read: every request reaches a service through the router's
/// `dispatch`, which is handed the WHOLE body already buffered
/// (`::smithy_server_runtime::MultiServiceRouter`'s `http::Request<bytes::Bytes>`), so a
/// body this endpoint serves is never a stream whose bytes are still to come.
pub(super) fn names_its_target(operation: &str, req: &RtRequest) -> bool {
    let Some(target) = TARGET_CARRIERS
        .iter()
        .find(|target| target.operation == operation)
    else {
        return true;
    };
    let read = req
        .body()
        .bytes()
        .map(::serde_json::from_slice::<::serde_json::Value>);
    let Some(Ok(::serde_json::Value::Object(members))) = read else {
        return false;
    };
    target.carriers.iter().any(|carrier| {
        members
            .get(*carrier)
            .and_then(|carried| carried.get(target.arn))
            .and_then(::serde_json::Value::as_str)
            .is_some_and(|arn| !arn.is_empty())
    })
}

/// The front door's refusal of a request it has no resource to authorize, as a
/// pre-dispatch operation.
///
/// An operation rather than a line in a handler because the refusal happens ahead of the
/// modelled walk, which no handler is reachable from: `override_operation` serves this
/// instead of the operation the request named, so nothing the request asked for runs.
pub(super) struct UnauthorizedTarget;

impl ErasedOperation for UnauthorizedTarget {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The status, the error code, the member and the sentence are all the shared
        // statement's, so this endpoint cannot word one front-door refusal differently
        // from the other.
        Box::pin(async move { ::service_common::front_door::unauthorized_response() })
    }

    // `auth_schemes` keeps its default `None`: this is not an operation of its own, so the
    // request is signed as the MODEL says for the operation it named, and the
    // authentication layer still refuses a caller it cannot verify ahead of this stage.
}
