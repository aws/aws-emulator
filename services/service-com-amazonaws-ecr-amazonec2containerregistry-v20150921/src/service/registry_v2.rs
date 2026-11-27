// SPDX-License-Identifier: CC0-1.0

//! The Docker Registry v2 data plane: the endpoint a `docker pull` (and anything built on
//! containerd) actually talks to.
//!
//! # The rule this module implements
//!
//! Amazon ECR serves the Docker Registry HTTP API V2 on the registry endpoint it publishes
//! for a registry — AWS's own `<account>.dkr.ecr.<region>.amazonaws.com` there, and here the
//! endpoint a caller reached this emulator on, which is what a repository's `repositoryUri`
//! is published at and what `GetAuthorizationToken` reports as its `proxyEndpoint`: a
//! request to `/v2/<name>/manifests/<reference>`
//! carrying the `docker login` credential answers the manifest that repository holds for
//! the reference, and one to `/v2/<name>/blobs/<digest>` answers the blob's own bytes. That
//! endpoint is what a pull IS — the control-plane operations (`BatchGetImage`,
//! `GetDownloadUrlForLayer`) are the API spelling of the same reads — so a registry that
//! does not serve it cannot be pulled from by any Docker-compatible client.
//!
//! The path space is claimed whatever host the request arrived on, because that endpoint is
//! whichever address the client was handed: this emulator serves one registry, and the
//! repository a request names is resolved in it.
//!
//! Those paths name no modelled operation of this service (every operation is `POST /` on
//! awsJson1_1), so the same two seams carry this plane as carry the layer download plane:
//! [`Service::can_handle`](smithy_server_runtime::Service::can_handle) claims the path
//! space, and [`Service::override_routing_error`](smithy_server_runtime::Service::override_routing_error)
//! serves it — which is also BEFORE authentication, because what authorises a registry
//! request is its own Basic credential rather than a SigV4 signature.
//!
//! # What authorises one
//!
//! The credential `GetAuthorizationToken` minted, presented as HTTP Basic authorization:
//! the token decodes to `user:password` "for private registry authentication using `docker
//! login`" (`AuthorizationData$authorizationToken`), so a client sends exactly that pair.
//! The password is checked against the issues this registry recorded
//! (`crate::state::registry_v2`), so a credential this registry never minted, one it minted
//! for a window that has closed, and no credential at all each reach no repository — a
//! private registry's contents stay private, and nothing here is served anonymously.
//!
//! # What a pull changes
//!
//! A manifest GET is the pull, and ECR records two things about one: the image's
//! `lastRecordedPullTime` and the `AWS/ECR` `RepositoryPullCount` datapoint for the
//! repository (`crate::state::pull_metrics`). A `HEAD` of the same address is a client
//! checking whether the manifest is there — containerd's existence probe — and transfers no
//! image, so it records neither. A blob fetch is part of the pull the manifest already
//! counted, so it records nothing of its own.
//!
//! # Its envelope
//!
//! A registry client is not an AWS SDK client: a refusal is the registry API's own error
//! document, `{"errors":[{"code":…,"message":…}]}` with the code and status the Docker
//! Registry HTTP API V2 error-code table states for that condition, and every response
//! carries the API version header a client reads to discover that this endpoint speaks V2.

use super::*;

/// The path space the Docker Registry v2 API is served under — the `/v2/` prefix that API
/// defines its whole surface beneath.
pub(super) const REGISTRY_V2_PATH_PREFIX: &str = "/v2";

/// The header a registry answers every request with to state which version of the registry
/// API it speaks, and its value for this one: "application/vnd.docker.distribution.…"
/// clients read it to decide they are talking to a V2 registry at all.
const API_VERSION_HEADER: &str = "docker-distribution-api-version";

/// See [`API_VERSION_HEADER`].
const API_VERSION: &str = "registry/2.0";

/// The header a registry states the content digest of what it served in, so a client can
/// verify the bytes it received address the content it asked for.
const CONTENT_DIGEST_HEADER: &str = "docker-content-digest";

/// The media type of a registry API document that is not content: the version check's own
/// body and every error document. It is also what a manifest stored under no media type at
/// all is served as — a JSON document whose own schema nothing stated.
const REGISTRY_JSON_CONTENT_TYPE: &str = "application/json";

