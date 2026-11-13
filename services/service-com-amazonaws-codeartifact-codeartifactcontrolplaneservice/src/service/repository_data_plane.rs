// SPDX-License-Identifier: CC0-1.0

//! What every repository ENDPOINT this service publishes has in common: how a request
//! ADDRESSES one, which credential authenticates it, what the repository's own policy says
//! about it, and how an answer is rendered.
//!
//! `GetRepositoryEndpoint` publishes one endpoint per `PackageFormat`, and each of them
//! speaks the native protocol of that format — an npm registry
//! ([`crate::service::npm_registry`]), a PyPI index and upload API
//! ([`crate::service::pypi_registry`]). The PROTOCOLS differ; everything around them does
//! not:
//!
//! * the three addressings a client reaches an endpoint through ([`resolve`]) — the
//!   domain's published host carried in the PATH, which is the address this service
//!   ANNOUNCES ([`announced_repository_endpoint`]) because it is the one a client on this
//!   machine can dial; that same host as the request's HOST, for a client whose DNS or
//!   proxy for `<domain>-<account>.d.codeartifact.<region>.<suffix>` points here; and the
//!   bare `/<format>/<repository>/` path on the emulator's own endpoint;
//! * the credential: a bearer (or HTTP Basic) presentation of the token
//!   `GetAuthorizationToken` minted, resolved against the tokens this service remembers
//!   minting ([`authenticated_principal`]) — a token this service did not mint, one the
//!   clock has expired, one scoped to another domain, and no token at all are each `401`;
//! * the repository's own resource policy, whose explicit `Deny` refuses whatever the
//!   request asked to do there ([`policy_refusal`]); and
//! * the answer itself ([`Served`], [`rendered`]).
//!
//! One module for all of it, so two formats' planes cannot resolve a request to different
//! repositories, authenticate it differently, or word one refusal two ways.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::serde_json::{json, Value};
use ::std::time::SystemTime;

use ::service_common::data_plane_addressing::{first_segment, host_only, names_a_service};

use crate::state::{
    service_state, ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState as CaState,
};

/// The infix every published repository-endpoint host carries between the domain label and
/// the region: `<domain>-<account>.d.codeartifact.<region>.<suffix>` (AWS CodeArtifact user
/// guide). Reading it is what distinguishes a repository endpoint's host from this
/// service's own API endpoint.
const ENDPOINT_HOST_INFIX: &str = "d.codeartifact.";

/// How many digits an account id carries (`AccountId` is modelled `^[0-9]{12}$`), which is
/// what makes the domain label of an endpoint host splittable: everything before the last
/// `-` is the domain name, and those digits are the owning account.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The media type a JSON document — a packument, a PEP 691 index, every error body — is
/// served with.
pub const JSON_MEDIA_TYPE: &str = "application/json";

/// The media type an asset's own bytes carry: opaque content, which is what a package
/// manager fetches a tarball, a wheel or a source distribution as.
pub const ASSET_MEDIA_TYPE: &str = "application/octet-stream";

// ---------------------------------------------------------------------------
// Addressing
// ---------------------------------------------------------------------------

/// What a data-plane request ADDRESSES, as the wire carries it: the host it was sent to —
/// absent on a request that carries neither a `Host` header nor an absolute URI — and its
/// path.
///
/// Built from BOTH shapes the router hands a plane, the buffered parts the claim reads and
/// the runtime-API request the serving seam reads, so one rule resolves both and the claim
/// cannot resolve a different repository than the answer does.
#[derive(Debug)]
pub struct Addressed<'a> {
    pub host: Option<&'a str>,
    pub path: &'a str,
}

impl<'a> Addressed<'a> {
    /// The addressing of the buffered request these parts came from.
    pub fn of_parts(parts: &'a ::http::request::Parts) -> Self {
        Self {
            host: parts
                .headers
                .get("host")
                .and_then(|host| host.to_str().ok())
                .or_else(|| parts.uri.authority().map(::http::uri::Authority::as_str)),
            path: parts.uri.path(),
        }
    }

    /// The addressing of a runtime-API request, whose URI arrives as a string and is parsed
    /// by the `http` crate rather than split by hand.
    pub fn of_request(req: &'a RtRequest, uri: &'a ::http::Uri) -> Self {
        Self {
            host: req
                .headers()
                .get("host")
                .or_else(|| uri.authority().map(::http::uri::Authority::as_str)),
            path: uri.path(),
        }
    }
}

