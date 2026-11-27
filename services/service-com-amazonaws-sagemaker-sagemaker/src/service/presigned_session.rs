// SPDX-License-Identifier: CC0-1.0

//! Serves the authenticated session a presigned URL this service handed out opens, at the
//! address it published for it.
//!
//! A presigned URL is a GRANT, not a formatted string:
//! `CreatePresignedNotebookInstanceUrl`, `CreatePartnerAppPresignedUrl`,
//! `CreatePresignedMlflowAppUrl`, `CreatePresignedMlflowTrackingServerUrl` and
//! `CreatePresignedDomainUrl` each record a
//! [`crate::state::PresignedUrlGrant`] on the resource they are asked about and publish a
//! URL addressing that resource on the endpoint the request arrived on
//! ([`crate::state::presigned_session_url`]). Fetching that URL is how the caller opens the
//! session, so this module serves it:
//!
//! * a `GET` carrying a live grant token whose resource publishes a web address of its OWN is
//!   HANDED OFF to it — `302`, with that address in `Location` and the grant travelling in
//!   the query — because that is what the URL exists for: "the user will be automatically
//!   signed in to the domain, and granted access to all of the Apps and files associated with
//!   the Domain's … volume". A Studio domain publishes one (its `Url`, the Studio landing of
//!   that domain), so a presigned domain URL lands its holder in that domain's own Studio
//!   rather than answering from the address it was fetched at;
//! * a `GET` of a resource that publishes no second address reports the session that grant
//!   opened — the resource it belongs to and when it stops authenticating;
//! * a token no grant of that resource holds, and one whose window the clock has closed,
//!   authenticate nothing: the answer is `403`, as AWS stops authenticating a URL past the
//!   window the request asked for, and nothing is handed off.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use super::published_address::{
    answer, decoded_segments, hand_off, query_field, FETCH_METHOD, JSON_MEDIA_TYPE,
    PLAIN_MEDIA_TYPE,
};

/// The method a presigned session is opened on: a browser following the URL.
const SESSION_METHOD: &str = FETCH_METHOD;

/// The media type the session is reported in.
const SESSION_MEDIA_TYPE: &str = JSON_MEDIA_TYPE;

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
    /// `mlflow-app`, `mlflow-tracking-server`, `user-profile`).
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
        // The token is the credential this address verifies, so a fetch that presents NONE —
        // no field at all, or an empty one — authenticates nothing and is answered before the
        // store is touched. It is read DECODED, the way the mint percent-encoded it into the
        // URL, so the value matched below is the one the grant holds.
        let token = query_field(req.uri(), crate::state::PRESIGNED_SESSION_TOKEN_FIELD)
            .filter(|token| !token.is_empty());
        let Some(token) = token else {
            return answer(403, PLAIN_MEDIA_TYPE, NO_SESSION);
        };
        let store = state.lock();
        let session = store.presigned_session(
            &addressed.kind,
            &addressed.resource,
            &token,
            std::time::SystemTime::now(),
        );
        // Where the resource this session belongs to publishes its own web address, when it
        // publishes one: a session that opens at another address is HANDED OFF to it below.
        let published = store.published_session_address(&addressed.kind, &addressed.resource);
        drop(store);

        match session {
            crate::state::PresignedSession::Open {
                subject_arn,
                session_expires_at,
            } => {
                // A presigned URL exists to sign its holder in to the resource's OWN web
                // experience ("the user will be automatically signed in to the domain, and
                // granted access to all of the Apps and files associated with the Domain's
                // … volume"), which is served at an address of that resource's own. So a
                // fetch that has not reached that address yet is answered with the hand-off
                // to it, carrying the grant and the session fields this plane published — and
                // the fetch that arrives there opens the session. A resource that publishes
                // no web address of its own serves the session where the URL was published,
                // and so does a hand-off this process could not address.
                let landing = landing_fields(req.uri());
                let landing: Vec<(&str, &str)> = landing
                    .iter()
                    .map(|(field, value)| (*field, value.as_str()))
                    .collect();
                let handed_off = published
                    .filter(|address| {
                        !crate::state::addresses_published_path(address, req.uri())
                    })
                    .and_then(|address| {
                        hand_off(&crate::state::handed_off_address(
                            &address, &token, &landing,
                        ))
                    });
                match handed_off {
                    Some(answer) => answer,
                    None => answer_session(&subject_arn, session_expires_at),
                }
            }
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

/// The session fields BESIDE the grant token that a fetch of a session address carries, as
/// the mint published them: where the session lands ([`crate::state::PRESIGNED_SESSION_SPACE_FIELD`],
/// [`crate::state::PRESIGNED_SESSION_LANDING_URI_FIELD`]), each only when the fetch names it.
///
/// Only these travel on to the address a hand-off leads to: the fetch's own query is not
/// copied, so nothing a caller appended to the URL reaches the next address.
fn landing_fields(uri: &str) -> Vec<(&'static str, String)> {
    [
        crate::state::PRESIGNED_SESSION_SPACE_FIELD,
        crate::state::PRESIGNED_SESSION_LANDING_URI_FIELD,
    ]
    .into_iter()
    .filter_map(|field| query_field(uri, field).map(|value| (field, value)))
    .collect()
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
        crate::state::TRACKING_SERVER_KIND,
        crate::state::USER_PROFILE_ARN_KIND,
    ];
    if !kinds.contains(&kind.as_str()) {
        return None;
    }
    Some(Addressed {
        kind: kind.clone(),
        resource: resource.clone(),
    })
}

