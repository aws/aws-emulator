// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-guardduty-guarddutyapiservice` service: how it claims requests and how it words the
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

/// Whether IAM lets a request perform the action it names — the decision taken in
/// front of every operation whose model declares `AccessDeniedException`, through
/// the [`authorization::Gated`] wrapper `override_operation` below resolves.
pub mod authorization;

/// The refusal sentences this service answers whatever the operation and whatever
/// made the request unacceptable — the wording that belongs to no one resource
/// family, beside `override_refusal` below, which words what the request boundary
/// refused.
pub mod refusals;

use smithy_server_runtime::{
    http, ErasedOperation, Operation, Refusal, Refused, Service,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The `com-amazonaws-guardduty-guarddutyapiservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`authorization::Gated`] per operation whose model declares
    /// `AccessDeniedException`, keyed by the routing key the model names that
    /// operation by ([`Operation::NAME`], the same key the generated routing table
    /// uses). Built once here, never per request; each entry carries the operation's
    /// TYPE and a handle on the one shared engine, so this is a table of names rather
    /// than a second set of operations.
    gated: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            gated: gated_table::<Eng>(&engine),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The [`authorization::Gated`] wrapper for every operation whose model declares
/// `AccessDeniedException` ([`authorization::GATED_OPERATIONS`]).
///
/// Authorization is a property of the request's identity, the action it names and the
/// resource it addresses rather than of what a handler goes on to do, so it is taken
/// here for all of them at once — implemented or still scaffolded. A stub answers 501
/// for a request whose caller IAM permits and the refusal otherwise, which is the
/// order the real service evaluates them in; leaving a modelled operation out would
/// serve an unauthorized caller, the very divergence this removes.
fn gated_table<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut wrappers: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                wrappers.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(authorization::Gated::<
                        crate::ops::$module::$operation<Eng>,
                        Eng,
                    >::new(Arc::clone(engine))) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    register!(
        create_custom_detection_rule_association::CreateCustomDetectionRuleAssociation,
        create_custom_detection_rule_org_configuration::CreateCustomDetectionRuleOrgConfiguration,
        create_investigation::CreateInvestigation,
        create_ip_set::CreateIPSet,
        create_malware_protection_plan::CreateMalwareProtectionPlan,
        create_threat_intel_set::CreateThreatIntelSet,
        delete_custom_detection_rule_association::DeleteCustomDetectionRuleAssociation,
        delete_custom_detection_rule_org_configuration::DeleteCustomDetectionRuleOrgConfiguration,
        delete_malware_protection_plan::DeleteMalwareProtectionPlan,
        get_custom_detection_rule::GetCustomDetectionRule,
        get_custom_detection_rule_association::GetCustomDetectionRuleAssociation,
        get_custom_detection_rule_org_configuration::GetCustomDetectionRuleOrgConfiguration,
        get_investigation::GetInvestigation,
        get_malware_protection_plan::GetMalwareProtectionPlan,
        list_custom_detection_rule_associations::ListCustomDetectionRuleAssociations,
        list_custom_detection_rule_org_configurations::ListCustomDetectionRuleOrgConfigurations,
        list_custom_detection_rules::ListCustomDetectionRules,
        list_investigation_records::ListInvestigationRecords,
        list_investigations::ListInvestigations,
        list_malware_protection_plans::ListMalwareProtectionPlans,
        list_tags_for_resource::ListTagsForResource,
        send_object_malware_scan::SendObjectMalwareScan,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_custom_detection_rule_association::UpdateCustomDetectionRuleAssociation,
        update_custom_detection_rule_org_configuration::UpdateCustomDetectionRuleOrgConfiguration,
        update_ip_set::UpdateIPSet,
        update_malware_protection_plan::UpdateMalwareProtectionPlan,
        update_threat_intel_set::UpdateThreatIntelSet,
    );
    wrappers
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
        // [`authorization::Gated`] so IAM's decision about the action is taken BEFORE
        // the handler runs, as AWS takes it. Only the operations whose model declares
        // `AccessDeniedException` are in that table
        // ([`authorization::GATED_OPERATIONS`]); every other key resolves to `None`
        // here exactly as it did before, so the generated table serves it unchanged
        // and the router's routing-error path is untouched.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.gated.get(key).map(::std::convert::AsRef::as_ref)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // GuardDuty needs it reworded, because the framework's default names
        // `ValidationException` — a shape this service's model does not declare
        // for any operation. Its only client error is `BadRequestException`
        // (`httpError` 400), which `smithy_query(query="service_info")` reports as
        // one of just two common errors, so a refusal under any other code is one
        // no GuardDuty client can catch: an SDK builds its exception classes from
        // the error code, and a caller's `BadRequestException` branch would fall
        // through to "unmodeled error" for a plainly fixable input.
        //
        // BOTH the code and the message are replaced, as the seam requires. The
        // code is the modelled error SHAPE rather than the narrower
        // `InvalidInputException` CLASS a handler rejection carries
        // (`crate::scaffolded::Scaffolded::invalid_input`): this path renders ONE
        // code into both the body's `__type` and the `x-amzn-errortype` header,
        // and the header is what restJson1 resolves the error shape from, so the
        // shape is the name that has to be there.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: crate::scaffolded::BAD_REQUEST_EXCEPTION.to_string(),
                message: crate::state::constraint_rejection(violations),
            }),
            // An authentication refusal is the emulator's, not this service's:
            // the model declares no 403 error for any operation, so the
            // framework's own wording stands.
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
    use super::*;

    /// RULE: the operations REGISTERED behind the authorization gate are exactly the ones
    /// [`authorization::GATED_OPERATIONS`] names — which that module's own test holds to
    /// the set the model declares `AccessDeniedException` for.
    ///
    /// Without this, the `register!` list above could drop an operation while the model and
    /// the constant still said it was judged, and that operation would serve an
    /// unauthorized caller. The table is built over a stand-in engine type carrying no
    /// state: this asserts which KEYS are registered, and no request is served.
    #[test]
    fn every_gated_operation_is_registered_behind_the_authorization_gate() {
        let registered = gated_table::<()>(&Arc::new(()));
        let mut keys: Vec<&str> = registered.keys().copied().collect();
        keys.sort_unstable();
        let mut gated: Vec<&str> = authorization::GATED_OPERATIONS.to_vec();
        gated.sort_unstable();
        assert_eq!(
            keys, gated,
            "case \"the registered table\": expected exactly the gated operations, got \
             {keys:?} against {gated:?}"
        );
    }
}
