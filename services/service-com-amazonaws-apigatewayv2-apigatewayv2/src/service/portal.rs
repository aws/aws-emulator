// SPDX-License-Identifier: CC0-1.0

//! The PORTAL data plane: the developer-portal website a published portal serves on the
//! domain this service generates for it.
//!
//! A portal is reached at the host its own `EndpointConfigurationResponse` reports — "This
//! domain name is generated and managed by API Gateway"
//! (`EndpointConfigurationResponse$PortalDefaultDomainName`), or the `DomainName` the
//! account brought with it — and `PublishPortal` is what puts content on that host:
//! "Publishes a portal". The request a consumer then makes is an ORDINARY HTTP request to
//! a website, which is no modelled operation of this service at all, so the emulator's
//! front door could resolve no target service for it and answered `404 UnknownService` —
//! for the domain this very service told the caller to use. A caller could therefore not
//! observe the thing publishing a portal exists to do, and — the security-relevant half —
//! could not tell whether `DisablePortal` actually took the site offline: an emulator that
//! only flips `PublishStatus` while nothing is served, or while something is served
//! regardless, lets a revocation silently pass.
//!
//! This module is therefore the third data-plane half of
//! [`crate::service::EmulatedService`] (beside [`super::invoke`]'s and
//! [`super::websocket`]'s): the claim ([`claims_parts`], read by `Service::can_handle`) and
//! the serving ([`serve`], reached from `Service::override_routing_error`, which the router
//! reaches BEFORE authentication — a consumer of a developer portal holds no AWS
//! credential). It is a READ of this service's own records: a visit changes nothing, and
//! every status and byte it answers is computed from them.
//!
//! # What it serves, and what decides it
//!
//! 1. the request names a `Portal` this service holds — by the host, which is that portal's
//!    own generated domain or the custom domain it carries, or, for a client that cannot
//!    resolve either, by the portal's endpoint behind the emulator's reserved path marker
//!    ([`resolve`]);
//! 2. the portal's domain serves the build of its LAST SETTLED PUBLISH
//!    ([`crate::state::Portal::live_site`]): `PublishPortal` builds the content the portal
//!    held when it was called and that build goes live when the publish settles, so a
//!    portal nothing has published yet serves nothing, a write to the portal's content
//!    changes nothing that is already live, and a `DisablePortal` takes the live build
//!    down from the instant it is ACCEPTED — a revocation of public access does not wait
//!    for its `PublishStatus` to settle, so the site cannot be served to a caller who has
//!    already revoked it. A domain serving nothing is `403` `{"message":"Forbidden"}` — the
//!    answer this service's other data planes give for an endpoint that serves nothing;
//! 3. the live build is rendered as the portal's page ([`site_page`]): its `PortalContent`
//!    — display name, description and the theme's own colours — over the portal products
//!    the build included, each with the product pages and product REST endpoint pages that
//!    product holds. Every value on the page is read from the store; nothing about a
//!    product, a page or an endpoint is invented;
//! 4. the preview `PreviewPortal` builds is served on the path `Preview$PreviewUrl`
//!    announces ([`crate::state::portal_preview_url`]), from the portal's CURRENT content —
//!    that is what a preview is — once that build is ready, and `403` until then;
//! 5. anything else under the domain is `404` `{"message":"Not Found"}`: the portal serves
//!    the host, but has no page there.
//!
//! What this does NOT do, stated so a served page is not read as more than it is: a
//! portal whose `Authorization` is a `CognitoConfig` is not gated on a user-pool session.
//! The page names the pool and app client the portal is configured with, because those are
//! stored facts, and serves the same public landing page it serves for the `None`
//! alternative; no sign-in is performed and no consumer identity is modelled, so nothing
//! here should be read as evidence that a portal's Cognito authorization keeps a visitor
//! out.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

use super::data_plane::{
    addresses_connections, forbidden, header_of, host_name, json_response, path_of, path_segments,
    split_first_segment, LOCAL_ENDPOINT_PREFIX, NOT_FOUND_MESSAGE, NOT_FOUND_STATUS,
    UNPLACEABLE_STATUS,
};
use crate::state::{ComAmazonawsApigatewayv2Apigatewayv2State, Portal};

/// The sentence a request this data plane cannot place at all carries, spelled as the
/// sibling HTTP data plane spells it ([`super::invoke`]).
const INTERNAL_MESSAGE: &str = "Internal Server Error";