/// What a request resolves to: the repository whose endpoint it addresses, and the path
/// WITHIN that endpoint — the part the format's own protocol is read from.
#[derive(Debug)]
pub struct Resolved {
    pub domain: String,
    pub domain_owner: String,
    pub repository: String,
    /// The `/<format>/<repository>` prefix the request addressed, kept so a URL a plane
    /// publishes back (a tarball's, a distribution file's) is under the address the client
    /// actually reached.
    pub endpoint_prefix: String,
    /// The path within the endpoint, leading `/` kept.
    pub path: String,
}

/// Which repository endpoint `addressed` addresses for the package format `format`, or
/// `None` when no endpoint of that format serves the request at all.
///
/// The three addressings are the module documentation's, in that order. Each resolves only
/// to a repository this service HOLDS, read from the one shared store, so a request keeps
/// one claimant.
pub fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    addressed: &Addressed<'_>,
    format: &str,
) -> Option<Resolved> {
    // The first segment is the published endpoint host when the request CARRIES that host
    // in the path, and the format segment otherwise; the endpoint's own path is whatever
    // follows it.
    let (first, rest) = first_segment(addressed.path)?;
    let named_in_path = endpoint_host_domain(first);
    let endpoint_path = match named_in_path {
        Some(_) => rest,
        None => addressed.path,
    };
    let (addressed_format, rest) = first_segment(endpoint_path)?;
    if addressed_format != format {
        return None;
    }
    let (repository, path) = first_segment(rest)?;
    if repository.is_empty() {
        return None;
    }
    let state = service_state(engine)?;
    let store = state.lock();
    let endpoint_prefix = match named_in_path {
        Some(_) => format!("/{first}/{format}/{repository}"),
        None => format!("/{format}/{repository}"),
    };

    // The published endpoint host CARRIED IN THE PATH, which is the addressing this service
    // announces ([`announced_repository_endpoint`]): a client on this machine resolves none
    // of AWS's hosts, so it reaches the emulator's own endpoint and carries in the path what
    // it cannot carry in the host. The host names the domain and the account that owns it,
    // so the request addresses exactly that domain's repository whichever address this
    // process was reached at.
    if let Some((domain, account)) = named_in_path {
        let held = store.domain(domain, Some(account))?;
        if !held.repositories.contains_key(repository) {
            return None;
        }
        return Some(Resolved {
            domain: held.name.clone(),
            domain_owner: held.owner.clone(),
            repository: repository.to_string(),
            endpoint_prefix,
            path: path.to_string(),
        });
    }

    // HOST-STYLE: the published endpoint host names the domain and the account that owns
    // it, so the request addresses exactly that domain's repository.
    if let Some((domain, account)) = addressed
        .host
        .map(host_only)
        .and_then(endpoint_host_domain)
    {
        let held = store.domain(domain, Some(account))?;
        if !held.repositories.contains_key(repository) {
            return None;
        }
        return Some(Resolved {
            domain: held.name.clone(),
            domain_owner: held.owner.clone(),
            repository: repository.to_string(),
            endpoint_prefix,
            path: path.to_string(),
        });
    }

    // PATH-STYLE on the emulator's OWN endpoint, which is the only address a client on this
    // machine can reach: the repository name alone, read only when one domain holds a
    // repository of that name. Two would make the address ambiguous, and a request is left
    // to whatever else it might have been addressed to rather than claimed on a guess. A
    // host that names some SERVICE — this service's own API endpoint, or another service's
    // — keeps answering as that service, so a path under it is not claimed here however it
    // is spelled.
    if addressed.host.is_some_and(names_a_service) {
        return None;
    }
    let mut holding = store
        .domains
        .values()
        .filter(|domain| domain.repositories.contains_key(repository));
    let only = holding.next()?;
    if holding.next().is_some() {
        return None;
    }
    Some(Resolved {
        domain: only.name.clone(),
        domain_owner: only.owner.clone(),
        repository: repository.to_string(),
        endpoint_prefix,
        path: path.to_string(),
    })
}