/// The media type a blob is served under: a layer is opaque bytes to the registry, which is
/// the type the layer download plane serves the same bytes under.
const BLOB_CONTENT_TYPE: &str = crate::state::LAYER_DOWNLOAD_CONTENT_TYPE;

/// The authentication scheme a registry request presents its credential in, lowercased for
/// the case-insensitive comparison an HTTP scheme token takes.
const BASIC_SCHEME: &str = "basic";

/// The service a 401's `WWW-Authenticate` challenge names, as ECR's registry endpoint
/// names itself to a client deciding where to fetch a credential from.
const CHALLENGE_SERVICE: &str = "ecr.amazonaws.com";

/// The most bytes of a presented `Authorization` header this plane decodes.
///
/// The header is a caller's and its value is base64-decoded, so the work one request can
/// set off is bounded before the decode. An authorization token is the base64 of
/// `AWS:<password>`, a few hundred characters, so this is well past any credential this
/// registry mints and far below anything that would cost the host.
const MAX_CREDENTIAL_HEADER_BYTES: usize = 8 * 1024;

/// The most `Accept` media types this plane reads off one request.
///
/// The list is a caller's, and each entry is compared against the schemas the stored
/// manifest can be served in, so its length is bounded rather than taken as given. A
/// registry client offers a handful (the manifest schemas and index types it understands).
const MAX_ACCEPTED_MEDIA_TYPES: usize = 32;

/// The most characters of a media type this plane states in a header. A media type is a
/// short label with a few parameters; a longer value is not one.
const MAX_MEDIA_TYPE_LENGTH: usize = 255;

/// The read methods this plane serves: a `GET` carries the content and a `HEAD` reports on
/// it without the bytes.
const REGISTRY_READ_METHODS: &str = "GET, HEAD";

/// What a request to the registry's `/v2` path space addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RegistryV2Target {
    /// `/v2/` itself: the version check a client makes to discover the API and the
    /// authentication it has to present.
    Version,
    /// `/v2/<name>/manifests/<reference>`: the manifest the repository holds for a tag or
    /// a digest — the document a pull starts from.
    Manifest {
        /// The repository the reference is resolved in.
        repository_name: String,
        /// The tag or digest the manifest is addressed by.
        reference: String,
    },
    /// `/v2/<name>/blobs/<digest>`: one blob of the repository, by its content digest.
    Blob {
        /// The repository the blob is held by.
        repository_name: String,
        /// The digest addressing it.
        digest: String,
    },
    /// An endpoint the registry API DEFINES beneath a repository name that this registry
    /// does not serve — a tag listing, a referrers query, the upload session a push opens —
    /// or one of the two it does serve, addressed by a path carrying no repository name or
    /// no reference. It is this registry's to refuse as an unsupported registry operation
    /// rather than to leave to service discovery.
    Unserved,
}

/// Whether this service claims `parts` as a registry request.
///
/// The path has to address an endpoint of the registry API ([`registry_v2_target`]; the `/v2`
/// space is NOT this registry's alone — a REST service of another family publishes `/v2/...`
/// paths of its own), and then one of two things has to hold, each of which says the request
/// is a REGISTRY client's rather than an AWS API client's:
///
/// - it names no service ([`smithy_server_runtime::resolve_route_key`]), which is the
///   `UnknownService` case this claim exists to prevent: a registry client presents no
///   `X-Amz-Target` and no SigV4 scope, so a request addressed to this registry's own
///   endpoint host (whose first label is the account id) or to the emulator's own socket
///   resolves no key at all;
/// - or it presents a BASIC credential at a host that is not an endpoint AWS publishes
///   ([`presents_a_registry_client_credential`]): Basic is how a `docker login` credential
///   travels ("for private registry authentication using `docker login`",
///   `AuthorizationData$authorizationToken`) and no AWS SDK client sends it, while a host
///   under a zone AWS publishes its own endpoints in names the service that answers there
///   and is left to it. That is what makes the address this registry PUBLISHES reachable: a
///   repository's URI and the URL a `docker login` is pointed at are the endpoint this
///   emulator is reached at (`crate::state::registry_address`), and the front door reads the
///   FIRST LABEL of any host as a service name — an operator's own name for this process, or
///   an IPv6 literal's bracket — so without this half a pull at the very address this
///   registry handed out is answered by no service.
///
/// What it does NOT claim is another service's API traffic: that carries a SigV4 scope or an
/// `X-Amz-Target`, never a Basic credential, and a registry-shaped path addressed to an AWS
/// endpoint host of another service is left where it was. A `/v2` path addressed to a host
/// whose first label names ANOTHER emulated service an operator serves locally, with a Basic
/// credential, is claimed here as well as by that service, which the router answers as an
/// ambiguous request — a registry request to such a host reached no registry before either.
pub(super) fn claims_registry_v2(parts: &::http::request::Parts) -> bool {
    registry_v2_target(parts.uri.path()).is_some()
        && (::smithy_server_runtime::resolve_route_key(parts).is_none()
            || presents_a_registry_client_credential(parts))
}

