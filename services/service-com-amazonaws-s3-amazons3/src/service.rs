// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-s3-amazons3` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Six opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. S3 answers every one of them with one of its OWN
//!   error codes (see `s3_refusal`), never the framework's generic
//!   `ValidationException`, because a client branches on the S3 code.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. Every one of them keeps the framework's default: the
//!   requests this service serves that no routing-table entry claims are resolved to
//!   operations of their own at `override_operation` instead (the browser-form upload, the
//!   website endpoint and the SELECT data plane, [`select_object_content`]), which is the one
//!   seam that can answer them from the store and from the calling principal.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//! - `override_protocol` — the codec the modeled sequence reads and answers in.
//!   The default returns `None`, serving the model-derived protocol; THIS service
//!   returns [`wire::UnwrappedXmlRestXml`], the model's own restXml with the
//!   envelope-free output document the model marks `s3UnwrappedXmlOutput`.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The one refusal this service answers in its OWN envelope
//!
//! An AUTHENTICATION refusal is the exception, and it is why this file implements
//! [`smithy_server_runtime::ErasedServiceDefinition`] over the generated entry rather than
//! handing the entry back bare: every model-derived datum is delegated to the entry
//! unchanged, and the one method that is not is `refusal_response`, the renderer the
//! framework calls for a caller authentication refused. The `override_refusal` seam cannot
//! word that one, because the answer it returns is a code and a sentence — and S3's
//! authentication refusals differ from the framework's rendering in two further ways that a
//! client reads: the STATUS (a presigned URL whose query string cannot be used is a `400`,
//! not a `403`) and the ELEMENTS (a signature mismatch carries the SigV4 signing
//! diagnostics beside its code and message). [`sigv4`] holds that rule; a refusal it
//! declines to word still renders through the entry's own protocol-correct default.

// The absolute URL this service ANNOUNCES for a bucket or an object a caller addresses next —
// rooted on the endpoint serving the request rather than on one of AWS's own hosts — see
// `service/announced_url.rs`.
pub mod announced_url;
// What AUTHORIZES one request: the bucket's own policy, evaluated with the condition keys
// the request carries, and the account the request says owns the bucket — see
// `service/authorization.rs`.
pub mod authorization;
// The S3 Express session token a Zonal endpoint request presents, and what it authorizes —
// see `service/express_session.rs`.
pub mod express_session;
// The refusal a request addressing a bucket by a name S3 can address no bucket by earns, for
// every operation of this service but the create — see `service/invalid_bucket_name.rs`.
pub mod invalid_bucket_name;
// What an update of one of an S3 Metadata configuration's tables must satisfy about the
// configuration's journal table — see `service/metadata_tables.rs`.
pub mod metadata_tables;
// What an object WRITE must satisfy about the key it names and the class it asks for, before
// anything is stored — see `service/object_write.rs`.
pub mod object_write;
// Whether every identity a bucket policy grants to is one this account's IAM can resolve —
// see `service/policy_principals.rs`.
pub mod policy_principals;
// What the BROWSER-FORM upload of an object is — the `POST` of a `multipart/form-data`
// document to a bucket, which no modelled operation is bound to — see
// `service/post_object.rs`.
pub mod post_object;
// WHO a signed request speaks for, and whether the identity-based documents in force on that
// principal and the bucket's own policy let it perform the action it asks for — see
// `service/request_authorization.rs`.
pub mod request_authorization;
// The `AWS/S3` request metrics a bucket's own metrics configurations publish about the
// requests this service serves on its objects — see `service/request_metrics.rs`.
pub mod request_metrics;
// What this service answers a request addressed to the S3 SELECT data plane, which no
// modelled operation is bound to — see `service/select_object_content.rs`.
pub mod select_object_content;
pub mod sigv4;
// The WEBSITE ENDPOINT a bucket configured for static website hosting is served on, which is
// a different endpoint from the REST API's — see `service/website_endpoint.rs`.
pub mod website_endpoint;
pub mod wire;

use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, OperationRoutingTable,
    Refusal, Refused, ServerHttpProtocol, Service, ServiceIdentity,
};
use std::borrow::Cow;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_schema_server::protocol::ServerProtocol;
use aws_smithy_schema_server::ProtocolError;

use wire::UnwrappedXmlRestXml;

/// The first URI path segments another emulated service that SHARES S3's `s3` SigV4
/// signing name owns, and which S3 must therefore not claim.
///
/// S3's own modeled path space cannot discriminate: every bucket/object operation binds
/// `/{Bucket}` or `/{Bucket}/{Key+}`, a path LABEL that matches ANY first segment, which
/// is why the generated `crate::ROUTE_PATH_SEGMENTS` is EMPTY ("no constraint") for this
/// service. S3 Control
/// (`services/service-com-amazonaws-s3control-awss3controlservicev20180820`) signs as
/// `s3` too — `smithy_query(query="service_info")` on its model reports sigv4Name `s3`
/// with endpointPrefix `s3-control` — and binds EVERY one of its operations under the
/// single literal first segment below (`/v20180820/bucket/{Bucket}`, `/v20180820/jobs`,
/// `/v20180820/accesspoint`, …). Excluding that one segment is what leaves exactly one
/// claimant for every request signed for `s3`, instead of two and a
/// `500 AmbiguousService` for both.
///
/// AWS itself separates the two by endpoint host, which an emulator cannot use: a client
/// configured with one local endpoint URL sends the same `Host` for both services. The
/// one request this costs is a bucket literally NAMED `v20180820`, which is the narrowest
/// possible loss and strictly better than refusing every `s3` request.
const PEER_PATH_SEGMENTS: &[&str] = &["v20180820"];

/// The SigV4 signing name the S3 Express One Zone surface of this model is signed for.
///
/// A client that has taken a session on a directory bucket signs its Zonal endpoint calls
/// for `s3express` (the endpoint ruleset's signing name for a directory bucket), not for
/// `s3` — the emulator's own signature verification already treats the two as one family —
/// so a request carrying that scope is this service's, and is claimed as such.
const S3_EXPRESS_SIGNING_NAME: &str = "s3express";

/// Whether `req`'s SigV4 credential scope names the S3 EXPRESS signing name
/// ([`S3_EXPRESS_SIGNING_NAME`]).
///
/// The scope is read through [`sigv4::credential_scope`], which reads it with the one parser
/// this whole emulator reads a credential scope with, so a truncated or malformed scope names
/// no endpoint at all. The signing name it carries is compared the way the framework's own
/// routing key compares one — trimmed and case-insensitively, so `S3Express` and `s3express`
/// name the same endpoint here and there.
///
/// It is what tells one endpoint of this model from another where the address cannot: a
/// client of this emulator reaches every endpoint under one host, and the S3 Express control
/// endpoint (`s3express-control.<region>.amazonaws.com`) and the general purpose one bind the
/// same method and path for two different operations.
fn signed_for_s3_express(req: &RtRequest) -> bool {
    sigv4::credential_scope(req).is_some_and(|scope| {
        scope
            .service
            .trim()
            .eq_ignore_ascii_case(S3_EXPRESS_SIGNING_NAME)
    })
}

/// The header a request announces the media type of its payload in, which is what tells a
/// BROWSER-FORM upload from every other `POST` addressed to a bucket
/// ([`EmulatedService::addresses_form_post_upload`]).
const CONTENT_TYPE_HEADER: &str = "content-type";

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses the path space of a peer service listed in
/// `peer_segments`. Matching is EXACT segment equality, deliberately never a prefix test:
/// a bucket named `v20180820-logs` is S3's, not the peer's.
fn addresses_peer_path(parts: &http::request::Parts, peer_segments: &[&str]) -> bool {
    match first_path_segment(parts.uri.path()) {
        Some(segment) => peer_segments.contains(&segment),
        // The ROOT path (`GET /?x-id=ListBuckets`) is S3's own; no peer claims it.
        None => false,
    }
}

/// The shortest and longest a general purpose bucket's name may be: "Bucket names must be
/// between 3 (min) and 63 (max) characters long" (Amazon S3 User Guide, *Bucket naming
/// rules*).
const BUCKET_NAME_CHARS: ::std::ops::RangeInclusive<usize> = 3..=63;

/// Whether `label` is SHAPED like a bucket name: "Bucket names can consist only of
/// lowercase letters, numbers, dots (.), and hyphens (-)", between 3 and 63 characters, and
/// must "begin and end with a letter or number" (same rules).
///
/// A shape test, not a claim that any such bucket exists: it is what tells a host whose
/// first label could be a bucket from one whose first label is an endpoint's own
/// (`s3.us-east-1.amazonaws.com`, whose `s3` is two characters).
///
/// `pub(crate)`: the copy-source reader asks the same question of the bucket half of an
/// `x-amz-copy-source` ([`crate::state::parse_copy_source`]), so one rule decides which
/// text can be a bucket name wherever this service reads one out of a request.
pub(crate) fn bucket_name_shaped(label: &str) -> bool {
    let allowed = |character: char| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '.' | '-')
    };
    let alphanumeric = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
    BUCKET_NAME_CHARS.contains(&label.len())
        && label.chars().all(allowed)
        && label.chars().next().is_some_and(alphanumeric)
        && label.chars().next_back().is_some_and(alphanumeric)
}

/// The shortest and longest a bucket name may be for S3 to ADDRESS a bucket by it: "bucket
/// names must be between 3 and 255 characters long" (Amazon S3 User Guide, *Bucket naming
/// rules*, the legacy US East (N. Virginia) rules).
const ADDRESSABLE_BUCKET_NAME_CHARS: ::std::ops::RangeInclusive<usize> = 3..=255;

/// Whether `name` is a name Amazon S3 can ADDRESS a bucket by at all.
///
/// The syntax a bucket name has to have for a request to be ABOUT a bucket: between
/// [`ADDRESSABLE_BUCKET_NAME_CHARS`] characters, made of "uppercase letters, lowercase
/// letters, numbers, periods (.), hyphens (-), and underscores (_)", and starting with a
/// letter or a number — the legacy US East (N. Virginia) naming rules, which are the WIDER
/// set: a bucket created under them still exists and only a path-style request can address
/// it, so S3 cannot refuse the name a request addresses it by. A name outside that set names
/// no bucket in any account, so a request carrying one is refused for the NAME
/// ([`wire::UnwrappedXmlRestXml::invalid_bucket_name_violation`]) rather than answered about
/// a bucket that is merely absent.
///
/// NOT the rules a bucket may be CREATED under today, which are narrower in every direction
/// — lowercase only, at most 63 characters, no underscore, no reserved prefix or suffix, not
/// an IP address: `CreateBucket` applies those to the name it is asked to mint, through the
/// one reading every service in this workspace judges a caller-supplied bucket name by
/// (`service_common::s3_bucket_name::is_well_formed`). A name no new bucket may carry is
/// still a name an existing bucket can be addressed by, so a read of it is answered about
/// the bucket rather than about the name.
///
/// `pub`: this is the question the wire layer asks of the bucket EVERY request addresses, and
/// `tests/all/general_purpose_bucket_names.rs` asks it to tell which of the two answers a
/// name earns.
pub fn names_a_bucket(name: &str) -> bool {
    name.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
    }) && ADDRESSABLE_BUCKET_NAME_CHARS.contains(&name.len())
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

/// The bucket a request names in the `{Bucket}` label of `path`, as the request spelled it —
/// decoded the way the codec decodes a path label.
///
/// `path` is the request's address in the PATH-STYLE form every route of this service is
/// bound in ([`EmulatedService::path_style_path`]), so a virtual-hosted-style request — whose
/// bucket is the first label of its host and whose path is the KEY alone — names the same
/// bucket a path-style one does. `None` for a path that carries no such segment, which is the
/// service root (`GET /?x-id=ListBuckets`).
///
/// The one reading the name-refusal seam and the operation that answers it share
/// ([`invalid_bucket_name`]), so the name judged and the name the envelope reports cannot
/// differ.
pub(crate) fn addressed_bucket_name(path: &str) -> Option<String> {
    first_path_segment(path).map(sigv4::percent_decoded)
}

/// Whether the store in `engine` holds a bucket named `name`.
///
/// One reader for the two places that ask — this service's own seams and the wire layer's
/// address normalization — so they cannot disagree about which names are buckets.
fn holds_bucket_in<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| state.lock().bucket(name).is_some())
}

