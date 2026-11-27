// SPDX-License-Identifier: CC0-1.0

//! The ROUTE that answers at a payment connector's published authorization address.
//!
//! A `QUICK_CREATE` payment connector reports `authorizationUrl` for an end user to open,
//! and this emulator publishes that address on its own endpoint
//! ([`crate::state::payment_connector_authorization_url`]). This module is the other half
//! of that address: the claim that says a fetch of it is this service's to answer, and the
//! answer itself — the page of the stored connector the path names
//! ([`crate::state::payment_connector_authorization_page`]), and a not-found page for a
//! path naming a connector this emulator does not hold.
//!
//! The fetch carries no credential and names no modelled operation: an authorization URL
//! is opened by whatever browser, probe or test holds it, exactly as AWS orchestrates its
//! consent on a host of its own. So the claim is on the PATH this service itself minted,
//! and the answer is a web page rather than a modelled API envelope.
//!
//! The consent itself is given to the payment provider by a person. This emulator does not
//! stand in for that provider, so the page REPORTS what the connector is waiting for and
//! grants nothing: the connector keeps reporting `PENDING_AUTHENTICATION` and no
//! service-managed credential is reported for a provisioning that never ran.

use smithy_server_runtime::http;

/// Whether `parts` addresses the authorization of a payment connector this service
/// publishes, for a request that resolved no routing key of its own: a `GET` (or the `HEAD`
/// that asks only what is there) of exactly the path
/// [`crate::state::payment_connector_authorization_path`] mints.
pub fn claimed(parts: &http::request::Parts) -> bool {
    (parts.method == http::Method::GET || parts.method == http::Method::HEAD)
        && crate::state::payment_connector_authorization_route(&parts.uri.to_string()).is_some()
}

/// The authorization page a fetch of `req` renders, or `None` when `req` addresses no
/// payment connector of this service — which leaves the request to the answer it would
/// otherwise get.
///
/// A READ of the store: opening the address changes nothing a later caller sees, and the
/// page states the status the connector already reports.
pub fn answer<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<aws_smithy_runtime_api::http::Response> {
    let body_wanted = match req.method() {
        "GET" => true,
        "HEAD" => false,
        _ => return None,
    };
    let (payment_manager_id, payment_connector_id) =
        crate::state::payment_connector_authorization_route(req.uri())?;
    let state = crate::state::service_state(engine)?;
    let now = ::std::time::SystemTime::now();
    let page = {
        let store = state.lock();
        // A READ, and nothing but a read: this address is fetched by whatever browser holds
        // it, with no credential of any kind, so nothing reached through it writes to the
        // store. The removals the clock owes an accepted deletion — the manager's and the
        // connector's — are therefore read as the owed instants they are rather than
        // reclaimed here, so the page reports exactly what an authenticated read of the
        // connector reports at this instant, and the records go when the next operation of
        // the payments plane materialises them.
        store
            .payment_managers
            .get(payment_manager_id)
            .filter(|manager| !manager.removal_due(now))
            .and_then(|manager| manager.payment_connectors.get(payment_connector_id))
            .filter(|record| !record.removal_due(now))
            .map(|record| crate::state::payment_connector_authorization_page(record, now))
    };
    // The page of a connector this emulator holds, and a page saying there is none for
    // every other address of this shape: nothing is invented for a connector that was
    // never created, or one created on another manager.
    let status = if page.is_some() {
        super::served_page::PAGE_FOUND
    } else {
        super::served_page::PAGE_NOT_FOUND
    };
    let document = page.unwrap_or_else(crate::state::payment_connector_authorization_not_found_page);
    super::served_page::served_page(
        status,
        crate::state::PAYMENT_CONNECTOR_AUTHORIZATION_MEDIA_TYPE,
        &document,
        body_wanted,
    )
}