/// The domain and the owning account a published repository-endpoint host names, or `None`
/// for every other host — this service's own API endpoint among them.
///
/// The published form is `<domain>-<account>.d.codeartifact.<region>.<suffix>` — the host
/// AWS itself publishes a repository endpoint at, with `amazonaws.com` for the IPv4
/// endpoint and `on.aws` for the dual-stack one ([`crate::state::ENDPOINT_TYPE_DUALSTACK`]
/// is the `EndpointType` that selects it). This emulator announces its OWN address instead
/// ([`announced_repository_endpoint`]), because that is the one a client here can dial; the
/// published form is still resolved so that an operator who points a client's DNS or proxy
/// for it at this process reaches the repository it names. The account is the last
/// `-`-separated field of the first label and the domain is everything before it. A
/// `DomainName` is
/// modelled `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$` and so may itself contain `-`, which is why
/// the split is from the RIGHT and why the account field has to be
/// [`ACCOUNT_ID_DIGITS`] digits to be read as one.
pub fn endpoint_host_domain(host: &str) -> Option<(&str, &str)> {
    let (label, rest) = host.split_once('.')?;
    let region_and_suffix = rest.strip_prefix(ENDPOINT_HOST_INFIX)?;
    if region_and_suffix.is_empty() {
        return None;
    }
    let (domain, account) = label.rsplit_once('-')?;
    let account_shaped =
        account.len() == ACCOUNT_ID_DIGITS && account.bytes().all(|byte| byte.is_ascii_digit());
    (!domain.is_empty() && account_shaped).then_some((domain, account))
}

// ---------------------------------------------------------------------------
// The address this service ANNOUNCES
// ---------------------------------------------------------------------------

/// The URL `GetRepositoryEndpoint` ANNOUNCES for the `format` endpoint of `repository` in
/// the domain `domain` owned by `domain_owner`, read in `region` for the `EndpointType`
/// `endpoint_type`.
///
/// A client is handed this URL to point a package manager at, so its authority has to be one
/// THIS responder answers on: AWS's own published host
/// (`<domain>-<account>.d.codeartifact.<region>.<suffix>`,
/// [`crate::state::published_endpoint_host`]) resolves away from this process — to public
/// AWS, where the domain and the repository do not exist, or, with no egress, to nothing at
/// all. So the authority is the endpoint this emulator is reached at
/// ([`announced_base`]), and the published host is carried in the PATH ahead of the
/// `/<format>/<repository>/` segments AWS documents
/// ([`crate::state::repository_endpoint_path`]) — which is the first addressing [`resolve`]
/// serves. Every part AWS varies is therefore still in the URL, and the address is a
/// client's to dial.
pub fn announced_repository_endpoint(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    domain: &str,
    domain_owner: &str,
    endpoint_type: &str,
    format: &str,
    repository: &str,
) -> String {
    format!(
        "{}/{}{}",
        announced_base(ctx),
        crate::state::published_endpoint_host(region, domain, domain_owner, endpoint_type),
        crate::state::repository_endpoint_path(format, repository)
    )
}

/// The base URL — `scheme://host[:port]`, no path — an announced endpoint is rooted on:
/// the endpoint serving THIS request.
///
/// [`::smithy_server_runtime::OperationContext::endpoint`] is "the base URL a later request
/// addresses… Callers send normally signed requests to it", so it is the authoritative
/// answer and the one this falls back on. Ahead of it comes the emulator's own endpoint the
/// request REALLY reached, read through the tree's one reading of that question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`): a client configured
/// with one local endpoint URL resolves nothing else, so it is handed back the very address
/// it is already talking to, port included. That reading answers `None` for a host that
/// names a service — this service's own API endpoint and every published repository-endpoint
/// host included — and such a request is answered the address this process is served at.
fn announced_base(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    let endpoint = ctx.endpoint();
    // Without a trailing slash, which would double the separator in front of the endpoint
    // path appended to it.
    let base = endpoint.as_str().trim_end_matches('/');
    // The base's own authority, read through the same shared reading rather than a second
    // split of a URL here; the scheme is whatever precedes it. `EndpointUrl` admits only an
    // absolute `https` URL, so the default stands in for nothing the core reports.
    let served = ::service_common::data_plane_addressing::url_authority(base);
    let scheme = match base.strip_suffix(served) {
        Some(scheme) if !scheme.is_empty() => scheme,
        _ => "https://",
    };
    let authority = ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &ctx.parts().uri.to_string(),
        ctx.parts()
            .headers
            .get("host")
            .and_then(|host| host.to_str().ok()),
    )
    .or_else(|| ::service_common::data_plane_addressing::served_endpoint_authority(base))
    .unwrap_or_else(|| served.to_string());
    format!("{scheme}{authority}")
}