/// Whether `parts` is a request a REGISTRY client made at an address this emulator may be
/// reached at: an `Authorization` header in the BASIC scheme — the scheme a `docker login`
/// credential travels in, and one no AWS SDK client uses — at a host that is not an endpoint
/// AWS publishes (`service_common::data_plane_addressing::addresses_an_aws_endpoint`, the
/// tree's one reading of that question), since such a host names the AWS service that answers
/// there.
///
/// Only the SCHEME of the credential is read: what it has to BE is decided where the request
/// is served ([`presented_password`], against the issues this registry recorded), so a claim
/// never depends on the store and a credential this registry did not mint is refused with the
/// registry API's own 401 rather than handed to another service. The header is a caller's, so
/// the read is bounded ([`MAX_CREDENTIAL_HEADER_BYTES`]) before anything is split.
fn presents_a_registry_client_credential(parts: &::http::request::Parts) -> bool {
    use ::service_common::data_plane_addressing as addressing;

    let basic = parts
        .headers
        .get(::http::header::AUTHORIZATION)
        .and_then(|presented| {
            (presented.len() <= MAX_CREDENTIAL_HEADER_BYTES)
                .then(|| presented.to_str().ok())
                .flatten()
        })
        .and_then(|presented| presented.split_once(' '))
        .is_some_and(|(scheme, _credential)| scheme.eq_ignore_ascii_case(BASIC_SCHEME));
    if !basic {
        return false;
    }
    addressing::addressed_host(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
    .is_none_or(|reached| !addressing::addresses_an_aws_endpoint(&reached))
}

/// What a request path addresses in the registry API, or `None` when the path addresses no
/// endpoint of that API.
///
/// The repository name is everything between the prefix and the endpoint's own two
/// segments, because a repository name "can be specified with namespace levels" — it
/// carries `/` separators of its own — so the endpoint is read from the END of the path.
/// Every segment is percent-decoded, the way the layer download plane reads one
/// (`crate::state::parse_layer_download_path`).
///
/// The endpoint names are the Docker Registry HTTP API V2's own: `manifests` and `blobs`,
/// which this registry serves, plus `tags`, `referrers` and the `uploads` of a push, which
/// it does not. A path naming none of them is no registry endpoint at all, whatever it is.
pub(super) fn registry_v2_target(path: &str) -> Option<RegistryV2Target> {
    let rest = path.strip_prefix(REGISTRY_V2_PATH_PREFIX)?;
    if rest.is_empty() || rest == "/" {
        return Some(RegistryV2Target::Version);
    }
    let rest = rest.strip_prefix('/')?;
    let mut segments: Vec<&str> = rest.split('/').collect();
    // The endpoint is the last two segments: `<kind>/<reference>`.
    let (Some(reference), Some(kind)) = (segments.pop(), segments.pop()) else {
        return None;
    };
    let repository_name = segments.join("/");
    let addressed = !repository_name.is_empty() && !reference.is_empty();
    let decoded = || ::smithy_server_runtime::percent_decode(&repository_name);
    Some(match kind {
        "manifests" if addressed => RegistryV2Target::Manifest {
            repository_name: decoded(),
            reference: ::smithy_server_runtime::percent_decode(reference),
        },
        "blobs" if addressed => RegistryV2Target::Blob {
            repository_name: decoded(),
            digest: ::smithy_server_runtime::percent_decode(reference),
        },
        // An endpoint of this API this registry does not serve, and a served one addressed
        // by a path that names no repository or no reference.
        "manifests" | "blobs" | "tags" | "referrers" | "uploads" => RegistryV2Target::Unserved,
        // No endpoint of the registry API: not this registry's path.
        _ => return None,
    })
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Serve one request to this registry's Docker Registry v2 endpoint.
    ///
    /// The order is the registry API's own: the method, then the credential — a client that
    /// presents none is challenged before anything about the repository is read, so a
    /// refused request learns nothing about what this registry holds — and only then the
    /// content the path addresses.
    pub(super) fn serve_registry_v2(
        &self,
        req: &RtRequest,
        target: &RegistryV2Target,
    ) -> RtResponse {
        if !matches!(req.method(), "GET" | "HEAD") {
            // The registry API names an operation by method and path together, so a method
            // this plane does not serve is a registry operation it does not support.
            return registry_unsupported();
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return registry_error(500, "UNKNOWN", "the ecr repository store is unavailable");
        };
        let now = ::std::time::SystemTime::now();
        // Which registry the client addressed: the Region of the endpoint it DIALLED, and
        // for an endpoint naming none (this emulator's own socket) the Region the
        // credential it presents was issued for — a `docker login` credential is obtained
        // per registry.
        let dialled = crate::state::dialled_registry_region(request_authority(req));

        // The credential this registry minted, or the challenge a client answers by
        // fetching one. The expiries the clock has already made due are written down
        // first: they fall due whether or not anybody presents a credential, so
        // materialising them changes nothing a later caller sees.
        let Some(password) = presented_password(req) else {
            return unauthorized(req);
        };
        let (principal_arn, region) = {
            let mut store = state.lock();
            store.reclaim_expired_registry_credentials(now);
            match store.honoured_registry_credential(&password, now) {
                Some(honoured) => (
                    honoured.principal_arn.clone(),
                    dialled.unwrap_or_else(|| honoured.region.clone()),
                ),
                None => return unauthorized(req),
            }
        };

        match target {
            RegistryV2Target::Version => version_check(),
            RegistryV2Target::Manifest {
                repository_name,
                reference,
            } => self.serve_manifest(
                &state,
                req,
                &AddressedRepository {
                    region: &region,
                    name: repository_name,
                },
                reference,
                &principal_arn,
                now,
            ),
            RegistryV2Target::Blob {
                repository_name,
                digest,
            } => serve_blob(
                &state,
                req,
                &AddressedRepository {
                    region: &region,
                    name: repository_name,
                },
                digest,
            ),
            RegistryV2Target::Unserved => registry_unsupported(),
        }
    }

    /// Serve the manifest `reference` addresses in `repository` — and, for a `GET`,
    /// cache what a pull through cache rule covers and record the pull it is.
    ///
    /// The reference is a digest when it is written as content is addressed
    /// (`<algorithm>:<hex>`) and a tag otherwise, which is how a registry reads the one
    /// path segment that carries either. The document served is the stored manifest, in a
    /// schema the request's `Accept` list admits — the same negotiation `BatchGetImage`
    /// makes for the same stored image (`crate::state::negotiated_manifest`), so the API
    /// and the registry endpoint cannot answer two different documents for one pull.
    ///
    /// A `GET` of a reference under a registered pull through cache rule's prefix is the
    /// pull that CACHES the upstream image, through the one import this registry makes
    /// whichever plane the pull arrives on
    /// (`crate::state::registry_pull::import_covered_image`) — so the first `docker pull`
    /// of a cached path creates the mirrored repository here exactly as `BatchGetImage`
    /// does. A `HEAD` is a client checking whether a manifest is there and caches nothing.
    fn serve_manifest(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsEcrAmazonec2containerregistryV20150921State,
        >,
        req: &RtRequest,
        repository: &AddressedRepository<'_>,
        reference: &str,
        principal_arn: &str,
        now: ::std::time::SystemTime,
    ) -> RtResponse {
        let identifier = addressed_image(reference);
        let accepted = accepted_media_types(req);
        let served = {
            // ONE lock over the import and the read it is for, so the document served is the
            // one the import stored: a repository another request pushes to or deletes
            // between the two cannot make this pull answer that it found nothing it had just
            // cached.
            let mut store = state.lock();
            let held = store
                .repository(repository.region, repository.name)
                .is_ok_and(|repo| repo.lookup_image(&identifier).is_ok());
            // A `GET` of a manifest IS the pull, and the first pull of a reference a rule
            // covers is what caches the upstream image. A reference addressing a digest names
            // CONTENT an import has none to match against, so only a tag is cached; a rule
            // that covers nothing here, or one the upstream cannot be reached through, leaves
            // the refusals below to answer. The copy is cached in the registry this pull
            // addresses, which is the Region the endpoint dialled names.
            if !held && req.method() == "GET" {
                crate::state::registry_pull::import_covered_image(
                    &mut store,
                    repository.region,
                    repository.name,
                    identifier.image_tag.as_deref(),
                    now,
                );
            }
            let Ok(repo) = store.repository(repository.region, repository.name) else {
                return registry_error(404, "NAME_UNKNOWN", NAME_UNKNOWN_MESSAGE);
            };
            let Ok(image) = repo.lookup_image(&identifier) else {
                return registry_error(404, "MANIFEST_UNKNOWN", MANIFEST_UNKNOWN_MESSAGE);
            };
            let (document, media_type) =
                crate::state::negotiated_manifest(image, accepted.as_deref());
            // The digest of the document SERVED, which is what the content digest header
            // states: for the stored document that is the image's own digest, and for the
            // same image re-spelled in another schema it is that document's digest. The
            // image's identity in the repository is `image.digest` either way, which is
            // what the pull below is recorded against.
            let served_digest = if document == image.manifest {
                image.digest.clone()
            } else {
                crate::state::manifest_digest(&document)
            };
            (document, media_type, image.digest.clone(), served_digest)
        };
        let (document, media_type, image_digest, served_digest) = served;

        // A `GET` of a manifest IS the pull: it is what hands a client the document the
        // rest of the image is fetched from, so it is what ECR records a pull time and a
        // pull count for. A `HEAD` reports on the manifest without transferring it.
        if req.method() == "GET" {
            state.lock().record_image_pull(
                repository.region,
                repository.name,
                &image_digest,
                principal_arn,
                now,
            );
            // Published with no store lock held, into the CloudWatch store rather than
            // this one.
            crate::state::pull_metrics::count_repository_pull(&*self.engine, repository.name);
        }

        let content_type = media_type
            .as_deref()
            .map_or(REGISTRY_JSON_CONTENT_TYPE, honoured_media_type);
        content_response(
            document.as_bytes(),
            content_type,
            &served_digest,
            req.method() == "GET",
        )
    }
}

/// The repository a registry request addresses: the name its path names, in the registry of
/// the Region the request resolved to (the endpoint it dialled, else the Region the
/// credential it presents was issued for).
struct AddressedRepository<'a> {
    /// The Region of the registry holding it.
    region: &'a str,
    /// The repository's name, as the path spells it.
    name: &'a str,
}

