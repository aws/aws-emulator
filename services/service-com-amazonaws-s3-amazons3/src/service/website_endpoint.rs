// SPDX-License-Identifier: CC0-1.0

//! The S3 WEBSITE ENDPOINT: the surface a bucket configured for static website hosting is
//! served on, which is a different endpoint from the REST API's.
//!
//! "Amazon S3 website endpoints" (Amazon S3 User Guide, *Website endpoints*) are a surface of
//! their own: a bucket with a website configuration is reachable at
//! `<bucket>.s3-website.<Region>.amazonaws.com` (and at the older
//! `<bucket>.s3-website-<Region>.amazonaws.com`), and what that surface answers is NOT the
//! REST API's document. It serves the configured index document for a request that addresses a
//! directory, the error document for a request that fails, a redirect for an object that
//! carries one, the redirects a bucket's routing rules call for, and it refuses every method
//! but a read. The REST endpoint is untouched by all of it: a missing key there is still
//! `NoSuchKey`, whatever routing rules the bucket carries.
//!
//! WHICH requests are this surface's is read off the address: the host's first label is the
//! bucket and the label after it begins with [`WEBSITE_ENDPOINT_LABEL`], which is what tells a
//! website-endpoint address from the REST endpoint's virtual-hosted one
//! ([`addressed_bucket`]). A client of this emulator reaches both on one port, so the HOST is
//! the only thing that can tell them apart — exactly as it is for AWS, where the two endpoints
//! differ in nothing else.
//!
//! The rules this module implements, each from the published behaviour of that endpoint:
//!
//! * only `GET` and `HEAD` are served: "the website endpoint supports only GET and HEAD
//!   requests on objects", so anything else is refused `405`
//!   ([`crate::state::METHOD_NOT_ALLOWED_CODE`]);
//! * a request that addresses a DIRECTORY — the root, or a path ending in `/` — is served the
//!   index document of that directory: the request's key with
//!   `IndexDocument$Suffix` appended, "a suffix that is appended to a request that is for a
//!   directory on the website endpoint";
//! * every request is ANONYMOUS: a website endpoint serves what the bucket grants to everyone,
//!   so content the bucket's policy and ACLs do not make public is refused `403` through the
//!   one rule this service answers an anonymous read by
//!   ([`crate::state::Bucket::anonymous_object_read_refusal`]);
//! * a key the caller may read that is not there is `404`, and the configured
//!   `ErrorDocument$Key` is what the body of such an answer is: "the object key name to use
//!   when a 4XX class error occurs";
//! * an object carrying `x-amz-website-redirect-location` is answered as a redirect to that
//!   location, while the REST endpoint still serves its bytes;
//! * `RoutingRules` redirect: a rule conditioned on `KeyPrefixEquals` redirects the requests
//!   whose key carries that prefix, and one conditioned on `HttpErrorCodeReturnedEquals`
//!   redirects a request whose answer IS that error — "in the event of an error, if the error
//!   code equals this value, then the specified redirect is applied";
//! * and `RedirectAllRequestsTo` redirects every request of the bucket, which is the one
//!   configuration that serves nothing at all.
//!
//! The answer is built here rather than through the service's XML protocol because a website
//! endpoint does not answer that protocol's documents: an error is an HTML page
//! ([`error_document`]), which is what a browser addressing a static site is given.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};
use ::std::sync::Arc;

/// The key this service serves the website endpoint under, carrying the `@` sigil no modelled
/// operation name holds — so nothing but [`super::EmulatedService::override_operation`] ever
/// looks it up (the same construction the sibling door keys use).
pub const WEBSITE_ENDPOINT: &str = "@s3-website-endpoint";

/// What the host label AFTER the bucket is on a website endpoint address:
/// `<bucket>.s3-website.<Region>.amazonaws.com` and the older
/// `<bucket>.s3-website-<Region>.amazonaws.com` (Amazon S3 User Guide, *Website endpoints*) —
/// so the label is this value, or this value with the Region after a hyphen.
pub const WEBSITE_ENDPOINT_LABEL: &str = "s3-website";

/// The bucket `parts` addresses on the WEBSITE endpoint, or `None` for a request addressed to
/// anything else.
///
/// The first label of the host is the bucket, and the label after it is what says this is the
/// website endpoint ([`WEBSITE_ENDPOINT_LABEL`]). The name is not resolved against the store
/// here: a website host naming a bucket this emulator does not hold is still a website-endpoint
/// request, and is answered as one ([`WebsiteEndpoint::serve`]) rather than left to another
/// surface.
pub fn addressed_bucket(parts: &::http::request::Parts) -> Option<String> {
    addressed_bucket_in_host(super::request_host(parts)?)
}