/// The bucket an ADDRESS reaches: the bucket itself when this emulator holds one under that
/// name, else the bucket the ACCESS POINT that address reaches fronts — "Amazon S3 access
/// points are named network endpoints that are attached to buckets that you can use to
/// perform S3 object operations" (Amazon S3 User Guide, *Managing data access with Amazon S3
/// access points*), so an address that names an access point addresses the bucket behind it.
/// `None` for an address that reaches neither.
///
/// [`crate::state::access_points`] carries WHICH addresses reach one access point. One
/// reader for every seam that resolves an address — this service's own seams and the wire
/// layer's — so they cannot disagree about which bucket a request reaches. Read-only, and
/// it holds no guard when it answers.
fn addressed_bucket_in<Eng: Send + Sync + 'static>(engine: &Eng, address: &str) -> Option<String> {
    if holds_bucket_in(engine, address) {
        return Some(address.to_string());
    }
    crate::state::access_points::addressed_access_point_in(engine, address)
        .map(|route| route.bucket)
}

/// Whether `host` carries a BUCKET as its first label, by shape alone: a host of at least
/// two labels whose first could be a bucket name.
///
/// Shape is not enough to decide the address — an endpoint's own first label can be
/// bucket-shaped (`s3-us-east-1.amazonaws.com`, `my-emulator.example.com`) — so
/// [`addressed_in_host`] is what resolves it, against the buckets this emulator holds.
fn host_style_label(host: &str) -> Option<&str> {
    let host = host.rsplit('@').next().unwrap_or(host);
    let (label, rest) = host.split_once('.')?;
    (!rest.is_empty() && bucket_name_shaped(label)).then_some(label)
}

/// The ADDRESS a VIRTUAL-HOSTED-STYLE request carries in the first label of `host`, when it
/// carries one — `addresses_a_bucket` answering whether a name is an address this emulator
/// serves a bucket's objects at (a bucket it holds, or an access point that fronts one; see
/// [`addressed_bucket_in`]).
///
/// "Both the virtual-hosted-style requests and the path-style requests are supported"
/// (`GetObject`'s own documentation): a path-style request carries the bucket as the first
/// segment of its path, and a virtual-hosted-style request carries it as the first label of
/// its host — `https://bucket-name.s3.region-code.amazonaws.com/key-name` — leaving the path
/// to be the key alone. Every AWS SDK's URL presigner writes the second form for an endpoint
/// addressed by name, so a client that reads an object through a presigned URL addresses it
/// that way.
///
/// WHICH form a request is in cannot be read off the host's shape alone, because an
/// endpoint's own first label may be bucket-shaped and an emulator's endpoint is reached
/// under a name nothing here knows. So it is resolved against the buckets this emulator
/// HOLDS: the first host label addresses a bucket when it names one of them and the
/// request's first path segment does not. A path-style request's first path segment names
/// its bucket, so it is never read out of the host; and where both could (a bucket whose
/// name is also the endpoint's first label), the PATH wins, which leaves every request that
/// resolves today resolving the same way.
///
/// What a bucket-shaped host label naming a bucket this emulator does NOT hold resolves to
/// is not decided here: this answers `None` for it, and the seams that read an address
/// decide it between them — `tests/all/presigned_object_reads.rs` pins what a read of such
/// an address is answered (`NoSuchBucket`, the code Amazon S3 answers a bucket that does not
/// exist with, about the bucket the HOST named).
pub(crate) fn addressed_in_host<'h>(
    host: &'h str,
    path: &str,
    addresses_a_bucket: &dyn Fn(&str) -> bool,
) -> Option<&'h str> {
    let label = host_style_label(host)?;
    if !addresses_a_bucket(label) {
        return None;
    }
    if first_path_segment(path).is_some_and(addresses_a_bucket) {
        return None;
    }
    Some(label)
}

/// `path` with `bucket` put back in front of it, which is the PATH-STYLE address of the same
/// object: a virtual-hosted-style request's path is the key alone, so the labels every
/// operation of this service binds (`/{Bucket}`, `/{Bucket}/{Key+}`) read it from here.
///
/// The one place the two address forms are reduced to one, so the request this service
/// ROUTES and the request it DESERIALIZES cannot disagree about which bucket was named.
pub(crate) fn path_with_bucket(bucket: &str, path: &str) -> String {
    match path {
        // `https://bucket.s3.amazonaws.com/` addresses the BUCKET itself (`HeadBucket`,
        // `ListObjects`), so its path-style form carries no trailing segment at all.
        "" | "/" => format!("/{bucket}"),
        path => format!("/{bucket}{path}"),
    }
}

/// Whether an ACCESS POINT's data plane serves a request of `method` whose path, after the
/// access point's own address, is `rest`.
///
/// An access point is "attached to [a bucket] that you can use to perform S3 object
/// operations" (Amazon S3 User Guide, *Managing data access with Amazon S3 access points*):
/// the objects of the bucket it fronts are its surface. So a request that addresses an OBJECT
/// through the access point reaches that bucket, and so does a GET or a HEAD of the access
/// point itself — the listings of its objects, and the HEAD that reports the bucket behind
/// it. A WRITE or a DELETE that names no object addresses no object of the access point at
/// all — creating a bucket, deleting one and writing its configuration are not an access
/// point's operations — so such a request is NOT resolved to the fronted bucket: it is
/// answered about the address it named, which is what keeps a bucket-level call made to an
/// access point from reaching the bucket behind it.
pub(crate) fn access_point_serves(method: &str, rest: &str) -> bool {
    if !rest.trim_start_matches('/').is_empty() {
        return true;
    }
    method.eq_ignore_ascii_case(http::Method::GET.as_str())
        || method.eq_ignore_ascii_case(http::Method::HEAD.as_str())
}

/// `path` with the bucket an ACCESS POINT fronts in front of the key, for a PATH-STYLE
/// request whose first segment is that access point's address (its ARN or its alias) rather
/// than a bucket's name — `resolve` answering which access point an address reaches.
///
/// `None` for a path whose first segment reaches no access point, and for a request that
/// access point's data plane does not serve ([`access_point_serves`]).
///
/// The ONE reading of that rewrite, so this service's own addressing seam
/// ([`EmulatedService::path_style_path`]) and the wire layer's
/// (`wire::UnwrappedXmlRestXml::path_style_path`) cannot drift on which bucket a request
/// reaches.
pub(crate) fn access_point_addressed_path(
    method: &str,
    path: &str,
    resolve: &dyn Fn(&str) -> Option<crate::state::access_points::AccessPointRoute>,
) -> Option<String> {
    let segment = first_path_segment(path)?;
    let rest = &path[path.find(segment)? + segment.len()..];
    if !access_point_serves(method, rest) {
        return None;
    }
    let route = resolve(&sigv4::percent_decoded(segment))?;
    Some(path_with_bucket(&route.bucket, rest))
}

/// The query parameters a LEGACY presigned URL carries its credential in: the access key
/// id, the signature and the expiry, all three required — "`AWSAccessKeyId`, `Signature`
/// and `Expires`" (AWS General Reference, *Authenticating requests: using query
/// parameters*). The signing scope of that form names no service, which is why a request
/// carrying it resolves no routing key at all.
const LEGACY_PRESIGNED_PARAMETERS: [&str; 3] = ["AWSAccessKeyId", "Signature", "Expires"];

/// Whether `uri`'s query carries all three of [`LEGACY_PRESIGNED_PARAMETERS`] — the shape
/// that makes a request a legacy presigned S3 URL rather than any other unroutable request.
fn carries_legacy_presigned_credential(uri: &http::Uri) -> bool {
    let Some(query) = uri.query() else {
        return false;
    };
    let names: ::std::collections::BTreeSet<&str> = query
        .split('&')
        .filter_map(|pair| pair.split('=').next())
        .collect();
    LEGACY_PRESIGNED_PARAMETERS
        .iter()
        .all(|parameter| names.contains(parameter))
}

/// The host a request was addressed to: its `Host` header, else the authority its URI
/// carries (which is where HTTP/2 puts it, and where an in-process caller's absolute URI
/// carries it).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    parts
        .headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())
}

/// [`request_host`] for the runtime-API request the routing seam is handed.
fn request_host_of(req: &aws_smithy_runtime_api::http::Request) -> Option<&str> {
    req.headers()
        .get(http::header::HOST.as_str())
        .or_else(|| authority_of(req.uri()))
}

/// The authority of an absolute request URI (`https://host/path` -> `host`), or `None` for
/// the origin-form URI (`/path`) a request from the wire carries.
fn authority_of(uri: &str) -> Option<&str> {
    let after_scheme = uri.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(after_scheme);
    (!authority.is_empty()).then_some(authority)
}

/// `req` with the PATH-STYLE address of the same object: the request the model's routing
/// table can name, which is what [`Service::override_operation`] resolves the operation
/// from. Everything else about the request — its method, its query and its headers — is
/// carried through unchanged, because the route is decided by all three.
///
/// `None` when the rewritten URI is not one the HTTP layer can hold, which leaves the
/// request to the generated routing table rather than failing the lookup.
fn with_path_style_address(
    req: &aws_smithy_runtime_api::http::Request,
    bucket: &str,
) -> Option<aws_smithy_runtime_api::http::Request> {
    let uri = req.uri();
    let path = wire::request_path(uri);
    let query = wire::request_query(uri);
    let rewritten = match query.is_empty() {
        true => path_with_bucket(bucket, path),
        false => format!("{}?{query}", path_with_bucket(bucket, path)),
    };
    // The body is not read by a route lookup (this service's protocol routes on the method,
    // the path, the query and the presence of bound headers), so an empty one stands in for
    // it rather than copying the payload of every object write.
    let mut routable =
        aws_smithy_runtime_api::http::Request::new(::aws_smithy_types::body::SdkBody::empty());
    routable.set_uri(rewritten.as_str()).ok()?;
    routable.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        routable
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    Some(routable)
}

/// The INPUT members this service binds as the request's XML DOCUMENT: every `@httpPayload`
/// member of an operation input whose target is a structured shape (a structure, union,
/// list or map), named the way a validation path names it — lowerCamel.
///
/// This is the one thing a refusal needs to know that a [`Violation`] does not carry: a
/// value that failed a constraint INSIDE the request document is a document that does not
/// validate against S3's published schema, and a value that failed in a header, a query
/// parameter or a URI label is a bad argument. The two are different wire codes (see
/// [`s3_refusal`]), and the only thing that tells them apart is whether the violation's
/// path starts at one of these members.
///
/// Source: the `@httpPayload` members of this service's operation inputs, re-derived from
/// the model by `tests/all/refusal_codes.rs` so a model update that binds another document
/// cannot quietly start reporting it as a bad argument.
pub const XML_DOCUMENT_PAYLOAD_MEMBERS: &[&str] = &[
    "abacStatus",
    "accelerateConfiguration",
    "accessControlPolicy",
    "analyticsConfiguration",
    "annotationTableConfiguration",
    "bucketLoggingStatus",
    "cORSConfiguration",
    "createBucketConfiguration",
    "delete",
    "intelligentTieringConfiguration",
    "inventoryConfiguration",
    "inventoryTableConfiguration",
    "journalTableConfiguration",
    "legalHold",
    "lifecycleConfiguration",
    "metadataConfiguration",
    "metadataTableConfiguration",
    "metricsConfiguration",
    "multipartUpload",
    "notificationConfiguration",
    "objectEncryption",
    "objectLockConfiguration",
    "ownershipControls",
    "publicAccessBlockConfiguration",
    "replicationConfiguration",
    "requestPaymentConfiguration",
    "restoreRequest",
    "retention",
    "serverSideEncryptionConfiguration",
    "tagging",
    "versioningConfiguration",
    "websiteConfiguration",
];

/// Whether `violation` names a value that arrived inside the request's XML document.
///
/// Two things decide it. The path is the trail of member names from the operation input
/// inwards, so its FIRST segment is the input member the value came in on and everything
/// below it travelled INSIDE that member — which is why a value refused in the document
/// always names a member of the container, and so always has a path that descends past
/// it. A path that is the member ALONE is either the DOCUMENT itself, which is the one
/// refusal this service mints for a body (the XML charset rule in
/// [`wire::xml_charset_violation`]), or a SCALAR binding of a member whose name another
/// operation uses for its document — `Tagging` is the one such name in this service, the
/// `@httpPayload` of the two tagging writes and the `x-amz-tagging` HEADER of the three
/// object writes — and a header is not a document.
fn violation_is_in_request_document(violation: &Violation) -> bool {
    let mut segments = violation.path.split('.');
    let Some(root) = segments.next() else {
        return false;
    };
    if !XML_DOCUMENT_PAYLOAD_MEMBERS.contains(&root) {
        return false;
    }
    segments.next().is_some()
        || violation.kind == ViolationKind::Pattern(wire::XML_CHARACTER_SET.to_string())
}