/// Serve the blob `digest` addresses in `repository`.
///
/// A blob is in a repository because a layer upload against it was COMPLETED, which is the
/// record this reads (`crate::state::Repo::layer_blobs`) and the same record the layer
/// download plane serves the bytes from. A pure READ: following the same address twice
/// answers the same bytes and changes nothing a later caller sees.
fn serve_blob(
    state: &::engine::ServiceState<
        crate::state::ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    >,
    req: &RtRequest,
    repository: &AddressedRepository<'_>,
    digest: &str,
) -> RtResponse {
    let store = state.lock();
    let Ok(repo) = store.repository(repository.region, repository.name) else {
        return registry_error(404, "NAME_UNKNOWN", NAME_UNKNOWN_MESSAGE);
    };
    // A digest this repository holds no blob under addresses no content here, whether it
    // is a digest of another repository's layer or not a content digest at all.
    let Some(blob) = repo.layer_blobs.get(digest) else {
        return registry_error(404, "BLOB_UNKNOWN", BLOB_UNKNOWN_MESSAGE);
    };
    content_response(
        &blob.content,
        BLOB_CONTENT_TYPE,
        &blob.digest,
        req.method() == "GET",
    )
}

/// `media_type` when it is one this plane may state in a `Content-Type`, and the generic
/// registry JSON type when it is not.
///
/// The value reaches here from a PUSHED manifest — `PutImage` records the media type the
/// caller named, else the one the caller's own document states — so it is checked before it
/// travels into a response header: a media type is a short run of visible ASCII
/// ([`MAX_MEDIA_TYPE_LENGTH`]), and a value carrying anything else is not one, never mind
/// one a header may carry. Checked here rather than at the push, because what the registry
/// RECORDS is what the caller stated and what an API read reports is that record; it is
/// only this plane that states it as a header.
fn honoured_media_type(media_type: &str) -> &str {
    let is_media_type = !media_type.is_empty()
        && media_type.len() <= MAX_MEDIA_TYPE_LENGTH
        && media_type
            .chars()
            .all(|character| character.is_ascii_graphic());
    if is_media_type {
        media_type
    } else {
        REGISTRY_JSON_CONTENT_TYPE
    }
}

