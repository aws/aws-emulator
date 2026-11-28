// SPDX-License-Identifier: CC0-1.0

//! The HTTP a running handler makes ITSELF: one request, carried by the host into this same
//! emulator.
//!
//! A function does not only call control planes through an SDK. The data plane of a search
//! domain, a document store or a queue's own endpoint is reached by SIGNING an HTTP request
//! and sending it — which is what the AWS SDK's own signer
//! ([`super::aws_sdk_guest`]'s `botocore.auth`) and the standard library's HTTP client exist
//! for, and what a handler that indexes or searches documents does. This module is the host
//! half of that: the guest hands over a method, a URL, headers and a body, and the host
//! builds that request, signs it, and dispatches it through the in-process callback
//! ([`::smithy_server_runtime::EmbeddedEmulator`]) into this process's router. The answer is
//! the emulated service's own answer.
//!
//! What the guest can and cannot do here:
//!
//! * The destination is ALWAYS this emulator, and always an endpoint it PUBLISHES: the URL
//!   has to name one — a host of AWS's own naming, whose endpoint prefix names the service
//!   ([`endpoint_prefix_of`]), this emulator's own endpoint with a data plane named in the
//!   path under `service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`, the
//!   `host:port` the core published a RESOURCE's own listener at
//!   ([`addressed_resource_plane`]) — the address a search collection, a database or a
//!   gateway reports in its control plane for a client to dial, which is an ADDRESS and so
//!   names no service in its host: WHICH service answers there is what the service that
//!   opened the listener recorded (`service_common::published_data_planes`) — or a
//!   PRESIGNED URL on an address of THIS MACHINE, which names the service it was signed
//!   for in its own query ([`addressed_presigned_url`]). A URL naming
//!   none of them is REFUSED and nothing is sent. No host name is resolved and no socket is
//!   opened on any path: the request is handed to the callback transport, so it cannot leave
//!   this process.
//! * The guest never sees a credential. The signature is put on by host code with the
//!   credential the framework hands this emulator's handlers, exactly as the SDK seam signs
//!   its calls ([`super::aws_api`]), and the authorization headers the guest supplied are
//!   DROPPED rather than forwarded, so a handler cannot have a request signed as anything
//!   else. The one address no signature of this host's goes on is a PRESIGNED URL's
//!   ([`Addressed::presigned_url`]): such a URL is authorized by the credential it was
//!   signed with rather than by whoever fetches it, so it is sent as it states itself and is
//!   verified and authorized by exactly what the front door applies to the same URL fetched
//!   from outside this process. Every other address, whatever its query carries, is made as
//!   the invocation's own identity.
//! * Every bound is the host's: which methods may be sent, how large the request and the
//!   answer may be, how many requests one run may make and how long one may take — the last
//!   two shared with the SDK seam, because they bound the RUN rather than one call
//!   ([`super::python`]).
//!
//! Nothing about an answer is synthesised: the status, the headers and the body the guest
//! reads back are the ones the emulated service answered, and a request this module cannot
//! make is reported as one that was not made.

use ::std::time::Duration;

use ::service_common::data_plane_addressing::{
    first_segment, host_names_the_bound_interface, host_only, names_a_service,
    names_the_loopback_interface, EMULATOR_PATH_NAMESPACE,
};
use ::smithy_server_runtime::EmbeddedEmulator;

use super::aws_api::{Refusal, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
use super::package::PackageModule;

/// The methods a handler may send.
///
/// The ones HTTP defines for a request with this shape; a value that is not one of them is
/// not a method, so no request is built for it and nothing of the guest's string reaches a
/// request line.
const METHODS: &[&str] = &[
    "GET", "PUT", "POST", "DELETE", "HEAD", "PATCH", "OPTIONS",
];

/// The headers the HOST decides, which a guest-supplied set never carries into the request:
/// the ones that say who the request is from (replaced by the host's own signature), the
/// ones that describe the connection, and the ones the framing is computed from.
const HOST_OWNED_HEADERS: &[&str] = &[
    "authorization",
    "x-amz-date",
    "x-amz-content-sha256",
    "x-amz-security-token",
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "expect",
    "upgrade",
    "proxy-authorization",
];

/// The port a URL that writes none dialled over `http`, which RFC 9110 fixes at 80 — the
/// counterpart of [`::service_common::data_plane_addressing::IMPLICIT_HTTPS_PORT`] for the
/// other scheme a handler may address ([`dialled_authority`]).
const DEFAULT_HTTP_PORT: u16 = 80;

/// What one request came back with: the emulated service's own answer.
#[derive(Debug, Clone)]
pub struct Answered {
    /// The HTTP status it answered.
    pub status: u16,
    /// The headers it carried, in the order it carried them.
    pub headers: Vec<(String, String)>,
    /// Its body, as the bytes it answered.
    pub body: Vec<u8>,
}

/// ONE request a handler made, as the host is handed it.
#[derive(Debug)]
pub struct Sending<'a> {
    /// The in-process callback it is dispatched through.
    pub callback: &'a EmbeddedEmulator,
    /// The Region a URL naming none of its own is signed for: the invocation's.
    pub default_region: &'a str,
    /// The method the handler named.
    pub method: &'a str,
    /// The URL it addressed.
    pub url: &'a str,
    /// The headers it built, as name/value pairs.
    pub headers: &'a [(String, String)],
    /// The body it sent.
    pub body: &'a [u8],
    /// What the run has left of its deadline, which bounds this request: it cannot outlast
    /// the invocation that made it.
    pub within: Duration,
    /// WHO it is made as: the credential the invocation was given
    /// ([`super::aws_api::CallerIdentity`]).
    pub identity: Option<&'a super::aws_api::CallerIdentity>,
    /// Which service answers at an address this emulator published for a resource's own
    /// data plane, for a URL that addresses one ([`addressed_resource_plane`]). `None`
    /// where this emulator vends no such records, and then only the two addressings that
    /// name a service in the URL itself resolve.
    pub published: Option<&'a ::service_common::published_data_planes::PublishedPlanes>,
    /// The state of the EXECUTION ENVIRONMENT the request was made from, for a URL that
    /// addresses the runtime API ([`super::runtime_api`]) or the environment's own metadata
    /// service ([`super::instance_metadata`]): the registrations of the invocation whose
    /// sandbox the handler is running in, and the metadata session it reads its own placement
    /// through. `None` for a run that was given no sandbox, where those two interfaces have no
    /// environment to answer from.
    pub sandbox: Option<&'a super::runtime_api::Sandbox>,
}

/// Make ONE request, as `sending` states it.
pub async fn sent(sending: &Sending<'_>) -> Result<Answered, Refusal> {
    let method = sending.method.to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return Err(Refusal::NotMade(format!(
            "{method} is not an HTTP method a handler may send"
        )));
    }
    if sending.body.len() > MAX_REQUEST_BYTES {
        return Err(Refusal::RequestTooLarge);
    }
    // The RUNTIME API is answered by the host itself, from the address
    // `AWS_LAMBDA_RUNTIME_API` names: it is not a service of this emulator and no signature
    // is put on it, so it is answered before an address is read as one
    // ([`super::runtime_api`]).
    match super::runtime_api::answered(&super::runtime_api::Requested {
        method: &method,
        url: sending.url,
        headers: sending.headers,
        body: sending.body,
        sandbox: sending.sandbox,
    }) {
        super::runtime_api::Reached::Answered(answered) => return Ok(answered),
        // A request that interface never answers is a request that does not complete: it is
        // held for as long as the caller gave it — which is what the run has left of its own
        // deadline — and then reported as one that ran out of time, exactly as the long poll
        // it is does inside an AWS sandbox.
        super::runtime_api::Reached::NeverAnswered => {
            ::tokio::time::sleep(sending.within).await;
            return Err(Refusal::TimedOut);
        }
        super::runtime_api::Reached::Elsewhere => {}
    }
    // The INSTANCE METADATA SERVICE is answered by the host for the same reason: the
    // link-local address a node's own facts are published at is no service of this emulator
    // and nothing is signed for it, so what a handler reading it reaches is the execution
    // environment's own metadata service ([`super::instance_metadata`]).
    match super::instance_metadata::answered(&super::runtime_api::Requested {
        method: &method,
        url: sending.url,
        headers: sending.headers,
        body: sending.body,
        sandbox: sending.sandbox,
    }) {
        super::runtime_api::Reached::Answered(answered) => return Ok(answered),
        // That interface answers every request it reaches, so neither of these is a request
        // it holds: a path it publishes nothing at is answered as that.
        super::runtime_api::Reached::NeverAnswered
        | super::runtime_api::Reached::Elsewhere => {}
    }
    let addressed = addressed(sending.url, sending.default_region)
        .or_else(|| {
            addressed_resource_plane(sending.url, sending.published, sending.default_region)
        })
        .or_else(|| addressed_presigned_url(sending.url, sending.default_region))
        .ok_or(Refusal::UnknownService)?;
    match ::tokio::time::timeout(
        sending.within,
        dispatch(
            sending.callback,
            &addressed,
            &method,
            sending.headers,
            sending.body,
            sending.identity,
        ),
    )
    .await
    {
        Ok(answer) => answer,
        Err(_elapsed) => Err(Refusal::TimedOut),
    }
}

