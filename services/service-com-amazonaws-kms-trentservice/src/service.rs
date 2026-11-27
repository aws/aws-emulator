// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kms-trentservice` service: how it claims requests and how it words the
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
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header this
//! protocol reports that id in (`x-amzn-RequestId`) — on a success and on a failure alike,
//! which is where an SDK reads a call's `ResponseMetadata.RequestId`. It is a property of
//! the RESPONSE and of no modelled shape, so no handler can set it, and for THIS service
//! that same id is the join key its audit surface is defined on — the `KmsRequestId` of a
//! key's usage record and the `requestID` of the call's CloudTrail record are the one id
//! the caller was answered under. Both halves are owned by the layers every call passes
//! through, which this file wires and [`management_events`] states:
//!
//! * what an OPERATION answers — a success, a modelled error, a constraint refusal raised
//!   while it was being served — is named by the audit layer
//!   ([`management_events::KmsAudit::names_the_request`]);
//! * what NO operation produced — a routing failure, a transport failure detected up
//!   front, the pre-dispatch authentication refusal below — is named by the shared
//!   definition wrapper ([`RequestIdEnvelopeDefinition::header_only`]).
//!
//! ## Credentials the account has withdrawn
//!
//! A long-term access key the account holds authenticates a request only while its stored
//! `Status` is `Active`, so a request signed with one whose status the account has moved
//! off `Active` resolves to NO principal: no key policy, no identity policy and no grant is
//! consulted, because there is no caller to consult them for. A TEMPORARY credential is in
//! the same position once the principal it speaks for is gone — a role session whose role the
//! account no longer holds, or holds under another id than the mint read
//! (`sts_store::credential_principal_is_withdrawn`). That is an AUTHENTICATION
//! outcome, so it is refused BEFORE any operation runs and with this service's own
//! unknown-credential code ([`UNUSABLE_CREDENTIAL_CODE`]) rather than with the
//! authorization denial `AccessDeniedException` or any modelled resource error.
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
//! whose key is gone reads the same unknown-credential code as one whose key was
//! deactivated instead of an authorization denial.

use service_common::authentication_refusal::{
    AuthenticationRefusalDefinition, StatedAuthenticationRefusal,
};
use service_common::query_error_envelope::RequestIdEnvelopeDefinition;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Every call this service serves, recorded as a management event of `kms.amazonaws.com`
/// — and the ONE id each response names its request by.
pub mod management_events;
/// The response members this service renders that its model does not declare.
pub mod recorded_response_members;
/// Which members of this service a refusal states no value for, read from the model.
pub mod sensitive_members;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::unusable_credential::UnusableCredential;

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

/// The `com-amazonaws-kms-trentservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers, and INSIDE that
    /// the AUDITED routing table every request this service serves is resolved through
    /// ([`management_events`]).
    entry: RequestIdEnvelopeDefinition<
        AuthenticationRefusalDefinition<
            management_events::AuditedDefinition<crate::ServiceDefinition<Eng>>,
        >,
    >,
    /// This service's wire protocol, built once: `override_protocol` borrows it from
    /// `&self` and must never build one per request.
    protocol: recorded_response_members::RecordedResponseJson,
    /// The SAME shared engine the generated entry threaded into every operation, kept here
    /// too because the pre-dispatch credential check below has to read the access keys the
    /// identity service's own operations wrote. Never a store of its own: it is the `Arc`
    /// the router handed to `new`.
    engine: Arc<Eng>,
    /// Answers a request presenting a credential this account will not accept.
    unusable_credential: UnusableCredential,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // The AUDITED table, built before the entry it is nested inside: every modelled
        // operation, each constructed from this same shared engine and wrapped in the
        // management-event recorder (see [`management_events`]).
        let audited = management_events::audited_table(&engine);
        Self {
            // The audited table is the INNERMOST wrapper, so every layer that resolves an
            // operation for a request — the router, this service's own dispatch — reaches
            // the audited operation, and no request can be served by a path that records
            // nothing. Around it, the request-id report every response of this service
            // carries, for the responses no operation produced: a routing failure, a
            // transport failure detected up front, and the pre-dispatch authentication
            // refusal below. What an OPERATION answers is named by the audit layer itself
            // ([`management_events::KmsAudit::names_the_request`]), which is the one place
            // that id and the id a usage record carries are the same value.
            entry: RequestIdEnvelopeDefinition::header_only(
                // Around the audited table, the layer that states an authentication refusal
                // in THIS service's own codes ([`AUTHENTICATION_REFUSAL`]): the framework
                // reports one structural refusal whichever way the credential failed, and
                // the code it reports is an authorization denial, which is not what AWS
                // answers a caller it never resolved.
                AuthenticationRefusalDefinition::new(
                    management_events::AuditedDefinition::new(
                        crate::ServiceDefinition::new(Arc::clone(&engine)),
                        audited,
                    ),
                    AUTHENTICATION_REFUSAL,
                ),
            ),
            protocol: recorded_response_members::RecordedResponseJson::new(),
            engine,
            unusable_credential: UnusableCredential::stated_as(UNUSABLE_CREDENTIAL_CODE),
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
        self.unusable_credential
            .refusal_for(req, &self.protocol, |access_key_id| {
                iam_store::service_state(&*self.engine)
                    .is_none_or(|iam| iam.lock().access_key_is_usable(access_key_id))
                    && !sts_store::credential_principal_is_withdrawn(&*self.engine, access_key_id)
            })
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        //
        // One rewording is wired in here: a constraint refusal is stated the way AWS
        // states it by the shared renderer every service on this protocol uses
        // (`service_common::validation_message`), with the clause for a member this
        // service does not disclose the value of stating no value at all. WHICH members
        // those are is this service's own model's `@sensitive`, read by
        // [`sensitive_members`]; the code, the envelope, the member paths and every
        // requirement stay the shared renderer's and the framework's.
        //
        // A `@pattern` requirement is quoted as the MODEL writes it, anchors included:
        // that is what this service's recorded refusal of an alias name outside
        // `AliasNameType`'s pattern carries, so the expression passes through unchanged
        // rather than through the unanchoring other services' recordings show.
        ::service_common::validation_message::aws_validation_refusal_withholding_with(
            refusal,
            |pattern: &str| pattern.to_string(),
            sensitive_members::withholds_value,
        )
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model-derived protocol is `awsJson1_1`, and so is this one: it delegates
        // routing, request deserialization and every error to the framework's own
        // `awsJson1_1`, and differs only in the response members this service's wire
        // reports beside the modelled ones. See [`recorded_response_members`] for the rule.
        Some(&self.protocol)
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
