// SPDX-License-Identifier: CC0-1.0

//! The refusal a request earns for the CREDENTIAL it presents, before anything asks what
//! that credential may do.
//!
//! A session credential speaks for the principal it was minted for, and a temporary
//! credential's principal can cease to exist while the credential is still inside its
//! validity window: a role session's principal is the role it was assumed from, and a role is
//! identified by its id rather than by its name ("the stable and unique string identifying
//! the role", `Role$RoleId`), so deleting the role withdraws every session of it and creating
//! a role again under the same name — a new id behind the same ARN — does not revive one.
//!
//! The access key id of such a session resolves to NO principal, so this service refuses the
//! request at credential resolution rather than at authorization:
//! [`crate::state::INVALID_ACCESS_KEY_ID_CODE`] at HTTP 403 with that code's fixed sentence,
//! and no policy is read. The two codes mean opposite things to a client — this one is
//! terminal and tells an SDK's credential provider to stop using the credential, while
//! `AccessDenied` says the credential is still believed valid and only the grant is missing —
//! so answering the authorization verdict here would keep a dead session in rotation and send
//! an operator looking for a permissions bug.
//!
//! WHETHER the principal is gone is STS's own answer for the credential
//! (`sts_store::Caller::principal_is_withdrawn`), so every service that resolves a credential
//! there refuses the same dead sessions; what this service ANSWERS such a request with is its
//! own published refusal, which is what lives here.
//!
//! Every operation of this service is refused the same way, so the rule lives in ONE place:
//! this operation, which [`super::EmulatedService::override_operation`] routes such a request
//! to instead of the operation its address names — the same way the bucket-name refusal
//! stands in front of every operation that addresses a bucket
//! ([`super::invalid_bucket_name`]).

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::protocol::ServerProtocol;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use super::wire::UnwrappedXmlRestXml;

/// The routing key this operation is held in the service's own table under — a key the
/// model's protocol never resolves, so no modelled request can reach it by name.
pub const REFUSE_WITHDRAWN_CREDENTIAL: &str = "@refuse-withdrawn-credential";

/// Whether `access_key_id` names a session of this engine whose PRINCIPAL is gone (see the
/// module docs).
///
/// The question itself is the token service's own
/// ([`sts_store::credential_principal_is_withdrawn`]), so every service that resolves a
/// credential there refuses the same dead sessions; what is this service's own is the refusal
/// below that answers one.
#[must_use]
pub fn presents_a_withdrawn_credential<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
) -> bool {
    sts_store::credential_principal_is_withdrawn(engine, access_key_id)
}

/// The operation that answers the refusal, holding the codec it renders the envelope with.
pub struct RefuseWithdrawnCredential {
    /// This service's own protocol, built ONCE (the service's constructor builds this
    /// operation): the envelope a refusal of this service goes out in — the bare `<Error>`
    /// root, and the `x-amz-request-id` / `x-amz-id-2` pair S3 names every answer by.
    protocol: UnwrappedXmlRestXml,
}

impl RefuseWithdrawnCredential {
    /// Build the refusal operation. INFALLIBLE, and it holds no state: what it answers is
    /// fixed text.
    #[must_use]
    pub fn new() -> Self {
        Self {
            protocol: UnwrappedXmlRestXml::new(),
        }
    }

    /// The envelope: the code S3 answers an access key id that resolves to nothing with, at
    /// that code's status, carrying its published sentence.
    fn refusal(&self) -> Option<RtResponse> {
        let refusal = crate::scaffolded::Unmodeled {
            code: crate::state::INVALID_ACCESS_KEY_ID_CODE,
            status: crate::state::INVALID_ACCESS_KEY_ID_STATUS,
            message: crate::state::INVALID_ACCESS_KEY_ID_MESSAGE.to_string(),
        };
        self.protocol
            .serialize_error(
                &refusal,
                &crate::scaffolded::UNMODELED_SCHEMA,
                refusal.code,
                refusal.status,
            )
            .ok()
    }
}

impl Default for RefuseWithdrawnCredential {
    fn default() -> Self {
        Self::new()
    }
}

impl ErasedOperation for RefuseWithdrawnCredential {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let _ = ctx;
            // Nothing of the request is read: the sentence this code carries is fixed text in
            // full, and the key id it refused is deliberately not echoed back.
            match self.refusal() {
                Some(response) => response,
                // The serializer cannot fail on a fixed code and a fixed sentence; if it ever
                // did, the request keeps the framework's own answer rather than panicking in
                // the request path.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}
