// SPDX-License-Identifier: CC0-1.0

//! Serves an export's zip archive at the pre-signed URL `DescribeExport` hands out for it.
//!
//! `DescribeExport` reports `downloadUrl`, "a pre-signed S3 URL that points to the bot or
//! bot locale archive", and the location this emulator keeps those archives at is this
//! emulator itself: the read signs a URL addressing the path
//! [`crate::state::export_archive_path`] gives the job's archive, so the URL leads back
//! here and verifies, and this route answers a `GET` on that address with the bytes the
//! job assembled when it was recorded.
//!
//! The model binds no operation to those paths — an archive is not an API call, it is the
//! artifact the API points at — so the route is claimed through
//! [`smithy_server_runtime::Service::override_operation`], and the path space it lives in
//! through [`smithy_server_runtime::Service::can_handle`], rather than through the
//! generated routing table. It is claimed only for a PRE-SIGNED request
//! ([`is_presigned`]), whose signature and expiry window the framework verifies before the
//! route runs; WHICH archive the address reaches is then decided by the stored jobs, so an
//! address naming a job this store no longer holds, or one that has not reached the status
//! in which the export is ready to download, reaches no archive.
//!
//! It only READS: a download neither records nor settles anything beyond the status
//! transitions the clock already owed the stored jobs.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

/// The method an archive is fetched on, which is the method its address was signed for.
const DOWNLOAD_METHOD: &str = "GET";

/// The media type the one answer of this route that is not an archive is served as.
const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// What this route answers an address that names no archive this emulator holds: a
/// pre-signed download URL is fetched by a plain HTTP client rather than by an AWS SDK, so
/// the answer is the status itself with a one-line reason, and no API error envelope.
const NO_ARCHIVE: &str = "no export archive at this location\n";

/// Serves the download of one export's archive.
///
/// Owns the shared engine, as an operation does, because the archive it serves is the one
/// the stored job carries: nothing is held beside the export jobs, so a download and a read
/// of the same export cannot disagree about what was exported.
pub struct ExportArchiveRoute<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ExportArchiveRoute<Eng> {
    /// Build the route from the shared engine, as this service's operations are built.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The export whose archive `req` asks for, or `None` when the request is not a
    /// download this route serves.
    ///
    /// Two things make it one: the path, read back by the same rule the address is built
    /// from ([`crate::state::export_archive_id`]), and the SigV4 query authentication of a
    /// pre-signed request ([`is_presigned`]). Whether a job is HELD under that id is not
    /// decided here but where the jobs are (see [`Self::stored_archive`]), so an address
    /// for an export the store has dropped is answered exactly as one naming an export
    /// that never existed.
    pub fn claims(&self, req: &RtRequest) -> Option<String> {
        if req.method() != DOWNLOAD_METHOD || !is_presigned(req.uri()) {
            return None;
        }
        crate::state::export_archive_id(service_common::request_path(req.uri()))
    }

    /// Whether `parts` addresses the archive path space at all — the claim
    /// [`smithy_server_runtime::Service::can_handle`] makes, so a URL this service handed
    /// out is not answered as a request naming no operation.
    ///
    /// The published segment is what makes a request this route's, read by the same rule
    /// [`Self::claims`] reads the whole address by, so the two cannot disagree about which
    /// requests belong to this plane.
    pub fn claims_path(&self, parts: &http::request::Parts) -> bool {
        crate::state::addresses_export_archive(parts.uri.path())
    }

    /// The archive the export `export_id` names carries, or `None` when this store serves
    /// no archive at that address: it holds no job under that id — a job it never recorded,
    /// or one it no longer keeps — or the job has not reached the status in which the
    /// export is ready to download.
    ///
    /// The status is read SETTLED, so a job whose completion the clock has already made due
    /// is downloadable here exactly when `DescribeExport` reports it as such.
    fn stored_archive(&self, export_id: &str) -> Option<::bytes::Bytes> {
        let state = crate::state::service_state(&*self.engine)?;
        let mut store = state.lock();
        let export = store.settled_export(export_id)?;
        // The bytes are refcounted, so what is taken out of the critical section is a
        // handle on the job's own archive rather than a copy of it.
        export.completed().then(|| export.archive.clone())
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ExportArchiveRoute<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The context is not consulted, because a PRE-SIGNED fetch carries no caller
        // identity for it to hold: what the framework verified before this route ran is a
        // signature over this method, this path and this query. WHICH archive the address
        // reaches is decided by the stored jobs alone.
        let answer = match self.claims(req) {
            Some(export_id) => match self.stored_archive(&export_id) {
                Some(archive) => {
                    archive_response(200, crate::state::EXPORT_ARCHIVE_MEDIA_TYPE, archive)
                }
                None => archive_response(
                    404,
                    PLAIN_MEDIA_TYPE,
                    ::bytes::Bytes::from_static(NO_ARCHIVE.as_bytes()),
                ),
            },
            // Unreachable through an `override_operation` that hands this route only the
            // requests it claims; answered as the framework answers a request that named
            // no operation rather than guessed at.
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

/// One download response: the bytes, their media type and their length.
fn archive_response(status: u16, media_type: &str, body: ::bytes::Bytes) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(download_status(status), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", media_type.to_string());
    response
        .headers_mut()
        .insert("content-length", length.to_string());
    response
}

/// A status this route answers with. Every call site passes a constant, so the conversion
/// cannot fail.
fn download_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("an export archive download answers constant statuses")
}

/// Whether `uri` carries the SigV4 query authentication of a pre-signed request: the
/// credential scope and the signature [`smithy_server_runtime::UrlSigner`] puts on a URL it
/// signs.
///
/// Presence is all this reads, and all it has to: the framework's own request
/// authentication is what VERIFIES such a request before any route runs — the signature
/// over the method, the path and the query, and the `X-Amz-Expires` window the signer
/// wrote, a URL past which is refused. What this test adds is that nothing which is not a
/// pre-signed request is served from these paths at all, so an archive's only address is a
/// URL this service signed for it.
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