/// The S3 error code and message for one boundary refusal.
///
/// S3 is a restXml service: every refusal it answers carries an S3 error code in an
/// `<Error><Code>` body, and which code depends on WHERE the offending value came from.
/// A value that fails a constraint inside the request's XML document is a document that
/// "did not validate against our published schema", which is `MalformedXML`; a value that
/// fails one in a header, a query parameter or a URI label is `InvalidArgument`, "invalid
/// argument". Both are HTTP 400, the status the protocol already renders a refusal with.
///
/// A field S3 PUBLISHES its own refusal for is answered with that refusal (see
/// [`published_refusal`]) ahead of either, because the published wording is the contract
/// for that field. Otherwise `MalformedXML` carries the fixed text its own error-responses
/// entry publishes, because that is what S3 sends however the document failed to validate,
/// and `InvalidArgument` keeps the clause the validator built from the request — which
/// member, and the constraint it broke — since that is the only thing left that says which
/// argument was rejected.
fn s3_refusal(violation: &Violation) -> Refused {
    if let Some(published) = published_refusal(violation) {
        return published;
    }
    if violation_is_in_request_document(violation) {
        return Refused {
            code: crate::state::MALFORMED_XML_CODE.to_string(),
            message: crate::state::MALFORMED_XML_MESSAGE.to_string(),
        };
    }
    Refused {
        code: crate::state::INVALID_ARGUMENT_CODE.to_string(),
        message: violation.message(),
    }
}

/// The XML element a member of a request DOCUMENT is spelled with: the member's own
/// declared name, which a violation path carries with its first character lower-cased (the
/// constraint walk's spelling of a member), so it is recovered by upper-casing that
/// character back. restXml writes a member under its declared name, and no member read
/// through here is renamed by the model.
fn element_of(member_path_segment: &str) -> String {
    let mut characters = member_path_segment.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The refusal S3 publishes for the request field `violation` names, or `None` for a field
/// it publishes none for.
///
/// S3 does not report a refused value by naming the Smithy member path it travelled on: for
/// each of these fields it publishes ONE sentence of its own, which a runbook and a
/// message-matching client key on. The path's FIRST segment is the input member the value
/// arrived on, so that is what the sentence is chosen by; everything the sentence does
/// interpolate comes from the request.
///
/// Each row's source is the AWS S3 API reference for the operation that binds the field —
/// `CopyObject`'s two directive headers, `GetObjectAttributes`'
/// `x-amz-object-attributes`, the `encoding-type` the four listings share
/// ([`crate::state::ENCODING_TYPE_PARAMETER`]), `CreateBucket`'s `x-amz-object-ownership`,
/// `ListBuckets`'
/// `max-buckets`, the `id` query parameter the id-addressed bucket configuration operations
/// share ([`CONFIGURATION_ID_PARAMETER`]), and `UpdateObjectEncryption`'s `ObjectEncryption`
/// payload, whose `InvalidRequest` messages that operation's own `@documentation` lists
/// (`smithy_query(query="doc",shape="UpdateObjectEncryption")`).
fn published_refusal(violation: &Violation) -> Option<Refused> {
    let mut segments = violation.path.split('.');
    let member = segments.next()?;
    // The validating walk WITHHOLDS the value of a `@sensitive` member as it records the
    // violation (`KMSKeyArn` is one), leaving the clause no value half at all, so a
    // sentence that quotes the value reads it from the request's own XML document instead
    // — the same reading an `<ArgumentValue>` echo makes, and the one place this service
    // recovers the TEXT a caller sent.
    let withheld = violation
        .sensitive
        .then(|| {
            violation
                .path
                .rsplit('.')
                .next()
                .map(element_of)
                .and_then(|element| wire::spelled_in_request(&element))
        })
        .flatten();
    let rejected = withheld
        .as_deref()
        .or(violation.value.as_deref())
        .unwrap_or_default();
    let (code, message) = match member {
        // The BUCKET every bucket-addressed route binds from its `{Bucket}` label: a name
        // outside the syntax S3 can address a bucket by names no bucket in any account,
        // which S3's published error list states one code and one sentence for, whichever
        // rule the name broke. The refusal is made where it can carry the name
        // (`service::invalid_bucket_name`) and by `CreateBucket` for the name it is asked to
        // mint; this words the one such value the model itself can refuse — a `{Bucket}`
        // label the request leaves empty — in the same class.
        "bucket" => (
            crate::state::INVALID_BUCKET_NAME_CODE,
            crate::state::INVALID_BUCKET_NAME_MESSAGE.to_string(),
        ),
        // The two `CopyObject` directives, each an enum over [COPY, REPLACE].
        "taggingDirective" => (
            crate::state::INVALID_ARGUMENT_CODE,
            "Unknown tagging directive.".to_string(),
        ),
        "metadataDirective" => (
            crate::state::INVALID_ARGUMENT_CODE,
            "Unknown metadata directive.".to_string(),
        ),
        // The `encoding-type` every listing binds: the value names the encoding S3 is to
        // write the response's key-name elements in, so a value outside the one the model
        // declares names no encoding method at all — which S3 reports against the METHOD
        // asked for rather than by quoting the member path the value travelled on. The
        // sentence is the one the listings themselves answer such a value with
        // ([`crate::state::INVALID_ENCODING_METHOD_MESSAGE`]), so the refusal reads the same
        // whichever layer makes it.
        "encodingType" => (
            crate::state::INVALID_ARGUMENT_CODE,
            crate::state::INVALID_ENCODING_METHOD_MESSAGE.to_string(),
        ),
        // `GetObjectAttributes`' `x-amz-object-attributes`: the header "specifies the fields
        // at the root level that you want returned in the response"
        // (`GetObjectAttributesRequest$ObjectAttributes`, a REQUIRED `@httpHeader` member), so
        // a request that SENT no such header asks for no attributes at all — which S3 reports
        // against the REQUEST, naming the header and that it was either missing or empty,
        // rather than as an attribute name that is not one. A header the request DID send
        // names attributes, and one of them being outside the published set is that second
        // refusal — including the header sent EMPTY, which names an attribute that is not one
        // ([`wire::object_attributes_header_as_sent`] is what tells the two apart, the parsed
        // input having bound both to an empty list).
        "objectAttributes" => match violation.kind {
            ViolationKind::MissingRequiredMember
                if wire::object_attributes_header_as_sent().is_none() =>
            {
                (
                    crate::state::INVALID_REQUEST_CODE,
                    crate::state::OBJECT_ATTRIBUTES_UNSTATED_MESSAGE.to_string(),
                )
            }
            _ => (
                crate::state::INVALID_ARGUMENT_CODE,
                crate::state::INVALID_OBJECT_ATTRIBUTE_MESSAGE.to_string(),
            ),
        },
        // The `x-amz-copy-source` of the two copies. The value is put into the form the
        // member's pattern is applied to before it is judged
        // ([`crate::state::canonical_copy_source`]), so what is left for the pattern to
        // refuse is a value naming no key at all — which is the same thing the operation's
        // own reader refuses a value it cannot resolve for, so the sentence is the one that
        // reader's fault publishes ([`crate::state::copy_source_fault`]) and the document
        // carries the same argument pair beside it ([`refused_argument`]).
        "copySource" => (
            crate::state::INVALID_ARGUMENT_CODE,
            crate::state::copy_source_fault(rejected)
                .unwrap_or(crate::state::CopySourceFault::Uri)
                .message()
                .to_string(),
        ),
        // `CreateBucket`'s location constraint, the one member of its document S3 publishes
        // a refusal of its own for: the value "specifies the Region where the bucket will be
        // created" (`CreateBucketConfiguration$LocationConstraint`), so a value outside the
        // Regions that member publishes names no location a bucket can be created in — which
        // S3 reports against the CONSTRAINT rather than as a document that does not validate
        // against its schema: the document is well formed and it is the Region that is not
        // one.
        //
        // A value the request SPELLED is withheld from the codec and answered by the
        // operation, whose envelope also names the rejected value
        // (`wire::rejected_location_constraint` and `ops::create_bucket`); this is what
        // words the one such value a document can carry without spelling any text — an
        // empty `<LocationConstraint></LocationConstraint>` — in the same class rather than
        // as a malformed document.
        "createBucketConfiguration" => {
            if !segments.any(|segment| segment == "locationConstraint") {
                return None;
            }
            (
                crate::state::INVALID_LOCATION_CONSTRAINT_CODE,
                crate::state::INVALID_LOCATION_CONSTRAINT_MESSAGE.to_string(),
            )
        }
        // The id a bucket configuration is addressed by ([`CONFIGURATION_ID_PARAMETER`]): a
        // call that states none names no configuration at all, which S3 reports against the
        // id itself rather than as a bare bad argument — the same refusal a handler answers
        // an id it was handed empty with
        // (`crate::state::ConfigurationIdFault::Unstated`), so the two cannot disagree about
        // a call that identifies nothing.
        "id" => (
            crate::state::INVALID_CONFIGURATION_ID_CODE,
            crate::state::INVALID_CONFIGURATION_ID_MESSAGE.to_string(),
        ),
        // `UpdateObjectEncryption`'s payload: a KMS key ARN that is not one is an
        // `InvalidRequest` naming the requirement and then WHY the value does not meet it,
        // the way S3 quotes an ARN parser's own diagnostic
        // ([`crate::state::malformed_arn_reason`]); anything else about the container is
        // the `InvalidRequest` that operation publishes for a payload that names no usable
        // encryption type.
        "objectEncryption" => {
            let message = if segments.any(|segment| segment == "kMSKeyArn") {
                format!(
                    "Requests modifying object encryption configuration to SSE-KMS require a \
                     valid target kms key arn: {}",
                    crate::state::malformed_arn_reason(rejected)
                )
            } else {
                "Requests that modify an object encryption configuration require a valid \
                 new encryption type. Valid values are SSEKMS. Modify the request to \
                 specify SSE-KMS for the updated encryption type, and then try again."
                    .to_string()
            };
            (crate::state::INVALID_REQUEST_CODE, message)
        }
        // `PutBucketNotificationConfiguration`'s container: S3 refuses every notification
        // validation failure with the one generic code, so the MESSAGE is what says which
        // rule was broken — and an `Events` entry outside the published `Event` set is the
        // one such value the model can refuse by itself, which would otherwise go out as the
        // generic schema refusal and blame the document's shape instead of the event name.
        // The sentence is FIXED text, and it is the same one the handler's own check of a
        // stored container answers with
        // (`crate::state::NotificationConfigurationRejection::UnsupportedEvent`).
        //
        // Nothing else in the container is refused here: every other rule it breaks is one
        // the model cannot state, which the handler refuses itself.
        "notificationConfiguration" => {
            let unsupported_event = matches!(violation.kind, ViolationKind::EnumValue(_))
                && segments.any(|segment| segment == "events");
            if !unsupported_event {
                return None;
            }
            (
                crate::state::INVALID_ARGUMENT_CODE,
                crate::state::UNSUPPORTED_EVENT_MESSAGE.to_string(),
            )
        }
        // `PutBucketLifecycleConfiguration`'s container: the target of a transition is the
        // one member of it the model can refuse by itself, because `TransitionStorageClass`
        // names the six classes a rule may move an object INTO
        // (`smithy_query(query="shape",shape="TransitionStorageClass")`) and `STANDARD` is
        // not among them — an object is already in it. S3 reports that as a target class
        // that is not valid for the action naming it, which is an `InvalidArgument` about
        // the VALUE rather than the generic schema refusal: the caller has to change the
        // class, not the shape of its document.
        //
        // Nothing else in the container is refused here: every other rule it breaks is one
        // the model cannot state, which the handler's own validator refuses
        // ([`crate::state::validate_lifecycle_configuration`]).
        "lifecycleConfiguration" => {
            let invalid_target = matches!(violation.kind, ViolationKind::EnumValue(_))
                && segments.any(|segment| segment == "storageClass");
            if !invalid_target {
                return None;
            }
            // Which ACTION named it: the two transition actions carry the same target
            // member, and S3's sentence names the action the caller has to fix.
            let action = match violation.path.contains("noncurrentVersionTransitions") {
                true => crate::state::NONCURRENT_VERSION_TRANSITION_ACTION.0,
                false => crate::state::TRANSITION_ACTION.0,
            };
            (
                crate::state::INVALID_ARGUMENT_CODE,
                format!("Invalid target Storage class for {action} action"),
            )
        }
        // `PutBucketLifecycleConfiguration`'s
        // `x-amz-transition-default-minimum-object-size` header: the value "indicates which
        // default minimum object size behavior is applied" (that member's own
        // documentation), and the member is an enum over two behaviours, so a value outside
        // it names none the service could apply. S3 reports it as an `InvalidRequest` that
        // ECHOES the value — the caller has to replace that value, and the sentence is what
        // says which one was not a behaviour — rather than as a constraint clause naming the
        // member's path.
        "transitionDefaultMinimumObjectSize" => (
            crate::state::INVALID_REQUEST_CODE,
            format!("Invalid TransitionDefaultMinimumObjectSize found: {rejected}"),
        ),
        _ => return None,
    };
    Some(Refused {
        code: code.to_string(),
        message,
    })
}

/// The query parameter that addresses S3's Select data plane.
///
/// `SelectObjectContent` is bound at `POST /{Bucket}/{Key+}?select&select-type=2`
/// (`smithy_query(query="operation",operation="SelectObjectContent",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports that `@http` uri; `OPERATIONS.md` restates it), so the `select` SUBRESOURCE is
/// what names the feature — `select-type` only says which generation of it the client
/// speaks, and the refusal below is the same for every value of it.
const SELECT_SUBRESOURCE: &str = "select";

/// The query parameter the model's own URI patterns disambiguate two operations bound to one
/// method and path by (`/?x-id=ListBuckets` against `/?x-id=ListDirectoryBuckets`).
const DISCRIMINATOR_PARAMETER: &str = "x-id";

/// The operation a root-path `GET` on the S3 EXPRESS control endpoint names, as the model's
/// routing table keys it.
const LIST_DIRECTORY_BUCKETS: &str = "ListDirectoryBuckets";

/// Every query parameter the DIRECTORY bucket listing itself binds, plus the model's own
/// discriminator: `ContinuationToken` `@httpQuery("continuation-token")` and
/// `MaxDirectoryBuckets` `@httpQuery("max-directory-buckets")`
/// (`smithy_query(query="operation",operation="ListDirectoryBuckets")`).
///
/// What tells that listing from a request that merely LOOKS root-path at this seam: a query
/// naming anything else names another operation of this service.
const DIRECTORY_BUCKET_LISTING_PARAMETERS: &[&str] = &[
    DISCRIMINATOR_PARAMETER,
    "continuation-token",
    "max-directory-buckets",
];

/// Whether `query` — a request URI's query string — carries the parameter `name`, with or
/// without a value. A subresource arrives as a bare flag (`?select`), so a reader that
/// insisted on `=` would miss it.
///
/// `pub(crate)`: the store asks the same question of a request's query string when it reads
/// whether the request presented a credential there
/// ([`crate::state::request_is_anonymous`]), so both read a parameter list the one way.
pub(crate) fn query_carries(query: &str, name: &str) -> bool {
    query
        .split('&')
        .any(|parameter| parameter.split('=').next().unwrap_or(parameter) == name)
}

/// The VALUE a request URI's query string gives the parameter `name`, or `None` when it
/// carries no such parameter. A parameter spelled as a bare flag (`?id`) states the empty
/// value, which is what it names.
///
/// The first occurrence is what answers, the way a query reader takes one value for one
/// parameter; nothing here decodes, because the callers ask only whether a value is empty.
fn query_value<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query.split('&').find_map(|parameter| {
        match parameter.split_once('=') {
            Some((key, value)) if key == name => Some(value),
            // A bare flag: the parameter is there and names the empty value.
            None if parameter == name => Some(""),
            _ => None,
        }
    })
}

