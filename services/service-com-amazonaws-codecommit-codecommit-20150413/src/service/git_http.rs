// SPDX-License-Identifier: CC0-1.0

//! The git smart-HTTP transport: the endpoints a `git clone`, `git fetch` and
//! `git push` against a CodeCommit repository speak.
//!
//! CodeCommit is a git host, and `RepositoryMetadata.cloneUrlHttp` is the address
//! the repository's git transport answers on. This module is that transport, bound
//! to the same `/v1/repos/<repositoryName>` path the clone URL names (see
//! [`crate::state::ServingEndpoint`], which derives that URL from the endpoint the
//! emulator is reached on), so the URL the API hands out is an address this
//! emulator actually serves:
//!
//! | request | meaning |
//! |---|---|
//! | `GET  /v1/repos/<name>/info/refs?service=git-upload-pack`  | the ref advertisement a fetch starts with |
//! | `GET  /v1/repos/<name>/info/refs?service=git-receive-pack` | the ref advertisement a push starts with |
//! | `POST /v1/repos/<name>/git-upload-pack`                    | a fetch or clone: wants in, a packfile out |
//! | `POST /v1/repos/<name>/git-receive-pack`                   | a push: commands and a packfile in, a status report out |
//!
//! The protocol spoken is version 0 (the "dumb capability" smart protocol): a
//! client that asks for version 2 is answered with a version 0 advertisement and
//! falls back, which the protocol requires it to do. Only capabilities this
//! module implements are advertised, so a client never negotiates a feature that
//! is not served.
//!
//! Both faces of the service read ONE object store. A push stores git objects in
//! [`crate::state::Repository::objects`] and projects the commits it made
//! reachable into the control plane's [`crate::state::Commit`] records, so
//! `GetBranch` and `GetFile` see pushed content; a fetch serves the objects
//! `PutFile` wrote, so a clone of API-written content resolves.
//!
//! # What this transport can and cannot reach
//!
//! Everything it serves comes from the engine's in-memory state: it opens no
//! file, runs no process and makes no outbound connection, and the repository a
//! request names must already have been created through `CreateRepository` — the
//! name is matched against the model's own `RepositoryName` pattern, so a path
//! cannot address anything but a repository record.
//!
//! Every input is bounded before it is acted on, and bounded on the PRODUCT, not
//! merely per item: the packfile's size, entry count, per-object inflated size,
//! TOTAL inflated size and read deadline ([`git_pack`]), one pkt-line's payload
//! and their count, how many refs one push may update
//! ([`MAX_REF_COMMANDS_PER_REQUEST`]), how long a ref name may be
//! ([`crate::state::MAX_REF_NAME`], applied with the rest of git's
//! `check-ref-format` rule by [`crate::state::is_valid_ref_name`], the one both
//! faces of the service use), how many branches and tags one repository keeps
//! ([`crate::state::MAX_BRANCHES_PER_REPOSITORY`],
//! [`crate::state::MAX_TAGS_PER_REPOSITORY`]), how many commits one request may
//! project, how many tree entries it may walk IN TOTAL — across every command's
//! closure check AND every projection, which share the one
//! [`crate::state::ProjectionBudget`] and its deadline, so the product of
//! commands and closure size is bounded rather than each factor — and what one
//! repository may hold
//! ([`crate::state::MAX_OBJECTS_PER_REPOSITORY`],
//! [`crate::state::MAX_OBJECT_BYTES_PER_REPOSITORY`]). No buffer is ever sized
//! from a length the client declared, and both of the long-running steps — the
//! pack read and the projection — carry a wall-clock deadline. One request
//! therefore costs a bounded amount of memory and time, and no sequence of
//! requests grows this service's state without a bound it names; that is also what
//! bounds how long a request holds the repository store's lock, which has to cover
//! a push's whole read-check-update sequence, because that atomicity is exactly
//! what makes a concurrent pusher's stale `old` value detectable.
//!
//! # Who may fetch, and who may push what
//!
//! Every request to it is made with git credentials, and the principal behind them
//! decides what it may do: a fetch is `codecommit:GitPull` and a push is
//! `codecommit:GitPush`, taken PER REFERENCE, which is the only branch protection
//! CodeCommit has. A request that presents no credential at all is answered `401`
//! with a `Basic` challenge — which is what makes a git client run its credential
//! helper and send the request again — and the credential it then presents is
//! resolved to its principal, whose IAM policies are evaluated once per
//! authorization step ([`super::git_authorization`], which also states what a
//! credential can and cannot prove here).
//!
//! It verifies no request SIGNATURE, and cannot: a git credential is not a SigV4
//! credential, and the secret half of one is held by the credential store, which
//! hands a secret back to nobody — so there is nothing here to check a password
//! against, and refusing every git request instead is exactly the defect this
//! transport fixes. That matches the emulator's posture for the data planes it
//! serves: it binds loopback by default (`EMULATOR_BIND_ADDRESS`) and holds only
//! emulated state.

use std::time::SystemTime;

use super::git_authorization::{
    GitPrincipal, GIT_PULL_ACTION, GIT_PUSH_ACTION, NO_PERMISSION_TO_PULL, NO_PERMISSION_TO_PUSH,
    NO_PERMISSION_TO_PUSH_REFERENCE,
};
use super::git_pack;
use crate::state::git_object::{self, Kind, Object};
use crate::state::{
    ComAmazonawsCodecommitCodecommit20150413State as Store, Repository, HEADS_PREFIX, TAGS_PREFIX,
};

