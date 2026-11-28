// SPDX-License-Identifier: CC0-1.0

//! How a request whose credential travels among its OWN QUERY PARAMETERS —
//! `AWSAccessKeyId`, `Signature` and `Expires` — is authenticated.
//!
//! "You can use query parameters to … authenticate the request" (AWS General Reference,
//! *Authenticating requests: using query parameters*), and Amazon S3 publishes TWO
//! presentations of such a credential: the `X-Amz-`-prefixed SigV4 one, which the framework's
//! own authenticator recognises and verifies, and the one named above, whose signature is the
//! base64 of an HMAC-SHA1 over a string built from the request alone. A client minting a
//! presigned URL chooses between them, so a URL in the second presentation reaches this
//! emulator carrying a complete credential that nothing registered with the framework can
//! read: the request matches no authenticator, and the caller is refused before any handler
//! runs even though the credential it names is one this emulator issued.
//!
//! So this surface is the one that authenticates those requests, exactly as Amazon S3's
//! browser-form upload authenticates its own ([`super::post_object`]): the request is served
//! under NO scheme the framework verifies
//! ([`super::unsigned_request::ServedUnsigned`] states an empty scheme list for it) and
//! NOTHING is waved through — [`denial_key`] below recomputes the signature the URL carries
//! before the operation it addresses is served, and refuses a request whose window has passed
//! or whose signature does not verify.
//!
//! # What refuses such a request, and how AWS words each
//!
//! * the WINDOW it asks for has passed — the signature is intact and the URL is simply no
//!   longer usable, which S3 states as the expiry it is ([`EXPIRED_WINDOW`]);
//! * the signature does not verify under the named credential's secret, which is the one
//!   condition left once the credential has resolved ([`SIGNATURE_MISMATCH`]).
//!
//! The order is the one the SigV4 presentation is judged in ([`super::sigv4::refusal_for`]):
//! the window first, because an expired URL is refused for its window whatever its signature
//! computes to, then the signature.
//!
//! # The string the signature covers
//!
//! "StringToSign = HTTP-VERB + "\n" + Content-MD5 + "\n" + Content-Type + "\n" + Expires +
//! "\n" + CanonicalizedAmzHeaders + CanonicalizedResource" (Amazon S3 API Reference, *REST
//! Authentication*, with the `Expires` of the query presentation standing in for the `Date` a
//! header-signed request carries). Each half is built below: [`canonical_amz_headers`] for
//! the `x-amz-` headers the request sent, and [`canonical_resource`] for the resource it
//! addressed — the path-style address of the bucket and key, plus the subresource parameters
//! that are part of the resource rather than of the request
//! ([`RESOURCE_PARAMETERS`]).
//!
//! # What this surface declines to judge
//!
//! It refuses a request only where it could VERIFY one: where this process holds the secret of
//! the key the URL names ([`resolvable_secret`]). The one such credential is the key pair this
//! process published — which is also the pair a client of this emulator signs with — because
//! the secrets of the credentials an operation created live in the framework's store, which
//! hands none of them back, and an emulator that verifies no signatures publishes no pair at
//! all. A key id whose secret cannot be read is therefore left exactly as it was before this
//! surface existed: with request authentication ON it keeps the framework's own refusal of a
//! credential it could not resolve — the `InvalidAccessKeyId` S3 publishes for that condition,
//! which [`super::sigv4::refusal_for`] words — and with it OFF the request is served, since
//! this one surface must not demand a credential the rest of the emulator does not. A URL
//! whose `Expires` is not a count of seconds states no window at all, so it is not read as
//! this presentation either ([`presents_a_query_credential`]) and keeps the answer it earns
//! today.

use ::std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The parameter naming the access key the URL was signed with — the first of the three
/// parameters this presentation's credential travels in, read from the one catalog of them
/// this tree holds (`service_common::presigned_url_credential`), so the form this module
/// verifies and the form the service CLAIMS cannot drift apart.
const ACCESS_KEY_ID_PARAMETER: &str =
    ::service_common::presigned_url_credential::LEGACY_CREDENTIAL_PARAMETERS[0];