/// Whether `path` addresses an OBJECT — S3's `/{Bucket}/{Key+}`, a non-empty bucket
/// segment with a non-empty key below it — rather than a bucket or the service root.
fn addresses_object(path: &str) -> bool {
    match path.trim_start_matches('/').split_once('/') {
        Some((bucket, key)) => !bucket.is_empty() && !key.is_empty(),
        None => false,
    }
}

/// Whether a request addresses the S3 Select data plane: the `select` subresource of an
/// object, read from `query` and from `path` — the request's address in PATH-STYLE form
/// ([`EmulatedService::path_style_path`]), so a Select request resolves the same whether
/// its bucket arrived in the path or in the host.
///
/// The method is deliberately not part of this predicate, and costs nothing: it is
/// consulted ONLY for a request no modeled operation claimed, and every other method on an
/// object path (`GET`/`HEAD`/`PUT`/`DELETE`) resolves to its own modeled operation before
/// ever reaching here. What is left is a request that asked S3 for Select and nothing else.
fn addresses_s3_select(path: &str, query: &str) -> bool {
    query_carries(query, SELECT_SUBRESOURCE) && addresses_object(path)
}

/// The query parameter that addresses an object's ATTRIBUTES.
///
/// `GetObjectAttributes` is bound at `GET /{Bucket}/{Key+}?attributes`
/// (`smithy_query(query="operation",operation="GetObjectAttributes")` reports that `@http`
/// uri), so the `attributes` SUBRESOURCE is what names the call.
const ATTRIBUTES_SUBRESOURCE: &str = "attributes";

/// The operation a read of an object's attributes names, as the model's routing table keys
/// it.
const GET_OBJECT_ATTRIBUTES: &str = "GetObjectAttributes";

/// The query parameter that addresses a DIRECTORY bucket's session.
///
/// `CreateSession` is bound at `GET /{Bucket}?session`
/// (`smithy_query(query="operation",operation="CreateSession",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports that `@http` uri), so the `session` SUBRESOURCE is what names the feature.
const SESSION_SUBRESOURCE: &str = "session";

/// The operation a `GET /{Bucket}?session` against a GENERAL PURPOSE bucket names.
///
/// A session is a DIRECTORY bucket's subresource: `CreateSession` "creates a session that
/// establishes temporary security credentials to support fast authentication and
/// authorization for the Zonal endpoint API operations on directory buckets" (its own
/// documentation), and a general purpose bucket has no such subresource at all. S3 does not
/// refuse a query parameter that names no subresource of the bucket it was addressed to — it
/// ignores it — so what is left of the request is `GET /{Bucket}`, the bucket's own listing,
/// which is the operation bound to that method and path with no subresource.
const LISTING_WITHOUT_SUBRESOURCE: &str = "ListObjects";

/// The query parameter a bucket configuration's id arrives in.
///
/// Every one of the twelve id-addressed bucket configuration operations binds its id as
/// `Id` `@httpQuery("id")` (`smithy_query(query="operation",
/// operation="PutBucketAnalyticsConfiguration")`; `OPERATIONS.md` restates the same binding
/// for the metrics, S3 Inventory and S3 Intelligent-Tiering families), so ONE parameter
/// name names the id of all of them.
const CONFIGURATION_ID_PARAMETER: &str = "id";

/// The bucket subresources whose READ, WRITE and DELETE are addressed by a configuration
/// id, as `(subresource, method, operation)`.
///
/// Each row is one `@http` uri the model binds — `GET`/`PUT`/`DELETE`
/// `/{Bucket}?<subresource>` with `Id` required — for the four families a bucket holds
/// per-id configurations of: analytics, metrics, S3 Inventory and S3 Intelligent-Tiering.
/// The `GET` row names the family's LISTING, which is the operation the model binds at that
/// same method and path with NO id and so the one a read that identifies no configuration
/// asks for.
///
/// Read by [`EmulatedService::configuration_subresource_operation`], the one place a
/// subresource call that states no id is kept off the bucket-level route.
const ID_ADDRESSED_CONFIGURATION_ROUTES: &[(&str, &str, &str)] = &[
    ("analytics", "GET", "ListBucketAnalyticsConfigurations"),
    ("analytics", "PUT", "PutBucketAnalyticsConfiguration"),
    ("analytics", "DELETE", "DeleteBucketAnalyticsConfiguration"),
    ("metrics", "GET", "ListBucketMetricsConfigurations"),
    ("metrics", "PUT", "PutBucketMetricsConfiguration"),
    ("metrics", "DELETE", "DeleteBucketMetricsConfiguration"),
    ("inventory", "GET", "ListBucketInventoryConfigurations"),
    ("inventory", "PUT", "PutBucketInventoryConfiguration"),
    ("inventory", "DELETE", "DeleteBucketInventoryConfiguration"),
    (
        "intelligent-tiering",
        "GET",
        "ListBucketIntelligentTieringConfigurations",
    ),
    (
        "intelligent-tiering",
        "PUT",
        "PutBucketIntelligentTieringConfiguration",
    ),
    (
        "intelligent-tiering",
        "DELETE",
        "DeleteBucketIntelligentTieringConfiguration",
    ),
];

/// The refusal S3 publishes for a body member whose value the validating walk WITHHELD.
///
/// The walk drops the value of a member the model marks `@sensitive`
/// (`SSEKMSEncryption$KMSKeyArn` is one), and S3's published refusal for that member names
/// WHY the submitted ARN is not one — a reason read off the value itself
/// ([`crate::state::malformed_arn_reason`]). So the value half is restored from the door's
/// own echo of the request and the clause is then worded by [`s3_refusal`], the one place
/// S3's refusal rules live: a value this service publishes a refusal for keeps that
/// refusal, and everything else keeps exactly the wording the walk behind the door would
/// have given it.
fn submitted_value_wording(
    refused: &[(Violation, ::service_common::validation::SubmittedEcho)],
) -> Option<Refused> {
    let (violation, echo) = refused.first()?;
    let restored =
        ::service_common::validation_message::quoting_the_whole_submitted_value(violation.clone(), echo);
    Some(s3_refusal(&restored))
}

/// The refusal this service words for a value the boundary rejected OUTSIDE the request
/// document, or `None` for one it leaves to the layers behind the door.
///
/// Only a value whose document carries MEMBERS of its own is worded here
/// ([`refused_argument`] decides which): those members can be written only by the renderer
/// below ([`EmulatedService::detailed_refusal`]), which the front door is what routes such a
/// refusal through — every other refused value keeps exactly the wording the walk behind the
/// door gives it, through [`Service::override_refusal`].
///
/// The value half is restored from the door's own echo of the request first, because the
/// sentence depends on it: which of the two copy-source faults a value carries is read off
/// the value ([`crate::state::copy_source_fault`]).
fn uri_value_wording(
    refused: &[(Violation, ::service_common::validation::SubmittedEcho)],
) -> Option<Refused> {
    let (violation, echo) = refused.first()?;
    let restored = ::service_common::validation_message::quoting_the_whole_submitted_value(
        violation.clone(),
        echo,
    );
    refused_argument(&restored).map(|_| s3_refusal(&restored))
}

/// The refusal an ID-ADDRESSED bucket configuration call words for a value the boundary
/// rejected outside its document — the `id` that addresses the configuration above all.
///
/// Worded through [`s3_refusal`], the one place this service's refusal rules live, so the
/// code and the sentence are the ones that seam already chooses for such a value (an `id`
/// the request states none of is [`crate::state::INVALID_CONFIGURATION_ID_CODE`], the same
/// refusal a handler answers an id it was handed empty with). What attaching it to the
/// operation BUYS is the renderer: a wording routes the refusal through
/// [`EmulatedService::worded_refusal`], so the answer carries this service's own
/// request-correlation ids rather than the framework renderer's.
fn configuration_id_wording(
    refused: &[(Violation, ::service_common::validation::SubmittedEcho)],
) -> Option<Refused> {
    let (violation, echo) = refused.first()?;
    let restored = ::service_common::validation_message::quoting_the_whole_submitted_value(
        violation.clone(),
        echo,
    );
    Some(s3_refusal(&restored))
}

/// The `<ArgumentName>` / `<ArgumentValue>` pair S3's document carries for the request field
/// `violation` names, or `None` for a field whose refusal carries no member of its own.
///
/// S3's `InvalidArgument` document names the field it rejected and the value that field
/// carried (Amazon S3 API Reference, *Error responses*), which for a HEADER is the header as
/// the API publishes it, for a QUERY PARAMETER the parameter as it publishes that, and the
/// value as the request sent it. The two copy DIRECTIVES and the listings' `encoding-type`
/// are that case; `x-amz-copy-source` is the one where the refusal is about the value being
/// unreadable AS a reference, so the document names the FIELD in both halves — there is no
/// text to echo that is not the caller's own.
///
/// Reached for a field of EVERY operation of this service, not only the ones served behind a
/// door ([`ErasedServiceDefinition::refusal_response`] is where the pair is attached): the
/// rows are the fields whose value the boundary itself can reject — the value is put into the
/// form the member's pattern is applied to before the pattern sees it
/// ([`crate::state::canonical_copy_source`]), so what is left for the boundary to reject on
/// the copies is one of these three headers.
fn refused_argument(violation: &Violation) -> Option<crate::scaffolded::ErrorDetail> {
    let member = violation.path.split('.').next()?;
    let (name, value) = match member {
        COPY_SOURCE_MEMBER => (
            crate::state::COPY_SOURCE_ARGUMENT,
            crate::state::COPY_SOURCE_ARGUMENT.to_string(),
        ),
        "metadataDirective" => (
            METADATA_DIRECTIVE_HEADER,
            violation.value.clone().unwrap_or_default(),
        ),
        "taggingDirective" => (
            TAGGING_DIRECTIVE_HEADER,
            violation.value.clone().unwrap_or_default(),
        ),
        "encodingType" => (
            crate::state::ENCODING_TYPE_PARAMETER,
            violation.value.clone().unwrap_or_default(),
        ),
        _ => return None,
    };
    Some(crate::scaffolded::ErrorDetail::Argument {
        name: name.to_string(),
        value: Some(value),
    })
}