/// The path prefix every CodeCommit git URL carries.
pub const REPOS_PREFIX: &str = "/v1/repos/";

/// The `agent` capability this server reports. A git client records it in its
/// trace output only; it selects no behaviour.
const AGENT: &str = concat!("agent=aws-emulator-codecommit/", env!("CARGO_PKG_VERSION"));

/// How many ref updates one push may carry.
///
/// The emulator's OWN bound, not a git or AWS limit: the command count is the one
/// factor a request chooses freely, and every command drives a projection whose
/// own budget would otherwise be per command rather than per request. A push
/// updates a handful of refs; `git push --all` of a repository this emulator can
/// hold stays far inside this.
const MAX_REF_COMMANDS_PER_REQUEST: usize = 256;

/// A flush packet: the end of a pkt-line stream or section.
const FLUSH: &[u8] = b"0000";

/// The challenge a request that presented NO credential is answered with, which is
/// what makes a git client ask its credential helper for one and send the request
/// again: the scheme CodeCommit's git transport authenticates under (a git client
/// presents HTTPS git credentials as HTTP Basic ones) and the realm it names.
const BASIC_CHALLENGE: &str = "Basic realm=\"CodeCommit\"";

/// What a request that presented no credential at all is told. A git client prints
/// its own message for a `401`; this is the body behind it.
const CREDENTIALS_REQUIRED: &str =
    "This repository requires the git credentials of an authorized IAM principal.";

/// Which of the two git services a request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitService {
    /// `git-upload-pack`, which serves a fetch or clone.
    Upload,
    /// `git-receive-pack`, which accepts a push.
    Receive,
}

impl GitService {
    /// The service's name, as it appears in the URL and the advertisement.
    pub fn name(self) -> &'static str {
        match self {
            Self::Upload => "git-upload-pack",
            Self::Receive => "git-receive-pack",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "git-upload-pack" => Some(Self::Upload),
            "git-receive-pack" => Some(Self::Receive),
            _ => None,
        }
    }
}

/// What a git client asked of one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// The ref advertisement for a service, or `None` when the request named no
    /// service this server speaks — which is how a client asking for the dumb
    /// HTTP protocol arrives.
    Advertisement(Option<GitService>),
    /// A fetch or clone.
    UploadPack,
    /// A push.
    ReceivePack,
}

/// One routed git transport request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub repository: String,
    pub endpoint: Endpoint,
}

/// A response to one git transport request, before it is rendered onto the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// The `WWW-Authenticate` challenge the answer carries, set only on the `401`
    /// that asks a client for credentials — RFC 9110 requires a challenge on one,
    /// and it is the challenge that makes a git client run its credential helper.
    pub www_authenticate: Option<String>,
}

impl Response {
    fn git(status: u16, content_type: String, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            body,
            www_authenticate: None,
        }
    }

    /// A plain-text refusal, the form a git client prints as the server's reason.
    pub fn text(status: u16, message: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: format!("{message}\n").into_bytes(),
            www_authenticate: None,
        }
    }

    /// The refusal of a request that presented no credential: a `401` carrying the
    /// challenge under which the client may present one.
    pub fn challenge(message: &str) -> Self {
        Self {
            www_authenticate: Some(BASIC_CHALLENGE.to_string()),
            ..Self::text(401, message)
        }
    }
}

/// The git transport request this method and URI address, or `None` when the
/// request is not one of the transport's endpoints.
///
/// This is the one place the transport's URL shape is written: the service's
/// `can_handle` claims exactly what this routes, so a request this returns `None`
/// for is never answered as git.
pub fn route(method: &str, path: &str, query: Option<&str>) -> Option<Request> {
    let (prefix, endpoint) = if let Some(prefix) = path.strip_suffix("/info/refs") {
        if method != "GET" {
            return None;
        }
        (prefix, Endpoint::Advertisement(service_query(query)))
    } else if let Some(prefix) = path.strip_suffix("/git-upload-pack") {
        if method != "POST" {
            return None;
        }
        (prefix, Endpoint::UploadPack)
    } else {
        // The last endpoint of the three, written as a `?` rather than a third
        // `else if let`: a path matching none of them is not a git request.
        let prefix = path.strip_suffix("/git-receive-pack")?;
        if method != "POST" {
            return None;
        }
        (prefix, Endpoint::ReceivePack)
    };
    let name = prefix.strip_prefix(REPOS_PREFIX)?;
    // A URL written with a trailing slash leaves one here, and a client may
    // append the conventional `.git` suffix; a CodeCommit repository name may end
    // in neither (`CreateRepository` refuses a `.git` suffix and the name pattern
    // allows no `/`), so removing them is unambiguous.
    let name = name.trim_end_matches('/');
    let name = name.strip_suffix(".git").unwrap_or(name);
    crate::state::is_repository_name(name).then(|| Request {
        repository: name.to_string(),
        endpoint,
    })
}

/// The `service` query parameter of an `info/refs` request.
fn service_query(query: Option<&str>) -> Option<GitService> {
    query?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "service").then(|| GitService::from_name(value))?
    })
}