/// The parameter carrying the signature itself. See [`ACCESS_KEY_ID_PARAMETER`].
const SIGNATURE_PARAMETER: &str =
    ::service_common::presigned_url_credential::LEGACY_CREDENTIAL_PARAMETERS[1];

/// The parameter stating the instant the URL stops being usable, in seconds since the epoch.
/// See [`ACCESS_KEY_ID_PARAMETER`].
const EXPIRES_PARAMETER: &str =
    ::service_common::presigned_url_credential::LEGACY_CREDENTIAL_PARAMETERS[2];

/// The query parameters of the OTHER presentation of a query credential — the SigV4 one the
/// framework's own authenticator recognises and verifies.
///
/// A URL carrying either of them is that authenticator's request, whatever else its query
/// holds, so it is not read here: one request is authenticated by one scheme.
const SIGV4_QUERY_PARAMETERS: [&str; 2] = ["X-Amz-Algorithm", "X-Amz-Signature"];

/// The header a request carries the digest of its payload in, the second line of the string
/// the signature covers.
const CONTENT_MD5_HEADER: &str = "content-md5";

/// The header a request carries its payload's media type in, the third line of that string.
const CONTENT_TYPE_HEADER: &str = "content-type";

/// The prefix of the headers that string carries in full: "CanonicalizedAmzHeaders" is built
/// from "all HTTP headers whose names begin with `x-amz-`" (Amazon S3 API Reference, *REST
/// Authentication*).
const AMZ_HEADER_PREFIX: &str = "x-amz-";

/// The query parameters that are part of the RESOURCE a request addresses rather than of the
/// request itself, so the string the signature covers names them.
///
/// The subresources the published derivation lists — "acl, lifecycle, location, logging,
/// notification, partNumber, policy, requestPayment, torrent, uploadId, uploads, versionId,
/// versioning, versions, website" — together with the response-header overrides the same
/// section names ("the overriding response header values": `response-cache-control`,
/// `response-content-disposition`, `response-content-encoding`, `response-content-language`,
/// `response-content-type`, `response-expires`), both from Amazon S3 API Reference, *REST
/// Authentication*, "Constructing the CanonicalizedResource Element".
///
/// And the subresources S3 has PUBLISHED SINCE that section was written, each the `?<name>`
/// a bucket- or object-level configuration call of this model is bound to
/// (`smithy_query(query="operation")` names the binding of each: `GetBucketTagging` is
/// `GET /{Bucket}?tagging`, `GetBucketCors` `?cors`, `DeleteObjects` `?delete`,
/// `RestoreObject` `?restore`, `GetBucketAccelerateConfiguration` `?accelerate`,
/// `GetBucketReplication` `?replication`, `GetBucketEncryption` `?encryption`,
/// `GetObjectLockConfiguration` `?object-lock`, `GetObjectRetention` `?retention`,
/// `GetObjectLegalHold` `?legal-hold`, `GetPublicAccessBlock` `?publicAccessBlock`,
/// `SelectObjectContent` `?select` with its `select-type`, `GetBucketAnalyticsConfiguration`
/// `?analytics`, `GetBucketInventoryConfiguration` `?inventory`,
/// `GetBucketMetricsConfiguration` `?metrics`, `GetObjectAttributes` `?attributes`): each
/// names a resource of its own rather than a parameter of a read, which is why the signature
/// covers it, and a client's signer carries the same set.
///
/// Sorted here as that element sorts them, so the rendering below walks the query once.
const RESOURCE_PARAMETERS: [&str; 41] = [
    "accelerate",
    "acl",
    "analytics",
    "attributes",
    "cors",
    "delete",
    "encryption",
    "intelligent-tiering",
    "inventory",
    "legal-hold",
    "lifecycle",
    "location",
    "logging",
    "metrics",
    "notification",
    "object-lock",
    "ownershipControls",
    "partNumber",
    "policy",
    "policyStatus",
    "publicAccessBlock",
    "replication",
    "requestPayment",
    "response-cache-control",
    "response-content-disposition",
    "response-content-encoding",
    "response-content-language",
    "response-content-type",
    "response-expires",
    "restore",
    "retention",
    "select",
    "select-type",
    "tagging",
    "torrent",
    "uploadId",
    "uploads",
    "versionId",
    "versioning",
    "versions",
    "website",
];

