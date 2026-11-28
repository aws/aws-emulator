// SPDX-License-Identifier: CC0-1.0

//! The URLs this service hands out for a caller to FOLLOW later, and the one place it
//! signs them.
//!
//! `StartContentUpload` answers where to write an asset, `CreateContent` answers where to
//! read one back, and `CreateMessageTemplateAttachment` answers where an attachment's file
//! is downloaded from. None of those paths binds a modelled operation, so each is served
//! by this service's own data plane (`crate::service`) rather than by the generated
//! routing table.
//!
//! Every one of them is SIGNED here, through the emulator's own URL signer: the signature
//! names the method and path it was minted for and the instant it stops verifying, and the
//! emulator verifies it before the plane runs, so following such a URL is admitted by a
//! credential this emulator issued rather than by possession of an identifier. The record
//! the URL names, its own expiry and the headers a start call published stay as the
//! further constraints they were — the signature says the URL came from here, the record
//! says what the request may touch.
//!
//! AWS answers these three members with an Amazon S3 presigned URL, which is a
//! query-signed URL with an expiry: the same shape, signed by whoever serves it.

use std::time::{Duration, SystemTime};

use super::*;

/// One signed URL this service hands out: the text for the output member that carries it,
/// the instant it stops verifying for the member that reports that, and the headers a
/// caller must send with it.
#[derive(Debug, Clone)]
pub struct SignedDataPlaneUrl {
    /// The URL text, query-signed and absolute.
    pub url: String,
    /// When the signature stops verifying, for the `urlExpiry` member beside it.
    pub expires_at: SystemTime,
    /// The headers the caller must send for the signature to verify, over and above the
    /// ones the operation itself publishes: for a request that sends a body to a service
    /// outside the Amazon S3 family, the unsigned-payload marker.
    pub required_headers: Vec<(String, String)>,
}

/// Sign `path` for `method`, valid for `time_to_live`, as the URL a caller follows.
///
/// THE ONE PLACE this service signs a URL, so every plane's locator is minted the same
/// way: the service's own signing name ([`ARN_NAMESPACE`], which is this endpoint's SigV4
/// signing name as well), the Region the request being served was signed for, and the path
/// exactly as the plane parses it back.
///
/// `None` when the signer refuses the request — an expiry outside the range it accepts, or
/// a path it cannot sign — which leaves the caller a modelled refusal rather than a URL
/// that could not be verified.
pub fn sign_data_plane_url(
    signer: &::smithy_server_runtime::UrlSigner,
    method: ::http::Method,
    region: &str,
    path: &str,
    time_to_live: Duration,
) -> Option<SignedDataPlaneUrl> {
    let request = ::smithy_server_runtime::SignedUrlRequest::new(method, ARN_NAMESPACE, path)
        .region(region);
    let signed = signer.sign(request, time_to_live).ok()?;
    Some(SignedDataPlaneUrl {
        url: signed.as_str().to_string(),
        expires_at: signed.expires_at(),
        required_headers: signed
            .required_headers()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    })
}