/// The path and query of a URI that may be absolute or origin-form.
///
/// Parsed by the `http` crate, which is what the rest of this service's routing
/// reads a URI with (`can_handle` uses `parts.uri.path()`), so an absolute form
/// with no path, userinfo, or a fragment is split by the parser rather than by
/// hand. A request target the parser does not accept falls back to the raw
/// string with its query split off, so such a request is still routed by its
/// path rather than silently unclaimed.
pub fn split_uri(uri: &str) -> (String, Option<String>) {
    if let Ok(parsed) = uri.parse::<::http::Uri>() {
        return (
            parsed.path().to_string(),
            parsed.query().map(str::to_string),
        );
    }
    let without_fragment = uri.split_once('#').map_or(uri, |(before, _)| before);
    match without_fragment.split_once('?') {
        Some((path, query)) => (path.to_string(), Some(query.to_string())),
        None => (without_fragment.to_string(), None),
    }
}

/// The largest request body this transport reads, before any decoding: the pack a
/// `receive-pack` request may carry ([`git_pack::MAX_PACK_BYTES`]) plus room for the
/// pkt-line prelude that names the ref updates and the capabilities. A larger body is
/// refused by the caller without being decoded or read, so neither the decompressor
/// nor the pack reader is ever handed a body the transport could not accept anyway.
pub const MAX_REQUEST_BODY_BYTES: usize = git_pack::MAX_PACK_BYTES + 64 * 1024;

/// Serve one git transport request against the store.
///
/// `body` is the request body with its `Content-Encoding` already undone
/// ([`decode_body`]), so the decompression — caller-driven CPU over caller-supplied
/// bytes — happens before the store guard this function is handed is taken.
///
/// `caller` is the principal the credentials the request PRESENTED speak for
/// ([`super::git_authorization`]), or `None` for a request that presented none —
/// which is answered with a challenge, because every request to a CodeCommit
/// repository's git transport is made with git credentials and the principal behind
/// them is what the authorization steps below are decided for.
///
/// Each of those steps is one IAM decision, taken BEFORE the state of the repository
/// is consulted: a refusal is an authorization refusal and never git's own
/// "fetch first". The ref advertisement is decided with no reference in hand,
/// because the client has not said what it intends to update yet; each ref update
/// command of a push is decided with its own.
pub fn serve(
    store: &mut Store,
    request: &Request,
    body: &[u8],
    caller: Option<&GitPrincipal>,
) -> Response {
    let Some(principal) = caller else {
        return Response::challenge(CREDENTIALS_REQUIRED);
    };
    let Some(repository) = store.repositories.get_mut(&request.repository) else {
        // The same refusal a git host gives for a path that names no repository;
        // a git client prints it as "repository not found".
        return Response::text(
            404,
            &format!("repository {} not found", request.repository),
        );
    };
    // The ARN of the repository the request addressed, which is the resource an IAM
    // statement's `Resource` element names for a git action on it.
    let resource = repository.arn.clone();
    match &request.endpoint {
        Endpoint::Advertisement(None) => Response::text(
            403,
            "this server speaks only the git smart HTTP protocol; \
             request info/refs with service=git-upload-pack or service=git-receive-pack",
        ),
        Endpoint::Advertisement(Some(service)) => {
            // The KEYLESS authorization step: the advertisement a fetch or a push
            // begins with names no reference, so the decision carries no value for
            // `codecommit:References`.
            match refusal_of(principal, *service, &resource, None) {
                Some(refusal) => refusal,
                None => advertise(repository, *service),
            }
        }
        Endpoint::UploadPack => match refusal_of(
            principal,
            GitService::Upload,
            &resource,
            None,
        ) {
            Some(refusal) => refusal,
            None => upload_pack(repository, body),
        },
        Endpoint::ReceivePack => receive_pack(repository, body, principal),
    }
}

/// The refusal a step of `service` on `resource` gets when the policies in force on
/// `principal` do not permit it, or `None` when they do.
///
/// `reference` is the fully qualified name of the ref the step is about, and `None`
/// for a step that is about none. The status is a `403`: the request identified a
/// principal, and that principal may not take this step — which is what a git client
/// prints as the server having refused it.
fn refusal_of(
    principal: &GitPrincipal,
    service: GitService,
    resource: &str,
    reference: Option<&str>,
) -> Option<Response> {
    let (action, refused) = match service {
        GitService::Upload => (GIT_PULL_ACTION, NO_PERMISSION_TO_PULL),
        GitService::Receive => (GIT_PUSH_ACTION, NO_PERMISSION_TO_PUSH),
    };
    if principal.permits(action, resource, reference) {
        return None;
    }
    // Audit record for the refusal: the acting principal, what it asked for and the
    // decision. The policy documents themselves are not logged.
    ::tracing::info!(
        audit = true,
        outcome = "refused_access_denied",
        principal = %principal.arn,
        action = %action,
        resource = %resource,
        reference = reference.unwrap_or_default(),
        "the policies in force on the caller do not permit this git request"
    );
    Some(Response::text(403, refused))
}

