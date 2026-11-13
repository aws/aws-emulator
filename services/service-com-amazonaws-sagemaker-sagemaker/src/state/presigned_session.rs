// SPDX-License-Identifier: CC0-1.0

//! The PRESIGNED-SESSION slice of this service's state: where a URL this service hands a
//! caller is ADDRESSED, the token it carries, and what a fetch of that address resolves
//! to.
//!
//! Three operations answer a URL whose whole point is that the caller FETCHES it later —
//! `CreatePresignedNotebookInstanceUrl`, `CreatePartnerAppPresignedUrl` and
//! `CreatePresignedMlflowAppUrl`. Such a URL has to lead back to THIS emulator: a host of
//! AWS's own (`<name>.notebook.<region>.sagemaker.aws`) resolves to nothing a client of
//! the emulator can reach, so every flow the operation exists for fails with a name or
//! network error instead of opening a session. So the address is the endpoint the request
//! arrived on ([`::smithy_server_runtime::OperationContext::endpoint`]) plus the path this
//! module lays out, under the reserved namespace every emulator-served data plane is
//! addressed in (`::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`), and
//! `crate::service::presigned_session` SERVES it.
//!
//! The token in the URL is the grant: it is presented back to this emulator at that
//! address and matched against the grant the minting call recorded on the resource
//! ([`PresignedUrlGrant`]), so what authenticates a session is state this service holds
//! rather than the shape of the URL. It is minted by the one component that generates
//! secrets ([`mint_session_token`]), never by a service.

use super::*;

/// The path segment that names this service's presigned-session plane, after the reserved
/// `_aws` namespace: `/_aws/sagemaker/<kind>/<resource>/auth`.
///
/// The namespace segment begins with `_`, which no modelled path segment of any service
/// carries, so the address collides with no control-plane route; this segment names WHICH
/// plane under it, exactly as the shared addressing module lays out
/// (`/_aws/{plane}/{resource}/{path}`).
pub const PRESIGNED_SESSION_PLANE: &str = "sagemaker";

/// The last path segment of a presigned-session address: the sign-in a fetch of the URL
/// performs, as AWS's own presigned URLs for these resources spell it.
pub const PRESIGNED_SESSION_AUTH_SEGMENT: &str = "auth";

/// The query field the grant token travels in, as AWS's presigned URLs name it.
pub const PRESIGNED_SESSION_TOKEN_FIELD: &str = "authToken";

/// The resource kind a partner AI app's ARN carries, i.e. the `<kind>` in
/// [`resource_arn`], as the modelled `PartnerAppArn` pattern spells it
/// (`partner-app\/app-[A-Z0-9]{12}`). Named once here so the create mints, the presign
/// addresses and the session route resolve the one kind.
pub const PARTNER_APP_ARN_KIND: &str = "partner-app";

/// The resource kind an MLflow App's ARN carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `MlflowAppArn` pattern spells it (`…:[0-9]{12}:mlflow-app/`). Named
/// once here for the same reason as [`PARTNER_APP_ARN_KIND`].
pub const MLFLOW_APP_ARN_KIND: &str = "mlflow-app";

/// The characters a percent-encoded path segment or query value keeps unescaped: RFC
/// 3986's unreserved set (`ALPHA / DIGIT / "-" / "." / "_" / "~"`). Everything else is
/// escaped, which is what a session token needs — the minted alphabet includes `+` and
/// `/`, neither of which may travel unescaped. The escaping itself is the
/// `percent-encoding` crate's, not this module's.
const URL_ENCODE_SET: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// `value` percent-encoded for a path segment or a query value.
pub fn encoded_url_value(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, URL_ENCODE_SET).to_string()
}

/// The path the authenticated session of one resource is served at:
/// `/_aws/sagemaker/<kind>/<resource>/auth`, with the resource's identifier percent-encoded.
///
/// One function for the mint and for the route, so the address a URL names and the
/// address this emulator serves cannot drift apart.
pub fn presigned_session_path(kind: &str, resource: &str) -> String {
    format!(
        "/{namespace}/{PRESIGNED_SESSION_PLANE}/{kind}/{resource}/{PRESIGNED_SESSION_AUTH_SEGMENT}",
        namespace = ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE,
        resource = encoded_url_value(resource),
    )
}

/// The URL an operation publishes for a session this emulator serves: the endpoint the
/// request arrived on, the session path of `kind`/`resource`, the grant `token`, and
/// whatever `extra` query fields that operation's URL carries.
///
/// The host is NEVER one of AWS's own names: a URL the caller is told to fetch has to
/// resolve to where the caller is already talking, which is what
/// [`::smithy_server_runtime::OperationContext::endpoint`] reports ("the base URL a later
/// request addresses").
pub fn presigned_session_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    kind: &str,
    resource: &str,
    token: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut url = format!(
        "{endpoint}{path}?{PRESIGNED_SESSION_TOKEN_FIELD}={token}",
        endpoint = ctx.endpoint().as_str().trim_end_matches('/'),
        path = presigned_session_path(kind, resource),
        token = encoded_url_value(token),
    );
    for (field, value) in extra {
        url.push_str(&format!(
            "&{}={}",
            encoded_url_value(field),
            encoded_url_value(value)
        ));
    }
    url
}