/// WHERE one URL addresses: the service of this emulator it names, the host and the path it
/// is sent to, the Region its signature is scoped to, and WHICH credential the request is
/// made with.
#[derive(Debug, PartialEq, Eq)]
struct Addressed {
    /// The signing name of the service the address names, which is also the credential
    /// scope that routes the request to it inside this emulator.
    signing_name: String,
    /// The host the request is addressed to, as the URL names it.
    host: String,
    /// The path and query it is addressed to.
    target: String,
    /// The Region the signature is scoped to.
    region: String,
    /// Whether the ADDRESS is a presigned URL's own — the one reading that resolves a
    /// service from the credential the URL carries ([`addressed_presigned_url`]), and so the
    /// one request this seam makes with no credential of its own.
    ///
    /// Such a request is dispatched exactly as the URL states it: the credential the URL
    /// carries is WHO the request is from — "a presigned URL is signed by whoever minted it
    /// and carries that credential's own scope, so the later request it names is judged on
    /// the policies in force on THAT principal", as the service serving one reads it — so
    /// putting another credential on it would judge the fetch against the holder of the URL
    /// instead of against its signer, and a URL handed to code that holds no credential of
    /// its own would then authorize nothing. Every OTHER address is made as the identity the
    /// invocation was given, whatever its query happens to carry, so a handler cannot shed
    /// its own identity by writing credential-shaped parameters onto an address that names
    /// its service some other way.
    ///
    /// What bounds the one that is: such a request is handed to the front door exactly as the
    /// same URL fetched from OUTSIDE this process is — whether the credential it presents
    /// verifies, and what the principal it names may do, are the front door's own two
    /// decisions, taken over the same stores and with the same wording. While this emulator
    /// verifies requests, a credential a handler wrote itself cannot verify and is refused
    /// before any handler runs. While it verifies none — the default, where it hands out no
    /// credential and checks no signature — no credential presented to it by anything is
    /// verified either, which is the position every other client of the front door is in.
    presigned_url: bool,
}

/// Read `url` as the address of a service of THIS emulator, or `None` for a URL that names
/// none — which is a request this seam does not make.
fn addressed(url: &str, default_region: &str) -> Option<Addressed> {
    let (authority, target) = authority_and_target(url)?;
    // A host of AWS's OWN naming names the service in it: the endpoint prefix, which is
    // also what a request to that service is signed for.
    let host = host_only(authority);
    if let Some(prefix) = endpoint_prefix_of(host) {
        return Some(Addressed {
            signing_name: prefix,
            host: authority.to_string(),
            target,
            region: region_of(host).unwrap_or_else(|| default_region.to_string()),
            presigned_url: false,
        });
    }
    // A host the front door itself resolves to a service is NOT read here: its reader
    // answers the first label of ANY host, so reading it would make every address resolve
    // to something. The two addressings below are the ones this emulator really serves.
    // Otherwise this is the emulator's own endpoint, where a data plane is named in the
    // path: `/_aws/<plane>/…`, which is the address such a plane publishes for itself.
    let (namespace, under) = first_segment(&target)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, _) = first_segment(under)?;
    if plane.is_empty() || !plane.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let signing_name = plane.to_string();
    Some(Addressed {
        signing_name,
        host: authority.to_string(),
        target,
        region: default_region.to_string(),
        presigned_url: false,
    })
}

/// The authority and the request target `url` names, or `None` for a value that is no
/// HTTP(S) URL at all — which addresses nothing and is a request this seam does not make.
///
/// Read by hand rather than with a URL parser because what the guest handed over is a
/// STRING: a value that is not a URL has to come back as "no address" rather than as a
/// parser's own error.
fn authority_and_target(url: &str) -> Option<(&str, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || !::service_common::data_plane_addressing::is_host_shaped(authority) {
        return None;
    }
    let target = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Some((authority, target))
}

/// WHERE a URL addressing a RESOURCE's own published listener goes, or `None` for one that
/// addresses no such listener.
///
/// A resource whose data plane speaks its own protocol is published on a listener the CORE
/// opened for it (`engine.listeners()`), and what the owning service reports for a client to
/// dial is that listener's `host:port` — an ADDRESS on the machine this emulator runs on,
/// which is the only address a client here resolves. A handler reading an endpoint out of a
/// control-plane response (a collection's `collectionEndpoint`, a cluster's `Endpoint`) was
/// told to dial exactly that.
///
/// Nothing about such an address names a service: its host is an address, so the two readings
/// above answer nothing for it. WHICH service answers there is what the service that OPENED
/// the listener recorded ([`::service_common::published_data_planes`]), so the request is
/// signed for — and routed inside this emulator to — the service that published the very
/// address the handler dialled, and an address this emulator published nothing at stays a
/// request this seam does not make. The Region is the invocation's own: a listener of this
/// emulator is not an endpoint of another Region.
///
/// A URL that writes NO port dialled the default port of its scheme, which is the address the
/// register holds for a plane published on that port ([`dialled_authority`]).
fn addressed_resource_plane(
    url: &str,
    published: Option<&::service_common::published_data_planes::PublishedPlanes>,
    default_region: &str,
) -> Option<Addressed> {
    let (authority, target) = authority_and_target(url)?;
    let published = published?;
    let signing_name = published.answering_service(authority).or_else(|| {
        dialled_authority(url, authority).and_then(|dialled| published.answering_service(&dialled))
    })?;
    Some(Addressed {
        signing_name,
        host: authority.to_string(),
        target,
        region: default_region.to_string(),
        presigned_url: false,
    })
}

/// The `host:port` a URL that writes NO port dialled: its authority with the port its SCHEME
/// fixes, or `None` for a URL that writes one — whose authority is that address already.
///
/// RFC 9110 fixes the default port of `http` at 80 and of `https` at 443, which is exactly why
/// a URL on one of them carries no port at all, while the register above is keyed by
/// `host:port` because one address per listener is its point
/// ([`::service_common::published_data_planes`]). So a plane published on the default port of
/// its protocol is dialled by a URL that writes no port, and this is the address such a URL
/// dialled. Read off the SCHEME the handler wrote rather than assumed: a URL naming neither
/// scheme is no request this seam makes at all ([`authority_and_target`]).
fn dialled_authority(url: &str, authority: &str) -> Option<String> {
    if ::service_common::data_plane_addressing::addressed_port(Some(authority)).is_some() {
        return None;
    }
    let (scheme, _) = url.split_once("://")?;
    let port = match scheme.to_ascii_lowercase().as_str() {
        "http" => DEFAULT_HTTP_PORT,
        "https" => ::service_common::data_plane_addressing::IMPLICIT_HTTPS_PORT,
        _ => return None,
    };
    Some(::service_common::data_plane_addressing::host_port_authority(
        authority, port,
    ))
}

