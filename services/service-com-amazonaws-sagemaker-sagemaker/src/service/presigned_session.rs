// SPDX-License-Identifier: CC0-1.0

//! Serves the authenticated session a presigned URL this service handed out opens, at the
//! address it published for it.
//!
//! A presigned URL is a GRANT, not a formatted string: `CreatePresignedNotebookInstanceUrl`,
//! `CreatePartnerAppPresignedUrl` and `CreatePresignedMlflowAppUrl` each record a
//! [`crate::state::PresignedUrlGrant`] on the resource they are asked about and publish a
//! URL addressing that resource on the endpoint the request arrived on
//! ([`crate::state::presigned_session_url`]). Fetching that URL is how the caller opens the
//! session, so this module serves it:
//!
//! * a `GET` of the session address carrying a live grant token reports the session that
//!   grant opened — the resource it belongs to and when it stops authenticating;
//! * a token no grant of that resource holds, and one whose window the clock has closed,
//!   authenticate nothing: the answer is `403`, as AWS stops authenticating a URL past the
//!   window the request asked for.
//!
//! The model binds no operation to that path — an opened session is not an API call, it is
//! what the API's URL points at — so it is claimed through
//! [`smithy_server_runtime::Service::override_operation`] rather than through the generated
//! routing table, and it is served however it arrives
//! ([`ErasedOperation::auth_schemes`] answers an empty list): a presigned URL is opened by
//! a browser, which signs nothing and carries the grant in the URL instead.
//!
//! Serving a session changes NOTHING: the grant was recorded when the URL was minted, and
//! this route only reads it, so a fetch and a `Describe*` of the same resource cannot
//! disagree.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

/// The method a presigned session is opened on: a browser following the URL.
const SESSION_METHOD: &str = "GET";

/// The media type the session is reported in.
const SESSION_MEDIA_TYPE: &str = "application/json";

/// The media type of the answers that are not a session.
const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// What a fetch that authenticates nothing is answered with. A presigned URL is opened by
/// a browser rather than by an AWS SDK, so the answer is the status itself with a one-line
/// reason and no API error envelope.
const NO_SESSION: &str = "this URL authenticates no session\n";

/// What a fetch past the window the URL was minted for is answered with.
const SESSION_OVER: &str = "this URL no longer authenticates: its session window has elapsed\n";

/// What a fetch is answered with once the resource serves no session any more.
const NOT_SERVING: &str = "the resource this URL opens a session on is no longer serving one\n";

/// The Smithy auth scheme an empty override is spelled with: see
/// [`ErasedOperation::auth_schemes`] below.
const ANONYMOUS: &[&str] = &[];

/// One presigned-session address, as its path names it.
#[derive(Debug, PartialEq, Eq)]
struct Addressed {
    /// The resource kind the address names (`notebook-instance`, `partner-app`,
    /// `mlflow-app`).
    kind: String,
    /// The resource's own identifier within that kind.
    resource: String,
}

/// Serves the presigned sessions of this service.
///
/// Owns the shared engine, as an operation does, because what it serves is the grant the
/// minting operation wrote: nothing is held beside those records, so a session and a
/// describe report the same resource.
pub struct PresignedSessionRoute<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> PresignedSessionRoute<Eng> {
    /// Build the route from the shared engine, as this service's operations are built.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// Whether this route serves `req`, which is what the service's `can_handle` and
    /// `override_operation` seams both ask.
    pub fn claims(&self, method: &str, uri: &str) -> bool {
        addressed(method, uri).is_some()
    }