/// The image one manifest path segment addresses: a digest when the segment is written the
/// way content is addressed, and a tag otherwise.
fn addressed_image(reference: &str) -> crate::ops::types::ImageIdentifier {
    if crate::state::is_layer_digest(reference) {
        return crate::ops::types::ImageIdentifier {
            image_digest: Some(reference.to_string()),
            image_tag: None,
        };
    }
    crate::ops::types::ImageIdentifier {
        image_digest: None,
        image_tag: Some(reference.to_string()),
    }
}

/// The manifest schemas a request's `Accept` headers admit, as the negotiation this
/// service already makes for a stored image takes them
/// (`crate::state::negotiated_manifest`), or `None` for a request that admits whatever the
/// registry holds.
///
/// A client may send the list as one header or as several, and each entry may carry media
/// type parameters (`;q=0.9`) which name no schema. A wildcard admits every schema, so a
/// list carrying one is the same as naming none.
fn accepted_media_types(req: &RtRequest) -> Option<Vec<String>> {
    let mut accepted: Vec<String> = Vec::new();
    for header in req.headers().get_all("accept") {
        for entry in header.split(',') {
            let media_type = entry.split(';').next().unwrap_or(entry).trim();
            if media_type.is_empty() {
                continue;
            }
            if media_type == "*/*" {
                return None;
            }
            if accepted.len() >= MAX_ACCEPTED_MEDIA_TYPES {
                break;
            }
            accepted.push(media_type.to_string());
        }
    }
    (!accepted.is_empty()).then_some(accepted)
}