/// The request URI of a runtime-API request, parsed by the `http` crate.
///
/// A URI this parse rejects is one the server would not have accepted, and it resolves to
/// `http::Uri`'s own default (`/`), which names no repository and claims nothing.
pub fn parsed_uri(uri: &str) -> ::http::Uri {
    uri.parse::<::http::Uri>().unwrap_or_default()
}

/// The request method, upper-cased as the wire carries it.
pub fn method_of(req: &RtRequest) -> String {
    req.method().to_ascii_uppercase()
}

/// The absolute URL of the endpoint this request reached, so a file URL a plane publishes
/// is one the SAME client can fetch: the host it addressed and the endpoint path it used.
///
/// The host is read exactly as the claim read it ([`Addressed::of_request`]), so a request
/// that resolved through the URI authority publishes the authority it arrived on. `https`
/// because that is the scheme every endpoint of this emulator is served on, and the scheme
/// the published endpoint URL names.
pub fn endpoint_url(addressed: &Addressed<'_>, resolved: &Resolved) -> String {
    let host = addressed.host.map(host_with_port).unwrap_or_default();
    format!("https://{host}{}", resolved.endpoint_prefix)
}

/// A `Host` header value with its port kept (the port is part of the address a client on
/// this machine reaches) and its userinfo removed.
fn host_with_port(host: &str) -> &str {
    host.rsplit('@').next().unwrap_or(host)
}

/// One path segment with its percent-escapes resolved, or `None` when they do not decode to
/// UTF-8 — a segment that addresses no name this service could hold.
pub fn percent_decoded(segment: &str) -> Option<String> {
    ::percent_encoding::percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(::std::borrow::Cow::into_owned)
}

// ---------------------------------------------------------------------------
// The credential
// ---------------------------------------------------------------------------

/// The authentication challenge a plane whose clients present the token as a BEARER
/// credential states (npm's `//<host>/<path>:_authToken=<token>`).
pub const BEARER_CHALLENGE: &str = "Bearer";

/// The challenge a plane whose clients present the token as the PASSWORD of an HTTP Basic
/// credential states (pip and twine, with `aws` as the user name).
pub const BASIC_CHALLENGE: &str = "Basic";

/// What a request PRESENTS as its credential, which is what separates a client that sent
/// none from one whose credential this plane will not accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presented {
    /// No `Authorization` header at all.
    Absent,
    /// A header carrying a value this plane reads as a presented token.
    Token(String),
    /// A header this plane can read no token out of: another authentication scheme, or a
    /// presentation of the scheme it does accept that carries nothing.
    Unreadable,
}

/// What an `Authorization` header presents to a plane whose clients carry the token as a
/// BEARER credential.
pub fn presented_bearer(authorization: Option<&str>) -> Presented {
    match authorization {
        None => Presented::Absent,
        Some(header) => match bearer_token(Some(header)) {
            Some(token) => Presented::Token(token.to_string()),
            None => Presented::Unreadable,
        },
    }
}

/// What an `Authorization` header presents to a plane whose clients carry the token as the
/// PASSWORD of an HTTP Basic credential — pip and twine — which also accept the bearer
/// presentation, so both are read here.
pub fn presented_basic_or_bearer(authorization: Option<&str>) -> Presented {
    match authorization {
        None => Presented::Absent,
        Some(header) => {
            let presented = basic_password(header)
                .or_else(|| bearer_token(Some(header)).map(str::to_string));
            match presented {
                Some(token) => Presented::Token(token),
                None => Presented::Unreadable,
            }
        }
    }
}