/// The media type a portal page is served as: a website, so a document rather than this
/// service's JSON envelope.
const HTML_CONTENT_TYPE: &str = "text/html; charset=utf-8";

/// The one path a portal's own content is served at: the root of its domain.
const SITE_PATH_SEGMENTS: usize = 0;

/// The most product sections one page carries, which bounds the response ONE visit can ask
/// for: a page renders at most as many products as this account can hold
/// ([`crate::state::MAX_INCLUDED_PORTAL_PRODUCTS`], the same figure the writers cap a
/// portal's `IncludedPortalProductArns` at), and each product is rendered at most once
/// however many times the list names it.
const MAX_RENDERED_PRODUCTS: usize = crate::state::MAX_INCLUDED_PORTAL_PRODUCTS;

/// The most bytes of one portal page that are rendered. The per-product content is
/// caller-supplied and bounded only by the modelled lengths of each page's own members, so
/// this is the explicit cap on the WHOLE document: a section that would carry the page past
/// it, and every section after it, is dropped and the page says so ([`TRUNCATION_MARKER`]),
/// so a small request can never ask for an arbitrarily large response. An EMULATOR bound,
/// generous enough for a developer portal's real content.
const MAX_PAGE_BYTES: usize = 1 << 20;

/// What a page says in place of the sections [`MAX_PAGE_BYTES`] left out, so a truncated
/// document is distinguishable from a complete one by whoever reads it.
const TRUNCATION_MARKER: &str =
    "<p data-portal-truncated=\"true\">This page was truncated.</p>\n";

/// What the footer of a PREVIEW page says, in place of the publication instant a published
/// page states.
const PREVIEW_FOOTER: &str = "preview";

/// What a request addresses: the host it names and the path within that host.
#[derive(Debug, Clone, Copy)]
struct Addressed<'r> {
    /// `None` when the request carries no host at all, which names no portal and no
    /// service.
    host: Option<&'r str>,
    path: &'r str,
}

impl<'r> Addressed<'r> {
    /// The reading of the request parts `Service::can_handle` is handed.
    fn of_parts(parts: &'r http::request::Parts) -> Self {
        Self {
            host: header_of(&parts.headers, "host")
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str)),
            path: parts.uri.path(),
        }
    }

    /// The reading of the runtime-API request the serving seam is handed, which carries its
    /// URI as a string.
    fn of_request(req: &'r RtRequest) -> Self {
        Self {
            host: req
                .headers()
                .get("host")
                .or_else(|| super::data_plane::authority_of(req.uri())),
            path: path_of(req.uri()),
        }
    }
}

/// What a request addresses, as this service's records resolve it: the portal, and the path
/// WITHIN that portal's own site.
#[derive(Debug)]
struct Resolved<'s, 'r> {
    portal: &'s Portal,
    site_path: &'r str,
}

/// The portal `addressed` addresses, or `None` when this data plane serves it not at all.
///
/// TWO addressings resolve, because a client reaches this emulator at a different address
/// than it reaches AWS at:
///
/// * HOST-STYLE, the addressing the endpoint configuration documents: the host IS one of
///   the portal's own domains — the default domain API Gateway generated for it, or the
///   `DomainName` the account brought. Compared against the host each portal RECORDS, so
///   no second spelling of that convention is written here and a portal is claimed only on
///   a domain it reports;
/// * LOCAL-ENDPOINT style: the domain is carried in the PATH behind the
///   [`LOCAL_ENDPOINT_PREFIX`] marker — `/_aws/portal/{portal-id}/…`, or
///   `/_aws/{portal-domain}/…` with the host itself. A client on the machine this emulator
///   runs on resolves the domain above nowhere, so it reaches the emulator's own endpoint
///   and keeps what it cannot carry in the host in the path. The marker is reserved for the
///   emulator — it is a path no AWS endpoint serves — so it is read on ANY host and cannot
///   take a request away from the service that host names.
///
/// A BARE first path segment is deliberately NOT read as a portal id: on the emulator's own
/// endpoint that segment is the first segment of every other service's path space too (a
/// bucket, a resource name), so claiming it would make two services claim one request, which
/// the router answers `500 AmbiguousService` for both. A client that cannot resolve the
/// portal's domain reaches it through the marker above.
///
/// This service's own CONTROL-plane path space (`crate::ROUTE_PATH_SEGMENTS`, the single
/// segment `v2`) is never read as a portal's: a request whose first segment is one of those
/// is a control-plane call on whatever host it arrives at, and answering it from a data
/// plane would take `/v2/portals` away from the operations that serve it. The
/// `@connections` space belongs to ApiGatewayManagementApi on either host, for the same
/// reason the api data planes leave it alone.
fn resolve<'s, 'r>(
    store: &'s ComAmazonawsApigatewayv2Apigatewayv2State,
    addressed: &Addressed<'r>,
) -> Option<Resolved<'s, 'r>> {
    if addresses_connections(addressed.path) {
        return None;
    }
    if path_segments(addressed.path)
        .first()
        .is_some_and(|first| crate::ROUTE_PATH_SEGMENTS.contains(first))
    {
        return None;
    }
    // LOCAL-ENDPOINT style first, because its marker is unambiguous on any host.
    if let Some(resolved) = local_endpoint_path(store, addressed.path) {
        return Some(resolved);
    }
    if let Some(host) = addressed.host {
        if let Some(portal) = portal_of_host(store, host) {
            return Some(Resolved {
                portal,
                site_path: addressed.path,
            });
        }
    }
    None
}