/// WHERE a PRESIGNED URL on this emulator's own endpoint goes, or `None` for a URL that
/// carries no presigned credential.
///
/// A presigned URL is handed out for a client to fetch LATER with an ordinary HTTP client,
/// which is how object access is given to code that holds no credential of its own, and a
/// handler is one such client. Its authorization travels in the QUERY rather than in a
/// header, and that query is the only part of such a URL that names a service: its host is
/// the endpoint the URL was minted against — this emulator's own, which names no service —
/// and its path is laid out by the addressing the minting client chose, which for the object
/// store may put the bucket in the first path segment (the path-style form) rather than in
/// the host (the virtual-hosted form). Reading the host alone therefore attributes such a URL
/// to nothing, for a service this emulator does serve.
///
/// So the service is read off the credential the URL itself carries, by the one reading of
/// that question this emulator has (`service_common::presigned_url_credential`, which reads
/// the SigV4 scope through the front door's own reader of it). Two things narrow which URLs
/// are read this way, because the credential says WHICH service answers and not WHERE:
///
/// * the host has to be an address of THIS PROCESS ([`addresses_this_process`]): the endpoint
///   a presigned URL of this emulator is minted against is an address this process is reached
///   at, and an address that is not is reached by nothing here. The port cannot narrow it
///   further — the request is carried in-process rather than dialled, and a presigned URL
///   names no port of its own beyond the endpoint's;
/// * a host that NAMES a service is not read here at all: the endpoint-prefix reading above
///   has already answered for it, and such a URL is addressed by that name rather than by a
///   credential.
///
/// The Region is the invocation's own, as it is for every address of this emulator: the
/// signature a presigned URL carries is already scoped to the Region it was signed for, and
/// this host puts no signature of its own on such a request ([`dispatch`]).
fn addressed_presigned_url(url: &str, default_region: &str) -> Option<Addressed> {
    let (authority, target) = authority_and_target(url)?;
    let host = host_only(authority);
    if names_a_service(host) || !addresses_this_process(host) {
        return None;
    }
    let signing_name =
        ::service_common::presigned_url_credential::signed_for(&target.parse().ok()?)?;
    Some(Addressed {
        signing_name,
        host: authority.to_string(),
        target,
        region: default_region.to_string(),
        presigned_url: true,
    })
}

/// Whether `host` is an address of THIS PROCESS: the loopback interface
/// (`service_common::data_plane_addressing::names_the_loopback_interface`), or the very
/// address an operator bound this emulator's listeners on
/// (`…::host_names_the_bound_interface`).
///
/// The second reading is the one every plane served on a socket of its own applies to a
/// request's host, and it answers for ANY host on a WILDCARD bind (`0.0.0.0`, `::`), where no
/// single address names the socket. That is right for a request that ARRIVED on that socket
/// and wrong here, where nothing arrived: the question is whether the address the URL NAMES is
/// one this process answers at, and on a wildcard bind an arbitrary host is not — so such a
/// bind leaves the loopback reading alone to answer, rather than admitting every host.
fn addresses_this_process(host: &str) -> bool {
    if names_the_loopback_interface(host) {
        return true;
    }
    let bound_to_one_address =
        ::emulator_server::bind_address_from_env().is_ok_and(|bound| !bound.is_unspecified());
    bound_to_one_address && host_names_the_bound_interface(host)
}

/// The ENDPOINT PREFIX an AWS-shaped host names, which is what a request to that service is
/// signed for.
///
/// AWS names a service's own endpoint `<prefix>.<region>.<zone>` and a RESOURCE's own
/// host either `<resource>.<region>.<prefix>.<zone>` (a search domain) or
/// `<resource>.<prefix>.<region>.<zone>` (a bucket), so the prefix is the label
/// NEXT TO the Region one ([`region_of`]) — after it when the Region is the last label
/// before the zone, and before it otherwise. A host naming no Region at all is a
/// global endpoint, whose prefix is the label before the zone.
///
/// `<zone>` is any of the DNS zones AWS publishes its own service endpoints in, read from the
/// one list of them this tree holds
/// (`service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES`): a service is reached
/// in a partition's own zone (`amazonaws.com`, `amazonaws.com.cn`) and under the suffixes AWS
/// serves its dual-stack and short-form endpoints at (`api.aws`, `on.aws`) — which is where an
/// SDK addresses a service whose endpoint rules resolve to one of them, rather than in the
/// partition zone.
///
/// `None` for a host that is not AWS's own naming — this emulator's own endpoint, an
/// address of anything else — which names no service this way.
fn endpoint_prefix_of(host: &str) -> Option<String> {
    let lowered = host.to_ascii_lowercase();
    let zone = ::service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES
        .iter()
        .find(|suffix| lowered.ends_with(*suffix))?;
    let named = &lowered[..lowered.len() - zone.len()];
    if named.is_empty() {
        return None;
    }
    let service: Vec<&str> = named.split('.').collect();
    let at = service.iter().position(|label| region_shaped(label));
    let prefix = match at {
        Some(at) if at + 1 < service.len() => service[at + 1],
        Some(at) if at > 0 => service[at - 1],
        Some(_) => return None,
        None => service.last().copied()?,
    };
    (!prefix.is_empty()
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    .then(|| prefix.to_string())
}

/// The Region an AWS-shaped host names: the label of it that is a REGION NAME.
///
/// A host of AWS's own naming carries the Region as one label among the service's
/// (`s3.<region>.amazonaws.com`, `search-things-abc.<region>.es.amazonaws.com`,
/// `<bucket>.s3.<region>.amazonaws.com`), so WHICH label it is cannot be read off a fixed
/// position; what every one of them has is exactly one label shaped like a Region name, and
/// that is the one taken. `None` for a host carrying none, whose request is signed for the
/// Region the invocation itself was addressed to.
fn region_of(host: &str) -> Option<String> {
    host.split('.')
        .rev()
        .find(|label| region_shaped(label))
        .map(str::to_string)
}

/// Whether `label` is shaped like a Region name: lower-case letters, then one or more
/// hyphen-separated words, then a number — `us-west-2`, `ap-southeast-4`, `eu-central-1`.
fn region_shaped(label: &str) -> bool {
    let mut parts = label.split('-');
    let Some(partition) = parts.next() else {
        return false;
    };
    let rest: Vec<&str> = parts.collect();
    let Some((number, words)) = rest.split_last() else {
        return false;
    };
    !partition.is_empty()
        && partition.bytes().all(|byte| byte.is_ascii_lowercase())
        && !words.is_empty()
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Build, sign and dispatch the request, and read the answer back.
async fn dispatch(
    callback: &EmbeddedEmulator,
    addressed: &Addressed,
    method: &str,
    headers: &[(String, String)],
    body: &[u8],
    identity: Option<&super::aws_api::CallerIdentity>,
) -> Result<Answered, Refusal> {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{}{}", addressed.host, addressed.target))
        .header("host", addressed.host.clone());
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if HOST_OWNED_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        // A name or a value that is not one a header can carry is not sent: nothing of the
        // guest's text reaches the request's header structure except as a header.
        let (Ok(name), Ok(value)) = (
            ::http::HeaderName::try_from(lower.as_str()),
            ::http::HeaderValue::try_from(value.as_str()),
        ) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    let mut request = builder
        .body(body.to_vec())
        .map_err(|error| Refusal::NotMade(error.to_string()))?;
    // A PRESIGNED URL's own address is sent as the URL states itself
    // ([`Addressed::presigned_url`]); every other address is made as the identity the
    // invocation was given, which is the only credential this seam ever puts on a request.
    if !addressed.presigned_url {
        super::aws_api::signed(
            &mut request,
            &addressed.signing_name,
            &addressed.region,
            callback,
            identity,
        )
        .await
        .map_err(Refusal::NotMade)?;
    }
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| Refusal::NotMade(error.to_string()))?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &callback.http_connector(),
        request,
    )
    .await
    .map_err(|error| Refusal::NotMade(error.to_string()))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let payload = response.body().bytes().ok_or_else(|| {
        Refusal::NotMade("the in-process answer carried a streaming body".to_string())
    })?;
    if payload.len() > MAX_RESPONSE_BYTES {
        return Err(Refusal::ResponseTooLarge);
    }
    Ok(Answered {
        status,
        headers,
        body: payload.to_vec(),
    })
}

/// What a guest reads back from ONE request it sent itself: the emulated service's own
/// status, reason, headers and body, or the reason no request was made.
///
/// ONE document, read by the HTTP client of every runtime this emulator runs a handler in
/// ([`HTTP_CLIENT_SOURCE`] for a Python handler, [`super::node`]'s `http` core module for a
/// node one), because what a request came to does not depend on the language that sent it.
/// The body travels base64-encoded, because the answer is bytes and this document is JSON.
#[must_use]
pub fn answer_document(answered: &Result<Answered, Refusal>) -> String {
    use ::base64::Engine;

    match answered {
        Ok(answer) => ::serde_json::json!({
            "status": answer.status,
            "reason": ::http::StatusCode::from_u16(answer.status)
                .ok()
                .and_then(|status| status.canonical_reason()),
            "headers": answer.headers,
            "body": ::base64::engine::general_purpose::STANDARD.encode(&answer.body),
        })
        .to_string(),
        Err(refused) => refusal_document(refused),
    }
}