/// A fresh authorization token for a presigned session.
///
/// The token is presented back to this emulator at the session address and matched
/// against the grant the minting call recorded, so it is a SECRET handed to a caller —
/// and nothing in a service may construct a secret itself. It is therefore minted by the
/// one component that generates secrets, the emulator's credential store, and removed
/// from that store again: the caller gets the secret, the emulator keeps no credential its
/// SigV4 path would accept, and nothing accumulates per call.
///
/// `None` when the store minted no session-token secret, or when the minted credential
/// could not be removed from the store again, which leaves the operation to report
/// nothing rather than hand out a token it made up or one the emulator would still accept.
/// (`CredentialStore::delete` answers `NotFound` for an id the store does not hold, which
/// for an id it has just minted means the credential is not where this function put it.)
pub fn mint_session_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    store.delete(&minted.id).ok()?;
    token
}

/// Record one presigned-URL grant on a resource's grant list and answer it: the opaque
/// token the URL carries, the `url_valid_for` window the URL itself is good for, and the
/// session `session_duration_seconds` opens once the URL is followed.
///
/// A grant whose session the clock has already ended is released first — it was over at
/// its stored instant whether or not anybody called — and the history is bounded by
/// [`GRANT_HISTORY_CAP`], so a caller asking for URL after URL cannot grow the store
/// without end.
///
/// One function for every resource that publishes a presigned session (a notebook
/// instance, a partner AI app, an MLflow App), so the three cannot drift on which grants
/// a resource keeps or on how long a URL stays valid.
pub fn record_presigned_grant(
    grants: &mut Vec<PresignedUrlGrant>,
    token: String,
    now: SystemTime,
    url_valid_for: Duration,
    session_duration_seconds: i32,
) -> PresignedUrlGrant {
    let grant = PresignedUrlGrant {
        token,
        issued_at: now,
        expires_at: now + url_valid_for,
        session_expires_at: now
            + Duration::from_secs(u64::try_from(session_duration_seconds).unwrap_or_default()),
    };
    grants.retain(|held| held.session_expires_at > now);
    grants.push(grant.clone());
    if grants.len() > GRANT_HISTORY_CAP {
        let overflow = grants.len() - GRANT_HISTORY_CAP;
        grants.drain(..overflow);
    }
    grant
}

/// What a fetch of a presigned-session address resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresignedSession {
    /// The grant is live: the session is the subject's own, and it authenticates until
    /// `session_expires_at`.
    Open {
        /// The ARN of the resource whose session this is, as its record holds it.
        subject_arn: String,
        /// When the session the minting call opened stops authenticating.
        session_expires_at: SystemTime,
    },
    /// A grant this subject holds whose window the clock has closed: the URL was minted,
    /// and it no longer authenticates.
    Expired,
    /// The subject serves no session at all any more (a notebook instance that has been
    /// stopped, an app that is no longer available), so there is nothing to open.
    NotServing,
    /// No grant of this subject under that token — including no such subject.
    Unknown,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The subject of a presigned-session address: the ARN it is held under, the grants it
    /// holds, and whether it still serves a session.
    ///
    /// One lookup per kind, so the route resolves exactly the records the minting
    /// operations recorded their grants on and holds no second copy of any of them.
    fn presigned_session_subject(
        &self,
        kind: &str,
        resource: &str,
    ) -> Option<(String, &[PresignedUrlGrant], bool)> {
        if kind == NOTEBOOK_INSTANCE_ARN_KIND {
            let notebook = self.notebook_instances.get(resource)?;
            // The URL connects its holder to the Jupyter server ON the instance, so an
            // instance that is no longer serving one (stopped after the URL was minted)
            // has no session to open.
            let serving = notebook.status == STATUS_IN_SERVICE;
            return Some((notebook.arn.clone(), &notebook.url_grants, serving));
        }
        if kind == PARTNER_APP_ARN_KIND {
            let app = self.partner_apps.get(resource)?;
            let serving = app.status == PARTNER_APP_STATUS_AVAILABLE;
            return Some((app.arn.clone(), &app.grants, serving));
        }
        if kind == MLFLOW_APP_ARN_KIND {
            // An MLflow App is keyed by its ARN, and the address carries the opaque id
            // that ARN's resource field holds — the app's name is not part of its ARN.
            let app = self.mlflow_apps.values().find(|app| {
                ::service_common::arn::resource_id_of_type(&app.arn, MLFLOW_APP_ARN_KIND)
                    == Some(resource)
            })?;
            // `CreatePresignedMlflowAppUrl` mints a URL for an app in any status — the
            // managed UI is reached while the app is still coming up — so the session is
            // served for as long as the app is held.
            return Some((app.arn.clone(), &app.grants, true));
        }
        None
    }

    /// What the session address of `kind`/`resource` presented with `token` resolves to at
    /// `now`.
    ///
    /// A pure READ: resolving a URL opens no record and moves no status, so what a session
    /// fetch reports and what the control plane reports cannot disagree. Both windows the
    /// service documents are honoured — the URL's own validity (the window in which it
    /// must be used to sign in) and the session the request asked for — so a URL stops
    /// authenticating once either has elapsed, whether or not anybody fetched it.
    pub fn presigned_session(
        &self,
        kind: &str,
        resource: &str,
        token: &str,
        now: SystemTime,
    ) -> PresignedSession {
        let Some((subject_arn, grants, serving)) = self.presigned_session_subject(kind, resource)
        else {
            return PresignedSession::Unknown;
        };
        let Some(grant) = grants.iter().find(|held| held.token == token) else {
            return PresignedSession::Unknown;
        };
        if now >= grant.expires_at || now >= grant.session_expires_at {
            return PresignedSession::Expired;
        }
        if !serving {
            return PresignedSession::NotServing;
        }
        PresignedSession::Open {
            subject_arn,
            session_expires_at: grant.session_expires_at,
        }
    }
}
