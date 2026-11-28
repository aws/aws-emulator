// SPDX-License-Identifier: CC0-1.0

//! The split between AUTHENTICATING a request and AUTHORIZING it, for the one caller of
//! Amazon S3 that presents no credential at all: the ANONYMOUS principal.
//!
//! "If you grant READ access to the anonymous user, the `GetObject` operation returns the
//! object without using an authorization header" (`GetObject`'s own documentation). So a
//! request carrying neither an `Authorization` header nor a presigned credential is not a
//! request that FAILED to authenticate: it is a request that speaks for the anonymous
//! principal, which S3 then authorizes like any other — against the addressed resource's own
//! ACL, the bucket policy, and the Block Public Access configuration in force
//! ([`crate::state::Bucket::anonymous_object_read_refusal`] and
//! [`crate::state::Bucket::anonymous_bucket_read_refusal`] are the two readings of that
//! rule). What it may not do is refused `403 AccessDenied` by that decision, not by the
//! signature boundary — which is the difference between a bucket whose Block Public Access
//! settings can be SHOWN to block public access and one where nothing public ever worked.
//!
//! ## Why this is a module rather than a line in a handler
//!
//! Whether a request is authenticated at all is decided before any handler runs: the
//! framework reads the auth schemes of the operation the request names and refuses the
//! request when no registered authenticator recognizes it, which no unsigned request does.
//! The schemes it reads are the MODEL's for that operation ("an operation's own schemes count
//! only when the model names none for the request"), and this model names SigV4 on every
//! operation — the model has no way to say "and also the anonymous principal, whom the
//! resource's own policy judges".
//!
//! Two pieces answer that, and both are needed because the seam that states an operation's
//! schemes ([`::smithy_server_runtime::ErasedOperation::auth_schemes`]) is handed no request:
//!
//! 1. [`ServedUnsigned`], the wrapper the operations this service serves itself are served
//!    behind. It states no auth scheme for a request the framework's own authenticator cannot
//!    verify — so such a request reaches the handler, whose own authorization then answers it
//!    — and the operation's own modelled schemes for every request that authenticator can
//!    verify, so a header-signed request is verified exactly as before. Which requests those
//!    are differs by surface: a request presenting NO credential reaches only the operations
//!    that judge the anonymous principal themselves ([`served_to_the_anonymous_caller`] is the
//!    catalog), while one presenting a credential in a presentation no authenticator can read
//!    ([`Admission::OwnQueryCredential`]) reaches every operation served behind this wrapper,
//!    because what admits it is the signature this service itself recomputed
//!    ([`super::legacy_query_credential`]). Nothing is waved through either way.
//! 2. [`remember`], which records WHICH surface the request being served is on, read once per
//!    request by this service's routing seam (the seam the framework reaches with the whole
//!    request, before it authenticates) and answered by [`admission`]. It is a request-scoped
//!    note rather than state, exactly like the note a refusal's wording is read from
//!    ([`super::sigv4`]): the framework calls both within one synchronous stretch of the one
//!    dispatch.
//!
//! Every operation NOT in the catalog keeps the refusal an unsigned request earns today: it
//! states the model's SigV4 scheme whatever the request presented, so the signature boundary
//! refuses the unsigned caller with S3's own `AccessDenied` ([`super::sigv4`] words it) before
//! any handler runs. That is the conservative half of the split — a handler that does not ask
//! who is calling must never be handed a caller it would read as the bucket owner.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};
use ::std::cell::Cell;

/// WHICH surface of this service the request being served is on, as far as authenticating it
/// goes — the note [`remember`] records and [`admission`] answers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Admission {
    /// The request presents a credential — an `Authorization` header or a presigned query —
    /// so it is the caller that credential names and the framework verifies its signature
    /// under the schemes the model gives the operation it addressed.
    Credentialed,
    /// The request presents none: the ANONYMOUS principal, served what the bucket grants to
    /// everyone by the operations of this service that judge that caller.
    Anonymous,
    /// The request is addressed to a bucket's WEBSITE endpoint, a surface the model names no
    /// operation on at all — every request of it is the anonymous principal's, whatever
    /// credential it happens to carry, because a website endpoint reads none.
    WebsiteEndpoint,
    /// The request presents a complete credential of ITS OWN, among its query parameters,
    /// that no authenticator registered with the framework can read — the `AWSAccessKeyId` /
    /// `Signature` / `Expires` presentation of a presigned URL. THIS SERVICE authenticates it
    /// ([`super::legacy_query_credential`]), so the framework is told no scheme signs the
    /// operation it addressed, and the signature the URL carries is recomputed by the routing
    /// seam before that operation is served.
    OwnQueryCredential,
}