/// The request body with its `Content-Encoding` undone.
///
/// `content_encoding` is the request's `Content-Encoding`: a git client may gzip the
/// body of an `upload-pack` request, and a body that declares an encoding this server
/// cannot decode is refused rather than read as plain bytes. The inflated size is
/// capped, so a small body that expands without bound is refused instead of read.
pub fn decode_body(content_encoding: Option<&str>, body: &[u8]) -> Result<Vec<u8>, String> {
    match content_encoding.map(str::trim) {
        None | Some("") | Some("identity") => Ok(body.to_vec()),
        Some("gzip") | Some("x-gzip") => {
            use std::io::Read as _;
            let decoder = flate2::read::GzDecoder::new(body);
            let mut out = Vec::new();
            // One byte past the bound, so a body AT the bound is read whole and a
            // larger one is refused rather than silently truncated — a truncated
            // pack would be reported as a corrupt one, which is a different and
            // misleading answer.
            let ceiling = git_pack::MAX_PACK_BYTES;
            decoder
                .take(u64::try_from(ceiling).unwrap_or(u64::MAX).saturating_add(1))
                .read_to_end(&mut out)
                .map_err(|error| format!("the gzip request body could not be read: {error}"))?;
            if out.len() > ceiling {
                return Err(format!(
                    "the request body inflates past the {ceiling} bytes this emulator accepts"
                ));
            }
            Ok(out)
        }
        Some(other) => Err(format!("content-encoding {other} is not supported")),
    }
}

// ---------------------------------------------------------------- advertisement

