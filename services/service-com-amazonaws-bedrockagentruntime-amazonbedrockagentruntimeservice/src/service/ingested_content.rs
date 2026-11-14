// SPDX-License-Identifier: CC0-1.0

//! Serves the content of an INGESTED document at the pre-signed URL `GetDocumentContent`
//! hands out for it.
//!
//! `GetDocumentContent` "returns a pre-signed URL for secure document access", and for a
//! document whose bytes travelled in the ingestion request the location this emulator
//! manages them at is this emulator itself: the operation signs a URL addressing the path
//! [`crate::state::document_content::ContentAddress::path`] gives the document, carrying
//! the authenticator this service's store minted for it, so the URL leads back here and
//! verifies, and this route answers a `GET` on that address with the bytes the build-time
//! plane holds for that document.
//!
//! The model binds no operation to those paths — the content of a document is not an API
//! call, it is the artifact the API points at — so the route is claimed through
//! [`smithy_server_runtime::Service::override_operation`] rather than through the generated
//! routing table, and `can_handle` claims the path space beside this service's modelled
//! one. It is claimed only for a PRE-SIGNED request ([`is_presigned`]), whose signature and
//! expiry window the framework verifies before the route runs; WHICH document an address may
//! reach is then decided by the store that minted it
//! (`crate::state::document_content::ingested_content`), against the authenticator the
//! address carries. So a document's content has exactly one address: the one a read that was
//! authorized for it handed out.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use crate::state::document_content as content;

/// The method a document's content is fetched on.
const DOWNLOAD_METHOD: &str = "GET";

/// The media type the one answer of this route that is not a document is served as.
const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// What this route answers an address that names no content this emulator holds: a
/// pre-signed URL is fetched by a plain HTTP client rather than by an AWS SDK, so the
/// answer is the status itself with a one-line reason, and no API error envelope.
const NO_CONTENT: &str = "no ingested document content at this location\n";

