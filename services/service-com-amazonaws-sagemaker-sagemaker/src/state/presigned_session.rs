// SPDX-License-Identifier: CC0-1.0

//! The PRESIGNED-SESSION slice of this service's state: where a URL this service hands a
//! caller is ADDRESSED, the token it carries, and what a fetch of that address resolves
//! to.
//!
//! Five operations answer a URL whose whole point is that the caller FETCHES it later —
//! `CreatePresignedNotebookInstanceUrl`, `CreatePartnerAppPresignedUrl`,
//! `CreatePresignedMlflowAppUrl`, `CreatePresignedMlflowTrackingServerUrl` and
//! `CreatePresignedDomainUrl`. Such a URL
//! has to lead back to THIS emulator: a host of
//! AWS's own (`<name>.notebook.<region>.sagemaker.aws`) resolves to nothing a client of
//! the emulator can reach, so every flow the operation exists for fails with a name or
//! network error instead of opening a session. So the address is the endpoint the request
//! arrived on ([`::smithy_server_runtime::OperationContext::endpoint`]) plus the path this
//! module lays out, under the reserved namespace every emulator-served data plane is
//! addressed in (`::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`), and
//! `crate::service::presigned_session` SERVES it.
//!
//! Following such a URL is a SIGN-IN, so where the resource publishes a web address of its
//! own ([`ComAmazonawsSagemakerSagemakerState::published_session_address`]) the fetch is
//! handed off to it rather than answered where the URL was published.
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

/// The query field a presigned Studio-domain URL carries the landing page in, named after
/// the modelled `LandingUri` member it is taken from: "The landing page that the user is
/// directed to when accessing the presigned URL."
pub const PRESIGNED_SESSION_LANDING_URI_FIELD: &str = "landingUri";

/// The query field a presigned Studio-domain URL carries the space the session lands in,
/// named after the modelled `SpaceName` member it is taken from.
pub const PRESIGNED_SESSION_SPACE_FIELD: &str = "spaceName";

/// How long a presigned Studio-domain URL stays valid when the request names no
/// `ExpiresInSeconds`: the value that member's model doc publishes ("The number of seconds
/// until the pre-signed URL expires. This value defaults to 300."), i.e. the "default
/// timeout of 5 minutes" the operation's own doc states.
pub const DEFAULT_DOMAIN_URL_EXPIRES_IN_SECONDS: i32 = 300;

/// How long the session a presigned Studio-domain URL opens lasts when the request names no
/// `SessionExpirationDurationInSeconds`: the value that member's model doc publishes ("The
/// session expiration duration in seconds. This value defaults to 43200."), i.e. the
/// "JupyterLab session default expiration time" of 12 hours the operation's own doc states.
pub const DEFAULT_DOMAIN_URL_SESSION_SECONDS: i32 = 43_200;

/// Why a Studio domain that does not authenticate against IAM can hand out no presigned
/// domain URL: the subject form the Studio refusals name a domain with
/// ([`crate::state::domain_not_found`]), then the `AuthMode` that stands in the way, because
/// the operation "can only be called when the authentication mode equals IAM".
pub fn domain_url_needs_iam_auth_mode(domain_arn: &str, auth_mode: &str) -> String {
    format!(
        "Unable to create a presigned domain URL for Domain [{domain_arn}] because its \
         AuthMode is {auth_mode} instead of {AUTH_MODE_IAM}."
    )
}

/// Why a Studio domain the service has not finished provisioning can hand out no presigned
/// domain URL: the FIXED sentence of this error class, with the status the domain is in now as
/// its only variable part.
///
/// The URL signs its holder in to the apps and the file system of the domain, which exist only
/// once the domain has been provisioned — and still exist for a domain whose last UPDATE
/// failed, which is why the sentence names both states a sign-in is served from
/// ([`DOMAIN_STATUSES_SIGNING_IN`]) and the one the domain is in instead. Recorded AWS
/// behaviour: the message names no ARN, no Region and no account, because the caller already
/// addressed the domain.
pub fn domain_url_needs_domain_in_service(status: &str) -> String {
    format!(
        "Domain must be in either {DOMAIN_STATUS_IN_SERVICE} or \
         {DOMAIN_STATUS_UPDATE_FAILED} state and not in: {status}"
    )
}

/// The `DomainStatus` members a presigned domain URL is minted for: the provisioned domain,
/// and the one whose last update failed — which still holds the apps and the file system the
/// URL signs its holder in to. Stated once here, beside the sentence a domain in any OTHER
/// status is refused with ([`domain_url_needs_domain_in_service`]), so the gate and the
/// refusal cannot disagree about which states sign a holder in.
pub const DOMAIN_STATUSES_SIGNING_IN: [&str; 2] =
    [DOMAIN_STATUS_IN_SERVICE, DOMAIN_STATUS_UPDATE_FAILED];

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
/// request addresses"). What is the RESOURCE's own half of the address is therefore its path,
/// which is what the one endpoint this emulator serves can carry.
pub fn presigned_session_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    kind: &str,
    resource: &str,
    token: &str,
    extra: &[(&str, &str)],
) -> String {
    format!(
        "{endpoint}{path}?{query}",
        endpoint = ctx.endpoint().as_str().trim_end_matches('/'),
        path = presigned_session_path(kind, resource),
        query = session_query(token, extra),
    )
}