/// The same document for a request that was NOT made, naming which refusal it was: the
/// client of each runtime turns it into that language's own failure, so nothing a request
/// never made is reported as an answer.
fn refusal_document(refused: &Refusal) -> String {
    let stated = |refused: &str, detail: String| {
        ::serde_json::json!({ "refused": refused, "detail": detail }).to_string()
    };
    match refused {
        Refusal::UnknownService => stated(
            "unknown_service",
            super::aws_api::reachable_services().join(", "),
        ),
        Refusal::UnknownOperation => stated("unknown_operation", String::new()),
        Refusal::TimedOut => stated("timed_out", String::new()),
        Refusal::RequestTooLarge => stated(
            "not_made",
            format!("the request is larger than the {MAX_REQUEST_BYTES} bytes one request may carry"),
        ),
        Refusal::ResponseTooLarge => stated(
            "not_made",
            format!(
                "the answer is larger than the {MAX_RESPONSE_BYTES} bytes one request may read \
                 back"
            ),
        ),
        Refusal::NotMade(detail) => stated("not_made", detail.clone()),
    }
}

/// The modules of the standard library this RUNTIME supplies in place of the frozen ones:
/// the socket module a Python program expects its operating system to have, the HTTP
/// client that is built on one, and the TLS layer a program imports beside them
/// ([`super::guest_tls`]).
///
/// Resolved before the frozen standard library and after the deployment package's own
/// entries, so these are the `socket`, `http.client` and `ssl` a handler imports, and a
/// package shipping its own still wins.
///
/// The socket module is [`SOCKET_SOURCE`], the DATAGRAM half of it
/// ([`super::xray_daemon::DATAGRAM_SOURCE`]) and the STREAM half beside it
/// ([`super::guest_socket::STREAM_SOURCE`]) together: the names, the errors and the
/// conversions a program reads off `socket` are the first's, the sockets themselves are the
/// second's, and the connections a stream socket carries bytes on are the third's — so there
/// is ONE `socket` a handler can import, whose reachable destinations are the in-process
/// X-Ray daemon, the data-plane listeners this emulator has opened and the addresses of the
/// VPC the function is attached to.
#[must_use]
pub fn runtime_stdlib_modules() -> Vec<PackageModule> {
    [
        (
            "socket.py",
            format!(
                "{SOCKET_SOURCE}{}{}",
                super::xray_daemon::DATAGRAM_SOURCE,
                super::guest_socket::STREAM_SOURCE
            ),
        ),
        ("http/client.py", HTTP_CLIENT_SOURCE.to_string()),
    ]
    .into_iter()
    .map(|(path, source)| PackageModule {
        path: path.to_string(),
        source,
    })
    .chain(super::guest_tls::runtime_stdlib_modules())
    .collect()
}

/// `socket`: the NAMES, errors and conversions a Python program reads off the module it
/// reaches a socket through. The sockets themselves are the datagram and stream halves
/// appended to this source ([`super::xray_daemon::DATAGRAM_SOURCE`],
/// [`super::guest_socket::STREAM_SOURCE`]).
///
/// A Lambda function in this emulator has no HOST network: there is no host interface to bind
/// or listen on, so every call that would do one of those raises the error a Python program
/// handles for it. What a function CAN reach is this emulator's own: the in-process X-Ray
/// daemon a datagram goes to, and the data-plane listeners this emulator has opened, which a
/// stream socket connects to. A NAME is resolved by the resolver of the execution environment
/// ([`super::guest_resolver`], reached through the host seam), which answers out of the zones
/// this emulator can name an address in. The NAMES are all here — the address families, the
/// socket types, the option levels, the errors and the default timeout — so a module that
/// imports `socket` to name one of them imports cleanly, which is what the standard library's
/// own HTTP, email and URL modules do.
const SOCKET_SOURCE: &str = r#"
"""The socket interface of this runtime: a connection reaches this emulator's own listeners."""

# The address families, the socket types and the protocol numbers, as `<sys/socket.h>` and
# `<netinet/in.h>` define them on this platform (the values Python's own `socket` module
# publishes for them). DATA: naming one of them opens nothing.
AF_UNSPEC = 0
AF_UNIX = 1
AF_INET = 2
AF_INET6 = 10
SOCK_STREAM = 1
SOCK_DGRAM = 2
SOCK_RAW = 3
SOCK_SEQPACKET = 5
SOL_SOCKET = 1
IPPROTO_IP = 0
IPPROTO_ICMP = 1
IPPROTO_TCP = 6
IPPROTO_UDP = 17
IPPROTO_IPV6 = 41
IPPROTO_RAW = 255

# The socket-level options a client library sets on a connection it opened — the send and
# receive buffer sizes, the linger and the timeouts among them — as
# `<asm-generic/socket.h>` numbers them.
SO_DEBUG = 1
SO_REUSEADDR = 2
SO_TYPE = 3
SO_ERROR = 4
SO_DONTROUTE = 5
SO_BROADCAST = 6
SO_SNDBUF = 7
SO_RCVBUF = 8
SO_KEEPALIVE = 9
SO_OOBINLINE = 10
SO_LINGER = 13
SO_REUSEPORT = 15
SO_RCVLOWAT = 18
SO_SNDLOWAT = 19
SO_RCVTIMEO = 20
SO_SNDTIMEO = 21
SO_ACCEPTCONN = 30

# The TCP-level options of the same kind — the keepalive timers a pooling client arms among
# them — as `<netinet/tcp.h>` numbers them.
TCP_NODELAY = 1
TCP_MAXSEG = 2
TCP_CORK = 3
TCP_KEEPIDLE = 4
TCP_KEEPINTVL = 5
TCP_KEEPCNT = 6
TCP_SYNCNT = 7
TCP_LINGER2 = 8
TCP_DEFER_ACCEPT = 9
TCP_WINDOW_CLAMP = 10
TCP_INFO = 11
TCP_QUICKACK = 12
TCP_CONGESTION = 13
TCP_USER_TIMEOUT = 18
TCP_FASTOPEN = 23

# The one IPv6-level option a program reads off this module, and the shutdown directions.
IPV6_V6ONLY = 26
SHUT_RD = 0
SHUT_WR = 1
SHUT_RDWR = 2

# The flags a send or a receive takes, as `<sys/socket.h>` defines them.
MSG_OOB = 1
MSG_PEEK = 2
MSG_DONTROUTE = 4
MSG_CTRUNC = 8
MSG_TRUNC = 32
MSG_DONTWAIT = 64
MSG_EOR = 128
MSG_WAITALL = 256
MSG_NOSIGNAL = 16384
MSG_MORE = 32768

# The flags an address lookup takes and the errors it reports, as `<netdb.h>` defines them:
# a client library branches on the error of a lookup this runtime answers for a name.
AI_PASSIVE = 1
AI_CANONNAME = 2
AI_NUMERICHOST = 4
AI_V4MAPPED = 8
AI_ALL = 16
AI_ADDRCONFIG = 32
AI_NUMERICSERV = 1024
EAI_BADFLAGS = -1
EAI_NONAME = -2
EAI_AGAIN = -3
EAI_FAIL = -4
EAI_NODATA = -5
EAI_FAMILY = -6
EAI_SOCKTYPE = -7
EAI_SERVICE = -8
EAI_ADDRFAMILY = -9
EAI_MEMORY = -10
EAI_SYSTEM = -11
NI_NUMERICHOST = 1
NI_NUMERICSERV = 2
NI_NOFQDN = 4
NI_NAMEREQD = 8
NI_DGRAM = 16

# The special addresses and the listen ceiling a program names, as `<netinet/in.h>` and
# `<sys/socket.h>` state them.
INADDR_ANY = 0
INADDR_BROADCAST = 0xFFFFFFFF
INADDR_LOOPBACK = 0x7F000001
INADDR_NONE = 0xFFFFFFFF
SOMAXCONN = 4096

has_ipv6 = False

# What `socket.timeout` and `socket.error` are in a current Python: the builtin errors,
# which is what a caller catches.
error = OSError
timeout = TimeoutError


class herror(OSError):
    """A name service failure."""


class gaierror(OSError):
    """An address resolution failure."""


# The sentinel the standard library passes to mean "the default timeout", which modules
# such as `http.client` read off this module.
_GLOBAL_DEFAULT_TIMEOUT = object()

_UNREACHABLE = "a Lambda function in this emulator has no host network to reach"

# The loopback interface of this machine, which is where the planes this emulator opens are
# served: the one name this runtime resolves, because it names THIS process's own host.
_LOOPBACK_NAMES = ("localhost", "localhost.localdomain", "ip6-localhost")
_LOOPBACK_ADDRESS = "127.0.0.1"


def getdefaulttimeout():
    return None


def setdefaulttimeout(timeout):
    return None