/// The `docker login` password a request presents, or `None` when it presents no Basic
/// credential this plane can read one out of.
///
/// The header is `Basic <base64 of user:password>`, which is the authorization token ECR
/// issues read back: the token IS that base64 (`AuthorizationData$authorizationToken`), so
/// a client that sends the token verbatim and one that decodes it for `docker login` send
/// the same header. What authenticates is the PASSWORD — the secret this registry minted —
/// so the user name is read past, the same reading (and the same reasoning) as
/// `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/service/repository_data_plane.rs`'s
/// own `basic_password`; neither service's private helper is reachable from the other, and
/// the shared home for it, if a third needs one, is `services/common`.
///
/// The decode is bounded ([`MAX_CREDENTIAL_HEADER_BYTES`]) and total: a header that is not
/// base64, or whose bytes are not text, carries no credential rather than failing the
/// request some other way.
fn presented_password(req: &RtRequest) -> Option<String> {
    let presented = req.headers().get(::http::header::AUTHORIZATION.as_str())?;
    if presented.len() > MAX_CREDENTIAL_HEADER_BYTES {
        return None;
    }
    let (scheme, credential) = presented.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case(BASIC_SCHEME) {
        return None;
    }
    let decoded = ::aws_smithy_types::base64::decode(credential.trim()).ok()?;
    let pair = String::from_utf8(decoded).ok()?;
    let (_user, password) = pair.split_once(':')?;
    (!password.is_empty()).then(|| password.to_string())
}

/// The version check `/v2/` answers: a 200 whose body is the empty JSON document, which is
/// how a client that has presented a credential learns the endpoint speaks this API.
fn version_check() -> RtResponse {
    let body = "{}";
    let mut response = RtResponse::new(status(200), SdkBody::from(body));
    registry_headers(&mut response);
    response
        .headers_mut()
        .insert("content-type", REGISTRY_JSON_CONTENT_TYPE);
    response
        .headers_mut()
        .insert("content-length", body.len().to_string());
    response
}