/// What a request this surface refuses is answered with: the `<Code>` a client branches on,
/// the status it carries and the `<Message>`, which is fixed text in each case — neither
/// condition names a value from the request.
#[derive(Clone, Copy, Debug)]
pub struct QueryCredentialRefusal {
    /// The `<Code>`.
    pub code: &'static str,
    /// The HTTP status it is answered with.
    pub status: u16,
    /// The `<Message>`.
    pub message: &'static str,
}

/// The window the URL asked for has PASSED: the signature is intact and the caller resolved,
/// so what refused the request is the window — the same code and sentence the SigV4
/// presentation of an expired presigned request is refused with.
const EXPIRED_WINDOW: QueryCredentialRefusal = QueryCredentialRefusal {
    code: crate::state::ACCESS_DENIED_CODE,
    status: crate::state::ACCESS_DENIED_STATUS,
    message: "Request has expired",
};

/// The credential resolved and the signature computed under its secret is not the one the URL
/// carried — the condition a tampered or re-signed URL earns. Fixed text in full, as the
/// signing layer words it for this one condition.
const SIGNATURE_MISMATCH: QueryCredentialRefusal = QueryCredentialRefusal {
    code: "SignatureDoesNotMatch",
    status: crate::state::ACCESS_DENIED_STATUS,
    message: "The request signature we calculated does not match the signature you provided. \
              Check your key and signing method.",
};

/// The routing key the refusal of a PASSED window is held in this service's own table under.
pub const REFUSE_EXPIRED_WINDOW: &str = "@refuse-query-credential-expired-window";
/// The routing key the refusal of a signature that did not verify is held under.
pub const REFUSE_SIGNATURE_MISMATCH: &str = "@refuse-query-credential-signature-mismatch";

/// The refusals this surface answers, each with the key it is served under: keys the model's
/// protocol never resolves, so no modelled request reaches one by name.
pub const QUERY_CREDENTIAL_DENIED_KEYS: [(&str, QueryCredentialRefusal); 2] = [
    (REFUSE_EXPIRED_WINDOW, EXPIRED_WINDOW),
    (REFUSE_SIGNATURE_MISMATCH, SIGNATURE_MISMATCH),
];

/// Whether `req` presents its credential among its own query parameters — all three of
/// [`ACCESS_KEY_ID_PARAMETER`], [`SIGNATURE_PARAMETER`] and [`EXPIRES_PARAMETER`], with an
/// `Expires` that really is a count of seconds since the epoch.
///
/// Three things narrow it, because what this answers is WHICH scheme authenticates the
/// request:
///
/// * a request carrying an `Authorization` header presents a header credential, which is the
///   framework's own to verify;
/// * a request carrying the SigV4 query presentation ([`SIGV4_QUERY_PARAMETERS`]) is that
///   authenticator's too, whatever else its query holds;
/// * an `Expires` that is not a count of seconds states no window, so such a URL is not this
///   presentation and keeps the refusal it earns today — nothing here invents a wording for a
///   value AWS answers its own.
#[must_use]
pub fn presents_a_query_credential(req: &RtRequest) -> bool {
    presented(req).is_some()
}