def socketpair(family=AF_UNIX, type=SOCK_STREAM, proto=0):
    """A pair of connected sockets, both ends in THIS process.

    Nothing of the host is involved — a pair is two byte queues and the two ends that read and
    write them ([`_lambda_pipes`]) — so this is served rather than refused: it is what the
    standard library's own event loop opens as its wakeup pipe, and what a program that wants
    a channel between two parts of itself opens.
    """
    import _lambda_pipes

    return _lambda_pipes.socketpair(family, type, proto)


def create_connection(
    address,
    timeout=_GLOBAL_DEFAULT_TIMEOUT,
    source_address=None,
    all_errors=False,
):
    """A stream socket connected to `address`, as the standard library's own builds one.

    The connection is the runtime's: an address this emulator serves a data plane at is
    connected to, and any other is answered the way a host with nothing listening there
    answers it. `source_address` binds nothing, because a function has no host interface to
    bind to.
    """
    opened = socket(AF_INET, SOCK_STREAM)
    try:
        if timeout is not _GLOBAL_DEFAULT_TIMEOUT:
            opened.settimeout(timeout)
        opened.connect(address)
    except BaseException:
        opened.close()
        raise
    return opened


def create_server(*_arguments, **_keywords):
    raise error(_UNREACHABLE)


def _is_ipv4_address(value):
    parts = value.split(".")
    if len(parts) != 4:
        return False
    return all(part.isdigit() and 0 <= int(part) <= 255 for part in parts)


def _is_ipv6_address(value):
    """Whether `value` is an IPv6 address literal: hexadecimal pieces separated by at least
    two colons, as an address has, so a name carrying one colon is not read as an address."""
    pieces = value.split(":")
    if len(pieces) < 3 or len(pieces) > 8:
        return False
    return all(
        not piece or len(piece) <= 4 and all(digit in "0123456789abcdefABCDEF" for digit in piece)
        for piece in pieces
    )


def getaddrinfo(host, port, family=AF_UNSPEC, type=0, proto=0, flags=0):
    """The addresses `host`:`port` names.

    An ADDRESS is a pure conversion: the address itself is the answer, resolved against
    nothing. A NAME is resolved by the resolver of this execution environment
    ([`_resolved`]): this machine's own names — which is where the planes this emulator opens
    are served — the private zone of the VPC the function is attached to, and the public names
    the DNS standards themselves fix. A name that resolver holds no address under raises what a
    resolver raises for a name that does not exist, and an address of a family the caller did
    not ask for raises what a resolver raises for that.
    """
    if isinstance(host, (bytes, bytearray)):
        host = bytes(host).decode("utf-8", "replace")
    named = "" if host is None else str(host)
    number = _stream_port(port)
    kind = type or SOCK_STREAM
    protocol = proto or (IPPROTO_UDP if kind == SOCK_DGRAM else IPPROTO_TCP)
    address = named.strip("[]")
    if named.lower() in _LOOPBACK_NAMES:
        address = _LOOPBACK_ADDRESS
    if _is_ipv4_address(address):
        if family not in (AF_UNSPEC, AF_INET):
            raise gaierror(-6, "Address family for hostname not supported")
        return [(AF_INET, kind, protocol, "", (address, number))]
    if _is_ipv6_address(address):
        if family not in (AF_UNSPEC, AF_INET6):
            raise gaierror(-6, "Address family for hostname not supported")
        return [(AF_INET6, kind, protocol, "", (address, number, 0, 0))]
    resolved = _resolved(address)
    if not resolved:
        raise gaierror(EAI_NONAME, "Name or service not known")
    answered = []
    for entry in resolved:
        held = entry.get("address") or ""
        if entry.get("family") == "inet6":
            if family in (AF_UNSPEC, AF_INET6):
                answered.append((AF_INET6, kind, protocol, "", (held, number, 0, 0)))
        elif family in (AF_UNSPEC, AF_INET):
            answered.append((AF_INET, kind, protocol, "", (held, number)))
    if not answered:
        # The name resolves, but to no address of the family the caller asked about.
        raise gaierror(EAI_ADDRFAMILY, "Address family for hostname not supported")
    return answered


def _resolved(host):
    """The addresses this environment's own resolver answers `host` with, as one entry per
    address naming its family — and none for a name it holds no address under.

    The resolver is the host's: a name is looked up in the zones this emulator can name an
    address in, and an answer it cannot be asked for at all is a name that resolved to
    nothing, which is what the caller reports.
    """
    answered = _resolution(host)
    held = answered.get("addresses")
    return held if isinstance(held, list) else []


def _resolution(host):
    """The WHOLE answer this environment's resolver gives for `host`: the addresses, the
    canonical name the lookup ended at and the aliases it passed through — and an empty
    answer for a name it holds no address under."""
    try:
        answered = _net_asked("resolve", {"host": host})
    except gaierror:
        return {}
    except OSError:
        return {}
    return answered if isinstance(answered, dict) else {}


def getnameinfo(*_arguments, **_keywords):
    raise gaierror(_UNREACHABLE)


def gethostbyname(name):
    """The address a name stands for: the first IPv4 address this environment's own resolver
    holds for it ([`_resolved`]), and what a resolver raises for a name that does not exist
    when it holds none."""
    named = str(name)
    if named.lower() in _LOOPBACK_NAMES:
        return _LOOPBACK_ADDRESS
    if _is_ipv4_address(named):
        return named
    for entry in _resolved(named):
        if entry.get("family") != "inet6":
            return entry.get("address") or ""
    raise gaierror(EAI_NONAME, "Name or service not known")


def gethostbyname_ex(name):
    """The whole answer to a name: the canonical name it resolved to, the aliases it stands
    behind, and every IPv4 address held under the canonical name.

    A name standing at an ALIAS resolves to the name that alias stands for, so the canonical
    name is that one and the names the lookup passed through are the aliases — which is what
    this reports, out of the one answer the environment's resolver gave
    ([`_resolution`]). A name it holds no address under raises what a resolver raises for a
    name that does not exist, as `gethostbyname` does.
    """
    named = str(name)
    if named.lower() in _LOOPBACK_NAMES:
        return (named, [], [_LOOPBACK_ADDRESS])
    if _is_ipv4_address(named):
        return (named, [], [named])
    answered = _resolution(named)
    held = answered.get("addresses")
    addresses = [
        entry.get("address") or ""
        for entry in (held if isinstance(held, list) else [])
        if entry.get("family") != "inet6"
    ]
    if not addresses:
        raise gaierror(EAI_NONAME, "Name or service not known")
    aliases = answered.get("aliases")
    canonical = answered.get("canonical")
    return (
        canonical if isinstance(canonical, str) and canonical else named,
        list(aliases) if isinstance(aliases, list) else [],
        addresses,
    )


def gethostbyaddr(*_arguments, **_keywords):
    raise gaierror(_UNREACHABLE)


def gethostname():
    raise error(_UNREACHABLE)


def getfqdn(name=""):
    raise error(_UNREACHABLE)


def inet_aton(address):
    """The four bytes of a dotted-quad address. A pure conversion: it resolves nothing."""
    parts = address.split(".")
    if len(parts) != 4:
        raise error("illegal IP address string passed to inet_aton")
    numbers = []
    for part in parts:
        if not part.isdigit() or not 0 <= int(part) <= 255:
            raise error("illegal IP address string passed to inet_aton")
        numbers.append(int(part))
    return bytes(numbers)


def inet_ntoa(packed):
    """The dotted quad four bytes spell, the other way round."""
    if len(packed) != 4:
        raise error("packed IP wrong length for inet_ntoa")
    return ".".join(str(byte) for byte in packed)


def htons(value):
    return ((value & 0xFF) << 8) | ((value >> 8) & 0xFF)


ntohs = htons


def htonl(value):
    return (
        ((value & 0xFF) << 24)
        | ((value & 0xFF00) << 8)
        | ((value >> 8) & 0xFF00)
        | ((value >> 24) & 0xFF)
    )


ntohl = htonl
"#;

/// `http.client`: the HTTP client of this runtime.
///
/// The surface is the standard library's — a connection you make a request on and read a
/// response from, with the same class and attribute names, so `urllib.request` and a handler
/// using the client directly both work — and the TRANSPORT is the host seam above: one
/// signed request into this emulator per `getresponse`. Nothing here opens a socket, because
/// there is none to open; a URL this emulator serves no service for is reported as the
/// connection failure it is.
const HTTP_CLIENT_SOURCE: &str = r##"
"""The HTTP client of this runtime: a request is carried by the runtime, not by a socket."""