/// One served piece of registry content: its bytes under `content_type`, its own content
/// digest, and its length.
///
/// `with_body` is false for a `HEAD`, which reports what a `GET` of the same address would
/// answer — the same headers, including the length of the content — without transferring
/// it.
fn content_response(
    content: &[u8],
    content_type: &str,
    digest: &str,
    with_body: bool,
) -> RtResponse {
    let body = if with_body {
        SdkBody::from(content.to_vec())
    } else {
        SdkBody::empty()
    };
    let mut response = RtResponse::new(status(200), body);
    registry_headers(&mut response);
    response
        .headers_mut()
        .insert("content-type", content_type.to_string());
    response
        .headers_mut()
        .insert("content-length", content.len().to_string());
    response
        .headers_mut()
        .insert(CONTENT_DIGEST_HEADER, digest.to_string());
    response
}

/// The body of a refusal that found no such repository in this registry.
const NAME_UNKNOWN_MESSAGE: &str = "repository name not known to registry";

/// The body of a refusal that found no manifest for the reference.
const MANIFEST_UNKNOWN_MESSAGE: &str = "manifest unknown";

/// The body of a refusal that found no blob under the digest.
const BLOB_UNKNOWN_MESSAGE: &str = "blob unknown to registry";

/// The body of a refusal that was presented no credential this registry honours.
const UNAUTHORIZED_MESSAGE: &str = "authentication required";

/// The body of a refusal to a registry operation this registry does not serve.
const UNSUPPORTED_MESSAGE: &str = "the operation is unsupported";

/// The challenge a request presenting no honoured credential is refused with: a `401`
/// naming the scheme and the realm a client fetches a credential from, which is how
/// `docker login` discovers where to authenticate.
fn unauthorized(req: &RtRequest) -> RtResponse {
    let mut response = registry_error(401, "UNAUTHORIZED", UNAUTHORIZED_MESSAGE);
    // The realm is the registry endpoint this request reached, because this emulator is
    // what serves it: a client that follows the challenge has to reach the registry it was
    // challenged by. The value is a CALLER's, so only an endpoint this registry may
    // announce itself at survives the one filter every announced address goes through
    // (`crate::state::honoured_endpoint_host`).
    let challenge = format!(
        "Basic realm=\"https://{}/\",service=\"{CHALLENGE_SERVICE}\"",
        crate::state::honoured_endpoint_host(request_authority(req))
    );
    response
        .headers_mut()
        .insert(::http::header::WWW_AUTHENTICATE.as_str(), challenge);
    response
}

/// The refusal a registry operation this registry does not serve is answered with. RFC 9110
/// requires a `405` to state which methods the target does support, so the refusal names
/// them in `Allow` as well.
fn registry_unsupported() -> RtResponse {
    let mut response = registry_error(405, "UNSUPPORTED", UNSUPPORTED_MESSAGE);
    response
        .headers_mut()
        .insert(::http::header::ALLOW.as_str(), REGISTRY_READ_METHODS);
    response
}

/// One refusal of the registry API, in that API's own envelope.
fn registry_error(status_code: u16, code: &str, message: &str) -> RtResponse {
    let mut response = error_document(status_code, code, message);
    registry_headers(&mut response);
    response
}

/// The `{"errors":[{"code":…,"message":…}]}` document the registry API states a refusal in.
///
/// Every code and message is a constant of this module, never built from the request, and
/// the document is rendered by the JSON serializer this crate already reads manifests with
/// rather than by string concatenation.
fn error_document(status_code: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({
        "errors": [{ "code": code, "message": message }]
    })
    .to_string();
    let length = body.len().to_string();
    let mut response = RtResponse::new(status(status_code), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", REGISTRY_JSON_CONTENT_TYPE);
    response.headers_mut().insert("content-length", length);
    response
}

/// The headers every answer of this plane carries: the registry API version a client reads
/// to discover the API, stated whether the answer is content or a refusal.
fn registry_headers(response: &mut RtResponse) {
    response
        .headers_mut()
        .insert(API_VERSION_HEADER, API_VERSION);
}