/// Whether THIS SURFACE, rather than the framework's own authenticator, authenticates `req`:
/// the request presents this presentation ([`presents_a_query_credential`]) AND this process
/// holds the secret of the key it names ([`resolvable_secret`]), so the signature it carries
/// can really be recomputed.
///
/// The second half is what keeps this surface from claiming a request it could say nothing
/// about: such a request is left to the boundary that already answers it — the framework's own
/// refusal of a credential it could not resolve where signatures are verified, and the serving
/// of it where they are not. See [`denial_key`], which declines the same requests for the same
/// reason.
#[must_use]
pub fn authenticates(req: &RtRequest) -> bool {
    presented(req).is_some_and(|presented| resolvable_secret(&presented.access_key_id).is_some())
}

/// What this surface reads off `req`: the credential it names, the signature it presents and
/// the window it asks for — or `None` for a request this scheme does not authenticate
/// ([`presents_a_query_credential`] states which).
fn presented(req: &RtRequest) -> Option<Presented> {
    if req
        .headers()
        .get(::http::header::AUTHORIZATION.as_str())
        .is_some()
    {
        return None;
    }
    let query = super::wire::request_query(req.uri());
    if SIGV4_QUERY_PARAMETERS
        .iter()
        .any(|parameter| super::query_carries(query, parameter))
    {
        return None;
    }
    let field = |name: &str| super::sigv4::query_parameter(query, name);
    let expires = field(EXPIRES_PARAMETER)?;
    Some(Presented {
        valid_until: UNIX_EPOCH.checked_add(Duration::from_secs(
            expires.trim().parse::<u64>().ok()?,
        ))?,
        expires,
        access_key_id: field(ACCESS_KEY_ID_PARAMETER)?,
        signature: field(SIGNATURE_PARAMETER)?,
    })
}

/// The credential one request presents in its query.
struct Presented {
    /// The access key id it names.
    access_key_id: String,
    /// The signature it carries, as the URL spelled it.
    signature: String,
    /// The window's end as the URL spelled it, which is the text the signature covers.
    expires: String,
    /// That same instant, read as the seconds since the epoch it states.
    valid_until: SystemTime,
}

/// The [`QUERY_CREDENTIAL_DENIED_KEYS`] key `req` is refused under, or `None` for a request
/// this surface does not refuse — one whose signature verifies inside the window it asks for,
/// and equally one this surface can say NOTHING about.
///
/// It judges a request only where it could verify it: where this process holds the SECRET of
/// the key the URL names. A key id it holds no secret for — one of the credentials an
/// operation created, whose secrets the framework's store hands back to nobody, and every key
/// id at all in an emulator that verifies no signatures, where no key pair is published — is
/// declined rather than refused, so this surface never demands a credential the rest of the
/// emulator does not: with request authentication ON such a request keeps the framework's own
/// refusal of a credential it could not resolve, which is the `InvalidAccessKeyId` S3
/// publishes for that condition ([`super::sigv4::refusal_for`] words it), and with it OFF the
/// request is served as it was before this surface existed. The same reading the browser-form
/// upload's policy signature is judged by ([`super::post_object`], which judges nothing when
/// it cannot resolve the credential the form named).
///
/// `resource` is the PATH-STYLE address of the request, which is what the string to sign
/// names: a request that addressed its bucket in the host signed the same resource a
/// path-style one does (the signing path of a virtual-hosted request carries the bucket), so
/// the caller hands the one reading of that address this service has.
///
/// `now` is the instant the window is judged against, taken by the caller so nothing here
/// reads the clock twice.
#[must_use]
pub fn denial_key(req: &RtRequest, resource: &str, now: SystemTime) -> Option<&'static str> {
    let presented = presented(req)?;
    // The CREDENTIAL first, as the question of whether this surface judges the request at all:
    // without the secret of the key the URL names, neither its window nor its signature is
    // this surface's to answer.
    let secret = resolvable_secret(&presented.access_key_id)?;
    // Then the WINDOW: a URL whose validity has passed is refused for it whatever its
    // signature computes to.
    if presented.valid_until < now {
        return Some(REFUSE_EXPIRED_WINDOW);
    }
    // And last the signature, the one condition left once the credential has resolved.
    let signed = string_to_sign(req, resource, &presented.expires);
    let computed = signature_for(&secret, &signed);
    match computed.is_some_and(|computed| computed == presented.signature) {
        true => None,
        false => Some(REFUSE_SIGNATURE_MISMATCH),
    }
}