import base64
import email.message
import http
import io
import json
import socket

_host = __import__("_lambda_host")

HTTP_PORT = 80
HTTPS_PORT = 443

# The methods a request carries a body on, which is what decides the `Content-Length` of a
# request with no body at all.
_METHODS_EXPECTING_BODY = frozenset(("PATCH", "POST", "PUT"))

# The reason phrase of each status, as the standard library's own mapping holds it.
responses = dict((status.value, status.phrase) for status in http.HTTPStatus)

globals().update((status.name, status.value) for status in http.HTTPStatus)


class HTTPException(Exception):
    pass


class NotConnected(HTTPException):
    pass


class InvalidURL(HTTPException):
    pass


class UnknownProtocol(HTTPException):
    pass


class UnknownTransferEncoding(HTTPException):
    pass


class BadStatusLine(HTTPException):
    pass


class LineTooLong(HTTPException):
    pass


class RemoteDisconnected(ConnectionResetError, BadStatusLine):
    pass


class ResponseNotReady(HTTPException):
    pass


class CannotSendRequest(HTTPException):
    pass


class CannotSendHeader(HTTPException):
    pass


class ImproperConnectionState(HTTPException):
    pass


class HTTPMessage(email.message.Message):
    """The headers of one answer, as the standard library's own class."""

    def getallmatchingheaders(self, name):
        name = name.lower() + ":"
        return [line for line in self.keys() if line.lower().startswith(name)]


# How long one header line may be, and how many of them one message may carry, as the
# standard library's own reader bounds them.
_MAXLINE = 65536
_MAXHEADERS = 100


def _read_headers(fp):
    """The header lines of one message, read off `fp` up to the blank line that ends them."""
    headers = []
    while True:
        line = fp.readline(_MAXLINE + 1)
        if len(line) > _MAXLINE:
            raise LineTooLong("header line")
        headers.append(line)
        if len(headers) > _MAXHEADERS:
            raise HTTPException("got more than %d headers" % _MAXHEADERS)
        if line in (b"\r\n", b"\n", b""):
            break
    return headers


def parse_headers(fp, _class=HTTPMessage):
    """The headers of one message, parsed off `fp`.

    What the standard library's own HTTP SERVER reads a request's headers through, and what
    its client reads a response's through: the lines up to the blank one, parsed by the email
    parser as the message they are. That parser is imported HERE rather than with this
    module, so importing the HTTP client costs only what the client itself needs.
    """
    import email.parser

    headers = _read_headers(fp)
    text = b"".join(headers).decode("iso-8859-1")
    return email.parser.Parser(_class=_class).parsestr(text)


class HTTPResponse(io.BytesIO):
    """One answer, read the way the standard library's response is read.

    The bytes are the ones the answer carried, so this is a reading position over them: a
    `read()` returns what the service answered and nothing is fetched a second time.
    """

    def __init__(self, status, reason, headers, body, url=None, method=None):
        io.BytesIO.__init__(self, body)
        self.status = status
        self.code = status
        self.reason = reason
        self.version = 11
        self.url = url
        self._method = method
        self.headers = HTTPMessage()
        for name, value in headers:
            self.headers[name] = value
        self.msg = self.headers
        self.chunked = False
        self.will_close = False
        self.length = len(body)

    def info(self):
        return self.headers

    def geturl(self):
        return self.url

    def getcode(self):
        return self.status

    def getheader(self, name, default=None):
        value = self.headers.get(name)
        return default if value is None else value

    def getheaders(self):
        return list(self.headers.items())

    def isclosed(self):
        return self.closed

    def begin(self):
        return None

    def fileno(self):
        raise OSError("this response has no file descriptor")

    def flush(self):
        return None