/// The QUERY a published session address carries: the grant token, then whatever `extra`
/// fields the minting operation's URL names, each percent-encoded.
///
/// Stated once, for the mint and for the HAND-OFF that forwards a fetch of such an address to
/// the web address its resource publishes ([`handed_off_address`]): the hand-off re-states
/// these fields rather than copying the fetch's own query, so only what this plane published
/// travels on and no other value a caller appended reaches the next address.
fn session_query(token: &str, extra: &[(&str, &str)]) -> String {
    let mut query = format!(
        "{PRESIGNED_SESSION_TOKEN_FIELD}={}",
        encoded_url_value(token)
    );
    for (field, value) in extra {
        query.push_str(&format!(
            "&{}={}",
            encoded_url_value(field),
            encoded_url_value(value)
        ));
    }
    query
}

/// The address a fetch of a session address is HANDED OFF to: the web address `url` its
/// resource publishes, carrying the grant `token` the fetch presented and the `extra` session
/// fields it carried ([`session_query`]), so the session travels on to it.
///
/// `url` is read off the resource this service STORED
/// ([`ComAmazonawsSagemakerSagemakerState::published_session_address`]), which this service
/// minted itself from the endpoint it is served on — never a value a request carried — so the
/// hand-off cannot name an address outside the one this emulator publishes.
#[must_use]
pub fn handed_off_address(url: &str, token: &str, extra: &[(&str, &str)]) -> String {
    format!(
        "{url}?{query}",
        query = session_query(token, extra)
    )
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
/// instance, a partner AI app, an MLflow App, an MLflow tracking server, a Studio user
/// profile), so they cannot drift on which grants a resource keeps or on how long a URL
/// stays valid.
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
        now: SystemTime,
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
        if kind == TRACKING_SERVER_KIND {
            // A tracking server is keyed by its `TrackingServerName`, which is also the
            // resource field of its ARN, so the address names it directly.
            let server = self.mlflow_tracking_servers.get(resource)?;
            // Every status this service moves a tracking server through still holds the
            // server, and its MLflow UI is reached through the presigned URL while it is
            // coming up as well, so the session is served for as long as the server is
            // held.
            return Some((server.arn.clone(), &server.grants, true));
        }
        if kind == USER_PROFILE_ARN_KIND {
            // A presigned domain URL signs its holder in to the domain AS one user profile,
            // so the subject is that profile: the address names it by the resource field of
            // its own ARN ([`user_profile_resource_id`]) — the pair that identifies it —
            // split back apart here.
            let (domain_id, user_profile_name) = resource.split_once('/')?;
            let profile = self.user_profile(domain_id, user_profile_name, now)?;
            // The session is the profile's own inside its domain, so neither may be on its
            // way out: a Studio resource the service is tearing down serves nothing to sign
            // in to any more.
            let serving = !profile.is_deleting()
                && self
                    .domains
                    .get(domain_id)
                    .is_some_and(|domain| !domain.is_deleting());
            return Some((profile.arn.clone(), &profile.grants, serving));
        }
        None
    }

    /// The session subject a Studio domain's SIGN-IN address resolves `token` to: the resource
    /// identifier ([`user_profile_resource_id`]) of the user profile of `domain_id` holding a
    /// grant under that token, or `None` when the domain holds no profile that does.
    ///
    /// A presigned domain URL carries the domain in its HOST and the profile in the GRANT, so
    /// WHO is being signed in is read off the grant the mint recorded rather than off the
    /// address: the token is the only thing that names the profile, which is what makes it the
    /// secret. Every grant of every profile of that domain is considered, and no grant of
    /// another domain is, so a token minted in one domain opens no session in another.
    ///
    /// A pure READ, like [`Self::presigned_session`], which is what the resolved subject is
    /// then put to.
    pub fn domain_session_subject(&self, domain_id: &str, token: &str) -> Option<String> {
        self.user_profiles
            .values()
            .find(|profile| {
                profile.domain_id == domain_id
                    && profile.grants.iter().any(|grant| grant.token == token)
            })
            .map(|profile| user_profile_resource_id(&profile.domain_id, &profile.name))
    }

    /// The web address the resource a session address names PUBLISHES for a caller to be
    /// signed in at, when that resource publishes one of its own; `None` otherwise.
    ///
    /// A Studio user profile is the one subject that does: a presigned domain URL signs its
    /// holder in to the DOMAIN's own Studio experience, which this service publishes at a host
    /// of that domain's own ([`crate::state::studio_domain_url`]) and reports as the domain's
    /// `Url`, so the address is read off the stored domain rather than composed a second time.
    /// The other subjects (a notebook instance, a partner AI app, an MLflow App, an MLflow
    /// tracking server) publish no second address, so their session is served where their URL
    /// was published.
    ///
    /// A pure READ, like [`Self::presigned_session`] beside it.
    pub fn published_session_address(&self, kind: &str, resource: &str) -> Option<String> {
        if kind != USER_PROFILE_ARN_KIND {
            return None;
        }
        let (domain_id, _user_profile_name) = resource.split_once('/')?;
        let domain = self.domains.get(domain_id)?;
        Some(domain.url.clone())
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
        let Some((subject_arn, grants, serving)) =
            self.presigned_session_subject(kind, resource, now)
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