/// The bearer credential an `Authorization` header presents, or `None` when it presents
/// none — including a header carrying another scheme, which is not a token this plane
/// resolves.
pub fn bearer_token(authorization: Option<&str>) -> Option<&str> {
    let (scheme, token) = authorization?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// The PASSWORD an HTTP Basic `Authorization` header presents, which is where a pip or
/// twine client carries the authorization token ("the password is the authorization token"),
/// or `None` for a header that is not one.
///
/// The user name is not what authenticates — the token is, and CodeArtifact's own
/// documentation has a client send the fixed user `aws` beside it — so any user name is read
/// past; what has to be there is a password.
pub fn basic_password(authorization: &str) -> Option<String> {
    use ::base64::Engine as _;

    let (scheme, encoded) = authorization.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = ::base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let credential = String::from_utf8(decoded).ok()?;
    let (_user, password) = credential.split_once(':')?;
    (!password.is_empty()).then(|| password.to_string())
}

/// The principal the credential this request presented speaks for, or the `401` it earns.
///
/// The token is domain-scoped — "The name of the domain that is in scope for the generated
/// authorization token" — so it authenticates every repository of that domain and no
/// repository outside it. The principal is carried out of the RECORD the mint wrote,
/// because what that principal may do here is what this repository's policy states
/// ([`policy_refusal`]).
///
/// `challenge` is the scheme the PLANE states on a refusal, which is the one its clients
/// re-present a refreshed token under.
///
/// A READ of the token store: a token past its expiry is simply not resolved, so presenting
/// one twice answers the same way both times.
pub fn authenticated_principal(
    store: &CaState,
    resolved: &Resolved,
    presented: &Presented,
    challenge: &str,
    now: SystemTime,
) -> Result<String, Served> {
    let token = match presented {
        Presented::Absent => return Err(unauthenticated_missing_header(challenge)),
        Presented::Unreadable => return Err(unauthenticated_invalid_credential(challenge)),
        Presented::Token(token) => token,
    };
    store
        .authorization_token(token, now)
        .filter(|held| held.domain == resolved.domain && held.domain_owner == resolved.domain_owner)
        .map(|held| held.principal.clone())
        .ok_or_else(|| unauthenticated_invalid_credential(challenge))
}

// ---------------------------------------------------------------------------
// Authorization
// ---------------------------------------------------------------------------

/// The IAM action a READ of a repository's packages asks for, as the IAM Service
/// Authorization Reference names it for CodeArtifact: the action a repository policy grants
/// a package manager that resolves and downloads from the repository.
pub const READ_ACTION: &str = "codeartifact:ReadFromRepository";

/// The IAM action a PUBLISH into a repository asks for, named the same way.
pub const PUBLISH_ACTION: &str = "codeartifact:PublishPackageVersion";

/// The refusal a repository's own resource policy states for `principal` performing
/// `action` on it, or `None` when the policy does not refuse it.
///
/// Only an explicit `Deny` refuses here. That is IAM's evaluation order within one account:
/// a deny in the resource policy wins over anything an identity policy allows, while a
/// resource policy that merely states nothing withholds no access — the caller's identity
/// policies may still permit it, and a data plane holds no session to read them for: it
/// authenticates a BEARER credential, whose principal is the one recorded at the mint, not a
/// signing session ([`crate::service::authorization`] is the decision for a control-plane
/// call, which does have one). A repository carrying no policy refuses nothing.
pub fn policy_refusal(
    store: &CaState,
    resolved: &Resolved,
    principal: &str,
    action: &str,
) -> Option<Served> {
    use ::service_common::iam_authorization as iam;

    let repository = store
        .domain(&resolved.domain, Some(&resolved.domain_owner))?
        .repositories
        .get(&resolved.repository)?;
    let document = repository.policy.as_ref()?.document.clone()?;
    // The spellings a statement's `Principal` element may name this caller by: the
    // principal itself and the account forms that stand for it.
    let mut principals = vec![principal.to_string()];
    principals.extend(iam::account_principals(principal));
    let request = iam::Request::new(action, &repository.arn, &principals);
    if iam::resource_decision(&document, &request) != iam::Decision::Denied {
        return None;
    }
    Some(Served::json(
        403,
        &json!({
            "error": iam::not_authorized_by_resource_policy(principal, action, &repository.arn)
        }),
    ))
}

/// Whether a version in `status` serves its assets.
///
/// `Published` and `Unlisted` are the two statuses in which a version's assets are
/// downloadable ("an unlisted version … can still be downloaded"), and every other status
/// withholds them — an `Archived` version's content is retained but its download is
/// blocked, and a `Disposed` one's is gone.
pub fn downloadable(status: &str) -> bool {
    status == crate::state::PACKAGE_VERSION_STATUS_PUBLISHED
        || status == crate::state::PACKAGE_VERSION_STATUS_UNLISTED
}

// ---------------------------------------------------------------------------
// The answers
// ---------------------------------------------------------------------------

/// One answer a plane renders: the status, the media type it carries, the body, and the
/// challenge an authentication refusal states.
#[derive(Debug, Default)]
pub struct Served {
    pub status: u16,
    pub content_type: Option<String>,
    pub authenticate: Option<String>,
    pub body: Vec<u8>,
}

impl Served {
    /// An answer carrying `document` as JSON.
    pub fn json(status: u16, document: &Value) -> Self {
        Self {
            status,
            content_type: Some(JSON_MEDIA_TYPE.to_string()),
            authenticate: None,
            body: ::serde_json::to_vec(document).unwrap_or_default(),
        }
    }

    /// An answer carrying opaque asset bytes.
    pub fn bytes(content: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: Some(ASSET_MEDIA_TYPE.to_string()),
            authenticate: None,
            body: content,
        }
    }

    /// An answer carrying `text` under the media type `content_type` — a document whose own
    /// format is the protocol's, such as a PEP 503 index page.
    pub fn text(status: u16, content_type: &str, text: String) -> Self {
        Self {
            status,
            content_type: Some(content_type.to_string()),
            authenticate: None,
            body: text.into_bytes(),
        }
    }

    /// An answer that carries no body at all, which is what an upload accepted by a plane
    /// whose protocol reports only the status answers with.
    pub fn status_only(status: u16) -> Self {
        Self {
            status,
            content_type: None,
            authenticate: None,
            body: Vec::new(),
        }
    }
}

