// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-secretsmanager-secretsmanager` service: how it claims requests and how it words the
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
//! ## Credentials the account has withdrawn
//!
//! A long-term access key the account holds authenticates a request only while its stored
//! `Status` is `Active`, so a request signed with one whose status the account has moved
//! off `Active` resolves to NO principal: neither a secret's resource policy nor the
//! caller's identity policies are consulted, because there is no caller to consult them
//! for. A TEMPORARY credential is in the same position once the principal it speaks for is
//! gone — a role session whose role the account no longer holds, or holds under another id
//! than the mint read (`sts_store::credential_principal_is_withdrawn`). That is an
//! AUTHENTICATION outcome, so it is refused BEFORE any operation runs and
//! with this service's own unknown-credential code ([`UNUSABLE_CREDENTIAL_CODE`]) rather
//! than with an authorization denial or any modelled resource error.
//!
//! The rule, the sentence and the pre-dispatch refusal are the tree's one copy
//! ([`::service_common::unusable_credential`]); this file only names the code this service
//! states it with and hands it the account's own answer for the presented key, read through
//! the identity service's OWN accessor over the records its `CreateAccessKey` /
//! `UpdateAccessKey` wrote — the same store this service's authorization reads its
//! principal's documents from ([`crate::state::authorization`]) — and through the token
//! service's own registry of the sessions it minted.
//!
//! A key the account DELETED is not a record with a status at all: it resolves to no
//! credential, which the emulator's authentication layer decides before any operation runs
//! and reports as ONE structural refusal carrying its own generic code. That refusal is
//! stated in this service's vocabulary by the shared layer the entry is wrapped in
//! ([`::service_common::authentication_refusal`], [`AUTHENTICATION_REFUSAL`]), so a caller
//! whose key is gone reads the same unknown-credential code as one whose key was deactivated
//! instead of an authorization denial.
//!
//! ## How a broken constraint is stated
//!
//! A value outside a rule the model declares is refused before any handler runs, and this
//! service words that refusal itself ([`validation`]). One part of the sentence the refusal
//! seam cannot produce: the value a clause states at a LIST member when one of its ELEMENTS
//! failed. AWS reports such a failure at the list and quotes the list the request carried,
//! while the violation carries the offending element alone — and
//! [`Service::override_refusal`] is handed the violations without the request they were
//! found in.
//!
//! [`StatedPerOperation`] is where that value comes from: every modelled operation is served
//! wrapped in it, the wrapper asks `service_common::validation` for the framework's OWN
//! violations against that operation's input schema together with the request's own lists,
//! and [`validation`] turns them into the sentence. `override_refusal` states a refusal that
//! reaches the centralized walk anyway through the SAME module, so the two paths cannot word
//! one constraint differently — that one simply has no list to quote.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::authentication_refusal::{
    AuthenticationRefusalDefinition, StatedAuthenticationRefusal,
};
use ::service_common::unusable_credential::UnusableCredential;

mod validation;

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

/// The error code this service answers a signed request whose credential it will not accept
/// with: an `awsJson` service's unknown-credential code, the one
/// [`::service_common::authentication_refusal`] states for this protocol family. An
/// AUTHENTICATION failure — no principal was resolved — rather than an authorization denial,
/// so it is the code an SDK's credential-refresh path keys on.
const UNUSABLE_CREDENTIAL_CODE: &str = "UnrecognizedClientException";

/// The codes this service answers a request the emulator's authentication layer refused with
/// (see [`::service_common::authentication_refusal`], which owns the rule and the sentences):
/// the `awsJson` unknown-credential code — the same one the withdrawn-key refusal above
/// answers with, because a credential the account will not accept and one that resolves to
/// nothing are the same failure to the caller — for an access key id that resolves to no
/// credential, and the `awsJson` signature code for one that resolves and whose signature
/// does not verify. Neither is `AccessDenied`, which is an authorization verdict about a
/// principal this service DID resolve.
///
/// A request that presented NO credential at all is not an unknown credential, so none is
/// named here and such a request keeps the answer the front door already gives it.
const AUTHENTICATION_REFUSAL: StatedAuthenticationRefusal = StatedAuthenticationRefusal {
    unresolved_credential: UNUSABLE_CREDENTIAL_CODE,
    signature_mismatch: "InvalidSignatureException",
    missing_credential: None,
};