/// [`addressed_bucket`] for the runtime-API request the routing seam is handed, which carries
/// its host the same two ways ([`super::request_host_of`]).
pub fn addressed_bucket_of(req: &RtRequest) -> Option<String> {
    addressed_bucket_in_host(super::request_host_of(req)?)
}

/// The bucket `host` addresses on the website endpoint — the one reading both forms above
/// share.
fn addressed_bucket_in_host(host: &str) -> Option<String> {
    // The port is not part of the name, and neither is any userinfo in front of it.
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let (bucket, rest) = host.split_once('.')?;
    let label = rest.split('.').next().unwrap_or(rest);
    // The two published spellings and nothing else: the label IS `s3-website` (the dotted
    // form), or it carries the Region after a hyphen (the older one). A label that merely
    // begins with those characters is another host.
    if label != WEBSITE_ENDPOINT_LABEL
        && !label.starts_with(&format!("{WEBSITE_ENDPOINT_LABEL}-"))
    {
        return None;
    }
    super::names_a_bucket(bucket).then(|| bucket.to_string())
}

/// The status a redirect takes when the rule that calls for it names no `HttpRedirectCode`:
/// the permanent redirect S3 answers a routing rule and an object redirect with.
const DEFAULT_REDIRECT_STATUS: u16 = 301;

/// The two statuses this surface can answer an error document for, which are also the two a
/// `Condition$HttpErrorCodeReturnedEquals` can match here: the key is not there, and the
/// public may not read it.
const NOT_FOUND_STATUS: u16 = 404;
/// See [`NOT_FOUND_STATUS`].
const FORBIDDEN_STATUS: u16 = 403;

/// The media type an object with no `Content-Type` of its own is served as — the type S3
/// stores an object under when the write named none.
const DEFAULT_MEDIA_TYPE: &str = "binary/octet-stream";

/// The scheme a redirect this endpoint builds uses when the rule names no `Protocol`: "the
/// default is the protocol that is used in the original request"
/// (`RedirectAllRequestsTo$Protocol`), and a website endpoint is reached over plain HTTP.
const DEFAULT_PROTOCOL: &str = "http";

/// The operation that serves the website endpoint, over the SAME shared engine every other
/// operation of this service reads.
pub struct WebsiteEndpoint<Eng> {
    engine: Arc<Eng>,
}

/// What one website request resolved to, before the bucket's routing rules are consulted: the
/// answer the endpoint would give if no rule redirected it.
enum Resolved {
    /// The object's bytes, with the media type it is stored under.
    Served { body: Vec<u8>, media_type: String },
    /// The object carries `x-amz-website-redirect-location`, so the answer is a redirect to it.
    Redirected { location: String },
    /// The request earns an error: the status, the code and the sentence it carries.
    Failed {
        status: u16,
        code: &'static str,
        message: &'static str,
    },
}