/// The SECRET of `access_key_id`, or `None` for a key id whose secret this process cannot
/// read.
///
/// The one credential whose secret is readable here is the key pair this process PUBLISHED
/// (`SigV4Authenticator::minted_credentials`), which is also the pair a client of this emulator
/// signs with; the secrets of the credentials an operation created live in the framework's
/// store, which hands none of them back, and a process that published no pair at all — an
/// emulator verifying no signatures — holds no secret for any key id.
fn resolvable_secret(access_key_id: &str) -> Option<String> {
    let published =
        ::emulator_server::request_authentication::SigV4Authenticator::minted_credentials()?;
    (published.access_key_id == access_key_id).then(|| published.secret_access_key.clone())
}

/// The string the signature of `req` covers (see the module docs for the derivation):
/// the method, the payload digest and media type the request declared, the window it asks
/// for, its `x-amz-` headers and the resource it addresses.
fn string_to_sign(req: &RtRequest, resource: &str, expires: &str) -> String {
    let header = |name: &str| {
        req.headers()
            .get(name)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    format!(
        "{}\n{}\n{}\n{expires}\n{}{}",
        req.method(),
        header(CONTENT_MD5_HEADER),
        header(CONTENT_TYPE_HEADER),
        canonical_amz_headers(req),
        canonical_resource(resource, super::wire::request_query(req.uri())),
    )
}

/// The `x-amz-` headers of `req`, each as `<lower-case name>:<value>\n`, sorted by name and
/// with the values of a repeated name joined by commas — "combine header fields with the same
/// name into one `header-name:comma-separated-value-list` pair" (Amazon S3 API Reference,
/// *REST Authentication*). The empty string for a request carrying none.
fn canonical_amz_headers(req: &RtRequest) -> String {
    let mut named: ::std::collections::BTreeMap<String, Vec<String>> =
        ::std::collections::BTreeMap::new();
    for (name, value) in req.headers().iter() {
        let name = name.to_ascii_lowercase();
        if !name.starts_with(AMZ_HEADER_PREFIX) {
            continue;
        }
        named.entry(name).or_default().push(value.trim().to_string());
    }
    named
        .into_iter()
        .map(|(name, values)| format!("{name}:{}\n", values.join(",")))
        .collect()
}

/// The resource `path` addresses, with the parameters of `query` that are part of that
/// resource ([`RESOURCE_PARAMETERS`]) appended: sorted by name, each as the client spelled it
/// (`name=value`, or the bare `name` for a parameter carrying no value at all), joined by `&`
/// behind one `?`.
fn canonical_resource(path: &str, query: &str) -> String {
    let mut named: Vec<String> = Vec::new();
    for parameter in query.split('&').filter(|item| !item.is_empty()) {
        let (name, value) = match parameter.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (parameter, None),
        };
        let name = super::sigv4::percent_decoded(name);
        if !RESOURCE_PARAMETERS.contains(&name.as_str()) {
            continue;
        }
        named.push(match value {
            Some(value) => format!("{name}={}", super::sigv4::percent_decoded(value)),
            None => name,
        });
    }
    named.sort();
    match named.is_empty() {
        true => path.to_string(),
        false => format!("{path}?{}", named.join("&")),
    }
}