/// The input member the two copies bind `x-amz-copy-source` from, as a violation path spells
/// it (lowerCamel): `CopySource`
/// (`smithy_query(query="operation",operation="CopyObject,UploadPartCopy")`).
const COPY_SOURCE_MEMBER: &str = "copySource";

/// The headers `CopyObject` binds its two directives from
/// (`smithy_query(query="operation",operation="CopyObject")` reports
/// `MetadataDirective: {httpHeader: "x-amz-metadata-directive"}` and
/// `TaggingDirective: {httpHeader: "x-amz-tagging-directive"}`) — the names S3's document
/// reports as the argument it rejected.
const METADATA_DIRECTIVE_HEADER: &str = "x-amz-metadata-directive";
/// See [`METADATA_DIRECTIVE_HEADER`].
const TAGGING_DIRECTIVE_HEADER: &str = "x-amz-tagging-directive";

/// The operations served behind this service's FRONT DOOR, and why each is.
///
/// A door re-reads the request it is handed, finds the modelled constraints it breaks, and
/// words the refusal through the service's own renderer — which is the only way a refusal
/// the BOUNDARY makes can carry the members S3's document carries for it (the walk the
/// framework runs behind the door renders its own refusal directly). Everything a door does
/// not refuse is handed to the operation, which owns deserialize -> validate -> handle ->
/// serialize exactly as before.
///
/// * `UpdateObjectEncryption` — the refusal of a KMS key ARN that is not one names WHY, a
///   reason read off the submitted ARN, which the validating walk withholds because the
///   model marks that member `@sensitive` ([`submitted_value_wording`]).
/// * `CopyObject` and `UploadPartCopy` — the refusal of an `x-amz-copy-source` the modelled
///   pattern rejects carries the sentence its fault publishes and the argument pair naming
///   the header ([`uri_value_wording`]).
/// * `PutBucketMetricsConfiguration` and `DeleteBucketMetricsConfiguration` — the two calls
///   of the metrics plane whose refusal the BOUNDARY makes: a `MetricsConfiguration`
///   document whose `Filter` is not exactly one member (refused by this service's own codec
///   before a member is built, see [`wire::union_arity_violation`]) and a call that states
///   no configuration id at all. Behind the door those answer in the envelope every other
///   answer of this service carries, named by the pair of ids S3 mints
///   ([`EmulatedService::worded_refusal`]); the framework's own renderer mints ids of a
///   different shape, which a client, a log correlator and a support request all read.
/// * The four listings of what a bucket holds — `ListObjects`, `ListObjectsV2`,
///   `ListObjectVersions` and `ListMultipartUploads` — each of which binds the
///   `encoding-type` whose refusal names the parameter and echoes its value
///   ([`uri_value_wording`]). They are wrapped where they are COUNTED
///   ([`request_metrics::serve_metered`]) so that one operation is not served twice.
///
/// The table also holds the one operation this service serves that the model does not
/// declare at all — the BROWSER-FORM upload ([`post_object`]) — because this is where the
/// operations this service resolves itself are kept.
///
/// And it is where the operations whose answers are COUNTED into the addressed bucket's
/// `AWS/S3` request metrics are served from, each wrapped in
/// [`request_metrics::Metered`]: this is the one table `override_operation` resolves an
/// operation out of before the generated one, so a wrapped operation is reached by both
/// address forms and by nothing else. An operation that is both wrapped here for its refusal
/// and counted is served as ONE entry, the door inside the counting.
fn front_door<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> smithy_server_runtime::OperationRoutingTable {
    use ::service_common::front_door::{FrontDoor, FrontDoorTable};
    use smithy_server_runtime::Operation;

    let mut door = FrontDoorTable::worded(submitted_value_wording);
    door.serve(crate::ops::update_object_encryption::UpdateObjectEncryption::new(
        Arc::clone(engine),
    ));
    let mut table = door.finish();
    table.add_operation(
        crate::ops::put_bucket_metrics_configuration::PutBucketMetricsConfiguration::<Eng>::NAME,
        Box::new(
            FrontDoor::worded(
                crate::ops::put_bucket_metrics_configuration::PutBucketMetricsConfiguration::new(
                    Arc::clone(engine),
                ),
                submitted_value_wording,
            )
            .wording_uri(configuration_id_wording),
        ),
    );
    table.add_operation(
        crate::ops::delete_bucket_metrics_configuration::DeleteBucketMetricsConfiguration::<Eng>::NAME,
        Box::new(
            FrontDoor::worded(
                crate::ops::delete_bucket_metrics_configuration::DeleteBucketMetricsConfiguration::new(
                    Arc::clone(engine),
                ),
                submitted_value_wording,
            )
            .wording_uri(configuration_id_wording),
        ),
    );
    // The two copies are the operations this service serves BOTH behind a door and with
    // their answers counted into the addressed bucket's request metrics: a copy writes the
    // destination object, so it is a `PUT` request of that object's bucket
    // ([`request_metrics`]). One entry per operation, so the door and the counting are the
    // same served operation rather than two.
    table.add_operation(
        crate::ops::copy_object::CopyObject::<Eng>::NAME,
        Box::new(request_metrics::Metered::new(
            FrontDoor::worded(
                crate::ops::copy_object::CopyObject::new(Arc::clone(engine)),
                submitted_value_wording,
            )
            .wording_uri(uri_value_wording),
            Arc::clone(engine),
            request_metrics::RequestKind::Put,
        )),
    );
    table.add_operation(
        crate::ops::upload_part_copy::UploadPartCopy::<Eng>::NAME,
        Box::new(request_metrics::Metered::new(
            FrontDoor::worded(
                crate::ops::upload_part_copy::UploadPartCopy::new(Arc::clone(engine)),
                submitted_value_wording,
            )
            .wording_uri(uri_value_wording),
            Arc::clone(engine),
            request_metrics::RequestKind::Put,
        )),
    );
    // And the operations whose answers are counted and nothing else: the object data plane
    // and the listings of what a bucket holds.
    request_metrics::serve_metered(&mut table, engine);
    // The BROWSER-FORM upload, which is not a modelled operation at all: no routing-table
    // entry is bound to `POST /{Bucket}`, so such a request resolved to nothing and the
    // framework answered it as unwritten. It is held in this table because that is where
    // this service keeps the operations it resolves itself, under a key the model's own
    // protocol never produces ([`post_object::POST_OBJECT`]).
    table.add_operation(
        post_object::POST_OBJECT,
        Box::new(post_object::PostObject::new(Arc::clone(engine))),
    );
    // The refusal of a Zonal endpoint request whose S3 EXPRESS SESSION TOKEN authorizes
    // nothing on the bucket it addressed, one entry per condition: the seam that decides
    // which condition it is hands back a borrow of an operation this service already holds,
    // so the three answers are built here rather than per request. See
    // [`express_session`] for the rule and for why it is answered as an operation.
    for (key, refusal) in express_session::SESSION_DENIED_KEYS {
        table.add_operation(key, Box::new(express_session::SessionDenied::new(refusal)));
    }
    // And the refusal a request earns for the bucket NAME it addresses, held here for the
    // same reason and under a key of the same kind
    // ([`invalid_bucket_name::REFUSE_INVALID_BUCKET_NAME`]): it stands in front of every
    // operation of this service that addresses a bucket, which no routing key can name.
    table.add_operation(
        invalid_bucket_name::REFUSE_INVALID_BUCKET_NAME,
        Box::new(invalid_bucket_name::RefuseInvalidBucketName::new()),
    );
    // And the WEBSITE ENDPOINT, the one surface of this service that is not the REST API at
    // all: a request addressed to a bucket's website host is served from here
    // ([`website_endpoint`]), under a key of the same kind as the refusals above, because no
    // modelled route can name an endpoint.
    table.add_operation(
        website_endpoint::WEBSITE_ENDPOINT,
        Box::new(website_endpoint::WebsiteEndpoint::new(Arc::clone(engine))),
    );
    // And the SELECT data plane, which no routing-table entry claims either: the one operation
    // of this model whose output is a `@streaming` event-stream union, so the scaffold
    // generator typed none of it. What this service answers such a request — the
    // authorization refusal its caller earns, else the refusal of the withdrawn feature — is
    // [`select_object_content`], served from here because the first half needs the store and
    // the calling principal, which only an operation is given.
    table.add_operation(
        select_object_content::SELECT_OBJECT_CONTENT,
        Box::new(select_object_content::SelectObjectContent::new(Arc::clone(
            engine,
        ))),
    );
    table
}