/// Serves the content of one ingested document.
///
/// Owns the shared engine, as an operation does, because the bytes it serves are the ones
/// the build-time plane holds for the document: nothing is copied aside, so a content read
/// and a fetch of the address it handed out cannot disagree about the document's bytes.
pub struct IngestedContentRoute<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> IngestedContentRoute<Eng> {
    /// Build the route from the shared engine, as the service's operations are built.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The document `req` asks for and the authenticator its address carries, or `None`
    /// when the request is not a fetch this route serves.
    ///
    /// Two things make it one: the path, read the way the signer wrote it — its segments
    /// percent-decoded and its empty ones kept, so a `documentId` that had to be encoded in
    /// the URL, or that holds the separator a path is cut on, names the same document here
    /// as it does in the store — and the SigV4 query authentication of a PRE-SIGNED request
    /// ([`is_presigned`]). Whether the address is one this emulator MINTED is decided where
    /// the store is: the authenticator read off the query travels with the address for the
    /// store to check, so a request carrying none is answered exactly as one naming content
    /// that does not exist.
    pub fn claims(&self, req: &RtRequest) -> Option<(content::ContentAddress, String)> {
        if req.method() != DOWNLOAD_METHOD || !is_presigned(req.uri()) {
            return None;
        }
        let address = content::parse_content_address(&decoded_segments(req.uri()))?;
        let token = query_field(req.uri(), content::DOCUMENT_TOKEN_FIELD).unwrap_or_default();
        Some((address, token))
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for IngestedContentRoute<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The context is not consulted, because a PRE-SIGNED fetch carries no caller
        // identity for it to hold: what the framework verified before this route ran is a
        // signature over this method, this path and this query, not who sent it. WHICH
        // document an address may reach is therefore decided by the store that minted the
        // address, against the authenticator the address carries.
        let answer = match self.claims(req) {
            Some((address, token)) => {
                match content::ingested_content(&*self.engine, &address, &token) {
                    Some((media_type, body)) => content_response(200, &media_type, body),
                    // The address is not one this store minted for content it holds: it
                    // carries no authenticator of this store's, or one for another document,
                    // or it names a document the plane no longer holds, or one whose bytes
                    // live in the object store instead. Each is answered the same way: there
                    // is nothing at this location.
                    None => {
                        content_response(404, PLAIN_MEDIA_TYPE, NO_CONTENT.as_bytes().to_vec())
                    }
                }
            }
            // Unreachable through an `override_operation` that hands this route only the
            // requests it claims; answered as the framework answers a request that named no
            // operation rather than guessed at.
            None => svc.definition().routing_error_response(req),
        };
        Box::pin(async move { answer })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` keeps the framework's own decision for this service: the URL this route
        // serves is SigV4 query-signed, and nothing here may weaken that.
        None
    }
}

/// One content response: the bytes, their media type and their length.
///
/// The media type is the one the ingestion DECLARED for the document's bytes, which the
/// model constrains only loosely (`ByteContentDoc$mimeType` is a `String` with a `@pattern`
/// that admits characters no HTTP header value can carry), so it is offered to the header
/// map through the FALLIBLE setter: a declared type no header can carry is served under the
/// media type bytes of no stated type take, rather than aborting the fetch. The modelled
/// `mimeType` a content read reports is unaffected — that is the document's own declared
/// type, and it travels in a JSON body.
fn content_response(status: u16, media_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(content_status(status), SdkBody::from(body));
    if response
        .headers_mut()
        .try_insert("content-type", media_type.to_string())
        .is_err()
    {
        response
            .headers_mut()
            .insert("content-type", UNTYPED_MEDIA_TYPE);
    }
    response
        .headers_mut()
        .insert("content-length", length.to_string());
    response
}

/// The media type bytes of no stated type are served under, which is the one the object
/// store reports for an object written without a content type
/// (`service_com_amazonaws_s3_amazons3::state::DEFAULT_CONTENT_TYPE`).
const UNTYPED_MEDIA_TYPE: &str = ::service_com_amazonaws_s3_amazons3::state::DEFAULT_CONTENT_TYPE;

/// A status this route answers with. Every call site passes a constant, so the conversion
/// cannot fail.
fn content_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("a document content fetch answers constant statuses")
}

/// Whether `uri` carries the SigV4 query authentication of a pre-signed request: the
/// credential scope and the signature [`smithy_server_runtime::UrlSigner`] puts on a URL it
/// signs.
///
/// Presence is all this reads, and all it has to: the framework's own request
/// authentication is what VERIFIES such a request before any route runs — the signature
/// over the method, the path and the query, and the `X-Amz-Expires` window the signer wrote,
/// a URL past which is refused. What this test adds is that nothing which is not a
/// pre-signed request is served from these paths at all, so a document's content has no
/// address but a URL this service signed for it.
fn is_presigned(uri: &str) -> bool {
    const PRESIGNED_FIELDS: [&str; 2] = ["X-Amz-Credential", "X-Amz-Signature"];
    let query = uri.split_once('?').map(|(_, query)| query).unwrap_or("");
    PRESIGNED_FIELDS.iter().all(|field| {
        query.split('&').any(|pair| {
            pair.split_once('=')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case(field))
        })
    })
}

/// The path segments of `uri`, percent-decoded, with the EMPTY ones kept — the same view of
/// a path the signer built the URL from.
///
/// An empty segment is kept because a `DocumentId` may itself hold the separator a path is
/// cut on, and dropping empties would make two different ids one address.
fn decoded_segments(uri: &str) -> Vec<String> {
    uri.parse::<http::Uri>()
        .map(|parsed| {
            parsed
                .path()
                .strip_prefix('/')
                .unwrap_or_default()
                .split('/')
                .map(::service_common::percent_decode_path_label)
                .collect()
        })
        .unwrap_or_default()
}

/// The value of one query field of `uri`, percent-decoded, or `None` when the request
/// carries no such field.
fn query_field(uri: &str, field: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_, query)| query)?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == field).then(|| smithy_server_runtime::percent_decode(value))
    })
}
