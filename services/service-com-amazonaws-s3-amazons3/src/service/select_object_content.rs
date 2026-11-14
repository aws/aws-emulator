// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request addressed to the S3 SELECT data plane.
//!
//! `SelectObjectContent` is bound at `POST /{Bucket}/{Key+}?select&select-type=2` and streams
//! a `SelectObjectContentEventStream`
//! (`smithy_query(query="operation",operation="SelectObjectContent",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`),
//! the one `@streaming` event-stream union of this model, which the scaffold generator
//! declines to type (`src/ops/SKIPPED_STUBS.md`) — so no routing-table entry claims `?select`
//! and the framework would answer such a request as one nobody has written. That is not one of
//! the answers Amazon S3 gives it, and the two it does give are decided in the same order
//! every other call of an object is:
//!
//! 1. WHO is asking, first: "you must have the `s3:GetObject` permission for this operation"
//!    (`SelectObjectContent`'s own documentation), so a caller the policies in force do not
//!    leave that action to is refused `AccessDenied` (HTTP 403) — the same decision, over the
//!    same action and the same object resource, that a plain read of the object's bytes takes
//!    ([`super::request_authorization`]), and the same sentence it is worded in. A refusal
//!    about the CALLER comes before anything about the feature, which is what keeps an
//!    unauthorized caller from learning anything about the object it addressed.
//! 2. Then the feature itself: Amazon S3 has withdrawn S3 Select from every account that had
//!    not already enabled it, and refuses the request with the 405 `MethodNotAllowed` it
//!    answers any resource that does not serve the method asked of it (`GetObjectTorrent`,
//!    which no account can enable, is refused the same way). The withdrawal is account-wide
//!    rather than per-object, so this refusal is the same for every Select request, whatever
//!    expression, serialization, scan range or encryption it carries and whichever bucket or
//!    key it names — and a client that probes Select and falls back on this code sees the code
//!    it branches on instead of an unmodelled exception.
//!
//! It is served as an OPERATION, through this service's own front door
//! ([`super::EmulatedService::override_operation`] routes to it), because the authorization
//! half needs the two things only an operation is given: the shared engine the bucket's policy
//! is read from, and the request's own context, from which the principal the request speaks
//! for is resolved. The framework has already verified the request's SIGNATURE by the time
//! this runs, so a refusal here is only ever about what the caller may do.

use ::std::borrow::Cow;
use ::std::sync::Arc;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The key this service serves the Select data plane under.
///
/// It carries the `@` sigil the sibling door keys carry
/// ([`super::invalid_bucket_name::REFUSE_INVALID_BUCKET_NAME`]), a character no modelled
/// operation's name holds, so the "no routing key the model's protocol can resolve reaches
/// this" claim is structural: nothing but
/// [`super::EmulatedService::override_operation`] ever looks it up.
pub const SELECT_OBJECT_CONTENT: &str = "@select-object-content";

/// The operation this service answers a Select request with — see the module docs.
pub struct SelectObjectContent<Eng> {
    /// The SAME shared engine every operation of this service holds: the authorization
    /// decision is taken against the bucket's own policy and the documents in force on the
    /// calling principal, which live in that one engine.
    engine: Arc<Eng>,
}

impl<Eng> SelectObjectContent<Eng> {
    /// Build the Select data plane's operation from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> SelectObjectContent<Eng> {
    /// The OBJECT this request addresses, in either address form: the bucket and the key of
    /// its PATH-STYLE address, so a virtual-hosted-style Select request — the form every SDK
    /// sends for an endpoint addressed by bucket name — names the same object a path-style one
    /// does. Each half is the decoded label the operations of this service bind
    /// ([`super::sigv4::percent_decoded`]).
    ///
    /// `None` for a request whose address names no object at all, which the routing seam does
    /// not send here ([`super::addresses_s3_select`] requires one).
    fn addressed_object(&self, req: &RtRequest) -> Option<(String, String)> {
        let path = super::wire::request_path(req.uri());
        let addresses_a_bucket =
            |name: &str| super::addressed_bucket_in(&*self.engine, name).is_some();
        let path_style = match super::request_host_of(req)
            .and_then(|host| super::addressed_in_host(host, path, &addresses_a_bucket))
        {
            Some(bucket) => Cow::Owned(super::path_with_bucket(bucket, path)),
            None => Cow::Borrowed(path),
        };
        let (bucket, key) = path_style.trim_start_matches('/').split_once('/')?;
        (!bucket.is_empty() && !key.is_empty()).then(|| {
            (
                super::sigv4::percent_decoded(bucket),
                super::sigv4::percent_decoded(key),
            )
        })
    }