/// The signature `string_to_sign` has under `secret`: "the HMAC-SHA1 of the UTF-8 encoding of
/// the StringToSign, base64-encoded" (Amazon S3 API Reference, *REST Authentication*).
/// `None` for a key the construction does not take, which HMAC has none of.
fn signature_for(secret: &str, string_to_sign: &str) -> Option<String> {
    use ::base64::Engine as _;
    use ::hmac::Mac as _;

    let mut mac =
        <::hmac::Hmac<::sha1::Sha1> as ::hmac::Mac>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(string_to_sign.as_bytes());
    Some(
        ::base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()),
    )
}

/// The operation this service answers a request whose query credential it refused with.
///
/// It holds the refusal rather than resolving it: WHICH condition refused the request is read
/// by the routing seam, which is the seam that has the request and its address.
pub struct QueryCredentialDenied {
    /// What that seam decided about the credential this request presented.
    refusal: QueryCredentialRefusal,
}

impl QueryCredentialDenied {
    /// The operation that answers `refusal`.
    #[must_use]
    pub fn new(refusal: QueryCredentialRefusal) -> Self {
        Self { refusal }
    }

    /// The error document this refusal is answered with, rendered through the SAME protocol
    /// every other answer of this service is rendered through, so it carries this service's
    /// envelope and its correlation headers.
    fn answer(&self, svc: &dyn Service) -> RtResponse {
        let protocol = svc
            .override_protocol()
            .unwrap_or_else(|| svc.definition().protocol());
        let document = crate::scaffolded::UnmodeledMessage {
            message: self.refusal.message.to_string(),
        };
        protocol
            .serialize_error(
                &document,
                &crate::scaffolded::UNMODELED_SCHEMA,
                self.refusal.code,
                self.refusal.status,
            )
            .unwrap_or_else(|error| {
                ::tracing::warn!(
                    code = self.refusal.code,
                    %error,
                    "this refusal of a query credential could not be rendered in this \
                     service's envelope"
                );
                let status = StatusCode::try_from(self.refusal.status).unwrap_or_else(|_| {
                    StatusCode::try_from(crate::state::ACCESS_DENIED_STATUS)
                        .expect("the status of a refused caller is a status")
                });
                RtResponse::new(status, SdkBody::empty())
            })
    }
}