/// The `com-amazonaws-s3-amazons3` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threaded into every operation, kept here
    /// too because the refusal wording below has to ask whether this service ISSUED the
    /// credential a refused request presented, and the sessions that answer that are the
    /// ones `CreateSession` recorded ([`Self::issued_credential`]). Never a store of its
    /// own: it is the `Arc` the router handed to `new`.
    engine: Arc<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it. See [`wire`] for what it
    /// changes about the generated one: the output document of the operations the model
    /// marks `s3UnwrappedXmlOutput` carries no envelope.
    protocol: UnwrappedXmlRestXml,
    /// The operations served behind this service's front door, each for the refusal only a
    /// door can word — see [`front_door`] for the list and the reason. Built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    door: smithy_server_runtime::OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // What the wire layer asks when it has to tell a virtual-hosted-style address from a
        // path-style one ([`addressed_in_host`]): whether a name is an address this emulator
        // serves a bucket's objects at. The closure captures the SAME shared engine every
        // operation reads, so the answer is the store's rather than a copy of it; it is
        // read-only.
        let buckets = Arc::clone(&engine);
        let addresses_a_bucket: wire::BucketLookup =
            Arc::new(move |name: &str| addressed_bucket_in(&*buckets, name).is_some());
        // And which ACCESS POINT an address reaches, when it reaches one: the wire layer
        // resolves the address ONCE per request, so every `{Bucket}` label binds the bucket
        // the request reaches and the decision taken on it knows the access point it came
        // through ([`crate::state::access_points`]). Read-only, over the same engine.
        let access_points = Arc::clone(&engine);
        let addressed_access_point: wire::AccessPointLookup = Arc::new(move |name: &str| {
            crate::state::access_points::addressed_access_point_in(&*access_points, name)
        });
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            door: front_door(&engine),
            engine,
            protocol: UnwrappedXmlRestXml::new()
                .with_buckets(addresses_a_bucket)
                .with_access_points(addressed_access_point),
        }
    }

    /// The operation a READ of an object's ATTRIBUTES names when it states no attribute at
    /// all — no `x-amz-object-attributes` header, or one carrying no field name — or `None`
    /// for every other request, which keeps the route the generated table gives it.
    ///
    /// The model binds `GetObjectAttributes` at `GET /{Bucket}/{Key+}?attributes` with
    /// `ObjectAttributes` as a REQUIRED `@httpHeader` member
    /// (`smithy_query(query="operation",operation="GetObjectAttributes")`), and the generated
    /// table drops a route whose required bindings the request does not carry — so such a call
    /// fell through to the route bound to the same method and path with no subresource, which
    /// is `GetObject`, and was answered with the object's BODY. The `attributes` subresource is
    /// not the object: S3 serves such a request as the attributes call it addressed, refused
    /// for the header it did not state ([`published_refusal`] words that refusal from the
    /// member the boundary reports absent), so it is resolved here rather than becoming a read
    /// of the object itself.
    ///
    /// The address is read in the PATH-STYLE form every route of this service is bound in
    /// ([`Self::path_style_path`]), so the two address forms resolve alike, and only an OBJECT
    /// path resolves: `?attributes` on a bucket addresses no object for attributes to be read
    /// off.
    fn object_attributes_without_the_header(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        if req.method() != http::Method::GET {
            return None;
        }
        if !query_carries(wire::request_query(req.uri()), ATTRIBUTES_SUBRESOURCE) {
            return None;
        }
        if !addresses_object(&self.path_style_path(req)) {
            return None;
        }
        // A header the request DID send leaves the generated table's own lookup to resolve
        // the call, and the boundary and the handler to judge the fields it names.
        if req
            .headers()
            .get(crate::state::OBJECT_ATTRIBUTES_HEADER)
            .is_some()
        {
            return None;
        }
        // Served behind this service's FRONT DOOR where that operation is one of the door's
        // ([`front_door`]), exactly as a request the generated table resolves is, and from the
        // generated table otherwise.
        self.door
            .get(GET_OBJECT_ATTRIBUTES)
            .or_else(|| self.entry.routing_table().get(GET_OBJECT_ATTRIBUTES))
    }

    /// The front-door entry for `req`, when the operation its address names is one served
    /// behind the door ([`front_door`]), else `None` so the generated routing table resolves
    /// the request exactly as it always did.
    ///
    /// The key is the one the ENTRY's protocol resolves — the same key the generated table
    /// is read by — so this names no route of its own.
    fn front_door_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.door.get(key)
    }

    /// Whether `name` is an address this emulator serves a bucket's objects at — a bucket it
    /// holds, or an access point that fronts one — the question an ADDRESS resolves against
    /// (see [`addressed_in_host`] and [`addressed_bucket_in`]). Read-only: the guard is
    /// taken, the answer is read, and nothing is written.
    fn addresses_a_bucket(&self, name: &str) -> bool {
        addressed_bucket_in(&*self.engine, name).is_some()
    }

    /// The bucket a VIRTUAL-HOSTED-STYLE request REACHES through the first label of its host,
    /// when that label is an address of one ([`addressed_in_host`]) — the one place both
    /// routing seams below ask that question, so the route a request resolves and the address
    /// its refusal is judged against cannot disagree about which bucket was named. `path` is
    /// the request's own path, which the caller has already read and which the answer depends
    /// on (a path-style request names its bucket there, and is never read out of the host).
    ///
    /// The bucket, not the label: an access point's endpoint label addresses the bucket that
    /// access point fronts, so what goes back in front of the key is the bucket every route
    /// of this service binds its `{Bucket}` label from.
    fn host_addressed_bucket(&self, req: &RtRequest, path: &str) -> Option<String> {
        let host = request_host_of(req)?;
        let label = addressed_in_host(host, path, &|name| self.addresses_a_bucket(name))?;
        if holds_bucket_in(&*self.engine, label) {
            return Some(label.to_string());
        }
        // An ACCESS POINT's endpoint label: the bucket it fronts is reached only by a request
        // that access point's data plane serves ([`access_point_serves`]); the whole path is
        // the key for this address form, so that is what decides it.
        let route = crate::state::access_points::addressed_access_point_in(&*self.engine, label)?;
        access_point_serves(req.method(), path).then_some(route.bucket)
    }

    /// `path` with the bucket its own first segment REACHES in front of it, for a PATH-STYLE
    /// request whose first segment is an access point's address rather than a bucket's name —
    /// `None` for every other request, whose path already names the bucket it reaches.
    fn access_point_addressed_path(&self, method: &str, path: &str) -> Option<String> {
        access_point_addressed_path(method, path, &|address| {
            crate::state::access_points::addressed_access_point_in(&*self.engine, address)
        })
    }

    /// The address `req` names, in the PATH-STYLE form every route of this service is bound
    /// in (`/{Bucket}`, `/{Bucket}/{Key+}`): a virtual-hosted-style request carries its
    /// bucket in the host and the key alone in the path, so the bucket is put back in front
    /// of it ([`path_with_bucket`], the same normalization [`Service::override_operation`]
    /// routes from); a path-style request's path already is that form and is borrowed
    /// unchanged.
    ///
    /// An ACCESS POINT's address names the bucket it fronts, in either form: the host label
    /// of its endpoint resolves through [`Self::host_addressed_bucket`], and its ARN or its
    /// alias in the first path segment through [`Self::access_point_addressed_path`] — so
    /// every seam of this service reads the bucket the request REACHES.
    fn path_style_path<'r>(&self, req: &'r RtRequest) -> Cow<'r, str> {
        let path = wire::request_path(req.uri());
        match self.host_addressed_bucket(req, path) {
            Some(bucket) => Cow::Owned(path_with_bucket(&bucket, path)),
            None => match self.access_point_addressed_path(req.method(), path) {
                Some(addressed) => Cow::Owned(addressed),
                None => Cow::Borrowed(path),
            },
        }
    }

    /// Whether `access_key_id` names a credential this emulator ISSUED, and so one an
    /// authentication refusal has RESOLVED: the key pair the process publishes at start, or
    /// a session `CreateSession` vended and has not released.
    ///
    /// Each is read from the record that owns it rather than from a copy, so a session that
    /// has expired stops resolving at the moment its own record goes
    /// ([`crate::state::ComAmazonawsS3Amazons3State::release_expired_sessions`]).
    fn issued_credential(&self, access_key_id: &str) -> bool {
        if ::emulator_server::request_authentication::SigV4Authenticator::minted_credentials()
            .is_some_and(|published| published.access_key_id == access_key_id)
        {
            return true;
        }
        crate::state::service_state(&*self.engine)
            .is_some_and(|state| state.lock().holds_vended_session(access_key_id))
    }

    /// Whether `req` asks for the `session` subresource of a bucket that has none: a
    /// GENERAL PURPOSE bucket this emulator holds, addressed in either form (the address is
    /// read in the PATH-STYLE form every route of this service is bound in, so a
    /// virtual-hosted-style request resolves the same way).
    ///
    /// Read-only: the guard is taken, the bucket's kind is read, and nothing is written. A
    /// name this emulator holds no bucket for answers `false` — there is no bucket whose kind
    /// says the subresource is not its, so the request keeps the route the model gives it and
    /// is answered about the address it named.
    fn addresses_session_of_general_purpose_bucket(&self, req: &RtRequest) -> bool {
        if !query_carries(wire::request_query(req.uri()), SESSION_SUBRESOURCE) {
            return false;
        }
        let path = self.path_style_path(req);
        let Some(name) = first_path_segment(&path) else {
            return false;
        };
        if addresses_object(&path) {
            return false;
        }
        crate::state::service_state(&*self.engine).is_some_and(|state| {
            state
                .lock()
                .bucket(name)
                .is_some_and(|bucket| !bucket.is_directory())
        })
    }

    /// Whether `req` is the listing of this account's DIRECTORY buckets rather than of its
    /// general purpose ones.
    ///
    /// Two operations of this model are bound to `GET /` — `ListBuckets` at
    /// `/?x-id=ListBuckets` and `ListDirectoryBuckets` at `/?x-id=ListDirectoryBuckets`
    /// (`smithy_query(query="operation",operation="ListBuckets,ListDirectoryBuckets")`) —
    /// and AWS tells them apart by the ENDPOINT the request reached: the directory-bucket
    /// listing is served by `s3express-control.<region>.amazonaws.com` and the general
    /// purpose one by `s3.<region>.amazonaws.com`. A client of this emulator reaches both
    /// under one host, so the address cannot decide it and the `x-id` discriminator is one
    /// AWS SDKs do not reliably put on the wire. What the request does carry is the SCOPE it
    /// was signed for: the control endpoint's signing name is `s3express`
    /// ([`signed_for_s3_express`]), and no other operation of this model is bound to the
    /// root path, so a root-path request signed for it is this listing. The `x-id` the model
    /// publishes is honoured too, for a client that does send it.
    ///
    /// The QUERY has to be this listing's own as well ([`DIRECTORY_BUCKET_LISTING_PARAMETERS`]):
    /// a request addressing a bucket this emulator does NOT hold carries that bucket in its
    /// host and nothing in its path-style address, so a Zonal `GET /{Bucket}?list-type=2`
    /// against an absent directory bucket reaches this seam looking like a root-path request —
    /// and a query naming another operation's subresource is that operation's request,
    /// answered about the address it named, never this listing.
    fn addresses_directory_bucket_listing(&self, req: &RtRequest) -> bool {
        if req.method() != http::Method::GET {
            return false;
        }
        if first_path_segment(&self.path_style_path(req)).is_some() {
            return false;
        }
        let query = wire::request_query(req.uri());
        let names_another_request = query
            .split('&')
            .filter(|parameter| !parameter.is_empty())
            .map(|parameter| parameter.split('=').next().unwrap_or(parameter))
            .any(|name| !DIRECTORY_BUCKET_LISTING_PARAMETERS.contains(&name));
        if names_another_request {
            return false;
        }
        signed_for_s3_express(req)
            || query_value(query, DISCRIMINATOR_PARAMETER) == Some(LIST_DIRECTORY_BUCKETS)
    }

    /// The refusal `req`'s S3 EXPRESS SESSION earns, served as the operation that answers it,
    /// or `None` for a request this rule has nothing to say about.
    ///
    /// A request addressing no bucket names nothing for a session to be scoped to, and one
    /// that neither presents a session token nor is signed with a session's own key pair
    /// presents no session at all. Everything else is judged against the sessions
    /// `CreateSession` vended — "temporary security credentials are scoped to the bucket and
    /// expire after 5 minutes" — on the request's address read in the PATH-STYLE form every
    /// route of this service is bound in, so a virtual-hosted-style Zonal request (the form a
    /// client addresses a directory bucket in) is judged about the bucket its host named.
    ///
    /// Read-only: the guard is taken, the verdict is read, and nothing is written.
    fn refused_session_token(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let path = self.path_style_path(req);
        let bucket = first_path_segment(&path)?;
        let state = crate::state::service_state(&*self.engine)?;
        let now = ::std::time::SystemTime::now();
        // WHICH session the request presents: the TOKEN it carries, else the key pair it is
        // SIGNED with — a request that presents no token may still be signed with a session's
        // own key pair, which carries the same scope and the same expiry: "temporary security
        // credentials are scoped to the bucket and expire after 5 minutes", so such a request
        // reaches the bucket its session was created for while that session lives. A
        // credential that is not a session's says nothing, which leaves every other request
        // of this service exactly as it was.
        let presented = match express_session::presented_token(req) {
            Some(token) => crate::state::PresentedSession::Token(token.to_string()),
            None => crate::state::PresentedSession::Credential(
                sigv4::credential_scope(req)?.access_key_id,
            ),
        };
        let (verdict, read_only) = {
            let store = state.lock();
            (
                store.presented_session_verdict(&presented, bucket, now)?,
                store
                    .presented_session_scope(&presented, now)
                    .is_some_and(|scope| scope.mode == crate::state::SessionMode::ReadOnly),
            )
        };
        if let Some(key) = express_session::denial_key(verdict) {
            return self.door.get(key);
        }
        // A session that covers the bucket still grants only what its MODE does: "a ReadOnly
        // session is constrained to execute the following Zonal endpoint API operations"
        // (`CreateSessionRequest$SessionMode`), so a request under one that names any other
        // operation of this model is refused here — whichever operation it addressed, and
        // before that operation runs. See [`express_session::READ_ONLY_SESSION_OPERATIONS`].
        if read_only
            && !express_session::read_only_session_serves(
                self.addressed_operation(req).as_deref(),
            )
        {
            return self.door.get(express_session::REFUSE_READ_ONLY_SESSION);
        }
        None
    }

    /// The operation of this model `req`'s address names, or `None` for a request whose
    /// address resolves to none.
    ///
    /// Resolved through the ENTRY's own protocol — the one carrying this service's
    /// model-derived routes — over the request's PATH-STYLE address
    /// ([`with_path_style_address`]), so a virtual-hosted-style Zonal request (the form a
    /// client addresses a directory bucket in) names the same operation a path-style one does.
    /// Nothing of the store is read and nothing is written: this is the route lookup the
    /// routing seam itself makes, asked for the NAME alone.
    fn addressed_operation(&self, req: &RtRequest) -> Option<String> {
        let routable = match self.host_addressed_bucket(req, wire::request_path(req.uri())) {
            Some(bucket) => Some(with_path_style_address(req, &bucket)?),
            None => None,
        };
        let asked = routable.as_ref().unwrap_or(req);
        self.entry
            .protocol()
            .routing_key(asked)
            .ok()
            .map(|key| key.to_string())
    }

    /// Whether `req` is the BROWSER-FORM upload of an object: "the POST operation adds an
    /// object to a specified bucket by using HTML forms" (Amazon S3 API Reference, *POST
    /// Object*), which is a `POST` of a `multipart/form-data` document to the bucket itself.
    ///
    /// Four things make it that request and nothing else, and the model can express none of
    /// them: the METHOD, an address naming the BUCKET rather than an object (read in the
    /// PATH-STYLE form every route of this service is bound in, so a virtual-hosted-style
    /// form POST — which carries its bucket in the host and `/` as its whole path —
    /// resolves the same way), a query naming NO subresource (`POST /{Bucket}?delete` is
    /// `DeleteObjects`, a modelled operation), and the form document as the payload.
    ///
    /// Nothing is read out of the store here, so this costs one method comparison for every
    /// request that is not a `POST`.
    fn addresses_form_post_upload(&self, req: &RtRequest) -> bool {
        if req.method() != http::Method::POST {
            return false;
        }
        if !wire::request_query(req.uri()).is_empty() {
            return false;
        }
        if !post_object::carries_form_payload(req.headers().get(CONTENT_TYPE_HEADER)) {
            return false;
        }
        let path = self.path_style_path(req);
        first_path_segment(&path).is_some() && !addresses_object(&path)
    }

    /// The operation a request on an ID-ADDRESSED bucket configuration subresource names
    /// when it states no configuration id — none at all, or an empty one — or `None` for
    /// every other request, which keeps the route the generated table gives it.
    ///
    /// The model binds each of those reads, writes and deletes at `/{Bucket}?<subresource>`
    /// with `Id` as a REQUIRED `@httpQuery` member
    /// ([`ID_ADDRESSED_CONFIGURATION_ROUTES`]), and the generated table drops a route whose
    /// required bindings the request does not carry — so a call that names the subresource
    /// but no id falls through to the route bound to the BARE path, which for `PUT` is
    /// `CreateBucket` and for `DELETE` is `DeleteBucket`. A bucket subresource is not the
    /// bucket: S3 serves such a request as the subresource call it addressed — the LISTING of
    /// that subresource for a `GET`, which is the operation the model binds there with no id,
    /// and the write or the delete it addressed for the other two, refused for the id it did
    /// not state — so it is resolved here rather than becoming a create or a delete of the
    /// bucket itself.
    ///
    /// An id the request states EMPTY identifies no configuration either, so it resolves the
    /// same way: a `GET` of the subresource with an empty id is that subresource's listing.
    ///
    /// The address is read in the PATH-STYLE form every route of this service is bound in
    /// ([`Self::path_style_path`]), so the two address forms resolve alike, and an OBJECT
    /// path is left alone: these subresources are a bucket's.
    fn configuration_subresource_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let query = wire::request_query(req.uri());
        // An id the request states non-empty leaves the generated table's own lookup to
        // resolve the call, and the handler to judge the value.
        if query_value(query, CONFIGURATION_ID_PARAMETER).is_some_and(|id| !id.is_empty()) {
            return None;
        }
        let path = self.path_style_path(req);
        if first_path_segment(&path).is_none() || addresses_object(&path) {
            return None;
        }
        let operation = ID_ADDRESSED_CONFIGURATION_ROUTES
            .iter()
            .find_map(|(subresource, method, operation)| {
                (*method == req.method() && query_carries(query, subresource)).then_some(*operation)
            })?;
        // Served behind this service's FRONT DOOR where that operation is one of the door's
        // ([`front_door`]), exactly as a request the generated table resolves is: a call
        // resolved here is the same call, and the door is what words the refusal of the id
        // it did not state in this service's own envelope.
        self.door
            .get(operation)
            .or_else(|| self.entry.routing_table().get(operation))
    }

    /// How this service answers the AUTHENTICATION refusal of the request it is serving, or
    /// `None` for a refusal this service does not word — which keeps the framework's own.
    ///
    /// The framework's structural refusal says only that authentication refused the caller,
    /// so which refusal it is follows from what the REQUEST presented, which [`sigv4`] read
    /// once for this request and answers from. Rendered through THIS service's own protocol,
    /// in the same channel every other refusal of this service goes out in, so the envelope
    /// a client parses — the bare `<Error>` root with its `<Code>`, `<Message>` and
    /// request-correlation elements, and the matching `x-amz-request-id` / `x-amz-id-2`
    /// headers — is the one it already reads; the one difference is the elements S3 carries
    /// INSIDE it for a signature mismatch, and the status it answers an unusable
    /// query string with.
    ///
    /// `None` also on the serializer failure fixed text cannot actually provoke, which
    /// leaves the framework's own refusal to answer rather than panicking in the request
    /// path.
    fn authentication_refusal_response(&self) -> Option<RtResponse> {
        let presented = sigv4::presented()?;
        let refusal = sigv4::refusal_for(
            &presented,
            &|access_key_id| self.issued_credential(access_key_id),
            &|bucket| self.anonymous_refusal_message(bucket),
        )?;
        self.protocol
            .serialize_error(
                &refusal,
                &sigv4::REFUSAL_SCHEMA,
                refusal.code,
                refusal.status,
            )
            .ok()
    }

    /// The sentence S3 refuses a request carrying NO credential with, for a request whose
    /// address resolves to `bucket` (`None` for one that addresses no bucket).
    ///
    /// The code is `AccessDenied` for every such refusal, so the sentence is the only thing
    /// telling a caller WHICH refusal it is — and which one that is turns on the bucket the
    /// request addresses, read from the store rather than from the request: a bucket that
    /// makes the REQUESTER pay cannot bill a caller that names no account, whatever it
    /// grants to everyone ([`crate::state::Bucket::requester_pays_anonymous_refusal`] holds
    /// that rule, the same one an operation's own authorization answers with), and anything
    /// else is the authorization refusal S3 answers a caller a bucket grants nothing to.
    ///
    /// Read-only: the guard is taken, the bucket's configuration is read, and nothing is
    /// written — including for a name the store holds no bucket for, which is the generic
    /// refusal because no bucket's configuration says otherwise.
    fn anonymous_refusal_message(&self, bucket: Option<&str>) -> &'static str {
        bucket
            .and_then(|name| {
                let state = crate::state::service_state(&*self.engine)?;
                let store = state.lock();
                store.bucket(name)?.requester_pays_anonymous_refusal()
            })
            .unwrap_or(crate::state::ACCESS_DENIED_MESSAGE)
    }

    /// The operation that answers `req` for the bucket NAME it addresses, when that name is
    /// one Amazon S3 can address no bucket by ([`names_a_bucket`]) — else `None`, which is
    /// every request that addresses a bucket by an addressable name and every request that
    /// addresses no bucket at all.
    ///
    /// Two requests keep the answer they would otherwise get. One whose address names no
    /// MODELLED operation of this service keeps the failure it earns — a routing error, or
    /// the answer one of the operations this service resolves itself gives it (the
    /// browser-form upload, [`post_object`], which judges the bucket it is POSTed to in that
    /// operation) — because a name refusal here would claim a route that is not there. And a
    /// `CreateBucket` judges the name itself: it is the call that MINTS one, against the
    /// narrower rules a bucket may be created under today (`crate::ops::create_bucket`), and
    /// its refusal names the rejected name the same way — so the two cannot disagree about a
    /// name, and the create's own rule is not weakened to the addressing one.
    ///
    /// And a VIRTUAL-HOSTED-STYLE request to a bucket this emulator HOLDS is judged by that
    /// bucket rather than by its key: the address is read in the PATH-STYLE form every route
    /// of this service is bound in ([`EmulatedService::path_style_path`]), which puts the
    /// host's bucket back in front of the key. A host-style request naming a bucket this
    /// emulator does NOT hold resolves no bucket out of its host at all ([`addressed_in_host`]
    /// answers only for one it holds), so the whole service — this seam and every route's own
    /// `{Bucket}` label alike — reads its first path segment as the bucket it addresses, and
    /// this refusal is about the same text that segment binds.
    ///
    /// See [`invalid_bucket_name`] for what the refusal is and why it is an OPERATION.
    fn refuses_the_bucket_name(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        use smithy_server_runtime::Operation;

        // The address in the PATH-STYLE form every route of this service is bound in, so a
        // virtual-hosted-style request to a bucket this emulator holds is judged by the
        // bucket its host named and not by the first segment of its key.
        let addressed = addressed_bucket_name(&self.path_style_path(req))?;
        if names_a_bucket(&addressed) {
            return None;
        }
        let operation = self.entry.protocol().routing_key(req).ok()?.to_string();
        if operation == crate::ops::create_bucket::CreateBucket::<Eng>::NAME
            || self.entry.routing_table().get(&operation).is_none()
        {
            return None;
        }
        self.door
            .get(invalid_bucket_name::REFUSE_INVALID_BUCKET_NAME)
    }

    /// `refused` rendered with the DETAIL members S3's document carries for it
    /// ([`refused_argument`]), in the same envelope and at the same status a constraint
    /// refusal is rendered with: HTTP 400, the status every protocol answers a value outside
    /// its declared constraints with.
    ///
    /// This is the one renderer a BOUNDARY refusal can carry members through: the wording
    /// seam ([`Service::override_refusal`]) chooses the code and the message and nothing
    /// else, so a document that also names the argument it rejected has to be written here.
    /// An implemented handler's own refusal carries the same members through the same
    /// serializer ([`crate::scaffolded::Scaffolded::naming`]).
    ///
    /// `None` on the serializer failure a fixed code, a fixed message and one named argument
    /// cannot actually provoke, which leaves the entry's renderer to answer rather than
    /// panicking in the request path.
    fn detailed_refusal(
        &self,
        refused: &Refused,
        detail: crate::scaffolded::ErrorDetail,
    ) -> Option<RtResponse> {
        let refusal = crate::scaffolded::UnmodeledWithDetail {
            message: refused.message.clone(),
            detail,
        };
        self.protocol
            .serialize_error(
                &refusal,
                &crate::scaffolded::UNMODELED_SCHEMA,
                &refused.code,
                crate::state::INVALID_ARGUMENT_STATUS,
            )
            .ok()
    }

    /// `refused` rendered in the envelope every OTHER answer of this service carries: the
    /// bare `<Error>` root with its `<Code>`, `<Message>` and request-correlation elements,
    /// and the matching `x-amz-request-id` / `x-amz-id-2` headers — named by the pair of ids
    /// S3 mints ([`crate::state::request_ids`]) rather than by whatever a generic renderer
    /// generates.
    ///
    /// Reached for EVERY refusal of a value the model declares a rule for, on every
    /// operation of this service and not only the ones served behind the door: what the ids
    /// are is a property of an S3 response, not of one operation. The code and the message
    /// are the ones the wording seam already chose, and the status is the 400 every protocol
    /// answers a value outside its declared rules with — which is the status of every code
    /// [`s3_refusal`] and [`published_refusal`] can choose, and the only class of refusal
    /// routed through here (see [`ErasedServiceDefinition::refusal_response`]). A published
    /// refusal at another status would need the status to travel with the code, which
    /// [`Refused`] does not carry.
    ///
    /// `None` on the serializer failure a code and a sentence cannot actually provoke, which
    /// leaves the entry's renderer to answer rather than panicking in the request path.
    fn worded_refusal(&self, refused: &Refused) -> Option<RtResponse> {
        let refusal = crate::scaffolded::UnmodeledMessage {
            message: refused.message.clone(),
        };
        self.protocol
            .serialize_error(
                &refusal,
                &crate::scaffolded::UNMODELED_SCHEMA,
                &refused.code,
                crate::state::INVALID_ARGUMENT_STATUS,
            )
            .ok()
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry, which
        // the implementation below delegates every one of them to, UNCHANGED. The one
        // exception is the renderer for an AUTHENTICATION refusal, which S3 answers with
        // its own status and its own elements; see this file's header and [`sigv4`]. Do
        // not edit the entry.
        self
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // The WEBSITE ENDPOINT is this service's before anything about a credential is read:
        // such a request is addressed to a bucket's website host ([`website_endpoint`]), which
        // no other service of this emulator is reached under, and it is ANONYMOUS by design —
        // so no credential scope names a service for the router to resolve it by.
        if website_endpoint::addressed_bucket(parts).is_some() {
            return true;
        }
        // This service signs as `s3` — a signing name it SHARES with S3 Control, whose
        // own endpoint is `s3-control`. The router's routing key comes from the SigV4
        // credential scope first, so both services resolve to `s3`, and matching on the
        // signing name ALONE makes both claim every request — which the router answers
        // `500 AmbiguousService` for, to BOTH services' traffic.
        //
        // S3 cannot narrow itself by its own path space (every operation is bound under
        // the `{Bucket}` label, so its segment list is empty), so it claims every path
        // under the shared signing name EXCEPT the peer's own (see `PEER_PATH_SEGMENTS`).
        //
        // It also claims the S3 EXPRESS signing name: the Zonal endpoint surface of this
        // same model — `CreateSession` and the object operations on a directory bucket —
        // is signed for `s3express` rather than for `s3`, and this is the service that
        // serves those operations. No other emulated service signs as `s3express`, so the
        // widened claim leaves exactly one claimant for such a request instead of none.
        let claimed = match smithy_server_runtime::resolve_route_key(parts) {
            Some(key)
                if key == self.entry.identity().signing_name() || key == S3_EXPRESS_SIGNING_NAME =>
            {
                !addresses_peer_path(parts, PEER_PATH_SEGMENTS)
            }
            // A VIRTUAL-HOSTED-STYLE request that names no service in a credential: with no
            // `Authorization` header and no presigned credential scope, the only routable
            // signal left is the host — whose first label is the BUCKET, not a service, so
            // the resolved key is that bucket's name and no service would claim the request
            // at all. It is this service's when that label addresses a bucket this emulator
            // holds ([`addressed_in_host`]); requiring the resolved key to BE that label is
            // what keeps a request signed for another service (whose own key the credential
            // named) out of this branch.
            Some(key) => request_host(parts)
                .and_then(|host| {
                    addressed_in_host(host, parts.uri.path(), &|name| {
                        self.addresses_a_bucket(name)
                    })
                })
                .is_some_and(|bucket| key == bucket),
            // A request whose ONLY credential is a query-string one the framework's routing
            // key does not read: the legacy presigned form, whose three parameters
            // ([`LEGACY_PRESIGNED_PARAMETERS`]) are S3's own — "you can use query parameters
            // to … authenticate the request" (AWS General Reference, *Authenticating
            // requests: using query parameters*). It names no service in a scope, so the
            // framework resolves no routing key and no service would claim it; where the
            // credential travels is not part of addressing, so it is this service's request
            // and is claimed as one. Whether it then VERIFIES is the framework's own
            // judgement, exactly as for the SigV4 query form.
            //
            // An ANONYMOUS (unsigned) request names no service either: it carries no
            // credential scope and no `X-Amz-Target`, and the endpoint host a client of this
            // emulator addresses is a loopback name the framework reads no service label out
            // of — so no routing key is resolved and no service would claim it, leaving the
            // router to answer its "no such service" 404. S3 is nonetheless reached
            // anonymously by design ("if you grant READ access to the anonymous user, the
            // GetObject operation returns the object without using an authorization header",
            // `GetObject`'s own documentation), and such a request is this service's when
            // its PATH addresses a bucket this emulator holds — the path-style half of the
            // host-addressed rule above, resolved against the same buckets. What the caller
            // may then do is the operation's own authorization question, which a refusal
            // answers with S3's own code rather than with a routing error.
            None => {
                carries_legacy_presigned_credential(&parts.uri)
                    || first_path_segment(parts.uri.path())
                        .is_some_and(|segment| self.addresses_a_bucket(segment))
            }
        };
        claimed
    }

    fn override_operation(
        &self,
        req: &RtRequest,
    ) -> Option<&dyn ErasedOperation> {
        // What this request presented to authentication, kept for the length of the
        // request so a refusal of it can be worded for the condition that refused it (see
        // [`sigv4`]). Recorded HERE because this is the seam the framework reaches with the
        // whole request — its body included, which is what the payload half of a canonical
        // request is built from — and it reaches it BEFORE it authenticates, for the one
        // service that claimed the request.
        //
        // The BUCKET the request addresses travels with it, resolved from the request's
        // PATH-STYLE address ([`Self::path_style_path`]) so a virtual-hosted-style request
        // names the same bucket a path-style one does: a refusal of an anonymous caller is
        // worded from that bucket's own configuration, and the seam that words it is handed
        // no request to read it from.
        sigv4::remember(req, first_path_segment(&self.path_style_path(req)));
        // The WEBSITE ENDPOINT answers the requests addressed to a bucket's website host, and
        // the model's routing table cannot name it at all: the endpoint is not the REST API, so
        // the same method and path mean something else there (`GET /` is the bucket's index
        // document, not a listing). Resolved FIRST, so nothing of the REST surface — the
        // bucket-name refusal, a session token, a subresource — is applied to a request that
        // never addressed it. See [`website_endpoint`].
        if website_endpoint::addressed_bucket_of(req).is_some() {
            return self.door.get(website_endpoint::WEBSITE_ENDPOINT);
        }
        // A request addressing a bucket by a name S3 can address no bucket by is refused for
        // the NAME, whichever operation its address names; see
        // [`Self::refuses_the_bucket_name`] and [`invalid_bucket_name`]. First, because a
        // name no bucket can be addressed by is refused for itself before anything about the
        // caller's session is asked.
        if let Some(refusal) = self.refuses_the_bucket_name(req) {
            return Some(refusal);
        }
        // A Zonal endpoint request carries the session it acts under in its
        // `x-amz-s3session-token` header, and may equally be signed with that session's own
        // key pair; a session "scoped to the bucket" authorizes nothing on another one — so a
        // session that does not cover the bucket this request addressed is refused here,
        // whichever operation it addressed. The framework has already verified the request's
        // SIGNATURE by the time it reaches this seam, so this refusal is only ever about the
        // session. See [`express_session`].
        if let Some(refusal) = self.refused_session_token(req) {
            return Some(refusal);
        }
        // A `?session` request against a bucket that HAS no session subresource — a general
        // purpose bucket — is the bucket's own listing and not `CreateSession`; see
        // [`LISTING_WITHOUT_SUBRESOURCE`]. Resolved before the model's routing table, which
        // matches the subresource whatever bucket it was addressed to, and from the request's
        // PATH-STYLE address so the two address forms resolve alike.
        if self.addresses_session_of_general_purpose_bucket(req) {
            return self
                .entry
                .routing_table()
                .get(LISTING_WITHOUT_SUBRESOURCE);
        }
        // The listing of this account's DIRECTORY buckets, which shares its method and path
        // with the listing of its general purpose ones and is told from it by the endpoint
        // the request reached — see [`Self::addresses_directory_bucket_listing`]. Resolved
        // before the model's routing table, which cannot tell two routes apart on a
        // discriminator the request does not carry.
        if self.addresses_directory_bucket_listing(req) {
            return self.entry.routing_table().get(LIST_DIRECTORY_BUCKETS);
        }
        // The BROWSER-FORM upload of an object, which the model's routing table cannot name
        // at all: no operation is bound at `POST /{Bucket}`, so the request resolved to
        // nothing and was answered as unwritten. See
        // [`Self::addresses_form_post_upload`] for what makes a request that one, and
        // [`post_object`] for what serving it is.
        if self.addresses_form_post_upload(req) {
            return self.door.get(post_object::POST_OBJECT);
        }
        // The SELECT data plane, which the model's routing table cannot name either: no entry
        // claims `?select`, so the request resolved to nothing and was answered as unwritten.
        // What this service answers it with is decided from the store and from the calling
        // principal ([`select_object_content`]), which is why it is an operation. Resolved from
        // the request's PATH-STYLE address ([`Self::path_style_path`]), so a
        // virtual-hosted-style Select request — the form every SDK sends for an endpoint
        // addressed by bucket name — resolves the same as a path-style one.
        if addresses_s3_select(&self.path_style_path(req), wire::request_query(req.uri())) {
            return self.door.get(select_object_content::SELECT_OBJECT_CONTENT);
        }
        // A write or a delete addressing an ID-ADDRESSED bucket configuration subresource
        // that states no `id` is that subresource's call with an id missing, never the
        // bucket-level `CreateBucket` or `DeleteBucket` bound to the same method and the bare
        // path; see [`Self::configuration_subresource_operation`]. Resolved before the
        // model's routing table, which drops the subresource route for the required binding
        // the request does not carry.
        if let Some(operation) = self.configuration_subresource_operation(req) {
            return Some(operation);
        }
        // A read of an object's ATTRIBUTES that states no `x-amz-object-attributes` is that
        // read with the header missing, never the `GetObject` bound to the same method and
        // path with no subresource; see [`Self::object_attributes_without_the_header`].
        // Resolved before the model's routing table, which drops the subresource route for
        // the required binding the request does not carry.
        if let Some(operation) = self.object_attributes_without_the_header(req) {
            return Some(operation);
        }
        // A VIRTUAL-HOSTED-STYLE request names its bucket in the host, so its path is the
        // key alone — `GET /key-name` — which the model's routing table cannot name: every
        // route of this service begins with the `{Bucket}` label, so the key would be read
        // as the bucket and a read of an object would resolve as a read of a BUCKET that
        // does not exist. The route is resolved from the PATH-STYLE address of the same
        // object instead ([`path_with_bucket`]), through the same routing table and the same
        // protocol the model-derived lookup uses; the matched operation then binds its
        // `{Bucket}` label from that same normalized path (see
        // [`wire::UnwrappedXmlRestXml::deserialize_input`]).
        //
        // For a PATH-STYLE request the generated routing table resolves it exactly as
        // before, except for the one operation served behind the front door below.
        let Some(bucket) = self.host_addressed_bucket(req, wire::request_path(req.uri())) else {
            return self.front_door_operation(req);
        };
        let rewritten = with_path_style_address(req, &bucket)?;
        // The ENTRY's protocol, which is the one carrying this service's model-derived
        // routes (this file's own `self.protocol` serializes and deserializes; it was built
        // with no route table, because routing is the entry's job).
        let operation = self
            .entry
            .protocol()
            .routing_key(&rewritten)
            .ok()?
            .to_string();
        self.front_door_operation(&rewritten)
            .or_else(|| self.entry.routing_table().get(&operation))
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, a value outside a declared bound, or a caller
        // authentication refused) to let this service word it. S3's refusals carry
        // S3's own error codes, never the framework's generic
        // `ValidationException`: see [`s3_refusal`] for the rule — a value refused
        // inside the request's XML document is `MalformedXML`, one refused in a
        // header, a query parameter or a URI label is `InvalidArgument`. The FIRST
        // violation is what the answer names, the way the protocol's own envelope
        // reports one code.
        match refusal {
            Refusal::Invalid(violations) => violations.first().map(s3_refusal),
            // An AUTHENTICATION refusal is worded in
            // [`Self::authentication_refusal_response`] instead, which this seam cannot
            // reach: S3's answer to one differs from the framework's rendering in its
            // STATUS and in the ELEMENTS it carries, and neither travels on a [`Refused`].
            // Declining here leaves the framework's own code and message for the renderer
            // to fall back on, which is what a refusal this service does not word keeps.
            _ => None,
        }
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501), which is what every one of them
        // keeps: an operation that really is merely unwritten answers the 501 that counts it
        // as such.
        //
        // The requests this service serves that no routing-table entry claims — the
        // browser-form upload, the website endpoint and the SELECT data plane — are resolved
        // to operations of their own at [`Self::override_operation`] and never reach here;
        // each needs the store and the calling principal to be answered, which only an
        // operation is given (see [`select_object_content`] for the one this seam used to
        // word).
        let _ = req;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restXml protocol in every respect but one: an operation the
        // model marks `aws.customizations#s3UnwrappedXmlOutput` answers the document AWS
        // sends for it, its single output member promoted to the response root, which the
        // framework's generic envelope cannot express. See [`wire`].
        Some(&self.protocol)
    }

    fn override_framework_error(
        &self,
        error: &ProtocolError,
    ) -> Option<RtResponse> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