class HTTPConnection:
    """A connection to one host, as the standard library's client builds one.

    No socket is opened: the request built here is handed to the runtime, which signs it
    and carries it into this emulator ([`super::sent`] in the host).
    """

    default_port = HTTP_PORT
    _scheme = "http"
    debuglevel = 0
    # The protocol version a request is made under, which `urllib.request` reads off the
    # class to decide whether it may keep the connection alive.
    _http_vsn = 11
    _http_vsn_str = "HTTP/1.1"
    auto_open = 1

    def __init__(
        self,
        host,
        port=None,
        timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
        source_address=None,
        blocksize=8192,
    ):
        self.host, self.port = self._split(host, port)
        self.timeout = timeout
        self.source_address = source_address
        self.blocksize = blocksize
        self.sock = None
        self._reset()

    def _reset(self):
        self._method = None
        self._path = None
        self._headers = []
        self._body = b""
        self._response = None

    @staticmethod
    def _split(host, port):
        if port is None and host and host.rfind(":") > host.rfind("]"):
            name, _, carried = host.rpartition(":")
            if carried.isdigit():
                return name.strip("[]"), int(carried)
            raise InvalidURL("nonnumeric port: '%s'" % (carried,))
        return host, port

    @property
    def _authority(self):
        if self.port is None or self.port == self.default_port:
            return self.host
        if ":" in self.host and not self.host.startswith("["):
            return "[%s]:%d" % (self.host, self.port)
        return "%s:%d" % (self.host, self.port)

    @staticmethod
    def _get_content_length(body, method):
        """The `Content-Length` a body of this shape gives a request of this method.

        `urllib.request` asks the class for it to decide whether to set the header itself.
        A method that expects a body carries `0` for no body at all; a stream's length is
        not known until it is read.
        """
        if body is None:
            return 0 if method.upper() in _METHODS_EXPECTING_BODY else None
        if hasattr(body, "read"):
            return None
        if isinstance(body, str):
            return len(body.encode("utf-8"))
        try:
            return len(body)
        except TypeError:
            return None

    def set_debuglevel(self, level):
        self.debuglevel = level

    def set_tunnel(self, host, port=None, headers=None):
        raise OSError("a Lambda function in this emulator cannot tunnel a connection")

    def connect(self):
        return None

    def close(self):
        self._reset()

    def putrequest(self, method, url, skip_host=False, skip_accept_encoding=False):
        self._reset()
        self._method = method
        self._path = url

    def putheader(self, header, *values):
        for value in values:
            if isinstance(value, bytes):
                value = value.decode("latin-1")
            self._headers.append((str(header), str(value)))

    def endheaders(self, message_body=None, *, encode_chunked=False):
        if message_body is not None:
            self.send(message_body)

    def send(self, data):
        self._body += _as_bytes(data)

    def request(self, method, url, body=None, headers=None, *, encode_chunked=False):
        self.putrequest(method, url)
        for header, value in (headers or {}).items():
            self.putheader(header, value)
        self.endheaders(body)

    def getresponse(self):
        if self._method is None or self._path is None:
            raise ResponseNotReady()
        url = "%s://%s%s" % (self._scheme, self._authority, self._path)
        answered = json.loads(
            _host.request(
                self._method,
                url,
                json.dumps(self._headers),
                base64.b64encode(self._body).decode("ascii"),
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            detail = answered.get("detail") or ""
            if refused == "timed_out":
                raise socket.timeout(detail)
            if refused == "unknown_service":
                raise socket.gaierror(
                    "%s addresses no service this emulator serves%s"
                    % (url, ": " + detail if detail else "")
                )
            raise OSError(detail or "the request was not made")
        self._response = HTTPResponse(
            answered.get("status", 0),
            answered.get("reason") or responses.get(answered.get("status", 0), ""),
            [tuple(pair) for pair in answered.get("headers") or []],
            base64.b64decode(answered.get("body") or ""),
            url=url,
            method=self._method,
        )
        return self._response


class HTTPSConnection(HTTPConnection):
    """The same connection for an `https` URL.

    There is no TLS to negotiate — the request never reaches a socket — so a context or a
    certificate a caller passes is accepted and unused; what the request carries instead is
    the runtime's own signature.
    """

    default_port = HTTPS_PORT
    _scheme = "https"

    def __init__(
        self,
        host,
        port=None,
        key_file=None,
        cert_file=None,
        timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
        source_address=None,
        *,
        context=None,
        check_hostname=None,
        blocksize=8192,
    ):
        HTTPConnection.__init__(
            self,
            host,
            port=port,
            timeout=timeout,
            source_address=source_address,
            blocksize=blocksize,
        )
        self.key_file = key_file
        self.cert_file = cert_file


def _create_https_context(http_version=11):
    """The TLS context an `https` client would negotiate with.

    `urllib.request` asks this module for one when a caller passed none. There is no socket
    to negotiate on here and nothing is negotiated, so there is no context either: the
    request's own signature is what it carries instead.
    """
    return None


def _as_bytes(data):
    """The bytes one `send` carries, out of what a caller passed."""
    if data is None:
        return b""
    if isinstance(data, (bytes, bytearray)):
        return bytes(data)
    if isinstance(data, str):
        return data.encode("utf-8")
    read = getattr(data, "read", None)
    if read is not None:
        return _as_bytes(read())
    if isinstance(data, (list, tuple)):
        return b"".join(_as_bytes(one) for one in data)
    raise TypeError("a request body cannot be %r" % (type(data).__name__,))
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the addressing table: a URL, and the service of this emulator it names.
    struct Addressing {
        case: &'static str,
        /// The URL the handler asked for.
        url: &'static str,
        /// The signing name, path and Region it addresses, or `None` for a URL that names
        /// no service of this emulator.
        expected: Option<(&'static str, &'static str, &'static str)>,
    }

    /// WHICH addresses a handler's own request may name, over a table whose rows span what
    /// the rule generalises over: a service's own endpoint host, a resource's own host,
    /// this emulator's own endpoint with a data plane named in the path, and every shape of
    /// address that names no service of this emulator at all.
    #[test]
    fn only_an_address_this_emulator_serves_is_addressed() {
        let cases: &[Addressing] = &[
            Addressing {
                case: "a service's own regional endpoint",
                url: "https://s3.us-west-2.amazonaws.com/bucket/key",
                expected: Some(("s3", "/bucket/key", "us-west-2")),
            },
            Addressing {
                case: "a search domain's own endpoint",
                url: "https://search-things-abc.eu-west-1.es.amazonaws.com/index/_search?q=a",
                expected: Some(("es", "/index/_search?q=a", "eu-west-1")),
            },
            Addressing {
                // The dual-stack zone AWS publishes a service endpoint in, which is where an
                // SDK whose endpoint rules resolve to it addresses the service.
                case: "a service's own endpoint in the dual-stack zone",
                url: "https://s3vectors.us-west-2.api.aws/CreateIndex",
                expected: Some(("s3vectors", "/CreateIndex", "us-west-2")),
            },
            Addressing {
                case: "a service's own endpoint in another partition's zone",
                url: "https://dynamodb.cn-north-1.amazonaws.com.cn/",
                expected: Some(("dynamodb", "/", "cn-north-1")),
            },
            Addressing {
                case: "this emulator's endpoint, with the plane named in the path",
                url: "https://127.0.0.1:9297/_aws/es/things/index/_doc/1",
                expected: Some(("es", "/_aws/es/things/index/_doc/1", "us-west-2")),
            },
            Addressing {
                case: "the same endpoint with no path at all",
                url: "https://127.0.0.1:9297",
                expected: None,
            },
            Addressing {
                case: "this emulator's endpoint naming no plane",
                url: "https://127.0.0.1:9297/index/_search",
                expected: None,
            },
            Addressing {
                case: "a host that is not this emulator's and names no service",
                url: "https://example.test/anything",
                expected: None,
            },
            Addressing {
                case: "a scheme that is not HTTP",
                url: "ftp://127.0.0.1/x",
                expected: None,
            },
            Addressing {
                case: "a value that is no URL at all",
                url: "not a url",
                expected: None,
            },
        ];
        for row in cases {
            let case = row.case;
            let addressed = addressed(row.url, "us-west-2");
            let actual = addressed.as_ref().map(|addressed| {
                (
                    addressed.signing_name.as_str(),
                    addressed.target.as_str(),
                    addressed.region.as_str(),
                )
            });
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected {:?}, got {actual:?}",
                row.expected
            );
        }
        // The Region a host names, over the label layouts AWS's own hosts put it in.
        let regions: &[(&str, &str, Option<&str>)] = &[
            ("a service endpoint", "s3.us-west-2.amazonaws.com", Some("us-west-2")),
            (
                "a resource's own host",
                "search-things-abc.eu-west-1.es.amazonaws.com",
                Some("eu-west-1"),
            ),
            (
                "a virtual-hosted bucket",
                "bucket.s3.ap-southeast-4.amazonaws.com",
                Some("ap-southeast-4"),
            ),
            ("one naming no Region", "127.0.0.1", None),
            ("a global endpoint", "iam.amazonaws.com", None),
        ];
        for (case, host, expected) in regions {
            let actual = region_of(host);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// WHICH of a handler's own requests addresses a PRESIGNED URL on this emulator's own
    /// endpoint, over a table whose rows span what the rule generalises over: which query
    /// form carries the credential, which service its scope names, which shape of this
    /// machine's address the endpoint is reached by, how the addressing lays the resource out
    /// in the path, and the URLs that carry no presigned credential, name a service in their
    /// host instead, or address a host off this machine.
    #[test]
    fn a_presigned_url_is_addressed_for_the_service_its_own_query_names() {
        /// The credential a legacy presigned URL carries, in the parameters that form uses.
        const LEGACY: &str = "AWSAccessKeyId=AKIDEXAMPLE&Signature=ZGVhZGJlZWY%3D&\
                              Expires=1762000000";
        /// The credential a SigV4-presigned URL carries, with the scope separators encoded
        /// as a client encodes them.
        const SIGV4: &str = "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE\
                             %2F20240101%2Feu-west-1%2Fs3%2Faws4_request&\
                             X-Amz-Date=20240101T000000Z&X-Amz-Expires=600&\
                             X-Amz-SignedHeaders=host&X-Amz-Signature=deadbeef";
        let cases: &[Addressing] = &[
            Addressing {
                case: "a path-style legacy presigned URL on this emulator's endpoint",
                url: "https://127.0.0.1:9297/reports/fetched.bin?AWSAccessKeyId=AKIDEXAMPLE&\
                      Signature=ZGVhZGJlZWY%3D&Expires=1762000000",
                expected: Some((
                    "s3",
                    "/reports/fetched.bin?AWSAccessKeyId=AKIDEXAMPLE&Signature=ZGVhZGJlZWY%3D&\
                     Expires=1762000000",
                    "us-west-2",
                )),
            },
            Addressing {
                case: "a key of several segments, on the loopback name",
                url: "https://localhost:9297/reports/nested/deep/object.json?\
                      AWSAccessKeyId=AKIDEXAMPLE&Signature=ZGVhZGJlZWY%3D&Expires=1762000000",
                expected: Some((
                    "s3",
                    "/reports/nested/deep/object.json?AWSAccessKeyId=AKIDEXAMPLE&\
                     Signature=ZGVhZGJlZWY%3D&Expires=1762000000",
                    "us-west-2",
                )),
            },
            Addressing {
                case: "the SigV4 query form, whose scope names the service",
                url: "https://127.0.0.1:9297/reports/fetched.bin?X-Amz-Algorithm=\
                      AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20240101%2Feu-west-1\
                      %2Fs3%2Faws4_request&X-Amz-Date=20240101T000000Z&X-Amz-Expires=600&\
                      X-Amz-SignedHeaders=host&X-Amz-Signature=deadbeef",
                // The Region is the invocation's own: the signature the URL carries is
                // already scoped to the one it was signed for, and this host adds none.
                expected: Some((
                    "s3",
                    "/reports/fetched.bin?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=\
                     AKIDEXAMPLE%2F20240101%2Feu-west-1%2Fs3%2Faws4_request&X-Amz-Date=\
                     20240101T000000Z&X-Amz-Expires=600&X-Amz-SignedHeaders=host&\
                     X-Amz-Signature=deadbeef",
                    "us-west-2",
                )),
            },
            Addressing {
                case: "the IPv6 loopback literal this emulator may be reached at",
                url: "https://[::1]:9297/reports/fetched.bin?AWSAccessKeyId=AKIDEXAMPLE&\
                      Signature=ZGVhZGJlZWY%3D&Expires=1762000000",
                expected: Some((
                    "s3",
                    "/reports/fetched.bin?AWSAccessKeyId=AKIDEXAMPLE&Signature=ZGVhZGJlZWY%3D&\
                     Expires=1762000000",
                    "us-west-2",
                )),
            },
            Addressing {
                case: "a URL on the same endpoint carrying no presigned credential",
                url: "https://127.0.0.1:9297/reports/fetched.bin",
                expected: None,
            },
            Addressing {
                case: "a URL whose query is some other request's parameters",
                url: "https://127.0.0.1:9297/reports/fetched.bin?versionId=3",
                expected: None,
            },
            Addressing {
                // An address no bind of this emulator makes its own: not loopback, and the
                // reading that admits the operator's own address admits ONE address rather
                // than any host, so this row holds under every bind but one naming this very
                // literal ([`addresses_this_process`]).
                case: "a presigned URL addressed to a host off this machine",
                url: "https://10.1.2.3:8080/reports/fetched.bin?AWSAccessKeyId=AKIDEXAMPLE&\
                      Signature=ZGVhZGJlZWY%3D&Expires=1762000000",
                expected: None,
            },
            Addressing {
                case: "a presigned URL addressed to a name that is not this machine's",
                url: "https://objects.example.test/reports/fetched.bin?\
                      AWSAccessKeyId=AKIDEXAMPLE&Signature=ZGVhZGJlZWY%3D&Expires=1762000000",
                expected: None,
            },
        ];
        for row in cases {
            let case = row.case;
            let addressed = addressed(row.url, "us-west-2")
                .or_else(|| addressed_presigned_url(row.url, "us-west-2"));
            let actual = addressed.as_ref().map(|addressed| {
                (
                    addressed.signing_name.as_str(),
                    addressed.target.as_str(),
                    addressed.region.as_str(),
                )
            });
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected {:?}, got {actual:?}",
                row.expected
            );
        }
        // A host that NAMES a service is addressed by that name, whichever credential form
        // the URL carries: the endpoint-prefix reading answers first and the presigned
        // reading declines such a URL outright.
        for (case, url, expected) in [
            (
                "a legacy presigned URL on a service's own endpoint",
                format!("https://s3.us-east-2.amazonaws.com/bucket/key?{LEGACY}"),
                ("s3", "us-east-2"),
            ),
            (
                "a SigV4 presigned URL on a virtual-hosted bucket host",
                format!("https://bucket.s3.ap-southeast-4.amazonaws.com/key?{SIGV4}"),
                ("s3", "ap-southeast-4"),
            ),
        ] {
            let declined = addressed_presigned_url(&url, "us-west-2");
            assert!(
                declined.is_none(),
                "case {case:?}: a host that names a service is not read as a presigned \
                 address, got {declined:?}"
            );
            let addressed = addressed(&url, "us-west-2");
            let actual = addressed
                .as_ref()
                .map(|addressed| (addressed.signing_name.as_str(), addressed.region.as_str()));
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// WHICH of a handler's own requests addresses a RESOURCE's own published listener, over
    /// a table whose rows span what the rule generalises over: every shape of address the
    /// core publishes such a listener at (a loopback literal with a port, a loopback name, a
    /// bracketed IPv6 literal, an interface address the operator bound), two services' own
    /// listeners on one engine, a plane published on the default port of its scheme — which a
    /// URL writes no port for — and the requests that address no published listener at all
    /// (another port of the same host, a listener that has been withdrawn, a host that is a
    /// name, a value that is no URL).
    #[test]
    fn a_published_listener_address_is_addressed_for_the_service_that_published_it() {
        use ::service_common::published_data_planes::PublishedPlanes;

        /// One row: the URL the handler dialled, and the signing name, target and Region the
        /// request is dispatched with.
        struct Row {
            case: &'static str,
            url: &'static str,
            expected: Option<(&'static str, &'static str, &'static str)>,
        }
        let engine = ::engine::InMemoryEngine::new();
        let published = PublishedPlanes::of(&engine).expect("the engine vends the records");
        // The addresses two services opened listeners at, as each published them.
        for (authority, signing_name) in [
            ("127.0.0.1:37333", "aoss"),
            ("localhost:37334", "aoss"),
            ("[::1]:37335", "aoss"),
            ("10.1.2.3:8443", "rds-data"),
            ("127.0.0.1:37336", "aoss"),
        ] {
            published.publish(authority, signing_name);
        }
        // One of them is closed again, so its address stops resolving.
        published.withdraw("127.0.0.1:37336");
        let rows: &[Row] = &[
            Row {
                case: "a collection's own listener on the loopback interface",
                url: "https://127.0.0.1:37333/items/_doc/good-doc",
                expected: Some(("aoss", "/items/_doc/good-doc", "us-west-2")),
            },
            Row {
                case: "a listener on the loopback name, with a query",
                url: "https://localhost:37334/items/_search?size=1",
                expected: Some(("aoss", "/items/_search?size=1", "us-west-2")),
            },
            Row {
                case: "a listener bound to the IPv6 loopback",
                url: "https://[::1]:37335/items",
                expected: Some(("aoss", "/items", "us-west-2")),
            },
            Row {
                case: "another service's listener, addressed with no path at all",
                url: "https://10.1.2.3:8443",
                expected: Some(("rds-data", "/", "us-west-2")),
            },
            Row {
                case: "a listener whose port has been closed again",
                url: "https://127.0.0.1:37336/items",
                expected: None,
            },
            Row {
                case: "another port of a host that has a listener",
                url: "https://127.0.0.1:9297/items",
                expected: None,
            },
            Row {
                case: "a host that is a name nothing was published at",
                url: "https://example.test/items",
                expected: None,
            },
            Row {
                case: "a scheme that is not HTTP",
                url: "ftp://127.0.0.1:37333/items",
                expected: None,
            },
            Row {
                case: "a value that is no URL at all",
                url: "not a url",
                expected: None,
            },
        ];
        for row in rows {
            let case = row.case;
            let addressed = addressed_resource_plane(row.url, Some(&published), "us-west-2");
            let actual = addressed.as_ref().map(|addressed| {
                (
                    addressed.signing_name.as_str(),
                    addressed.target.as_str(),
                    addressed.region.as_str(),
                )
            });
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected {:?}, got {actual:?}",
                row.expected
            );
        }
        // An emulator vending no such records resolves none of them, which is the position
        // every address was in before a service published one.
        let unrecorded = addressed_resource_plane("https://127.0.0.1:37333/items", None, "us-west-2");
        assert!(
            unrecorded.is_none(),
            "case \"an emulator holding no published addresses\": expected no address, got \
             {unrecorded:?}"
        );
        // A URL that writes NO port dialled the default port of its SCHEME, which is the
        // address the register is keyed by for a plane published on that port
        // ([`dialled_authority`]): the one address per listener the register holds.
        published.publish("shop-0123.localhost:80", "vpc-lattice");
        published.publish("docs.example.test:443", "vpc-lattice");
        let default_ports: &[Row] = &[
            Row {
                case: "an http URL writing no port, published on 80",
                url: "http://shop-0123.localhost/probe/1",
                expected: Some(("vpc-lattice", "/probe/1", "us-west-2")),
            },
            Row {
                case: "an https URL writing no port, published on 443",
                url: "https://docs.example.test/index.html",
                expected: Some(("vpc-lattice", "/index.html", "us-west-2")),
            },
            Row {
                case: "the same host written with its port",
                url: "http://shop-0123.localhost:80/probe/1",
                expected: Some(("vpc-lattice", "/probe/1", "us-west-2")),
            },
            Row {
                case: "an https URL whose host is published on 80 only",
                url: "https://shop-0123.localhost/probe/1",
                expected: None,
            },
            Row {
                case: "an http URL whose host is published on 443 only",
                url: "http://docs.example.test/index.html",
                expected: None,
            },
            Row {
                case: "a port neither scheme fixes",
                url: "http://shop-0123.localhost:8080/probe/1",
                expected: None,
            },
        ];
        for row in default_ports {
            let case = row.case;
            let addressed = addressed_resource_plane(row.url, Some(&published), "us-west-2");
            let actual = addressed.as_ref().map(|addressed| {
                (
                    addressed.signing_name.as_str(),
                    addressed.target.as_str(),
                    addressed.region.as_str(),
                )
            });
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected {:?}, got {actual:?}",
                row.expected
            );
        }
        // The two readings that name a service in the ADDRESS itself answer FIRST, so a plane
        // named in the path keeps its own signing name and an AWS-shaped host keeps its
        // endpoint prefix, whatever has been published.
        for (case, url, expected) in [
            (
                "a plane named in the path of the emulator's own endpoint",
                "https://127.0.0.1:37333/_aws/es/things/index/_doc/1",
                "es",
            ),
            (
                "a host of AWS's own naming",
                "https://s3.us-west-2.amazonaws.com/bucket/key",
                "s3",
            ),
        ] {
            let addressed = addressed(url, "us-west-2")
                .or_else(|| addressed_resource_plane(url, Some(&published), "us-west-2"));
            let actual = addressed
                .as_ref()
                .map(|addressed| addressed.signing_name.as_str());
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected the address to name {expected:?}, got {actual:?}"
            );
        }
    }
}