/// The portal whose own domain `host` is, or `None` for a host no portal of this service
/// records.
///
/// Both domains a portal reports are its own: the default one it publishes — the address this
/// emulator serves that portal's site at, which is the `host:port` of the socket it was given
/// or this emulator's own endpoint with the portal's reserved path
/// ([`crate::state::announced_portal_site_base`]) — and the `DomainName` the account brought
/// ("You can provide a custom domain to override this value").
///
/// A host is compared case-insensitively (RFC 4343) and without its userinfo, BOTH as the
/// request wrote it (with the `:port`, which is what matches a published address) and with the
/// port removed (which is what matches a name, where the port is no part of it).
///
/// Only a portal the account still HOLDS claims its domains: a portal `DeletePortal` has
/// taken away serves nothing, so the host it recorded is a host this service answers for no
/// longer, whatever tombstone of it the store keeps for `GetPortal` to report.
fn portal_of_host<'s>(
    store: &'s ComAmazonawsApigatewayv2Apigatewayv2State,
    host: &str,
) -> Option<&'s Portal> {
    let authority = host.rsplit('@').next().unwrap_or(host).trim();
    let name = host_name(host);
    if authority.is_empty() || name.is_empty() {
        return None;
    }
    let addresses = |reported: &str| {
        reported.eq_ignore_ascii_case(authority) || reported.eq_ignore_ascii_case(name)
    };
    store.live_portals().find(|portal| {
        let endpoint = &portal.endpoint_configuration;
        addresses(&endpoint.portal_default_domain_name)
            || endpoint.domain_name.as_deref().is_some_and(addresses)
    })
}

/// The portal id and site path of a request that names the portal's domain INSIDE ITS PATH,
/// behind the [`LOCAL_ENDPOINT_PREFIX`] marker, or `None` for a path carrying no marker.
///
/// Both spellings of the endpoint behind the marker resolve — the endpoint LABEL followed
/// by the portal id (`/_aws/portal/{portal-id}/…`) and the portal's own domain
/// (`/_aws/{portal-domain}/…`) — because what a rewriter has to carry across is the host,
/// and those are the two ways to write it.
fn local_endpoint_path<'s, 'r>(
    store: &'s ComAmazonawsApigatewayv2Apigatewayv2State,
    path: &'r str,
) -> Option<Resolved<'s, 'r>> {
    let (marker, rest) = split_first_segment(path)?;
    if marker != LOCAL_ENDPOINT_PREFIX {
        return None;
    }
    let (endpoint, rest) = split_first_segment(rest)?;
    if let Some(portal) = portal_of_host(store, endpoint) {
        return Some(Resolved {
            portal,
            site_path: rest,
        });
    }
    // The endpoint LABEL spelling: the portal id is the segment after it.
    if endpoint != crate::state::PORTAL_ENDPOINT_LABEL {
        return None;
    }
    let (portal_id, rest) = split_first_segment(rest)?;
    store.live_portal(portal_id).map(|portal| Resolved {
        portal,
        site_path: rest,
    })
}

/// Whether THIS data plane serves the request these parts came from — the portal half of
/// `Service::can_handle`.
///
/// Narrowed to a portal this service HOLDS, read from the one shared store, so nothing is
/// claimed that this service cannot answer about and exactly one service claims a request.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    claims(engine, &Addressed::of_parts(parts))
}