    /// Why the policies in force refuse this caller the Select of the object it addressed, or
    /// `None` when they leave the action to it.
    ///
    /// The action is `s3:GetObject` on that object, whatever the key holds: Select reads an
    /// object's bytes, and "you must have the `s3:GetObject` permission for this operation"
    /// (`SelectObjectContent`'s own documentation) — the action this service names once
    /// ([`crate::state::GET_OBJECT_ACTION`]). The decision is the tree's one evaluation
    /// of the documents in force on the principal and of the bucket's own policy
    /// ([`super::request_authorization::denial`]), so this refusal and a plain read's cannot
    /// disagree about the same caller, the same object and the same action.
    ///
    /// Read-only: the guard is taken, the bucket's policy is copied out, and nothing is
    /// written. WHO is asking is resolved BEFORE that guard is taken, so the STS and IAM
    /// stores are never reached while this service's own guard is held.
    fn denial(
        &self,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> Option<String> {
        let (bucket, key) = self.addressed_object(req)?;
        let caller = super::request_authorization::request_caller(&*self.engine, ctx);
        // The policy of the bucket the address reaches, or `None` for one carrying none and
        // for an address this emulator holds no bucket for — the documents in force on the
        // principal decide the request either way.
        let policy = crate::state::service_state(&*self.engine).and_then(|state| {
            state
                .lock()
                .bucket(&bucket)
                .and_then(|held| held.policy.clone())
        });
        super::request_authorization::denial(
            &caller,
            policy.as_deref(),
            crate::state::GET_OBJECT_ACTION,
            &crate::state::object_arn(&bucket, &key),
            &super::authorization::read_conditions(),
        )
    }

    /// The answer this request earns: the authorization refusal when the policies in force
    /// withhold the action, else the refusal of the feature itself — see the module docs.
    fn answer(&self, svc: &dyn Service, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        match self.denial(req, ctx) {
            Some(message) => refusal(
                svc,
                crate::state::ACCESS_DENIED_CODE,
                crate::state::ACCESS_DENIED_STATUS,
                message,
            ),
            None => refusal(
                svc,
                crate::state::METHOD_NOT_ALLOWED_CODE,
                crate::state::METHOD_NOT_ALLOWED_STATUS,
                crate::state::METHOD_NOT_ALLOWED_MESSAGE.to_string(),
            ),
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for SelectObjectContent<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.answer(svc, req, ctx) })
    }
}

/// `code`, `status` and `message` rendered through the SAME protocol every other answer of
/// this service is rendered through, so the envelope a client parses — the bare `<Error>` root
/// with its `<Code>`, `<Message>` and request-correlation elements, and the matching
/// `x-amz-request-id` / `x-amz-id-2` headers — is the one every other S3 refusal carries.
///
/// On the serializer failure a fixed code and a rendered sentence cannot actually provoke, the
/// status alone is answered rather than panicking in the request path.
fn refusal(svc: &dyn Service, code: &'static str, status: u16, message: String) -> RtResponse {
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| svc.definition().protocol());
    let document = crate::scaffolded::Unmodeled {
        code,
        status,
        message,
    };
    protocol
        .serialize_error(&document, &crate::scaffolded::UNMODELED_SCHEMA, code, status)
        .unwrap_or_else(|error| {
            ::tracing::warn!(
                code,
                %error,
                "this refusal of a Select request could not be rendered in this service's \
                 envelope"
            );
            let status = StatusCode::try_from(status)
                .unwrap_or_else(|_| StatusCode::try_from(400).expect("400 is a status"));
            RtResponse::new(status, SdkBody::empty())
        })
}