/// The refusal a package manager reads as "this registry does not have that": the status,
/// with the `error` member such a client renders. The sentence states what was not found,
/// in the same words this service's control plane states it in, so one client reads one
/// wording.
pub fn not_found(reason: String) -> Served {
    Served::json(404, &json!({ "error": reason }))
}

/// The refusal a request presenting NO credential at all earns.
///
/// `401` with the challenge, which is what tells a package manager to present (or refresh)
/// a token rather than to retry the same request, and with the `success`/`error` envelope a
/// repository endpoint words its refusals in — `success: false` is what a client branches on
/// to decide whether to log in again. The sentence names the ABSENT header, which is a
/// different failure from a credential that was presented and not accepted, so the two are
/// distinguishable without comparing anything but the body.
pub fn unauthenticated_missing_header(challenge: &str) -> Served {
    unauthenticated(
        challenge,
        "The request did not include an Authorization header.",
    )
}

/// The refusal a request presenting a credential this plane will not accept earns: a token
/// this service did not mint, one the clock has expired, one scoped to another domain, or a
/// header carrying no token at all. The sentence names the CREDENTIAL, never quoting the
/// value presented.
pub fn unauthenticated_invalid_credential(challenge: &str) -> Served {
    unauthenticated(challenge, "The request used invalid credentials.")
}

/// The `401` both refusals above render: the envelope, the sentence, and the challenge.
fn unauthenticated(challenge: &str, reason: &str) -> Served {
    let mut served = Served::json(401, &json!({ "success": false, "error": reason }));
    served.authenticate = Some(challenge.to_string());
    served
}

/// The refusal a request the plane understood but will not perform earns: a publish origin
/// control blocks, or a repository policy denies.
pub fn forbidden(reason: String) -> Served {
    Served::json(403, &json!({ "error": reason }))
}

/// The refusal a download of a version whose STATUS withholds its content earns.
///
/// A client asked for a version this repository holds and will not serve, which is a
/// request it cannot make rather than a resource that is missing: the version is listed by
/// the control plane, its assets are not downloadable, and a `404` would say it does not
/// exist. The sentence names the status, so a client can tell a blocked download from a
/// missing one.
pub fn blocked_download(reason: String) -> Served {
    Served::json(400, &json!({ "error": reason }))
}

/// The refusal a document the plane cannot read earns.
pub fn bad_request(reason: String) -> Served {
    Served::json(400, &json!({ "error": reason }))
}

/// The refusal a publish into a package whose origin configuration BLOCKS new versions
/// earns: `400`, with the `success`/`error` envelope a repository endpoint words its
/// refusals in and the sentence CodeArtifact states for that control.
///
/// The sentence states the CONTROL and interpolates nothing — it is the same for whatever
/// package was addressed — and the status is `400` rather than the `403` an authorization
/// refusal carries: the credential was accepted and the repository reachable; what the
/// request asked for is a version this package does not accept. One wording for every
/// plane, because it is one control ([`crate::state::Package::publish_allowed`]).
pub fn publish_blocked() -> Served {
    Served::json(
        400,
        &json!({
            "success": false,
            "error": "This package is configured to block new versions from being published."
        }),
    )
}

// ---------------------------------------------------------------------------
// What a plane records about what it did
// ---------------------------------------------------------------------------