/// [`claims_parts`] for the runtime-API request the serving seam is handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims(engine, &Addressed::of_request(req))
}

/// Whether this data plane serves `addressed`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), addressed).is_some())
}

/// Serve one request addressed to a portal's domain, from this service's own records.
///
/// Free-standing and reached with the ONE shared engine, the way the other two data planes'
/// `serve` is, so the claim above and this answer read the same store.
///
/// The store lock is held only while the request is DECIDED and the records the answer
/// needs are snapshotted ([`Snapshot::of`]); the document is built after the guard is
/// dropped, so one visitor's page is never assembled while every other caller of this
/// service — the control plane included — waits behind the lock.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let addressed = Addressed::of_request(req);
    let Some(state) = crate::state::service_state(engine) else {
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    };
    let now = SystemTime::now();
    let decided = {
        let store = state.lock();
        decide(&store, &addressed, now)
    };
    match decided {
        Decision::Page(snapshot) => html_response(page(&snapshot)),
        Decision::Refusal(refusal) => refusal,
    }
}

/// What one request to a portal's domain is answered with: the page to render, or the
/// refusal it earns.
enum Decision {
    Page(Snapshot),
    Refusal(RtResponse),
}

/// What this request is answered with: the page to render, or the refusal it earns.
///
/// Everything here reads the store; nothing writes it, and nothing it answers borrows from
/// it, so the caller can let the lock go before rendering.
fn decide(
    store: &ComAmazonawsApigatewayv2Apigatewayv2State,
    addressed: &Addressed<'_>,
    now: SystemTime,
) -> Decision {
    // Unreachable through the claim, which resolves the same portal from the same store;
    // the portal can still have been deleted between the claim and now, which is a domain
    // this service no longer serves.
    let Some(resolved) = resolve(store, addressed) else {
        return Decision::Refusal(forbidden());
    };
    let portal = resolved.portal;
    let segments = path_segments(resolved.site_path);
    if segments.len() == 1 && segments[0] == crate::state::PORTAL_PREVIEW_SEGMENT {
        // The preview `PreviewPortal` built, of the portal's content as it stands: it is
        // what a publish WOULD put live, so it is taken from the record rather than from
        // any build that is already live.
        if !portal.preview_is_ready(now) {
            tracing::debug!(
                portal_id = %portal.portal_id,
                "the portal's preview is not ready, so its preview URL serves nothing"
            );
            return Decision::Refusal(forbidden());
        }
        return Decision::Page(Snapshot::of(
            store,
            portal,
            &portal.portal_content,
            portal.logo_uri.as_deref(),
            &portal.included_portal_product_arns,
            PREVIEW_FOOTER.to_string(),
            true,
        ));
    }
    if segments.len() != SITE_PATH_SEGMENTS {
        tracing::debug!(
            portal_id = %portal.portal_id,
            path = %resolved.site_path,
            "the portal serves no page at this path"
        );
        return Decision::Refusal(json_response(NOT_FOUND_STATUS, NOT_FOUND_MESSAGE, None));
    }
    // The build the clock has settled, which is what this domain serves. A portal nothing
    // has published, one whose first publish is still building, and one whose publication
    // a disable took down all serve nothing.
    let Some(site) = portal.live_site(now) else {
        tracing::info!(
            portal_id = %portal.portal_id,
            publish_status = %portal.publish_status,
            "the portal's domain serves nothing: no published build is live"
        );
        return Decision::Refusal(forbidden());
    };
    Decision::Page(Snapshot::of(
        store,
        portal,
        &site.content,
        site.logo_uri.as_deref(),
        &site.included_portal_product_arns,
        format!("published {}", instant(site.at)),
        false,
    ))
}

/// One portal page as an HTTP response: a document, with its length, so a client can read
/// it without the connection closing.
fn html_response(body: String) -> RtResponse {
    let bytes = body.into_bytes();
    let length = bytes.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(200u16).expect("200 is a valid status code"),
        SdkBody::from(bytes),
    );
    let headers = response.headers_mut();
    let _ = headers.try_insert("content-type", HTML_CONTENT_TYPE);
    let _ = headers.try_insert("content-length", length.to_string());
    response
}