/// The `com-amazonaws-secretsmanager-secretsmanager` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, wrapped in the layer that states an authentication refusal in
    /// this service's own codes ([`AUTHENTICATION_REFUSAL`]); identity, the protocol and the
    /// routing table are the generated entry's untouched.
    entry: AuthenticationRefusalDefinition<crate::ServiceDefinition<Eng>>,
    /// The SAME shared engine the generated entry threaded into every operation, kept here
    /// too because the pre-dispatch credential check below has to read the access keys the
    /// identity service's own operations wrote. Never a store of its own: it is the `Arc`
    /// the router handed to `new`.
    engine: Arc<Eng>,
    /// Answers a request presenting a credential this account will not accept.
    unusable_credential: UnusableCredential,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words it,
    /// with the list a clause states read from the request. Keyed by the routing key the
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
        // EVERY modelled operation, one per file in `src/ops/`, because the rule is the
        // SERVICE's and not one operation's: an operation whose input declares no list of
        // constrained elements simply never has such a violation to state, which the wrapper
        // reads off that operation's own input schema. An operation added later is added
        // here as well, or a clause about one of its lists quotes no list; the test below
        // fails when one is missing.
        wrap_operations!(
            stated,
            engine,
            associate_cross_account_replica::AssociateCrossAccountReplica,
            batch_get_secret_value::BatchGetSecretValue,
            cancel_rotate_secret::CancelRotateSecret,
            create_secret::CreateSecret,
            delete_resource_policy::DeleteResourcePolicy,
            delete_secret::DeleteSecret,
            describe_secret::DescribeSecret,
            disassociate_cross_account_replica::DisassociateCrossAccountReplica,
            get_random_password::GetRandomPassword,
            get_resource_policy::GetResourcePolicy,
            get_secret_value::GetSecretValue,
            list_resource_policies::ListResourcePolicies,
            list_secret_version_ids::ListSecretVersionIds,
            list_secrets::ListSecrets,
            put_resource_policy::PutResourcePolicy,
            put_secret_value::PutSecretValue,
            remove_regions_from_replication::RemoveRegionsFromReplication,
            replicate_secret_to_regions::ReplicateSecretToRegions,
            restore_secret::RestoreSecret,
            rotate_secret::RotateSecret,
            stop_replication_to_replica::StopReplicationToReplica,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_secret::UpdateSecret,
            update_secret_version_stage::UpdateSecretVersionStage,
            validate_resource_policy::ValidateResourcePolicy,
        );
        Self {
            entry: AuthenticationRefusalDefinition::new(
                crate::ServiceDefinition::new(Arc::clone(&engine)),
                AUTHENTICATION_REFUSAL,
            ),
            engine,
            unusable_credential: UnusableCredential::stated_as(UNUSABLE_CREDENTIAL_CODE),
            stated,
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way this service
/// states it: a constraint one of a list's ELEMENTS broke reported at that list member,
/// quoting the list the REQUEST carried.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse. The cost is that a request this
/// wrapper does not refuse is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation.
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
/// the violations the framework's own walk finds, and the wording [`validation`] gives them.
/// `None` when the request breaks none — and for a failure that is NOT a refusal (a body the
/// codec cannot read at all, a member of the wrong type), which the framework's own walk
/// answers in its own envelope.
///
/// The collecting half is the tree's shared one
/// (`service_common::validation::refusing_violations_with_mapped_echoes`): one read of the
/// request yields the framework's own violations (the protocol's deserializer for an absent
/// `@required` member, `constraints::validate` over the built input), each paired with how a
/// clause can state the value it is about, plus the request's own lists — the value a clause
/// stated at a LIST member needs and no violation carries. Nothing is re-checked here, and no
/// second copy of that walk lives in this crate: both halves are bounded where those bounds
/// are stated.
///
/// Two restorations are made from that echo, both the shared ones, because the framework
/// writes neither half the way AWS states it: a value the framework ELIDED into its own
/// marker is restored to what the caller sent
/// (`validation_message::quoting_the_whole_submitted_value`, after which this service applies
/// its OWN echo bound), and a COLLECTION the framework summarised as `<n> items` is restored
/// to the collection itself (`validation_message::quoting_the_submitted_collection`) — a list
/// of structures included, which is the one value no list of scalars could supply.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (echoed, lists) =
        ::service_common::validation::refusing_violations_with_mapped_echoes::<O>(protocol, req);
    let violations: Vec<Violation> = echoed
        .into_iter()
        .map(|(violation, echo)| {
            let quoted =
                ::service_common::validation_message::quoting_the_whole_submitted_value(
                    violation, &echo,
                );
            ::service_common::validation_message::quoting_the_submitted_collection(quoted, echo)
        })
        .collect();
    // The operation is named as well as its violations: a limit this service enforces
    // itself belongs to one operation's parameter, so the wording of such a refusal is that
    // operation's ([`validation`] holds the table).
    let stated = validation::reword_quoting(
        Some(O::NAME),
        &Refusal::Invalid(violations.clone()),
        || lists,
    )?;
    Some((violations, stated))
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
        Box::pin(async move {
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                );
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // A credential the account has withdrawn is refused HERE, before the operation it
        // named runs (see the module docs): the modelled walk deserializes, validates and
        // dispatches before any handler could answer, so this pre-dispatch seam is the only
        // place an authentication outcome is reachable from. A READ — the records are only
        // looked up — so a refused request changes nothing a later caller sees, and the
        // request's auth schemes are unaffected: the router reads them off the MODELLED
        // operation the routing key names.
        if let Some(refused) = self
            .unusable_credential
            .refusal_for(req, self.entry.protocol(), |access_key_id| {
                iam_store::service_state(&*self.engine)
                    .is_none_or(|iam| iam.lock().access_key_is_usable(access_key_id))
                    && !sts_store::credential_principal_is_withdrawn(&*self.engine, access_key_id)
            })
        {
            return Some(refused);
        }
        // Otherwise NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // [`StatedPerOperation`] so a refused request is worded the way this service words
        // it, with the list a clause states read from the request. A key the model does not
        // name resolves to `None` here exactly as it does in the generated table, so the
        // router's routing-error path is unchanged, and the wrapper declares the wrapped
        // operation's own auth schemes, so nothing about signing changes.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording; a case reworded here replaces BOTH the code and the message,
        // because a client branches on the code.
        //
        // Which refusals this service words itself, and how, lives in
        // [`validation`] — one table for the whole service, so two operations
        // cannot word one refusal two ways. It is handed the violations ALONE — no request
        // — so it cannot quote the list a failed element belongs to;
        // [`StatedPerOperation`] answers a modelled operation's refusals before this walk
        // is reached, with that list in hand.
        validation::reword(refusal)
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

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json"
    );

    /// RULE applied to the whole surface: every operation the generated table serves is
    /// served behind [`StatedPerOperation`], under the same key that table is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand-written list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering a refusal without the list a clause states — and a key spelled
    /// differently from the generated table's fails here instead of silently never being
    /// reached by `override_operation`.
    #[test]
    fn every_served_operation_is_served_with_this_services_refusal_wording() {
        let model = ::std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            !operations.is_empty(),
            "case \"the model's surface\": a service model declares operations"
        );

        let service = EmulatedService::new(Arc::new(::engine::InMemoryEngine::new()));
        let mut served = 0_usize;
        for operation in operations {
            if service.entry.routing_table().get(operation).is_none() {
                // An operation the model binds and this crate generated no handler for is
                // served by nothing, so there is nothing to wrap.
                continue;
            }
            served += 1;
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation:?}: every operation the generated table serves is served \
                 behind StatedPerOperation; this one is missing from wrap_operations!, so a \
                 clause about one of its lists would quote no list"
            );
        }
        assert!(
            served >= 20,
            "case \"the served surface\": expected every generated operation to be checked, \
             got {served}"
        );
    }
}
