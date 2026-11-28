// SPDX-License-Identifier: CC0-1.0

//! WHICH of this service's endpoints an operation is hosted on, read off the model, and
//! the one thing that decides: the error CODE a request the model's constraints do not
//! admit is refused with.
//!
//! # The rule
//!
//! An operation hosted on an endpoint of its own — one the model gives an `@endpoint`
//! host prefix ([`DATA_ENDPOINT_PREFIX`] here) — is fronted by AWS's generic
//! request-validation layer, which refuses a value outside a modelled constraint with
//! HTTP 400 and the error code [`GENERIC_VALIDATION_CODE`], a shape the service's own
//! model does not declare. An operation on the service's main endpoint is refused by the
//! service itself, under the one error ITS model declares for the class
//! ([`super::EmulatedService::override_refusal`]).
//!
//! The two discovery reads are the operations this model hosts on a prefixed endpoint —
//! the data plane a client resolves instances through, a frontend of its own in front of
//! the same resources the control plane holds — and the classification is read from the
//! shipped Smithy model rather than listed here, so it cannot drift from the model and
//! covers every operation the model binds there, not only the ones a recording covers.
//!
//! # What does NOT change
//!
//! Only the code. The status, the counted envelope, each clause, its member path, the
//! value it echoes and the order an enumerated type's admissible set is listed in stay
//! exactly where [`super::EmulatedService::override_refusal`] takes them from
//! ([`super::enum_value_sets::stated_message`], the one home for the words), and the
//! violations are the framework's own (the shared collecting walk over the protocol's
//! deserializer and `constraints::validate`) together with the ones that walk declares and
//! cannot apply, collected and ordered by the shared home for that rule
//! ([`::service_common::unapplied_patterns`], which the service's own refusal layer
//! [`super::modelled_patterns`] states too) — so this layer cannot word a constraint
//! differently from the endpoint beside it, nor apply a different set of them. A request
//! that breaks no modelled constraint is handed to the operation untouched, which keeps
//! every modelled error, every success and the whole handler path where they are.
//!
//! # Why a wrapper
//!
//! [`smithy_server_runtime::Service::override_refusal`] is handed the violations ALONE —
//! no request and no operation — so a service-level seam cannot tell which endpoint was
//! addressed, and two operations refusing the same member name are indistinguishable to
//! it. The seam that CAN place them is a per-operation wrapper handed back from
//! `override_operation` ([`AtTheDataEndpoint`], served from
//! [`super::submitted_collections`]), which has both the request and the operation's own
//! input schema.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::serde_json::Value;
use ::service_common::front_door::{body_violations, worded_rejection};
use ::service_common::submitted_collections::AnsweredAs;
use ::service_common::unapplied_patterns::{stated_together, unapplied_violations};
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refused, Service,
};

/// This service's Smithy model: the one source for which endpoint hosts an operation.
const MODEL_JSON: &str = include_str!(
    "../../smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json"
);

/// The Smithy trait that carries an operation's endpoint host prefix.
const ENDPOINT_TRAIT: &str = "smithy.api#endpoint";

/// The host prefix of the DATA endpoint — the plane a client discovers instances through.
///
/// The main endpoint is not a constant here: nothing is decided by the ABSENCE of this
/// prefix, and a second prefix a model refresh added would be classified with the main
/// endpoint rather than silently read as this one.
pub(super) const DATA_ENDPOINT_PREFIX: &str = "data-";

/// The error code AWS's generic request-validation layer refuses a request with, which is
/// also the framework's own default for the class — NOT a shape this model declares, which
/// is why it cannot travel through any of this service's modelled error channels.
pub(super) const GENERIC_VALIDATION_CODE: &str = "ValidationException";

/// Every operation the model hosts on the data endpoint, by its own shape name.
///
/// Built once: the model is a compile-time constant, so the answer cannot change while the
/// process runs, and the census is forced by this module's own tests, so a model this
/// cannot read fails the build rather than one request. Neither the parse nor the shapes it
/// reads is swallowed into an empty set that would quietly state every refusal in the
/// control plane's code.
static OPERATIONS_ON_THE_DATA_ENDPOINT: LazyLock<BTreeSet<String>> =
    LazyLock::new(operations_on_the_data_endpoint);