/// Everything one page is rendered from, OWNED — so the document is built with no borrow of
/// the store and no lock held.
///
/// The products are resolved here rather than during rendering, which is what bounds the
/// work: the ARNs are de-duplicated (a list naming one product twice describes one product)
/// and cut to [`MAX_RENDERED_PRODUCTS`], so one product is snapshotted at most once and a
/// page carries at most as many sections as this account can hold products.
struct Snapshot {
    portal_id: String,
    content: crate::ops::types::PortalContent,
    logo_uri: Option<String>,
    /// The user-pool domain and app client id of a Cognito-authorized portal.
    cognito: Option<(String, String)>,
    products: Vec<ProductSnapshot>,
    footer: String,
    preview: bool,
}

/// One included product as the page states it: the ARN the portal named, and the product's
/// own records when this account still holds it.
struct ProductSnapshot {
    arn: String,
    /// `None` for an ARN naming a product this account does not hold — a product another
    /// account shared, or one that has been deleted. The build named it, and nothing about
    /// a product this store has no record of is invented.
    product: Option<crate::state::PortalProduct>,
}

impl Snapshot {
    fn of(
        store: &ComAmazonawsApigatewayv2Apigatewayv2State,
        portal: &Portal,
        content: &crate::ops::types::PortalContent,
        logo_uri: Option<&str>,
        included_portal_product_arns: &[String],
        footer: String,
        preview: bool,
    ) -> Self {
        // Indexed once rather than scanned per ARN, so snapshotting one page costs one pass
        // over the store's products plus one lookup per ARN the portal names.
        let indexed: BTreeMap<&str, &crate::state::PortalProduct> = store
            .portal_products
            .values()
            .map(|product| (product.portal_product_arn.as_str(), product))
            .collect();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let products = included_portal_product_arns
            .iter()
            .filter(|arn| seen.insert(arn.as_str()))
            .take(MAX_RENDERED_PRODUCTS)
            .map(|arn| ProductSnapshot {
                arn: arn.clone(),
                product: indexed.get(arn.as_str()).map(|product| (*product).clone()),
            })
            .collect();
        Self {
            portal_id: portal.portal_id.clone(),
            content: content.clone(),
            logo_uri: logo_uri.map(str::to_string),
            cognito: portal
                .authorization
                .cognito_config
                .as_ref()
                .map(|cognito| {
                    (
                        cognito.user_pool_domain.clone(),
                        cognito.app_client_id.clone(),
                    )
                }),
            products,
            footer,
            preview,
        }
    }
}

/// Render one portal page from the snapshot taken of this service's records.
///
/// The document is bounded: [`MAX_PAGE_BYTES`] of it is rendered, after which the remaining
/// sections are dropped and the page says so ([`TRUNCATION_MARKER`]), so a small request
/// cannot ask for an arbitrarily large response however much content the portal's products
/// hold.
fn page(snapshot: &Snapshot) -> String {
    let content = &snapshot.content;
    let colors = &content.theme.custom_colors;
    let mut body = String::new();
    body.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    body.push_str(&format!(
        "<title>{}</title>\n",
        escaped(&content.display_name)
    ));
    // The theme's own colours, as the custom properties the page styles itself from: a
    // portal's theme is what its site looks like, so the values the caller stored are what
    // the document carries.
    body.push_str(&format!(
        "<style>:root {{\n  --accent-color: {};\n  --background-color: {};\n  \
         --error-validation-color: {};\n  --header-color: {};\n  --navigation-color: {};\n  \
         --text-color: {};\n}}\nbody {{ background: var(--background-color); color: \
         var(--text-color); }}\n</style>\n",
        escaped(&colors.accent_color),
        escaped(&colors.background_color),
        escaped(&colors.error_validation_color),
        escaped(&colors.header_color),
        escaped(&colors.navigation_color),
        escaped(&colors.text_color),
    ));
    body.push_str("</head>\n");
    body.push_str(&format!(
        "<body data-portal-id=\"{}\" data-portal-preview=\"{}\">\n",
        escaped(&snapshot.portal_id),
        snapshot.preview
    ));
    body.push_str("<header>\n");
    if let Some(logo_uri) = snapshot.logo_uri.as_deref() {
        body.push_str(&format!(
            "<img alt=\"{}\" src=\"{}\">\n",
            escaped(&content.display_name),
            escaped(logo_uri)
        ));
    }
    body.push_str(&format!("<h1>{}</h1>\n", escaped(&content.display_name)));
    if let Some(description) = content.description.as_deref() {
        body.push_str(&format!("<p>{}</p>\n", escaped(description)));
    }
    if let Some((user_pool_domain, app_client_id)) = snapshot.cognito.as_ref() {
        // Stored configuration, named so a visitor sees which pool the portal is
        // configured against. No session is established here; see the module docs.
        body.push_str(&format!(
            "<p data-portal-authorization=\"COGNITO\" data-user-pool-domain=\"{}\" \
             data-app-client-id=\"{}\">Sign in with the user pool this portal is \
             configured with.</p>\n",
            escaped(user_pool_domain),
            escaped(app_client_id)
        ));
    }
    body.push_str("</header>\n<main>\n");
    for product in &snapshot.products {
        let section = product_section(product);
        // The budget is checked against what this section WOULD add, so the document can
        // never exceed it by a section's size: the sections that do not fit are dropped and
        // the page says so.
        if body.len() + section.len() > MAX_PAGE_BYTES {
            body.push_str(TRUNCATION_MARKER);
            break;
        }
        body.push_str(&section);
    }
    body.push_str("</main>\n");
    body.push_str(&format!("<footer>{}</footer>\n", escaped(&snapshot.footer)));
    body.push_str("</body>\n</html>\n");
    body
}