/// The model-derived half of this service, DELEGATED to the generated entry — every datum
/// baked at codegen is read off it, so nothing here can drift from the model — with the one
/// renderer this service owns.
///
/// `refusal_response` is that renderer, and it is the reason this file implements the trait
/// at all rather than handing the entry back bare: it is the only place an AUTHENTICATION
/// refusal's STATUS and ELEMENTS can be chosen, and S3 chooses both (see the file header and
/// [`sigv4`]). Everything else is the entry's.
impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for EmulatedService<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.entry.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.entry.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        // The ONE refusal the framework renders through here is the authentication one: a
        // caller it refused before any handler ran, which this service words itself.
        // Anything else — and an authentication refusal this service declines to word — falls
        // through to the ENTRY's renderer, `chosen` included, so a wording chosen through
        // `override_refusal` is never dropped on the way past.
        if matches!(error, ProtocolError::Unauthenticated { .. }) {
            if let Some(response) = self.authentication_refusal_response() {
                return response;
            }
        }
        // A refusal a FRONT DOOR worded ([`front_door`]) reaches this renderer with the
        // violations it found, which is what lets the document also carry the members S3
        // names the refused argument by ([`refused_argument`]); everything else keeps the
        // entry's rendering of the code and message alone.
        if let ProtocolError::Invalid(violations) = &error {
            if let Some(violation) = violations.first() {
                // The wording this service chose for the refusal, derived ONCE: both
                // renderers below carry the same code and message, and only the document's
                // members differ.
                let refused = chosen.clone().unwrap_or_else(|| s3_refusal(violation));
                if let Some(detail) = refused_argument(violation) {
                    if let Some(response) = self.detailed_refusal(&refused, detail) {
                        return response;
                    }
                }
                // Every other refusal of a value the model declares a rule for is rendered
                // through THIS service's own error channel too, for the one thing the
                // entry's renderer cannot name: the pair of request-correlation ids an S3
                // response carries. The entry's renderer mints ids of its own shape, where
                // every other answer of this service — a served response and a handler's
                // refusal alike — is named by S3's ([`crate::state::request_ids`]), and
                // the SHAPE is what a client, a log correlator or a support request reads.
                // The status is the 400 every protocol answers a value outside its declared
                // rules with (see [`EmulatedService::worded_refusal`]).
                if let Some(response) = self.worded_refusal(&refused) {
                    return response;
                }
            }
        }
        self.entry.refusal_response(error, chosen, req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.entry.wire_protocol(req)
    }
}