/// The ref advertisement: what every clone, fetch and push starts by reading.
fn advertise(repository: &Repository, service: GitService) -> Response {
    let mut out: Vec<u8> = Vec::new();
    write_pkt(&mut out, format!("# service={}\n", service.name()).as_bytes());
    out.extend_from_slice(FLUSH);

    let refs = repository.refs();
    let head = repository
        .default_branch
        .as_ref()
        .and_then(|branch| repository.branches.get(branch))
        .map(|branch| (branch.name.clone(), branch.commit_id.clone()));

    let mut capabilities: Vec<String> = match service {
        // `ofs-delta` is what lets a client send the compact delta form, which
        // `git_pack::read_pack` resolves; `delete-refs` lets it push a deletion;
        // `report-status` is what makes a per-ref verdict readable by the client.
        GitService::Receive => vec![
            "report-status".to_string(),
            "delete-refs".to_string(),
            "ofs-delta".to_string(),
        ],
        // A fetch needs no capability from this list to complete: the response is
        // a plain NAK followed by the packfile.
        GitService::Upload => Vec::new(),
    };
    if let (GitService::Upload, Some((branch, _))) = (service, &head) {
        // Only a fetching client reads HEAD, and only it is told which branch
        // HEAD is a symbolic ref to; receive-pack advertises neither.
        capabilities.push(format!("symref=HEAD:{HEADS_PREFIX}{branch}"));
    }
    capabilities.push(AGENT.to_string());
    let capabilities = capabilities.join(" ");

    let mut first = true;
    if let (GitService::Upload, Some((_, tip))) = (service, &head) {
        // A fetching client reads HEAD to learn which branch to check out.
        write_pkt(
            &mut out,
            format!("{tip} HEAD\0{capabilities}\n").as_bytes(),
        );
        first = false;
    }
    for (name, id) in &refs {
        if first {
            write_pkt(&mut out, format!("{id} {name}\0{capabilities}\n").as_bytes());
            first = false;
        } else {
            write_pkt(&mut out, format!("{id} {name}\n").as_bytes());
        }
        // An annotated tag is advertised with the commit it peels to, so a client
        // can want the commit without fetching the tag object first.
        if name.starts_with(TAGS_PREFIX) {
            if let Some(peeled) = repository
                .peel(id)
                .filter(|peeled| peeled != id)
            {
                write_pkt(&mut out, format!("{peeled} {name}^{{}}\n").as_bytes());
            }
        }
    }
    if first {
        // An empty repository still has to advertise its capabilities, which git
        // does on a line naming the null object.
        write_pkt(
            &mut out,
            format!(
                "{} capabilities^{{}}\0{capabilities}\n",
                git_object::NULL_OBJECT_ID
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(FLUSH);
    Response::git(
        200,
        format!("application/x-{}-advertisement", service.name()),
        out,
    )
}

// ------------------------------------------------------------------ upload-pack

/// Serve a fetch or clone: read the client's wants and haves, answer with the
/// packfile holding every object reachable from the wants that the client does
/// not already have.
fn upload_pack(repository: &Repository, body: &[u8]) -> Response {
    let content_type = format!("application/x-{}-result", GitService::Upload.name());
    // A fetch request is pkt-lines all the way down — the wants, a flush, then
    // the haves and `done` — and carries no packfile, so every section is read.
    let lines = match read_pkt_lines(body, false) {
        Ok((lines, _)) => lines,
        Err(message) => return Response::text(400, &message),
    };

    let mut wants: Vec<String> = Vec::new();
    let mut haves: Vec<String> = Vec::new();
    let mut done = false;
    for line in &lines {
        let text = String::from_utf8_lossy(line);
        let text = text.trim_end_matches('\n');
        let (verb, rest) = text.split_once(' ').unwrap_or((text, ""));
        match verb {
            "want" => {
                let id = rest.split(' ').next().unwrap_or_default();
                if !git_object::is_object_id(id) {
                    return Response::git(
                        200,
                        content_type,
                        error_line(&format!("upload-pack: not a valid object name {id}")),
                    );
                }
                wants.push(id.to_string());
            }
            "have" => haves.push(rest.trim().to_string()),
            "done" => done = true,
            // `deepen`/`filter` arrive only from a client that negotiated a
            // capability this server does not advertise.
            "deepen" | "deepen-since" | "deepen-not" | "filter" => {
                return Response::git(
                    200,
                    content_type,
                    error_line(&format!("upload-pack: {verb} is not supported")),
                );
            }
            _ => {}
        }
    }

    if wants.is_empty() {
        return Response::git(200, content_type, nak());
    }
    // A want has to be an object an ADVERTISED ref reaches, which is what
    // `git-upload-pack` itself requires of one (a server that serves any object
    // id it holds does so only under `uploadpack.allowAnySHA1InWant`). So an
    // object no ref reaches — one a refused push left behind, say — is not
    // fetchable, and the answer is the one git gives: "not our ref".
    let roots: Vec<String> = repository.refs().into_iter().map(|(_, id)| id).collect();
    let advertised = repository.reachable_from(&roots);
    if let Some(missing) = wants.iter().find(|want| !advertised.contains(*want)) {
        return Response::git(
            200,
            content_type,
            error_line(&format!("upload-pack: not our ref {missing}")),
        );
    }
    if !done {
        // Negotiation is not over: the client will send another round, and ends
        // the last one with `done`. Reporting no common object is what a server
        // that keeps no negotiation state across requests must answer.
        return Response::git(200, content_type, nak());
    }

    let common: Vec<String> = haves
        .into_iter()
        .filter(|have| repository.objects.contains_key(have))
        .collect();
    let excluded = repository.reachable_from(&common);
    let wanted = repository.reachable_from(&wants);
    // Size the answer BEFORE building it, so a repository near its byte cap is
    // refused rather than copied first.
    let bytes = wanted
        .difference(&excluded)
        .filter_map(|id| repository.object(id))
        .map(|object| object.data.len())
        .fold(0usize, usize::saturating_add);
    if bytes > git_pack::MAX_PACK_BYTES {
        return Response::git(
            200,
            content_type,
            error_line(&format!(
                "upload-pack: the pack would exceed the {} bytes this emulator serves",
                git_pack::MAX_PACK_BYTES
            )),
        );
    }
    // One pass, then git's conventional order: commits, tags, trees, blobs.
    let mut objects: Vec<Object> = wanted
        .difference(&excluded)
        .filter_map(|id| repository.object(id))
        .cloned()
        .collect();
    objects.sort_by_key(|object| match object.kind {
        Kind::Commit => 0u8,
        Kind::Tag => 1,
        Kind::Tree => 2,
        Kind::Blob => 3,
    });

    let mut out = match common.last() {
        Some(last) => {
            let mut out = Vec::new();
            write_pkt(&mut out, format!("ACK {last}\n").as_bytes());
            out
        }
        None => nak(),
    };
    out.extend_from_slice(&git_pack::write_pack(&objects));
    Response::git(200, content_type, out)
}

/// The `NAK` line: this server shares no object with the client.
fn nak() -> Vec<u8> {
    let mut out = Vec::new();
    write_pkt(&mut out, b"NAK\n");
    out
}

/// An `ERR` line, which a git client prints verbatim as the server's reason.
fn error_line(message: &str) -> Vec<u8> {
    let mut out = Vec::new();
    write_pkt(&mut out, format!("ERR {message}\n").as_bytes());
    out
}

// ----------------------------------------------------------------- receive-pack

/// One ref update a push asks for.
#[derive(Debug, Clone)]
struct Command {
    old: String,
    new: String,
    name: String,
}

/// Serve a push: read the ref commands and the packfile, store what the pack
/// carries, apply the updates the caller is authorized for and the repository's
/// state allows, and report a verdict per ref.
fn receive_pack(repository: &mut Repository, body: &[u8], principal: &GitPrincipal) -> Response {
    let content_type = format!("application/x-{}-result", GitService::Receive.name());
    let resource = repository.arn.clone();
    let (lines, pack) = match read_pkt_lines(body, true) {
        Ok(parsed) => parsed,
        Err(message) => return Response::text(400, &message),
    };

    let mut commands: Vec<Command> = Vec::new();
    let mut report_status = false;
    for line in &lines {
        let text = String::from_utf8_lossy(line);
        let (update, capabilities) = text
            .split_once('\0')
            .map_or((text.as_ref(), ""), |(update, caps)| (update, caps));
        if capabilities
            .trim_end_matches('\n')
            .split(' ')
            .any(|capability| capability == "report-status")
        {
            report_status = true;
        }
        let mut fields = update.trim_end_matches('\n').splitn(3, ' ');
        let (Some(old), Some(new), Some(name)) = (fields.next(), fields.next(), fields.next())
        else {
            // A command line that is not `<old> <new> <ref>` names no ref, so
            // there is nothing to report a verdict for: the whole request is
            // refused rather than answered with a status report that silently
            // omits it.
            return Response::text(400, "a ref update command is malformed");
        };
        commands.push(Command {
            old: old.to_string(),
            new: new.to_string(),
            name: name.to_string(),
        });
    }
    if commands.is_empty() {
        // Nothing was asked for: a client probing the endpoint.
        return Response::git(200, content_type, Vec::new());
    }
    if commands.len() > MAX_REF_COMMANDS_PER_REQUEST {
        // A push updates a handful of refs; this bounds the FACTOR a request
        // chooses, so what the whole request costs is bounded rather than each
        // ref update alone.
        return Response::text(
            400,
            &format!(
                "a push may update at most the {MAX_REF_COMMANDS_PER_REQUEST} refs this emulator accepts at once"
            ),
        );
    }

    // Store what the pack carries. The objects are content-addressed, so storing
    // them before the commands are judged cannot corrupt anything: whatever no
    // accepted ref reaches is released by `prune_history` below, which is the
    // quarantine a git host unpacks into.
    let unpacked = if pack.is_empty() {
        Ok(())
    } else {
        store_pack(repository, pack)
    };

    // Judge each command, then — only for one that passes — build the control
    // plane's projection of what it makes readable, and only then move the ref.
    // A commit the projection cannot hold leaves the ref where it was, so the two
    // faces of the service never disagree about what the repository holds.
    //
    // ONE budget covers the whole request, and it covers BOTH walking steps —
    // each command's closure check and each accepted command's projection —
    // because the cost is a product: the command count is the caller's to choose,
    // and every command names a tip whose closure is a whole history of tree
    // snapshots. Per-command caps would bound each factor and none of the
    // product. A command that exhausts the budget, or that is reached after its
    // deadline has passed, is refused like any other the repository cannot hold.
    // `verified` is the same request's memo: a tip whose closure this request has
    // already walked is not walked again, so repeating a tip costs nothing.
    let mut projection = crate::state::ProjectionBudget::for_one_request();
    let mut verified: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut report: Vec<(String, Result<(), String>)> = Vec::new();
    let mut accepted = 0usize;
    // ONE instant for the whole push: every ref this request moves moved at the
    // same time, so the events a multi-ref push records are stamped together
    // rather than a few microseconds apart.
    let pushed_at = SystemTime::now();
    for command in commands {
        let verdict = match &unpacked {
            // git's own wording: the detail is on the `unpack` line above.
            Err(_) => Err("unpacker error".to_string()),
            // ONE authorization step per ref update, decided with that ref as the
            // value of `codecommit:References` and BEFORE the repository's state is
            // consulted: a refused ref is refused for authorization, so it never
            // reports git's own "fetch first", it leaves the ref where it stands,
            // and the other refs of the same push are decided on their own.
            Ok(()) if !principal.permits(GIT_PUSH_ACTION, &resource, Some(&command.name)) => {
                ::tracing::info!(
                    audit = true,
                    outcome = "refused_access_denied",
                    principal = %principal.arn,
                    action = %GIT_PUSH_ACTION,
                    resource = %resource,
                    reference = %command.name,
                    "the policies in force on the caller do not permit this ref update"
                );
                Err(NO_PERMISSION_TO_PUSH_REFERENCE.to_string())
            }
            Ok(()) => judge(repository, &command, &mut projection, &mut verified)
                .and_then(|()| project(repository, &command, &mut projection)),
        };
        if verdict.is_ok() {
            apply(repository, &command, pushed_at);
            accepted += 1;
        }
        report.push((command.name, verdict));
    }

    if accepted > 0 {
        choose_default_branch(repository);
        repository.last_modified_date = pushed_at;
    }
    // Always: a push whose refs were all refused still stored its pack's objects,
    // and nothing points at them, so this is where they are released.
    repository.prune_history();

    if !report_status {
        return Response::git(200, content_type, Vec::new());
    }
    let mut out: Vec<u8> = Vec::new();
    match &unpacked {
        Ok(()) => write_pkt(&mut out, b"unpack ok\n"),
        Err(message) => write_pkt(&mut out, format!("unpack {message}\n").as_bytes()),
    }
    for (name, verdict) in &report {
        match verdict {
            Ok(()) => write_pkt(&mut out, format!("ok {name}\n").as_bytes()),
            Err(reason) => write_pkt(&mut out, format!("ng {name} {reason}\n").as_bytes()),
        }
    }
    out.extend_from_slice(FLUSH);
    Response::git(200, content_type, out)
}

/// Read a pushed packfile into the repository's object store, refusing one that
/// would take the store past its bounds.
fn store_pack(repository: &mut Repository, pack: &[u8]) -> Result<(), String> {
    let objects = git_pack::read_pack(pack, &|id| repository.object(id).cloned())
        .map_err(|error| error.to_string())?;
    let new: Vec<Object> = objects
        .into_iter()
        .filter(|object| !repository.objects.contains_key(&object.id()))
        .collect();
    if repository.objects.len().saturating_add(new.len()) > crate::state::MAX_OBJECTS_PER_REPOSITORY
    {
        return Err(format!(
            "the push would take this repository past the {} objects this emulator holds",
            crate::state::MAX_OBJECTS_PER_REPOSITORY
        ));
    }
    let added: usize = new
        .iter()
        .map(|object| object.data.len())
        .fold(0usize, usize::saturating_add);
    if repository.object_bytes().saturating_add(added)
        > crate::state::MAX_OBJECT_BYTES_PER_REPOSITORY
    {
        return Err(format!(
            "the push would take this repository past the {} object bytes this emulator holds",
            crate::state::MAX_OBJECT_BYTES_PER_REPOSITORY
        ));
    }
    for object in new {
        repository.insert_object(object);
    }
    Ok(())
}

/// Which namespace a ref lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Namespace {
    /// `refs/heads/…`: a branch, which must point at a commit.
    Branch,
    /// `refs/tags/…`: a tag, which may point at a commit or at an annotated tag
    /// object.
    Tag,
}

/// Judge one ref update against the repository's state. `Err` carries the reason
/// the `ng` line reports, which is the vocabulary git itself uses.
///
/// The checks run CHEAPEST FIRST: everything decided by a lookup — the object
/// names, the ref's namespace, the client's view of where the ref stands, what
/// kind of object the new value is, and how many refs the repository already
/// holds — comes before the one expensive check, the walk of the new value's
/// whole object closure. A command that will be refused therefore never pays for
/// that walk; the walk itself is charged to `budget`, which this one request
/// shares across every command and the projection, and `verified` memoises the
/// tips this request has already walked.
fn judge(
    repository: &Repository,
    command: &Command,
    budget: &mut crate::state::ProjectionBudget,
    verified: &mut std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if !git_object::is_object_id(&command.old) || !git_object::is_object_id(&command.new) {
        return Err("invalid object name".to_string());
    }
    let (namespace, short) =
        ref_namespace(&command.name).ok_or_else(|| "funny refname".to_string())?;
    let current = current_ref(repository, &command.name);
    let declared_old = (!git_object::is_null_object_id(&command.old)).then(|| command.old.clone());
    if current != declared_old {
        // The client's view of this ref is stale — another push moved it, or it
        // already exists where the client expected to create it. The ref is left
        // where it is; git's own wording for this is "fetch first".
        return Err("fetch first".to_string());
    }

    if git_object::is_null_object_id(&command.new) {
        if current.is_none() {
            return Err("ref does not exist".to_string());
        }
        if namespace == Namespace::Branch && repository.default_branch.as_deref() == Some(short) {
            // CodeCommit keeps a repository's default branch: it cannot be
            // deleted while it is the default.
            return Err("the default branch cannot be deleted".to_string());
        }
        return Ok(());
    }

    let object = repository
        .object(&command.new)
        .ok_or_else(|| format!("missing necessary object {}", command.new))?;
    // What kind of object a ref may point at: one lookup, so it is decided before
    // the closure walk at the end of this function.
    match (namespace, object.kind) {
        (Namespace::Branch, Kind::Commit) | (Namespace::Tag, Kind::Commit | Kind::Tag) => {}
        (Namespace::Branch, other) => {
            return Err(format!("a branch cannot point at a {}", other.name()))
        }
        (Namespace::Tag, other) => return Err(format!("a tag cannot point at a {}", other.name())),
    }
    // A ref is a root of reachability, so it is never pruned: this is what bounds
    // how many a repository accumulates over any number of pushes. The bounds are
    // the STATE's own, so a branch a push creates and a branch `CreateBranch`
    // creates are held to the one figure. Only a ref this repository does not
    // already hold costs one.
    if current.is_none() {
        let (held, bound, kind) = match namespace {
            Namespace::Branch => (
                repository.branches.len(),
                crate::state::MAX_BRANCHES_PER_REPOSITORY,
                "branches",
            ),
            Namespace::Tag => (
                repository.git_tags.len(),
                crate::state::MAX_TAGS_PER_REPOSITORY,
                "tags",
            ),
        };
        if held >= bound {
            return Err(format!(
                "this repository already holds the {bound} {kind} this emulator keeps"
            ));
        }
    }
    // The whole closure has to be here, not just the tip: a ref whose history or
    // trees are incomplete is one a later fetch cannot serve, so git refuses the
    // push that would create it rather than advertising a ref it cannot answer
    // for. This is what makes the clone this emulator announces resolvable.
    //
    // It is the one expensive check, so it comes last, it is charged to the
    // REQUEST's budget rather than to this command alone, and a tip this request
    // has already walked is not walked again.
    if !verified.contains(&command.new) {
        if let Some(missing) =
            repository.first_missing_object_within(std::slice::from_ref(&command.new), budget)?
        {
            return Err(format!("missing necessary objects: {missing}"));
        }
        verified.insert(command.new.clone());
    }
    if namespace == Namespace::Tag {
        // A tag has to name a commit in the end, through at most the bounded
        // chain of tag objects `peel` follows.
        repository
            .peel(&command.new)
            .ok_or_else(|| "the tag does not peel to a commit this repository holds".to_string())?;
    }
    Ok(())
}

/// Build the control plane's projection of what an accepted update makes
/// readable: the commit the new value peels to, and its ancestors, charged to the
/// request's own projection budget.
fn project(
    repository: &mut Repository,
    command: &Command,
    budget: &mut crate::state::ProjectionBudget,
) -> Result<(), String> {
    if git_object::is_null_object_id(&command.new) {
        return Ok(());
    }
    let commit = repository
        .peel(&command.new)
        .ok_or_else(|| "the pushed object does not name a commit".to_string())?;
    repository
        .project_history(&commit, budget)
        .map_err(|message| format!("the commit could not be read back: {message}"))
}

/// Apply an accepted ref update.
///
/// A branch moves through [`Repository::set_branch_tip`], the one place a branch
/// tip is set, so a push updates every OPEN pull request reading from that branch
/// exactly as a commit written through the control plane does. A push carries no
/// caller identity for the update to be attributed to — a git client
/// authenticates with git credentials, which this emulator does not mint — so the
/// event it records names no actor.
fn apply(repository: &mut Repository, command: &Command, at: SystemTime) {
    let Some((namespace, short)) = ref_namespace(&command.name) else {
        return;
    };
    let name = short.to_string();
    let deleting = git_object::is_null_object_id(&command.new);
    match namespace {
        Namespace::Branch if deleting => {
            repository.branches.remove(&name);
        }
        Namespace::Branch => {
            repository.set_branch_tip(&name, &command.new, at, None);
        }
        Namespace::Tag if deleting => {
            repository.git_tags.remove(&name);
        }
        Namespace::Tag => {
            repository.git_tags.insert(name, command.new.clone());
        }
    }
}

/// The namespace and short name a full refname carries, or `None` for a name no
/// push may write.
fn ref_namespace(name: &str) -> Option<(Namespace, &str)> {
    let (namespace, short) = name
        .strip_prefix(HEADS_PREFIX)
        .map(|short| (Namespace::Branch, short))
        .or_else(|| {
            name.strip_prefix(TAGS_PREFIX)
                .map(|short| (Namespace::Tag, short))
        })?;
    crate::state::is_valid_ref_name(short).then_some((namespace, short))
}

/// What a ref currently points at, or `None` when the repository has no such ref.
fn current_ref(repository: &Repository, name: &str) -> Option<String> {
    if let Some(branch) = name.strip_prefix(HEADS_PREFIX) {
        return repository
            .branches
            .get(branch)
            .map(|branch| branch.commit_id.clone());
    }
    name.strip_prefix(TAGS_PREFIX)
        .and_then(|tag| repository.git_tags.get(tag).cloned())
}

/// Give a repository that has branches but no default branch one, the way
/// CodeCommit's first push does: the branch a client would check out — `main`,
/// then `master`, then the first in name order.
fn choose_default_branch(repository: &mut Repository) {
    if repository
        .default_branch
        .as_ref()
        .is_some_and(|branch| repository.branches.contains_key(branch))
    {
        return;
    }
    repository.default_branch = ["main", "master"]
        .into_iter()
        .find(|candidate| repository.branches.contains_key(*candidate))
        .map(str::to_string)
        .or_else(|| repository.branches.keys().next().cloned());
}

// --------------------------------------------------------------------- pkt-line

/// Append one pkt-line carrying `data`.
///
/// A pkt-line's length is four hex digits, so a payload longer than
/// [`MAX_PKT_PAYLOAD`] cannot be framed at all: it is truncated rather than
/// written with a five-digit length that would desynchronise the whole response.
/// Only a message built from a client-supplied name (an `ERR` or `ng` line) can
/// approach that length; every other line this module writes is a fixed shape.
fn write_pkt(out: &mut Vec<u8>, data: &[u8]) {
    let data = &data[..data.len().min(MAX_PKT_PAYLOAD)];
    out.extend_from_slice(format!("{:04x}", data.len() + 4).as_bytes());
    out.extend_from_slice(data);
}

/// The largest payload one pkt-line may carry (65520 bytes, the protocol's
/// `LARGE_PACKET_MAX` less its four length bytes).
const MAX_PKT_PAYLOAD: usize = 65_516;

/// Read the pkt-lines at the head of a request body.
///
/// `stop_at_flush` is what distinguishes the two request shapes: a push's
/// commands are terminated by a flush packet and followed by a packfile, which is
/// reported as the remainder; a fetch's wants, haves and `done` span several
/// flush-separated sections and are followed by nothing.
fn read_pkt_lines(body: &[u8], stop_at_flush: bool) -> Result<(Vec<Vec<u8>>, &[u8]), String> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut cursor = 0usize;
    while cursor + 4 <= body.len() {
        let header = std::str::from_utf8(&body[cursor..cursor + 4])
            .map_err(|_| "a pkt-line length is not hexadecimal".to_string())?;
        let length = usize::from_str_radix(header, 16)
            .map_err(|_| format!("the pkt-line length {header:?} is not hexadecimal"))?;
        cursor += 4;
        if length == 0 {
            // A flush packet ends a section.
            if stop_at_flush {
                return Ok((lines, &body[cursor..]));
            }
            continue;
        }
        if length == 1 {
            // A delimiter packet: protocol v2 only, which this server does not
            // advertise.
            continue;
        }
        if length < 4 {
            return Err(format!("the pkt-line length {length} is not valid"));
        }
        let payload = length - 4;
        if payload > MAX_PKT_PAYLOAD {
            return Err(format!("a pkt-line declares {payload} bytes of payload"));
        }
        let end = cursor + payload;
        let line = body
            .get(cursor..end)
            .ok_or_else(|| "a pkt-line is truncated".to_string())?;
        lines.push(line.to_vec());
        cursor = end;
        if lines.len() > git_pack::MAX_PACK_OBJECTS {
            return Err("the request holds more pkt-lines than this emulator reads".to_string());
        }
    }
    Ok((lines, &body[body.len()..]))
}