/// One portal product's section of the page: the product, its product pages and its product
/// REST endpoint pages, each from the record the control plane stored.
///
/// A product ARN the build included that the account does not hold renders as the ARN
/// alone: the build named it, and nothing about a product this store has no record of is
/// invented.
fn product_section(snapshot: &ProductSnapshot) -> String {
    let mut section = format!(
        "<section data-portal-product=\"{}\">\n",
        escaped(&snapshot.arn)
    );
    let Some(product) = snapshot.product.as_ref() else {
        section.push_str("</section>\n");
        return section;
    };
    section.push_str(&format!("<h2>{}</h2>\n", escaped(&product.display_name)));
    if let Some(description) = product.description.as_deref() {
        section.push_str(&format!("<p>{}</p>\n", escaped(description)));
    }
    for page in product.product_pages.values() {
        section.push_str(&format!(
            "<article data-product-page=\"{}\">\n<h3>{}</h3>\n<div>{}</div>\n</article>\n",
            escaped(&page.product_page_arn),
            escaped(&page.display_content.title),
            escaped(&page.display_content.body)
        ));
    }
    for page in product.product_rest_endpoint_pages.values() {
        let content = &page.display_content;
        let title = content
            .operation_name
            .as_deref()
            .unwrap_or(content.endpoint.as_str());
        section.push_str(&format!(
            "<article data-product-rest-endpoint-page=\"{}\" data-status=\"{}\">\n\
             <h3>{}</h3>\n<p><code>{}</code></p>\n",
            escaped(&page.product_rest_endpoint_page_arn),
            escaped(page.status()),
            escaped(title),
            escaped(&content.endpoint)
        ));
        if let Some(documentation) = content.body.as_deref() {
            section.push_str(&format!("<div>{}</div>\n", escaped(documentation)));
        }
        section.push_str("</article>\n");
    }
    section.push_str("</section>\n");
    section
}

/// `text` with the five characters that are markup in an HTML document replaced by their
/// named references, so a stored value is rendered as the text the caller stored and can
/// never be read as markup. `&` is replaced FIRST, so a reference this introduces is not
/// escaped a second time.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// One instant as the page states it: the `date-time` of RFC 3339, the form every timestamp
/// this service reports on the wire carries.
fn instant(at: SystemTime) -> String {
    crate::state::to_timestamp(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    //! The pure rules this module owns that a wire test cannot vary cheaply: which text is
    //! safe to put in a document. The served behaviour is pinned end to end in
    //! `tests/all/portal_data_plane.rs`.

    use super::*;

    #[test]
    fn stored_text_is_rendered_as_text_and_never_as_markup() {
        let cases: &[(&str, &str, &str)] = &[
            ("nothing to escape", "Orders API", "Orders API"),
            ("an ampersand", "a & b", "a &amp; b"),
            (
                "a tag",
                "<script>alert('x')</script>",
                "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;",
            ),
            ("a quote", "say \"hi\"", "say &quot;hi&quot;"),
            // The `&` of a reference the caller wrote is itself escaped, so the text the
            // page shows is the text that was stored.
            ("a reference the caller wrote", "&lt;", "&amp;lt;"),
            ("the empty string", "", ""),
        ];
        for (case, stored, expected) in cases {
            let actual = escaped(stored);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
