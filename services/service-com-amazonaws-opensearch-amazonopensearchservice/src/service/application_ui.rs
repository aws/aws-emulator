// SPDX-License-Identifier: CC0-1.0

//! The ROUTE that answers at the endpoint an OpenSearch UI application reports.
//!
//! An application reports its `endpoint` for a caller to FOLLOW, and this emulator publishes
//! that address on its own endpoint ([`crate::state::application_endpoint`]). This module is
//! the other half of that address: the claim that says a fetch of it is this service's to
//! answer, and the answer itself — the stored application the path names
//! ([`crate::state::application_ui_document`]), and the statement that there is no such
//! application for a path naming one this emulator does not hold.
//!
//! The fetch names no modelled operation: an application's endpoint is followed by whatever
//! client, probe or test holds it, exactly as AWS serves its own interface host. So the claim
//! is on the PATH this service itself minted, and the answer is the plane's own document
//! rather than a modelled API envelope.
//!
//! Being answered is not being admitted. An application's interface is reached through a
//! SIGN-IN, so a fetch that presents no AWS credential at all is refused before the store is
//! read and is told nothing of any application ([`crate::state::application_ui_admits`], the
//! same shape as the authentication a domain's own plane performs before it reads anything of
//! the cluster). A fetch that DOES present one is named as an operation by
//! [`crate::service`]'s claim seam ([`claims_request`]), so the router verifies that
//! credential's signature before this plane runs — the verification every other surface of
//! this service is served behind.

use smithy_server_runtime::http;

/// Whether `parts` addresses the endpoint of an application this service publishes: a `GET`
/// (or the `HEAD` that asks only what is there) of exactly the path
/// [`crate::state::application_ui_path`] mints.
pub fn claimed(parts: &http::request::Parts) -> bool {
    (parts.method == http::Method::GET || parts.method == http::Method::HEAD)
        && crate::state::application_ui_route(&parts.uri.to_string()).is_some()
}

/// The same claim over the whole request, for the seam that names this plane as an OPERATION
/// so the router verifies the credential a fetch presents before anything is served.
pub fn claims_request(req: &aws_smithy_runtime_api::http::Request) -> bool {
    (req.method() == "GET" || req.method() == "HEAD")
        && crate::state::application_ui_route(req.uri()).is_some()
}

/// The document a fetch of `req` answers with, or `None` when `req` addresses no application
/// endpoint of this service — which leaves the request to the answer it would otherwise get.
///
/// A pure READ of the store, apart from the clock-owed application transitions every reader of
/// these records materialises
/// ([`crate::state::ComAmazonawsOpensearchAmazonopensearchserviceState::settle_applications`]),
/// so following the endpoint changes nothing a later caller sees: the answer states the status
/// the application already reports.
///
/// What it states depends on whether the fetch is ADMITTED: one presenting no AWS credential
/// is refused and told nothing of any application, before the store is read at all.
pub fn answer<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<aws_smithy_runtime_api::http::Response> {
    let body_wanted = match req.method() {
        "GET" => true,
        "HEAD" => false,
        _ => return None,
    };
    let id = crate::state::application_ui_route(req.uri())?;
    // AUTHENTICATION first, before anything of any application is read: an application is
    // reached through a sign-in, so a fetch presenting no AWS credential reads nothing of one
    // — not even whether the address names one. Read the way the domain plane reads it, so one
    // request is read the same way wherever it lands; a credential that IS presented was
    // verified by the router before this plane ran ([`claims_request`]).
    if !crate::state::application_ui_admits(super::domain_rest::carries_sigv4_credential(req)) {
        return served(
            crate::state::APPLICATION_UI_UNAUTHENTICATED_STATUS,
            &crate::state::application_ui_unauthenticated_document(),
            body_wanted,
            Some(crate::state::APPLICATION_UI_UNAUTHENTICATED_CODE),
        );
    }
    let state = crate::state::service_state(engine)?;
    let found = {
        let mut store = state.lock();
        store.settle_applications(::std::time::SystemTime::now());
        store
            .applications
            .get(id)
            .map(crate::state::application_ui_document)
    };
    // The application this emulator holds, and the statement that there is none for every
    // other address of this shape: nothing is invented for an id that was never created or
    // whose deletion has completed.
    let status = if found.is_some() { 200u16 } else { 404 };
    let document = found.unwrap_or_else(crate::state::application_ui_not_found_document);
    served(status, &document, body_wanted, None)
}

/// One answer of this plane: `document` on `status`, with the body left out for the `HEAD`
/// that asks only what is there, and the error code a refusal travels under.
///
/// ONE renderer, so every answer this plane gives carries the same headers.
fn served(
    status: u16,
    document: &str,
    body_wanted: bool,
    code: Option<&'static str>,
) -> Option<aws_smithy_runtime_api::http::Response> {
    let body = if body_wanted {
        document.as_bytes().to_vec()
    } else {
        Vec::new()
    };
    let mut response = aws_smithy_runtime_api::http::Response::new(
        aws_smithy_runtime_api::http::StatusCode::try_from(status).ok()?,
        ::aws_smithy_types::body::SdkBody::from(body),
    );
    let headers = response.headers_mut();
    headers.insert("content-type", crate::state::APPLICATION_UI_MEDIA_TYPE);
    // The size of the document itself, which is what a `HEAD` of it reports with no body.
    headers.insert("content-length", document.len().to_string());
    // The document is served from this emulator's own origin, so the type it announces is the
    // only type it may be treated as.
    headers.insert("x-content-type-options", "nosniff");
    if let Some(code) = code {
        headers.insert("x-amzn-errortype", code);
    }
    Some(response)
}