thread_local! {
    /// Which surface the request this thread is serving reached, for the length of that one
    /// request.
    ///
    /// Not state: a request-scoped note, needed because the seam that states an operation's
    /// auth schemes is handed no request. Its value is the one the request being served
    /// recorded, which three things together are what make true:
    ///
    /// * the DEFAULT is the surface that demands the model's own schemes
    ///   ([`Admission::Credentialed`]), so a note nothing wrote waives nothing;
    /// * it is FORGOTTEN at both ends of a dispatch — before this service routes a request
    ///   (its protocol seam, the first thing the framework asks it per request) and again
    ///   once the decision has been taken and the operation runs ([`ServedUnsigned::run`]) —
    ///   so a request the routing seam never reached, or one that unwound before it did,
    ///   reads the default rather than another request's note ([`forget`]);
    /// * and it is RECORDED for every request this service routes, by the seam the framework
    ///   reaches with the whole request before it authenticates it.
    ///
    /// So the only way the note can be wrong is conservative: the framework verifies a
    /// signature this service would have admitted, which refuses a caller rather than
    /// serving one.
    static ADMISSION: Cell<Admission> = const { Cell::new(DEMANDS_THE_MODELS_SCHEMES) };
}

/// The surface the note holds when no request recorded one: the one whose operations state
/// the schemes their model gives them, so nothing is admitted on an absent note.
const DEMANDS_THE_MODELS_SCHEMES: Admission = Admission::Credentialed;

/// Record that the request this thread is serving reached `admission`'s surface.
pub(super) fn remember(admission: Admission) {
    ADMISSION.with(|note| note.set(admission));
}

/// Forget whatever note this thread holds, putting it back to the surface that admits nobody
/// ([`DEMANDS_THE_MODELS_SCHEMES`]).
///
/// Called at both ends of one dispatch (see the note's own documentation), so no decision is
/// ever taken on a value another request recorded.
pub(super) fn forget() {
    remember(DEMANDS_THE_MODELS_SCHEMES);
}

/// The surface [`remember`] recorded for the request this thread is serving.
pub(super) fn admission() -> Admission {
    ADMISSION.with(Cell::get)
}

/// Whether `request` presents NO credential at all — the ANONYMOUS caller.
///
/// The one reading of that question ([`crate::state::presents_no_credential`]), asked of the
/// runtime-API request this service's routing seam is handed — so the seam that admits the
/// anonymous caller, the wording of a refusal and the handler that authorizes it cannot
/// disagree about which requests those are.
pub(super) fn presents_no_credential(request: &RtRequest) -> bool {
    crate::state::presents_no_credential(
        request
            .headers()
            .get(::http::header::AUTHORIZATION.as_str()),
        super::wire::request_query(request.uri()),
    )
}

/// The operations of this service that judge the ANONYMOUS principal themselves, keyed by the
/// modelled name the protocol resolves for each.
///
/// Exactly the reads whose authorization this service decides against what the bucket grants
/// to EVERYONE, and no others:
///
/// * the object reads — `GetObject` and `HeadObject`, authorized under `s3:GetObject` ("a
///   `HEAD` request has the same options as a `GET` operation on an object"), which
///   [`crate::state::Bucket::anonymous_object_read_refusal`] answers;
/// * the bucket reads — `ListObjects`, `ListObjectsV2` and `HeadBucket`, authorized under
///   `s3:ListBucket` ("you must have READ access to the bucket"), which
///   [`crate::state::Bucket::anonymous_bucket_read_refusal`] answers.
///
/// An operation outside this catalog keeps the refusal the signature boundary gives an
/// unsigned request: its handler resolves the calling principal from the credential the
/// request presents, and a request presenting none must not be read as the account that owns
/// the bucket.
pub(super) const SERVED_TO_THE_ANONYMOUS_CALLER: [&str; 5] = [
    "GetObject",
    "HeadObject",
    "ListObjects",
    "ListObjectsV2",
    "HeadBucket",
];