/// Whether the model hosts the operation named `operation` on the data endpoint.
#[must_use]
pub(super) fn serves_the_data_endpoint(operation: &str) -> bool {
    OPERATIONS_ON_THE_DATA_ENDPOINT.contains(operation)
}

/// The census: the shape name of every operation whose `@endpoint` host prefix is
/// [`DATA_ENDPOINT_PREFIX`].
fn operations_on_the_data_endpoint() -> BTreeSet<String> {
    host_prefixes()
        .into_iter()
        .filter(|(_, prefix)| prefix == DATA_ENDPOINT_PREFIX)
        .map(|(operation, _)| operation)
        .collect()
}

/// Each operation the model binds that carries an `@endpoint` host prefix, as
/// `(shape name, the prefix)`.
fn host_prefixes() -> Vec<(String, String)> {
    let model: Value = ::serde_json::from_str(MODEL_JSON)
        .expect("MODEL_JSON is this crate's own Smithy model and parses");
    // As loud as the parse above, and for the same reason: a document whose shapes this
    // cannot read would classify every operation with the main endpoint, which is a
    // broken build rather than something to answer a request from.
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("this crate's own Smithy model declares shapes");
    shapes
        .iter()
        .filter(|(_, shape)| shape.get("type").and_then(Value::as_str) == Some("operation"))
        .filter_map(|(id, operation)| {
            let prefix = operation
                .pointer(&format!("/traits/{ENDPOINT_TRAIT}/hostPrefix"))
                .and_then(Value::as_str)?;
            Some((short_name(id).to_string(), prefix.to_string()))
        })
        .collect()
}

/// The short name of a shape id: what follows the namespace.
fn short_name(shape_id: &str) -> &str {
    shape_id.rsplit('#').next().unwrap_or(shape_id)
}

/// One modelled operation served at the data endpoint: a request outside the constraints
/// the model declares is refused by the generic validation in front of that endpoint, and
/// everything else is handed to the operation itself.
///
/// Every part of the refusal is shared: the collecting walk and the rendering are
/// [`::service_common::front_door`]'s (the home for a layer AHEAD of the code that owns a
/// service's modelled errors), the words are [`super::enum_value_sets`]'s and the response
/// pass is the [`AnsweredAs`] of [`::service_common::submitted_collections`]. What is this
/// crate's own is only the combination — refuse under the code the generic layer names, then
/// answer as this service answers — which belongs in the shared home beside `FrontDoor`
/// rather than being written a second time by the next service that needs it, as does the
/// host-prefix census above (`service-com-amazonaws-iotsitewise-awsiotsitewise`'s
/// `src/service/endpoints.rs` derives the same census for its three frontends).
///
/// The cost of the pass is the DELIBERATE one every per-operation refusal layer in this
/// tree pays ([`::service_common::front_door`]): a request this wrapper refuses is read
/// twice, here and in the operation's own sequence, because the framework hands no
/// already-built input to an operation.
pub(super) struct AtTheDataEndpoint<Op> {
    operation: Op,
    /// How this service answers a rendered response — the member an error body reports its
    /// text in ([`super::error_message_member`]), as every other operation of this service
    /// is served.
    answered_as: AnsweredAs,
}

impl<Op> AtTheDataEndpoint<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub(super) fn new(operation: Op, answered_as: AnsweredAs) -> Self {
        Self {
            operation,
            answered_as,
        }
    }
}