/// The coordinate a package is addressed and stored by: its `PackageFormat` and the
/// `(namespace, name)` pair inside it — the triple [`crate::state::package_key`] keys a
/// package under.
///
/// Passed as one value because it IS one: a plane that split them could pass a format from
/// one request beside a name from another.
#[derive(Debug, Clone, Copy)]
pub struct Coordinate<'a> {
    pub format: &'a str,
    /// Absent for a format whose packages carry no namespace component.
    pub namespace: Option<&'a str>,
    pub name: &'a str,
}

/// Record the state change each version in `versions` just underwent in the addressed
/// repository: it came to exist there, whether by a publish into it or by a retention from
/// an upstream.
///
/// Read back off the STORED record, so the revision, status and sequence number the event
/// announces are the ones a later `DescribePackageVersion` reports for the same version.
/// A version the store does not hold contributes nothing: there is no change to announce.
///
/// Shared by every plane, so a version that came to exist over a pypi endpoint is announced
/// exactly as one that came to exist over an npm endpoint is.
pub fn record_created(
    store: &mut CaState,
    resolved: &Resolved,
    coordinate: &Coordinate<'_>,
    versions: &[String],
    now: SystemTime,
) {
    use crate::state::package_version_events::{VersionChange, VersionChanges, OPERATION_CREATED};

    let mut changes: Vec<VersionChange> = Vec::new();
    if let Some(repository) = store
        .domain(&resolved.domain, Some(&resolved.domain_owner))
        .and_then(|domain| domain.repositories.get(&resolved.repository))
    {
        if let Some(held) = repository.package(
            coordinate.format,
            coordinate.namespace,
            coordinate.name,
        ) {
            for version in versions {
                if let Some(stored) = held.versions.get(version) {
                    changes.push(VersionChange::of(
                        repository,
                        held,
                        stored,
                        OPERATION_CREATED,
                        VersionChanges::published(stored.assets.len(), stored.manifest.is_some()),
                        now,
                    ));
                }
            }
        }
    }
    for change in changes {
        store.record_version_change(change);
    }
}