/// Whether the operation named `operation` is one this service serves the anonymous caller
/// ([`SERVED_TO_THE_ANONYMOUS_CALLER`]).
pub(super) fn served_to_the_anonymous_caller(operation: &str) -> bool {
    SERVED_TO_THE_ANONYMOUS_CALLER.contains(&operation)
}

/// One operation of this service, served to the callers THIS SERVICE admits as well as to the
/// one the framework's own authenticator verifies: the ANONYMOUS principal, and the caller
/// whose credential travels in a presentation no registered authenticator can read.
///
/// The operation behind it is untouched — it owns deserialize -> validate -> handle ->
/// serialize exactly as before, authorization included. What this wrapper adds is the one
/// thing an operation cannot say for itself: that the request being served presents nothing
/// the signature boundary can verify, so it is admitted and judged by what this service
/// decided about it instead of being refused for a signature the boundary could not read.
///
/// Nothing is waved through either way: an anonymous request is authorized by the handler
/// against what the bucket grants to everyone, and a request presenting its own query
/// credential has had that credential's signature recomputed by the routing seam
/// ([`super::legacy_query_credential`]) before it reaches this wrapper.
pub(super) struct ServedUnsigned {
    /// The modelled name of the operation behind this wrapper, which is what decides whether
    /// the ANONYMOUS caller reaches it ([`SERVED_TO_THE_ANONYMOUS_CALLER`] is the catalog).
    operation: &'static str,
    served: Box<dyn ErasedOperation>,
}

impl ServedUnsigned {
    /// Serve `served`, the operation modelled as `operation`, to the callers above as well.
    /// Takes the operation already wrapped in whatever else this service serves it behind (its
    /// front door, its request counting, its data-event record), so one operation is served
    /// once however the caller is authenticated.
    pub(super) fn new(operation: &'static str, served: Box<dyn ErasedOperation>) -> Self {
        Self { operation, served }
    }
}

impl ErasedOperation for ServedUnsigned {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The admission decision has been taken by the time an operation runs: the framework
        // read it before it dispatched. So the note is FORGOTTEN here, at the far end of the
        // one dispatch that recorded it — nothing this request does from now on may be
        // decided on it, and a later dispatch on this thread that never reaches this service's
        // routing seam reads the surface that admits nobody instead.
        forget();
        self.served.run(svc, req, ctx)
    }

    /// No scheme at all for a request the framework's authenticator cannot verify, and the
    /// wrapped operation's own modelled schemes for every request it can.
    ///
    /// The caller presenting its OWN query credential is admitted for every operation served
    /// behind this wrapper, because what admits it is the signature this service recomputed
    /// ([`super::legacy_query_credential`]) rather than anything about the operation. The
    /// ANONYMOUS caller signs nothing, so there is no signature to verify and no credential to
    /// resolve: what it may do is the authorization question the handler behind this wrapper
    /// answers from the bucket's own ACL, policy and Block Public Access settings — for the
    /// operations this service serves that caller, and for no others, which is why the catalog
    /// is consulted here.
    ///
    /// Every other request keeps the model's own schemes, so a wrong signature is still
    /// refused for the signature: a request presenting a credential the framework CAN verify,
    /// an anonymous one addressing an operation outside the catalog, and a request on the
    /// WEBSITE ENDPOINT — which this wrapper never serves, because that surface's own
    /// operation is what states how such a request is authenticated and this service's routing
    /// seam resolves it ahead of every entry of the door table.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        match admission() {
            Admission::OwnQueryCredential => Some(&[]),
            Admission::Anonymous if served_to_the_anonymous_caller(self.operation) => Some(&[]),
            Admission::Credentialed | Admission::Anonymous | Admission::WebsiteEndpoint => {
                self.served.auth_schemes()
            }
        }
    }
}