impl<Op: Operation> AtTheDataEndpoint<Op> {
    /// How the generic validation in front of this endpoint refuses `req`, or `None` when
    /// the request breaks no constraint the model declares — which leaves it to the
    /// operation behind this layer.
    ///
    /// The protocol is read, used and dropped inside this one function, so nothing
    /// borrowed from the service definition is held across the caller's await.
    fn refusal(&self, svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
        // Which endpoint hosts this operation is read off the MODEL, so an operation
        // served through this layer that the model hosts on the main endpoint is refused
        // by the service's own code exactly as its peers are.
        if !serves_the_data_endpoint(Op::NAME) {
            return None;
        }
        // The codec the request is SERVED in, resolved the way the framework's own
        // dispatch resolves it — per REQUEST, because this model declares more than one
        // protocol and each request is served in the one it names: a layer that read the
        // request with a different codec from the operation behind it would find a
        // different violation set, or none at all.
        let protocol = svc
            .override_protocol()
            .unwrap_or_else(|| svc.definition().wire_protocol(req).protocol());
        // Every member of this model travels in the request BODY: both protocols it
        // declares are RPC ones, which bind no member to the request itself
        // (`smithy_query(query="operation")` reports the bindings of an RPC protocol
        // inert), so the body half of the shared walk is the whole request. A model refresh that bound a member to the URI would leave that
        // member's refusal to the service's own seam, which is why the walk is named here
        // rather than assumed. Beside the constraints that walk applies are the ones it
        // declares and cannot — a `@pattern` its engine will not compile — which are
        // collected and ordered by the shared home for that rule, so this endpoint applies
        // the same set of modelled constraints as the service's own
        // ([`super::modelled_patterns`]).
        let violations = stated_together(
            <Op as Operation>::INPUT_SCHEMA,
            body_violations::<Op>(protocol, req),
            unapplied_violations::<Op>(protocol, req),
        );
        if violations.is_empty() {
            return None;
        }
        // `stated_message` answers `None` for an empty violation set alone, which the guard
        // above has already left to the operation.
        let message = super::enum_value_sets::stated_message(&violations)?;
        Some(worded_rejection(
            svc,
            req,
            violations,
            Refused {
                code: GENERIC_VALIDATION_CODE.to_string(),
                message,
            },
        ))
    }
}

impl<Op> ErasedOperation for AtTheDataEndpoint<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The whole refusal is decided — and the borrow of the definition it is
            // rendered through taken and dropped — BEFORE the await below:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across that
            // await would make this future non-`Send`.
            if let Some(refused) = self.refusal(svc, req) {
                return (self.answered_as)(refused);
            }
            (self.answered_as)(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the endpoint an operation is hosted on is the model's, so the operations
    /// served through [`AtTheDataEndpoint`] in `super::submitted_collections` are exactly
    /// the ones the model gives the data prefix to. A model refresh that hosts a third
    /// operation there fails here rather than serving it in the control plane's code.
    #[test]
    fn the_model_hosts_the_two_discovery_reads_on_the_data_endpoint() {
        let census: Vec<&str> = OPERATIONS_ON_THE_DATA_ENDPOINT
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            census,
            ["DiscoverInstances", "DiscoverInstancesRevision"],
            "expected the two discovery reads on the {DATA_ENDPOINT_PREFIX} endpoint, got \
             {census:?}"
        );
    }

    /// RULE: a prefix is read per operation, and the absence of one is the main endpoint.
    /// The rows are this model's own operations: both prefixed reads, a read and a write of
    /// the control plane, and a shape that is no operation at all.
    #[test]
    fn an_operation_is_classified_by_the_endpoint_hosting_it() {
        // (case, the operation's shape name, whether the model hosts it on the data endpoint)
        let rows: [(&str, &str, bool); 6] = [
            ("the instance discovery read", "DiscoverInstances", true),
            (
                "the revision discovery read",
                "DiscoverInstancesRevision",
                true,
            ),
            ("a control-plane read", "GetService", false),
            ("a control-plane listing", "ListInstances", false),
            ("a control-plane write", "RegisterInstance", false),
            ("a shape that is no operation", "HealthStatusFilter", false),
        ];
        for (case, operation, expected) in rows {
            let answered = serves_the_data_endpoint(operation);
            assert_eq!(
                answered, expected,
                "case {case:?}: expected {expected}, got {answered}"
            );
        }
    }

    /// RULE: every prefix the model declares is the data one — nothing is classified by
    /// the absence of a class this crate does not know about — and the operations carrying
    /// one are a strict subset of the operations the model binds.
    #[test]
    fn the_only_prefix_the_model_declares_is_the_data_endpoints() {
        let declared = host_prefixes();
        assert!(
            !declared.is_empty(),
            "the model must bind a prefixed operation, got none"
        );
        for (operation, prefix) in &declared {
            assert_eq!(
                prefix, DATA_ENDPOINT_PREFIX,
                "case {operation:?}: expected the only prefix this model declares, got {prefix:?}"
            );
        }
    }
}