/// One rendered answer: the status, its headers and its body, as the wire carries them.
pub fn rendered(served: &Served) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(served.status).unwrap_or_else(|_| {
            StatusCode::try_from(500).expect("500 is a status")
        }),
        SdkBody::from(served.body.clone()),
    );
    if let Some(content_type) = &served.content_type {
        if let Err(refused) = response
            .headers_mut()
            .try_insert("content-type", content_type.clone())
        {
            ::tracing::warn!(
                error = %refused,
                "the media type cannot be sent on the wire and is left off the response"
            );
        }
    }
    if let Some(challenge) = &served.authenticate {
        if let Err(refused) = response
            .headers_mut()
            .try_insert("www-authenticate", challenge.clone())
        {
            ::tracing::warn!(
                error = %refused,
                "the authentication challenge cannot be sent on the wire and is left off the \
                 response"
            );
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the endpoint-host table: the case, the host, and the `(domain, account)`
    /// it names.
    type HostRow = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str)>,
    );

    /// RULE: a published repository-endpoint host names the domain and the owning account;
    /// every other host names none. The rows span what the reading generalises over: the
    /// IPv4 and dual-stack suffixes, a domain name that itself carries `-`, and the hosts
    /// this service's own API is reached at.
    #[test]
    fn an_endpoint_host_names_the_domain_and_the_account_that_owns_it() {
        let rows: &[HostRow] = &[
            (
                "the published IPv4 endpoint",
                "acme-000000000000.d.codeartifact.us-west-2.amazonaws.com",
                Some(("acme", "000000000000")),
            ),
            (
                "the dual-stack endpoint",
                "acme-000000000000.d.codeartifact.us-west-2.on.aws",
                Some(("acme", "000000000000")),
            ),
            (
                "a domain name carrying a hyphen",
                "sim2real-ca-repo-123456789012.d.codeartifact.eu-west-1.amazonaws.com",
                Some(("sim2real-ca-repo", "123456789012")),
            ),
            (
                "this service's own API endpoint",
                "codeartifact.us-east-1.amazonaws.com",
                None,
            ),
            ("the emulator's own address", "127.0.0.1", None),
            (
                "a label whose last field is not an account id",
                "acme-notanaccount.d.codeartifact.us-east-1.amazonaws.com",
                None,
            ),
            (
                "another service's endpoint",
                "acme-000000000000.s3.us-east-1.amazonaws.com",
                None,
            ),
        ];
        for (case, host, expected) in rows {
            let named = endpoint_host_domain(host);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }

    /// RULE: a version's status decides whether its assets are downloadable — `Published`
    /// and `Unlisted` serve them, every other status withholds them.
    #[test]
    fn only_a_published_or_unlisted_version_serves_its_assets() {
        let rows: &[(&str, bool)] = &[
            (crate::state::PACKAGE_VERSION_STATUS_PUBLISHED, true),
            (crate::state::PACKAGE_VERSION_STATUS_UNLISTED, true),
            (crate::state::PACKAGE_VERSION_STATUS_ARCHIVED, false),
            (crate::state::PACKAGE_VERSION_STATUS_DISPOSED, false),
            (crate::state::PACKAGE_VERSION_STATUS_UNFINISHED, false),
        ];
        for (status, expected) in rows {
            let reached = downloadable(status);
            assert_eq!(
                reached, *expected,
                "case {status:?}: expected downloadable={expected}, got {reached}"
            );
        }
    }

    /// RULE: a header presents a token only under the scheme the plane reads — the bearer
    /// presentation for npm's clients, the HTTP Basic password for pip's — and a request
    /// that sent no header at all is a different presentation from one this plane cannot
    /// read.
    #[test]
    fn a_header_presents_a_token_only_under_the_scheme_the_plane_reads() {
        use ::base64::Engine as _;

        let basic = format!(
            "Basic {}",
            ::base64::engine::general_purpose::STANDARD.encode("aws:abc.def")
        );
        // (case, the header, what a bearer plane reads, what a basic plane reads)
        let rows: &[(&str, Option<&str>, Presented, Presented)] = &[
            (
                "a bearer token",
                Some("Bearer abc.def"),
                Presented::Token("abc.def".to_string()),
                Presented::Token("abc.def".to_string()),
            ),
            (
                "the scheme in another case",
                Some("bearer abc"),
                Presented::Token("abc".to_string()),
                Presented::Token("abc".to_string()),
            ),
            (
                "the token as a basic password",
                Some(basic.as_str()),
                Presented::Unreadable,
                Presented::Token("abc.def".to_string()),
            ),
            (
                "a SigV4 signature",
                Some("AWS4-HMAC-SHA256 Credential=x"),
                Presented::Unreadable,
                Presented::Unreadable,
            ),
            (
                "an empty bearer token",
                Some("Bearer  "),
                Presented::Unreadable,
                Presented::Unreadable,
            ),
            (
                "no header at all",
                None,
                Presented::Absent,
                Presented::Absent,
            ),
        ];
        for (case, header, bearer, basic_or_bearer) in rows {
            let read = presented_bearer(*header);
            assert_eq!(
                read, *bearer,
                "case {case:?}: a bearer plane expected {bearer:?}, got {read:?}"
            );
            let read = presented_basic_or_bearer(*header);
            assert_eq!(
                read, *basic_or_bearer,
                "case {case:?}: a basic plane expected {basic_or_bearer:?}, got {read:?}"
            );
        }
    }

    /// RULE: the two `401`s a plane states are told apart by their sentences, and both
    /// carry the `success: false` envelope a package manager branches on.
    #[test]
    fn the_two_unauthenticated_refusals_state_different_sentences() {
        let missing = unauthenticated_missing_header(BEARER_CHALLENGE);
        let invalid = unauthenticated_invalid_credential(BASIC_CHALLENGE);
        for (case, served) in [("the absent header", &missing), ("the credential", &invalid)] {
            assert_eq!(served.status, 401, "case {case:?}: expected HTTP 401");
            let body: Value = ::serde_json::from_slice(&served.body).unwrap_or(Value::Null);
            assert_eq!(
                body.get("success"),
                Some(&Value::Bool(false)),
                "case {case:?}: expected success=false, got {body:?}"
            );
            assert!(
                body.get("error")
                    .and_then(Value::as_str)
                    .is_some_and(|sentence| !sentence.is_empty()),
                "case {case:?}: expected an error sentence, got {body:?}"
            );
        }
        let sentence = |served: &Served| {
            ::serde_json::from_slice::<Value>(&served.body)
                .ok()
                .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default()
        };
        assert_ne!(
            sentence(&missing),
            sentence(&invalid),
            "case \"the two refusals\": expected different sentences"
        );
    }
}