    /// Serve one fetch of a session address.
    fn serve(&self, req: &RtRequest) -> RtResponse {
        let Some(addressed) = addressed(req.method(), req.uri()) else {
            return answer(404, PLAIN_MEDIA_TYPE, NO_SESSION);
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return answer(500, PLAIN_MEDIA_TYPE, NO_SESSION);
        };
        // The token is read DECODED, the way the mint percent-encoded it into the URL, so
        // the value matched here is the one the grant holds.
        let token = query_field(req.uri(), crate::state::PRESIGNED_SESSION_TOKEN_FIELD)
            .unwrap_or_default();
        let store = state.lock();
        let session = store.presigned_session(
            &addressed.kind,
            &addressed.resource,
            &token,
            std::time::SystemTime::now(),
        );
        drop(store);

        match session {
            crate::state::PresignedSession::Open {
                subject_arn,
                session_expires_at,
            } => answer_session(&subject_arn, session_expires_at),
            crate::state::PresignedSession::Expired => {
                answer(403, PLAIN_MEDIA_TYPE, SESSION_OVER)
            }
            crate::state::PresignedSession::NotServing => {
                answer(403, PLAIN_MEDIA_TYPE, NOT_SERVING)
            }
            crate::state::PresignedSession::Unknown => answer(403, PLAIN_MEDIA_TYPE, NO_SESSION),
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for PresignedSessionRoute<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        let answer = self.serve(req);
        Box::pin(async move { answer })
    }

    /// Who may reach this address: whoever holds the URL. A presigned URL is followed by a
    /// browser, which presents no SigV4 credential — the grant token in the URL is what
    /// authenticates the session, and this route verifies it against the grant the minting
    /// call recorded.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(ANONYMOUS)
    }
}

/// The session an open grant reports: the resource it belongs to and when it stops
/// authenticating, both read off the stored grant.
fn answer_session(subject_arn: &str, session_expires_at: std::time::SystemTime) -> RtResponse {
    let expires_at = session_expires_at
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs_f64())
        .unwrap_or_default();
    let session = ::serde_json::json!({
        "ResourceArn": subject_arn,
        "SessionExpiresAt": expires_at,
    })
    .to_string();
    answer(200, SESSION_MEDIA_TYPE, &session)
}

/// The address `uri` names on a `GET`, or `None` when the request is not one this route
/// serves.
///
/// The path is read the way the mint wrote it — its segments percent-decoded — so a
/// resource identifier that had to be encoded names the same resource here as it does in
/// the record. Only a kind this service publishes sessions for is claimed, so nothing else
/// under the namespace is answered from here.
fn addressed(method: &str, uri: &str) -> Option<Addressed> {
    if method != SESSION_METHOD {
        return None;
    }
    let segments = decoded_segments(uri);
    let [namespace, plane, kind, resource, auth] = segments.as_slice() else {
        return None;
    };
    if namespace != ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
        || plane != crate::state::PRESIGNED_SESSION_PLANE
        || auth != crate::state::PRESIGNED_SESSION_AUTH_SEGMENT
    {
        return None;
    }
    let kinds = [
        crate::state::NOTEBOOK_INSTANCE_ARN_KIND,
        crate::state::PARTNER_APP_ARN_KIND,
        crate::state::MLFLOW_APP_ARN_KIND,
    ];
    if !kinds.contains(&kind.as_str()) {
        return None;
    }
    Some(Addressed {
        kind: kind.clone(),
        resource: resource.clone(),
    })
}

/// One answer of this route: the bytes, their media type and their length.
///
/// The headers are set through the NON-panicking setter: `Headers::insert` aborts on a
/// value it cannot represent, and nothing a request can reach may panic. Every value here
/// is this module's own, so neither can fail; a header that somehow did is dropped rather
/// than taken down the process with.
fn answer(status: u16, media_type: &str, body: &str) -> RtResponse {
    let bytes = body.as_bytes().to_vec();
    let length = bytes.len();
    let mut response = RtResponse::new(session_status(status), SdkBody::from(bytes));
    let _ = response
        .headers_mut()
        .try_insert("content-type", media_type.to_string());
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// A status this route answers with. Every call site passes a constant, so the conversion
/// cannot fail.
fn session_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("a presigned session answers constant statuses")
}

/// The decoded value of one query field of `uri`, or `None` when it carries none.
fn query_field(uri: &str, field: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_, query)| query).unwrap_or("");
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        name.eq_ignore_ascii_case(field)
            .then(|| smithy_server_runtime::percent_decode(value))
    })
}

/// The path segments of `uri`, percent-decoded, empty segments dropped — the same view of
/// a path the mint built the address from.
fn decoded_segments(uri: &str) -> Vec<String> {
    uri.parse::<http::Uri>()
        .map(|parsed| {
            parsed
                .path()
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(smithy_server_runtime::percent_decode)
                .collect()
        })
        .unwrap_or_default()
}