impl<Eng: Send + Sync + 'static> WebsiteEndpoint<Eng> {
    /// Build the operation from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// Serve one website-endpoint request.
    fn serve(&self, ctx: &OperationContext<'_>) -> RtResponse {
        let parts = ctx.parts();
        let Some(name) = addressed_bucket(parts) else {
            // Nothing routes here without a website host, so this cannot be reached; a bare
            // 404 is the honest answer to an address that names no bucket at all.
            return answered(
                NOT_FOUND_STATUS,
                error_document(NOT_FOUND_STATUS, crate::state::NO_SUCH_BUCKET_CODE, ""),
            );
        };
        let method = parts.method.as_str().to_ascii_uppercase();
        if method != "GET" && method != "HEAD" {
            return answered(
                crate::state::METHOD_NOT_ALLOWED_STATUS,
                error_document(
                    crate::state::METHOD_NOT_ALLOWED_STATUS,
                    crate::state::METHOD_NOT_ALLOWED_CODE,
                    crate::state::METHOD_NOT_ALLOWED_MESSAGE,
                ),
            );
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return answered(
                NOT_FOUND_STATUS,
                error_document(NOT_FOUND_STATUS, crate::state::NO_SUCH_BUCKET_CODE, ""),
            );
        };
        // The key the request addressed, as the request spelled it: the path with its leading
        // separator off, decoded the way this service decodes a path label.
        let requested = super::sigv4::percent_decoded(parts.uri.path().trim_start_matches('/'));
        let host = super::request_host(parts).unwrap_or_default().to_string();

        let store = state.lock();
        let Some(bucket) = store.bucket(&name) else {
            return answered(
                NOT_FOUND_STATUS,
                error_document(
                    NOT_FOUND_STATUS,
                    crate::state::NO_SUCH_BUCKET_CODE,
                    crate::state::NO_SUCH_BUCKET_MESSAGE,
                ),
            );
        };
        let Some(website) = bucket.website().cloned() else {
            // A bucket with no website configuration serves no website: the endpoint answers
            // the code S3 publishes for a bucket that was never configured.
            return answered(
                NOT_FOUND_STATUS,
                error_document(
                    NOT_FOUND_STATUS,
                    crate::state::NO_SUCH_WEBSITE_CONFIGURATION_CODE,
                    crate::state::NO_SUCH_WEBSITE_CONFIGURATION_MESSAGE,
                ),
            );
        };
        // The one configuration that serves nothing: every request is redirected to the host
        // it names, keeping the key it addressed.
        if let Some(redirect) = &website.redirect_all_requests_to {
            let protocol = redirect.protocol.as_deref().unwrap_or(DEFAULT_PROTOCOL);
            return redirected(
                DEFAULT_REDIRECT_STATUS,
                &format!("{protocol}://{}/{requested}", redirect.host_name),
            );
        }
        // A rule conditioned on the KEY alone redirects the request whatever the object plane
        // would have answered, which is what makes a prefix rule a redirect of the ADDRESS.
        if let Some(redirect) = routed(&website, &requested, None, &host) {
            return redirect;
        }
        // The key that is read: a request addressing a directory — the root, or a path ending
        // in the separator — reads the index document of that directory.
        let suffix = website.index_document_suffix.clone().unwrap_or_default();
        let key = if requested.is_empty() || requested.ends_with('/') {
            format!("{requested}{suffix}")
        } else {
            requested.clone()
        };
        let resolved = resolve(&store, bucket, &name, &key);
        match resolved {
            Resolved::Served { body, media_type } => served(&body, &media_type, method == "HEAD"),
            Resolved::Redirected { location } => redirected(DEFAULT_REDIRECT_STATUS, &location),
            Resolved::Failed {
                status,
                code,
                message,
            } => {
                // A rule conditioned on the error this request earned redirects it instead of
                // answering it, which is what "if the error code equals this value, then the
                // specified redirect is applied" says.
                if let Some(redirect) = routed(&website, &requested, Some(status), &host) {
                    return redirect;
                }
                // Else the configured error document is the body of the answer, served with
                // the status the request earned — and nothing but the code's own sentence
                // where the bucket names no error document, or holds no public object at it.
                let document = website
                    .error_document_key
                    .as_deref()
                    .and_then(|error_key| match resolve(&store, bucket, &name, error_key) {
                        Resolved::Served { body, media_type } => Some((body, media_type)),
                        _ => None,
                    });
                match document {
                    Some((body, media_type)) => {
                        let mut response = served(&body, &media_type, method == "HEAD");
                        set_status(&mut response, status);
                        response
                    }
                    None => answered(status, error_document(status, code, message)),
                }
            }
        }
    }
}

/// What the object plane answers for `key` in the bucket named `name`, as the website endpoint
/// reads it.
///
/// Every website request is ANONYMOUS, so what may be read is what the bucket grants to
/// everyone ([`crate::state::Bucket::anonymous_object_read_refusal`], the one rule this service
/// answers an anonymous read by): content it does not make public is the `403` that rule
/// states, and a key the public may read that is not there is a `404`.
fn resolve(
    store: &crate::state::ComAmazonawsS3Amazons3State,
    bucket: &crate::state::Bucket,
    name: &str,
    key: &str,
) -> Resolved {
    let object = bucket.object(key);
    if bucket
        .anonymous_object_read_refusal(
            name,
            key,
            object,
            store.owner_account_public_access_block(),
        )
        .is_some()
    {
        return Resolved::Failed {
            status: FORBIDDEN_STATUS,
            code: crate::state::ACCESS_DENIED_CODE,
            message: crate::state::ACCESS_DENIED_MESSAGE,
        };
    }
    let Some(object) = object else {
        return Resolved::Failed {
            status: NOT_FOUND_STATUS,
            code: crate::state::NO_SUCH_KEY_CODE,
            message: crate::state::NO_SUCH_KEY_MESSAGE,
        };
    };
    if let Some(location) = object.website_redirect_location.as_deref() {
        return Resolved::Redirected {
            location: location.to_string(),
        };
    }
    Resolved::Served {
        body: object.body.clone(),
        media_type: object
            .content_type
            .clone()
            .unwrap_or_else(|| DEFAULT_MEDIA_TYPE.to_string()),
    }
}