impl ErasedOperation for QueryCredentialDenied {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.answer(svc) })
    }

    /// Served under NO scheme the framework verifies: the credential this request presents is
    /// one no registered authenticator can read, which is why this service authenticates it
    /// itself. Nothing is waved through — this operation IS the refusal of such a request,
    /// and a request that verifies is served by the operation it addressed, behind the same
    /// empty scheme list ([`super::unsigned_request::ServedForItsOwnCredential`]).
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request as a row states it: the method, the address, the headers and the query.
    fn request(method: &str, uri: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut builder = ::http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(SdkBody::empty())
            .expect("the row must state a request");
        RtRequest::try_from(request).expect("the row's request must convert")
    }

    /// RULE: the string a request's signature covers is the published derivation of it — the
    /// method, the digest and media type it declared, the window it asks for, its `x-amz-`
    /// headers and the resource it addressed.
    #[test]
    fn the_string_to_sign_is_the_published_derivation() {
        // The rows span what the derivation generalises over: which of the two optional
        // headers the request carries, how many `x-amz-` headers it sends, and whether the
        // address carries a parameter that is part of the resource. Every row carries the
        // whole credential, as a client's URL does — and none of its three parameters is part
        // of the resource, which is what the expected strings also show.
        let credential = "AWSAccessKeyId=AKIDEXAMPLE&Signature=Oeb3o%2FbtlnBhyd0%2BZ6Qc8w%3D";
        let rows: [(&str, RtRequest, &str, &str); 5] = [
            (
                "a read carrying nothing but its window",
                request(
                    "GET",
                    &format!("/bucket/key.txt?{credential}&Expires=1762000000"),
                    &[],
                ),
                "/bucket/key.txt",
                "GET\n\n\n1762000000\n/bucket/key.txt",
            ),
            (
                "a write declaring its digest and media type",
                request(
                    "PUT",
                    &format!("/bucket/written.bin?{credential}&Expires=1762000000"),
                    &[
                        ("content-md5", "q2dJlmTZ6AsbFqPbRyV1pw=="),
                        ("content-type", "text/plain"),
                    ],
                ),
                "/bucket/written.bin",
                "PUT\nq2dJlmTZ6AsbFqPbRyV1pw==\ntext/plain\n1762000000\n/bucket/written.bin",
            ),
            (
                "a write carrying two `x-amz-` headers, which sort",
                request(
                    "PUT",
                    &format!("/bucket/written.bin?{credential}&Expires=1"),
                    &[
                        ("x-amz-storage-class", "STANDARD_IA"),
                        ("x-amz-acl", "private"),
                        ("x-not-amz", "ignored"),
                    ],
                ),
                "/bucket/written.bin",
                "PUT\n\n\n1\nx-amz-acl:private\nx-amz-storage-class:STANDARD_IA\n\
                 /bucket/written.bin",
            ),
            (
                "a read of a subresource, which is part of the resource",
                request("GET", &format!("/bucket?acl&{credential}&Expires=1"), &[]),
                "/bucket",
                "GET\n\n\n1\n/bucket?acl",
            ),
            (
                "a read naming a version and a response override, sorted",
                request(
                    "GET",
                    &format!(
                        "/bucket/key.txt?response-content-type=text%2Fplain&versionId=3&\
                         {credential}&Expires=1"
                    ),
                    &[],
                ),
                "/bucket/key.txt",
                "GET\n\n\n1\n/bucket/key.txt?response-content-type=text/plain&versionId=3",
            ),
        ];
        for (case, request, resource, expected) in rows {
            let expires = presented(&request)
                .unwrap_or_else(|| panic!("case {case:?}: the row must present a credential"))
                .expires;
            let actual = string_to_sign(&request, resource, &expires);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a request presents this credential only when all three of its parameters are
    /// there, its window is a count of seconds, and no other scheme's credential is.
    #[test]
    fn only_a_complete_query_credential_is_this_schemes() {
        let signed = "AWSAccessKeyId=AKID&Signature=abc%2Bdef&Expires=1762000000";
        let rows: [(&str, RtRequest, bool); 7] = [
            (
                "the three parameters, as a client orders them",
                request("GET", &format!("/bucket/key.txt?{signed}"), &[]),
                true,
            ),
            (
                "the same, with a response override beside them",
                request(
                    "GET",
                    &format!("/bucket/key.txt?response-content-type=text%2Fplain&{signed}"),
                    &[],
                ),
                true,
            ),
            (
                "a query missing the signature",
                request(
                    "GET",
                    "/bucket/key.txt?AWSAccessKeyId=AKID&Expires=1762000000",
                    &[],
                ),
                false,
            ),
            (
                "a window that is not a count of seconds",
                request(
                    "GET",
                    "/bucket/key.txt?AWSAccessKeyId=AKID&Signature=abc&Expires=tomorrow",
                    &[],
                ),
                false,
            ),
            (
                "a header-signed request whose query carries the three anyway",
                request(
                    "GET",
                    &format!("/bucket/key.txt?{signed}"),
                    &[("authorization", "AWS4-HMAC-SHA256 Credential=AKID/x")],
                ),
                false,
            ),
            (
                "a URL carrying the SigV4 presentation as well",
                request(
                    "GET",
                    &format!("/bucket/key.txt?{signed}&X-Amz-Signature=deadbeef"),
                    &[],
                ),
                false,
            ),
            (
                "a request carrying no credential at all",
                request("GET", "/bucket/key.txt", &[]),
                false,
            ),
        ];
        for (case, request, expected) in rows {
            let actual = presents_a_query_credential(&request);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