/// The redirect the bucket's routing rules call for, or `None` when none of them applies.
///
/// `error` is the status the request earned, when it earned one: a rule naming
/// `HttpErrorCodeReturnedEquals` applies only to a request that failed with THAT status, and a
/// rule naming only `KeyPrefixEquals` applies to the address whatever it would have answered.
/// Both members of a `Condition` have to hold where a rule names both. The FIRST rule that
/// applies is the one applied, which is the order the configuration names them in.
fn routed(
    website: &crate::state::BucketWebsite,
    requested: &str,
    error: Option<u16>,
    host: &str,
) -> Option<RtResponse> {
    for rule in &website.routing_rules {
        let condition = rule.condition.clone().unwrap_or_default();
        let prefix = condition.key_prefix_equals.clone().unwrap_or_default();
        if !requested.starts_with(&prefix) {
            continue;
        }
        match condition.http_error_code_returned_equals.as_deref() {
            // A rule conditioned on an error applies only where the request earned exactly
            // that status.
            Some(stated) => {
                if error.is_none_or(|status| stated.trim() != status.to_string()) {
                    continue;
                }
            }
            // A rule conditioned on the key alone is applied BEFORE the object plane answers,
            // so it is not applied again to the error that answer carried.
            None => {
                if error.is_some() {
                    continue;
                }
            }
        }
        let redirect = &rule.redirect;
        let key = match (
            redirect.replace_key_with.as_deref(),
            redirect.replace_key_prefix_with.as_deref(),
        ) {
            (Some(replacement), _) => replacement.to_string(),
            (None, Some(replacement)) => {
                format!("{replacement}{}", &requested[prefix.len()..])
            }
            (None, None) => requested.to_string(),
        };
        let protocol = redirect.protocol.as_deref().unwrap_or(DEFAULT_PROTOCOL);
        let host = redirect.host_name.as_deref().unwrap_or(host);
        let status = redirect
            .http_redirect_code
            .as_deref()
            .and_then(|code| code.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_REDIRECT_STATUS);
        return Some(redirected(status, &format!("{protocol}://{host}/{key}")));
    }
    None
}

/// The HTML page a website endpoint answers an error with — "a website endpoint returns an
/// HTML document" — carrying the code and the sentence a client reads the failure from.
fn error_document(status: u16, code: &str, message: &str) -> Vec<u8> {
    let escaped = |value: &str| ::quick_xml::escape::escape(value).into_owned();
    format!(
        "<html><head><title>{status} {}</title></head>\
         <body><h1>{status} {}</h1><ul><li>Code: {}</li><li>Message: {}</li></ul></body></html>",
        escaped(code),
        escaped(code),
        escaped(code),
        escaped(message)
    )
    .into_bytes()
}

/// One answer of this endpoint: the status, the body, and the HTML media type an error page
/// carries.
fn answered(status: u16, body: Vec<u8>) -> RtResponse {
    let mut response = response_with(status, SdkBody::from(body.clone()));
    insert_header(&mut response, "content-type", "text/html");
    insert_header(&mut response, "content-length", &body.len().to_string());
    response
}

/// The object `body` served with the media type it is stored under.
///
/// `head` sends NO body while still reporting the length the same request's `GET` would carry:
/// "the HEAD method is identical to GET except that the server MUST NOT send a message body"
/// (RFC 9110), and the entity length is what a client sizing the object reads.
fn served(body: &[u8], media_type: &str, head: bool) -> RtResponse {
    let sent = if head { SdkBody::empty() } else { SdkBody::from(body.to_vec()) };
    let mut response = response_with(200, sent);
    insert_header(&mut response, "content-type", media_type);
    insert_header(&mut response, "content-length", &body.len().to_string());
    response
}

/// A redirect to `location` with the status the rule that called for it names.
fn redirected(status: u16, location: &str) -> RtResponse {
    let mut response = response_with(status, SdkBody::empty());
    insert_header(&mut response, "location", location);
    insert_header(&mut response, "content-length", "0");
    response
}

/// One response of this endpoint, with the correlation pair every answer of this service
/// carries ([`crate::state::request_ids`]).
fn response_with(status: u16, body: SdkBody) -> RtResponse {
    let status = StatusCode::try_from(status)
        .unwrap_or_else(|_| StatusCode::try_from(500).expect("500 is a status"));
    let mut response = RtResponse::new(status, body);
    let correlation = crate::state::request_ids::answering()
        .unwrap_or_else(crate::state::request_ids::RequestCorrelation::mint);
    insert_header(&mut response, "x-amz-request-id", &correlation.request_id);
    insert_header(&mut response, "x-amz-id-2", &correlation.extended_request_id);
    response
}

/// Put `status` on a response already built, which is how the ERROR document's body is served
/// with the status the request earned rather than with the `200` the object read would have.
fn set_status(response: &mut RtResponse, status: u16) {
    if let Ok(status) = StatusCode::try_from(status) {
        *response.status_mut() = status;
    }
}

/// Set one response header, dropping a value the wire cannot carry rather than turning a
/// served request into a transport failure.
fn insert_header(response: &mut RtResponse, name: &'static str, value: &str) {
    if let Err(refused) = response.headers_mut().try_insert(name, value.to_string()) {
        ::tracing::warn!(
            header = name,
            error = %refused,
            "this header cannot be sent on the wire and is left off the website answer"
        );
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for WebsiteEndpoint<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(ctx) })
    }
}
