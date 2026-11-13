// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the PORTAL data plane: what the domain a portal reports is
//! answered with, before a publish, after one, and after a `DisablePortal`.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue (one
//! portal created with `Authorization={None:{}}` and API Gateway's default domain, published
//! and then disabled):
//!
//! > A portal's own domain — the one API Gateway generated for it
//! > (`EndpointConfigurationResponse$PortalDefaultDomainName`) or the one the account
//! > brought — serves the build of that portal's LAST SETTLED PUBLISH and nothing else: a
//! > portal nothing has published, a portal whose publish is still in flight and a portal
//! > whose publication `DisablePortal` has taken down serve no page at all (`403`
//! > `{"message":"Forbidden"}`); a settled publish puts a document live that is rendered
//! > from the content that publish built, so a later write to the portal's content changes
//! > nothing that is already live until another publish builds it; a path the portal has no
//! > page at is `404` `{"message":"Not Found"}`; and the preview URL `Preview$PreviewUrl`
//! > announces serves the portal's CURRENT content once that preview is ready.
//!
//! Serving the domain is what makes a revocation observable: a caller can only tell that
//! `DisablePortal` took a developer portal offline by asking the domain, so these cases
//! assert the status BEFORE and AFTER each build rather than only the `PublishStatus` the
//! control plane reports.
//!
//! The rows span what the rule generalises over: how the request addresses the portal (its
//! generated domain, the custom domain it carries, and the emulator's own endpoint with the
//! portal's endpoint behind the reserved path marker, in both of its spellings), the
//! portal's `Authorization` alternative (`None` and a `CognitoConfig`), what its content
//! carries (the required members only, and a description, a logo and two portal products
//! with product pages and product REST endpoint pages), and the publication state the
//! request meets (never published, publish in flight, published, published again over
//! changed content, disabled, published after a disable).

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    MAX_INCLUDED_PORTAL_PRODUCTS, PORTAL_BUILD, PORTAL_PREVIEW_SEGMENT,
};

use crate::api_lifecycle::{
    as_str, call, call_at_host, emulator, emulator_with_rest_apis, error_code, rest_endpoint,
};
use crate::portal_lifecycle::{custom_colors, portal_content};
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The region [`call`] signs for, so the region the generated portal domain names.
const REGION: &str = "eu-west-1";

/// The emulator's own endpoint, which is the only address a client on this machine can
/// reach: the host the path addressings are read on.
const LOCAL_ENDPOINT: &str = "127.0.0.1:9297";

/// The status and sentence a domain that serves nothing answers — a portal with no live
/// publication. Shared with `portal_publication_revocation`, which asserts the same class
/// of answer over the publish → revoke flow.
pub(crate) const FORBIDDEN: u16 = 403;
pub(crate) const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status and sentence for a path the portal serves no page at.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The media type a portal page is served as.
const HTML: &str = "text/html; charset=utf-8";

/// The error code the front door answers when NO service claims a request — the answer this
/// suite exists to keep off a portal's domain.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// How long to wait so a build the service owes has certainly come due: its span plus
/// slack. A BLOCKING wait on purpose — this workspace's `tokio` carries no `time` feature.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// One answer from a portal's domain. The harness is the one every data-plane suite of this
/// crate uses (`websocket_handshake_data_plane::send`), which carries both readings of the
/// body: the `{"message": …}` envelope a refusal answers in, and the document a page is.
pub(crate) async fn fetch(emulator: &MultiServiceRouter, host: &str, path: &str) -> Answer {
    send(emulator, host, "GET", path, &[]).await
}

/// What a client needs to address a portal: its id and the two domains it reports.
#[derive(Clone)]
struct Published {
    portal_id: String,
    default_domain: String,
    custom_domain: Option<String>,
}

/// Create one portal and answer what a client needs to address it.
async fn portal(emulator: &MultiServiceRouter, case: &str, request: Value) -> Published {
    let (status, created) = call(emulator, "POST", "/v2/portals", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreatePortal must answer 201, got {status} {created}"
    );
    let endpoint = created
        .get("endpointConfiguration")
        .cloned()
        .unwrap_or(Value::Null);
    Published {
        portal_id: as_str(&created, "portalId"),
        default_domain: as_str(&endpoint, "portalDefaultDomainName"),
        custom_domain: endpoint
            .get("domainName")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Start one of the portal builds and assert it was accepted.
async fn build(emulator: &MultiServiceRouter, case: &str, portal_id: &str, build: Build) {
    let (status, body) = call(
        emulator,
        build.method(),
        &build.uri(portal_id),
        build.request(),
    )
    .await;
    assert_eq!(
        status,
        build.accepted_status(),
        "case {case:?}: {build:?} must answer {}, got {status} {body}",
        build.accepted_status()
    );
}

/// The three calls that start a server-managed portal build, with the request and success
/// status each one's model declares.
#[derive(Debug, Clone, Copy)]
enum Build {
    /// `POST /v2/portals/{PortalId}/publish`, modelled 202.
    Publish,
    /// `DELETE /v2/portals/{PortalId}/publish`, modelled 204.
    Disable,
    /// `POST /v2/portals/{PortalId}/preview`, modelled 202.
    Preview,
}

impl Build {
    fn method(self) -> &'static str {
        match self {
            Build::Publish | Build::Preview => "POST",
            Build::Disable => "DELETE",
        }
    }

    fn uri(self, portal_id: &str) -> String {
        match self {
            Build::Publish | Build::Disable => format!("/v2/portals/{portal_id}/publish"),
            Build::Preview => format!("/v2/portals/{portal_id}/preview"),
        }
    }

    fn request(self) -> Option<Value> {
        match self {
            Build::Publish => Some(json!({})),
            Build::Disable | Build::Preview => None,
        }
    }

    fn accepted_status(self) -> u16 {
        match self {
            Build::Publish | Build::Preview => 202,
            Build::Disable => 204,
        }
    }
}

/// Assert one answer from a portal's domain is the status and `{"message": …}` body the
/// class maps to — and is not one of the answers that would mean no data plane decided it.
pub(crate) fn refused(case: &str, page: &Answer, status: u16, sentence: &str) {
    assert_ne!(
        page.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the portal's domain must be served by this service, got {}",
        page.summary()
    );
    assert_eq!(
        page.status,
        status,
        "case {case:?}: expected {status}, got {}",
        page.summary()
    );
    assert_eq!(
        message(page),
        sentence,
        "case {case:?}: expected the {sentence:?} body, got {}",
        page.summary()
    );
}

/// Assert one answer is the portal's own page.
pub(crate) fn served(case: &str, page: &Answer, portal_id: &str) {
    assert_eq!(
        page.status,
        200,
        "case {case:?}: expected the portal's page, got {}",
        page.summary()
    );
    assert_eq!(
        page.content_type, HTML,
        "case {case:?}: a portal page is a document, got {}",
        page.summary()
    );
    assert!(
        page.text.contains(&format!("data-portal-id=\"{portal_id}\"")),
        "case {case:?}: the page must name the portal it was built for, got {}",
        page.summary()
    );
}

/// How long a wait for a CONDITION may take before a case gives up: far beyond the span a
/// portal's build is owed, so a loaded host reaches the same outcome as an idle one.
const CONDITION_DEADLINE: Duration = Duration::from_secs(60);

/// Fetch the root page of `host` until the portal's own page is SERVED there — the answer a
/// portal whose publish has come due gives — and answer that fetch.
///
/// The wait is for the CONDITION, never for a fixed time: the page is asked for again until
/// it is served or [`CONDITION_DEADLINE`] passes, so neither a fast nor a loaded host changes
/// what the case observes.
async fn fetch_once_published(emulator: &MultiServiceRouter, case: &str, host: &str) -> Answer {
    fetch_until(emulator, case, host, "the portal's page", |page| {
        page.status == 200
    })
    .await
}

/// Fetch the root page of `host` until `holds` is true of the answer, and answer that fetch.
/// `waited_for` names the condition in the failure message.
///
/// The one poll-until reading in this suite: the page is asked for again until the condition
/// holds or [`CONDITION_DEADLINE`] passes — a wait for what the case waits ON, not for a
/// fixed time.
async fn fetch_until(
    emulator: &MultiServiceRouter,
    case: &str,
    host: &str,
    waited_for: &str,
    holds: impl Fn(&Answer) -> bool,
) -> Answer {
    let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
    loop {
        let page = fetch(emulator, host, "/").await;
        if holds(&page) {
            return page;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {host} never served {waited_for}, last answer {}",
            page.summary()
        );
        tokio::task::yield_now().await;
    }
}

/// One row: how the portal is created, and how the request addresses it.
struct Row {
    case: &'static str,
    request: fn() -> Value,
    /// The host and path a client reaches this portal's page at.
    address: fn(&Published) -> (String, String),
}

/// A portal on API Gateway's own generated domain, with no authorization.
fn open_portal() -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content("Open Portal")
    })
}

/// A Cognito-authorized portal on a domain the account brought, with a description, a logo
/// and every theme colour set.
fn cognito_portal() -> Value {
    json!({
        "authorization": {
            "cognitoConfig": {
                "appClientId": "1h57kf5cpq17m0eml12eqemjq1",
                "userPoolArn": format!("arn:aws:cognito-idp:{REGION}:000000000000:userpool/{REGION}_pooL1"),
                "userPoolDomain": "https://portal-users.auth.eu-west-1.amazoncognito.com"
            }
        },
        "endpointConfiguration": {
            "acmManaged": {
                "certificateArn": format!("arn:aws:acm:{REGION}:000000000000:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"),
                "domainName": "developer.example.com"
            }
        },
        "portalContent": {
            "displayName": "Example Developer Portal",
            "description": "the catalogue of our public APIs",
            "theme": { "customColors": custom_colors() }
        },
        "logoUri": "s3://example-portal-assets/logo.png"
    })
}

fn rows() -> &'static [Row] {
    &[
        Row {
            case: "the generated domain, the addressing the endpoint configuration reports",
            request: open_portal,
            address: |published| (published.default_domain.clone(), "/".to_string()),
        },
        Row {
            case: "the custom domain the account brought",
            request: cognito_portal,
            address: |published| {
                (
                    published
                        .custom_domain
                        .clone()
                        .expect("this row's portal carries a custom domain"),
                    "/".to_string(),
                )
            },
        },
        Row {
            case: "the emulator's own endpoint with the portal id in the path",
            request: open_portal,
            address: |published| {
                (
                    LOCAL_ENDPOINT.to_string(),
                    format!("/_aws/portal/{}/", published.portal_id),
                )
            },
        },
        Row {
            case: "the emulator's own endpoint with the generated domain in the path",
            request: cognito_portal,
            address: |published| {
                (
                    LOCAL_ENDPOINT.to_string(),
                    format!("/_aws/{}/", published.default_domain),
                )
            },
        },
    ]
}

/// The whole publication sequence, on every addressing: nothing is served until a publish
/// has settled, the page is served once it has, and a disable takes it down again.
#[tokio::test]
async fn a_portals_domain_serves_exactly_what_its_last_settled_publish_put_live() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        let published = portal(&emulator, case, (row.request)()).await;
        let (host, path) = (row.address)(&published);

        // Nothing has published this portal, so its domain serves nothing.
        let page = fetch(&emulator, &host, &path).await;
        refused(
            &format!("{case}: before the first publish"),
            &page,
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        );

        // A publish only STARTS the build, so the domain still serves nothing until that
        // build comes due.
        build(&emulator, case, &published.portal_id, Build::Publish).await;
        let page = fetch(&emulator, &host, &path).await;
        refused(
            &format!("{case}: while the publish is still building"),
            &page,
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        );

        sleep(past_the_build());
        let page = fetch(&emulator, &host, &path).await;
        served(
            &format!("{case}: once the publish has settled"),
            &page,
            &published.portal_id,
        );

        // A path the portal has no page at is its own class, and not the domain's refusal.
        let deeper = if path.ends_with('/') {
            format!("{path}nowhere")
        } else {
            format!("{path}/nowhere")
        };
        let unknown = fetch(&emulator, &host, &deeper).await;
        refused(
            &format!("{case}: a path the portal has no page at"),
            &unknown,
            NOT_FOUND,
            NOT_FOUND_MESSAGE,
        );

        // The disable likewise only starts a build; once it settles the domain serves
        // nothing, which is how a caller observes the revocation.
        build(&emulator, case, &published.portal_id, Build::Disable).await;
        sleep(past_the_build());
        let page = fetch(&emulator, &host, &path).await;
        refused(
            &format!("{case}: once the disable has settled"),
            &page,
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        );

        // And it stays down: a second read, with nothing called in between, answers the
        // same.
        let page = fetch(&emulator, &host, &path).await;
        refused(
            &format!("{case}: still down on a later read"),
            &page,
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        );

        // Publishing again puts a build back live, so the revocation is not terminal.
        build(&emulator, case, &published.portal_id, Build::Publish).await;
        sleep(past_the_build());
        let page = fetch(&emulator, &host, &path).await;
        served(
            &format!("{case}: after a publish that followed the disable"),
            &page,
            &published.portal_id,
        );
    }
}

/// The page a publish put live is rendered from the content THAT publish built: every
/// stored value a portal's build carries reaches the document, and a later write to the
/// portal changes nothing live until another publish builds it.
#[tokio::test]
async fn the_served_page_is_rendered_from_the_build_the_publish_made() {
    let case = "a portal including two products, each with pages";
    // A product REST endpoint page documents an endpoint the account HOLDS, so the service
    // that owns REST APIs is mounted beside this one.
    let emulator = emulator_with_rest_apis();

    // Two portal products with pages of their own, so the page is asserted over the
    // records the control plane stored rather than over the portal's content alone.
    let mut product_arns = Vec::new();
    let mut invoke_urls = Vec::new();
    for (index, name) in ["Orders API", "Invoices API"].into_iter().enumerate() {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/portalproducts",
            Some(json!({ "displayName": name, "description": format!("product {index}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreatePortalProduct must answer 201, got {status} {created}"
        );
        let product_id = as_str(&created, "portalProductId");
        product_arns.push(as_str(&created, "portalProductArn"));

        let (status, page) = call(
            &emulator,
            "POST",
            &format!("/v2/portalproducts/{product_id}/productpages"),
            Some(json!({
                "displayContent": {
                    "title": format!("Getting started with {name}"),
                    "body": format!("How to call {name} <safely>")
                }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateProductPage must answer 201, got {status} {page}"
        );

        let rest_api = rest_endpoint(&emulator, &format!("portal-rest-{index}"), "prod").await;
        invoke_urls
            .push(format!("https://{rest_api}.execute-api.eu-west-1.amazonaws.com/prod"));
        let (status, endpoint_page) = call(
            &emulator,
            "POST",
            &format!("/v2/portalproducts/{product_id}/productrestendpointpages"),
            Some(json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": rest_api,
                        "stage": "prod",
                        "method": "GET",
                        "path": "/orders"
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateProductRestEndpointPage must answer 201, got {status} \
             {endpoint_page}"
        );
    }

    let published = portal(
        &emulator,
        case,
        json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": {
                // Markup in a stored value must reach the document as TEXT.
                "displayName": "Acme <APIs> & Co",
                "description": "everything we publish",
                "theme": { "customColors": custom_colors() }
            },
            "includedPortalProductArns": product_arns.clone(),
            "logoUri": "s3://acme-portal/logo.png"
        }),
    )
    .await;
    build(&emulator, case, &published.portal_id, Build::Publish).await;

    let page = fetch_once_published(&emulator, case, &published.default_domain).await;
    served(case, &page, &published.portal_id);
    let expected: Vec<(&str, String)> = vec![
        ("the display name, as text", "Acme &lt;APIs&gt; &amp; Co".to_string()),
        ("the description", "everything we publish".to_string()),
        ("the logo the portal carries", "s3://acme-portal/logo.png".to_string()),
        // Each theme colour the caller stored.
        ("the accent colour", "--accent-color: #ff9900".to_string()),
        ("the header colour", "--header-color: #232f3e".to_string()),
        ("the first product", "Orders API".to_string()),
        ("the second product", "Invoices API".to_string()),
        (
            "a product page's title",
            "Getting started with Orders API".to_string(),
        ),
        (
            "a product page's body, as text",
            "How to call Orders API &lt;safely&gt;".to_string(),
        ),
        (
            "a REST endpoint page's invoke URL",
            invoke_urls[0].clone(),
        ),
        ("each product's ARN", product_arns[0].clone()),
    ];
    for (what, fragment) in &expected {
        assert!(
            page.text.contains(fragment.as_str()),
            "case {case:?}: the page must carry {what} ({fragment:?}), got {}",
            page.summary()
        );
    }
    assert!(
        !page.text.contains("<APIs>"),
        "case {case:?}: a stored value must never reach the document as markup, got {}",
        page.summary()
    );

    // A write to the portal's content changes what the NEXT publish would build, not what
    // is already live.
    let (status, updated) = call(
        &emulator,
        "PATCH",
        &format!("/v2/portals/{}", published.portal_id),
        Some(json!({ "portalContent": portal_content("Renamed Portal") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdatePortal must answer 200, got {status} {updated}"
    );
    let page = fetch(&emulator, &published.default_domain, "/").await;
    served(case, &page, &published.portal_id);
    assert!(
        page.text.contains("Acme &lt;APIs&gt; &amp; Co") && !page.text.contains("Renamed Portal"),
        "case {case:?}: the live build must be the one the publish made, got {}",
        page.summary()
    );

    // Publishing again builds the content as it now stands, which is what then goes live.
    // The wait is for the republished page to be SERVED — the condition — not for a time.
    build(&emulator, case, &published.portal_id, Build::Publish).await;
    let page = fetch_until(
        &emulator,
        case,
        &published.default_domain,
        "the republished build",
        |page| page.text.contains("Renamed Portal"),
    )
    .await;
    assert!(
        !page.text.contains("Acme &lt;APIs&gt;"),
        "case {case:?}: the republished build must replace the one before it, got {}",
        page.summary()
    );
}

/// The preview URL the service announces serves the portal's CURRENT content — what a
/// publish would build — once the preview is ready, and nothing before it.
#[tokio::test]
async fn the_preview_url_serves_the_portals_current_content_once_it_is_ready() {
    let case = "a portal previewed before it is ever published";
    let emulator = emulator();
    let published = portal(
        &emulator,
        case,
        json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("Previewed Portal")
        }),
    )
    .await;

    // The path the service itself announces for this portal's preview
    // (`crate::state::portal_preview_url`, whose segment this is).
    let preview_path = format!("/{PORTAL_PREVIEW_SEGMENT}");
    let page = fetch(&emulator, &published.default_domain, &preview_path).await;
    refused(
        &format!("{case}: before any preview was built"),
        &page,
        FORBIDDEN,
        FORBIDDEN_MESSAGE,
    );

    build(&emulator, case, &published.portal_id, Build::Preview).await;
    let page = fetch(&emulator, &published.default_domain, &preview_path).await;
    refused(
        &format!("{case}: while the preview is still building"),
        &page,
        FORBIDDEN,
        FORBIDDEN_MESSAGE,
    );

    sleep(past_the_build());
    let page = fetch(&emulator, &published.default_domain, &preview_path).await;
    served(
        &format!("{case}: once the preview is ready"),
        &page,
        &published.portal_id,
    );
    assert!(
        page.text.contains("data-portal-preview=\"true\"")
            && page.text.contains("Previewed Portal"),
        "case {case:?}: the preview must be marked as one and carry the portal's current \
         content, got {}",
        page.summary()
    );

    // A preview changes nothing about what the portal PUBLISHES, so the domain's own page
    // is still not served.
    let page = fetch(&emulator, &published.default_domain, "/").await;
    refused(
        &format!("{case}: the published page of a portal that was only previewed"),
        &page,
        FORBIDDEN,
        FORBIDDEN_MESSAGE,
    );
}

/// The domain a portal ANNOUNCES is an address the client that created it can reach: a
/// create addressed at the emulator's own endpoint is announced on that endpoint, a create
/// addressed at this service's AWS host on the generated AWS host, and in both cases the
/// announced address is the one that serves the portal's page and its preview.
#[tokio::test]
async fn a_portal_announces_the_endpoint_its_creator_can_reach() {
    // (case, the host CreatePortal was addressed at, whether that host is the emulator's
    // own endpoint)
    let cases: &[(&str, &str, bool)] = &[
        (
            "created through the emulator's own endpoint",
            LOCAL_ENDPOINT,
            true,
        ),
        ("created through localhost", "localhost:9297", true),
        (
            "created through this service's own AWS host",
            "apigateway.eu-west-1.amazonaws.com",
            false,
        ),
    ];
    for (case, host, local) in cases {
        let emulator = emulator();
        let (status, created) = call_at_host(
            &emulator,
            host,
            REGION,
            "POST",
            "/v2/portals",
            Some(open_portal()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreatePortal must answer 201, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let endpoint = created
            .get("endpointConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        let announced = as_str(&endpoint, "portalDefaultDomainName");
        let expected = if *local {
            format!("{host}/_aws/portal/{portal_id}")
        } else {
            format!("{portal_id}.portal.{REGION}.amazonaws.com")
        };
        assert_eq!(
            announced, expected,
            "case {case:?}: the portal must be announced at the endpoint its creator \
             reaches, got {endpoint}"
        );
        // Either form is within the modelled 3..256 bound of the member.
        assert!(
            (3..=256).contains(&announced.chars().count()),
            "case {case:?}: the announced domain must be a 3..256 string, got {announced:?}"
        );

        // And it is the address that SERVES the portal: a client that fetches what it was
        // announced reaches this portal's page once a publish has settled.
        let (host_part, path_part) = match announced.split_once('/') {
            Some((host, path)) => (host.to_string(), format!("/{path}/")),
            None => (announced.clone(), "/".to_string()),
        };
        let page = fetch(&emulator, &host_part, &path_part).await;
        refused(
            &format!("{case}: before the publish"),
            &page,
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        );
        build(&emulator, case, &portal_id, Build::Publish).await;
        sleep(past_the_build());
        let page = fetch(&emulator, &host_part, &path_part).await;
        served(case, &page, &portal_id);

        // The preview URL the service announces is built from that same address, and the
        // preview is served there.
        let (status, started) = call_at_host(
            &emulator,
            host,
            REGION,
            "POST",
            &format!("/v2/portals/{portal_id}/preview"),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: PreviewPortal must answer 202, got {status} {started}"
        );
        sleep(past_the_build());
        let (status, read) = call_at_host(
            &emulator,
            host,
            REGION,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPortal must answer 200, got {status} {read}"
        );
        let preview_url = read
            .get("preview")
            .map(|preview| as_str(preview, "previewUrl"))
            .unwrap_or_default();
        assert_eq!(
            preview_url,
            format!("https://{announced}/{PORTAL_PREVIEW_SEGMENT}"),
            "case {case:?}: the preview URL must name the announced address, got {read}"
        );
        let page = fetch(
            &emulator,
            &host_part,
            &format!(
                "{}/{PORTAL_PREVIEW_SEGMENT}",
                path_part.trim_end_matches('/')
            ),
        )
        .await;
        served(&format!("{case}: the preview"), &page, &portal_id);
    }
}

/// The page one visit can ask for is BOUNDED: a product the portal includes twice is
/// rendered once, a page past the byte budget is truncated and says so, and a write that
/// would store more inclusions than a portal may hold is refused with the modelled
/// `TooManyRequestsException` rather than stored.
#[tokio::test]
async fn the_page_one_visit_can_ask_for_is_bounded() {
    let emulator = emulator();

    // Enough content that the document passes its byte budget: each product page's `Body`
    // is the modelled maximum (`__stringMin1Max32768`), so a handful of products with a few
    // pages each is already past a megabyte.
    const PRODUCTS: usize = 4;
    const PAGES_PER_PRODUCT: usize = 10;
    const BODY_LEN: usize = 32_768;
    let body = "x".repeat(BODY_LEN);

    let mut arns = Vec::new();
    for index in 0..PRODUCTS {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/portalproducts",
            Some(json!({ "displayName": format!("Bulky API {index}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "CreatePortalProduct must answer 201, got {status} {created}"
        );
        let product_id = as_str(&created, "portalProductId");
        arns.push(as_str(&created, "portalProductArn"));
        for page in 0..PAGES_PER_PRODUCT {
            let (status, stored) = call(
                &emulator,
                "POST",
                &format!("/v2/portalproducts/{product_id}/productpages"),
                Some(json!({
                    "displayContent": { "title": format!("Page {page}"), "body": body }
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "CreateProductPage must answer 201, got {status} {stored}"
            );
        }
    }

    // Each product named THREE times: a list naming one product repeatedly describes one
    // product, so the page must carry one section per product.
    let repeated: Vec<String> = arns
        .iter()
        .flat_map(|arn| std::iter::repeat_n(arn.clone(), 3))
        .collect();
    let published = portal(
        &emulator,
        "a portal including every product three times",
        json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("Bounded Portal"),
            "includedPortalProductArns": repeated
        }),
    )
    .await;
    build(
        &emulator,
        "a portal including every product three times",
        &published.portal_id,
        Build::Publish,
    )
    .await;
    sleep(past_the_build());

    let page = fetch(&emulator, &published.default_domain, "/").await;
    served("the bounded page", &page, &published.portal_id);
    for arn in &arns {
        let sections = page.text.matches(&format!("data-portal-product=\"{arn}\"")).count();
        assert!(
            sections <= 1,
            "expected one section per product however often it is included, got {sections} \
             for {arn}"
        );
    }
    assert!(
        page.text.len() < BODY_LEN * PRODUCTS * PAGES_PER_PRODUCT,
        "the page must be bounded rather than rendering every byte the products hold, got \
         {} bytes",
        page.text.len()
    );
    assert!(
        page.text.contains("data-portal-truncated=\"true\""),
        "a page cut at its byte budget must say so, got {} bytes",
        page.text.len()
    );
    // Still a complete document: the budget drops sections, it does not cut the page off
    // mid-markup.
    assert!(
        page.text.ends_with("</html>\n"),
        "a truncated page is still a whole document, got {}",
        page.summary()
    );

    // And the write side: a portal may not store more inclusions than it may hold.
    let too_many: Vec<String> = (0..=MAX_INCLUDED_PORTAL_PRODUCTS)
        .map(|index| {
            format!("arn:aws:apigateway:{REGION}:000000000000:/portalproducts/prod{index:06}")
        })
        .collect();
    let (status, refused) = call(
        &emulator,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("Too Many Products"),
            "includedPortalProductArns": too_many.clone()
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "a create including more products than a portal may hold must be refused 429, got \
         {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {refused}"
    );
    let (status, refused) = call(
        &emulator,
        "PATCH",
        &format!("/v2/portals/{}", published.portal_id),
        Some(json!({ "includedPortalProductArns": too_many })),
    )
    .await;
    assert_eq!(
        status, 429,
        "an update including more products than a portal may hold must be refused 429, got \
         {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {refused}"
    );
    // The refused update stored nothing: the portal still serves the build it had.
    let page = fetch(&emulator, &published.default_domain, "/").await;
    served("after the refused update", &page, &published.portal_id);
}

/// A visit is a READ: repeating it answers the same bytes, and the portal reports exactly
/// what it reported before — no status moved and no publication was spent.
#[tokio::test]
async fn visiting_a_portal_changes_nothing_a_later_caller_sees() {
    let case = "a published portal visited three times";
    let emulator = emulator();
    let published = portal(&emulator, case, open_portal()).await;
    build(&emulator, case, &published.portal_id, Build::Publish).await;
    sleep(past_the_build());

    let (status, before) = call(
        &emulator,
        "GET",
        &format!("/v2/portals/{}", published.portal_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPortal must answer 200, got {status} {before}"
    );

    let first = fetch(&emulator, &published.default_domain, "/").await;
    served(case, &first, &published.portal_id);
    for attempt in 1..=3 {
        let again = fetch(&emulator, &published.default_domain, "/").await;
        served(case, &again, &published.portal_id);
        assert_eq!(
            again.text, first.text,
            "case {case:?}: visit {attempt} served a different page"
        );
        let (status, after) = call(
            &emulator,
            "GET",
            &format!("/v2/portals/{}", published.portal_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPortal must answer 200 after visit {attempt}, got {status} {after}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: visit {attempt} changed the portal GetPortal reports"
        );
    }
}

/// A host no portal of this account reports is not this service's to answer, so the data
/// plane claims nothing on it — the control plane's own path space included.
#[tokio::test]
async fn a_domain_no_portal_reports_is_not_claimed() {
    let case = "one published portal, and hosts that are not its domain";
    let emulator = emulator();
    let published = portal(&emulator, case, open_portal()).await;
    build(&emulator, case, &published.portal_id, Build::Publish).await;
    sleep(past_the_build());

    let foreign = [
        (
            "another portal id's generated domain",
            "zzzzzzzzzz.portal.eu-west-1.amazonaws.com".to_string(),
            "/".to_string(),
        ),
        (
            "a domain nobody brought",
            "developer.example.org".to_string(),
            "/".to_string(),
        ),
        (
            "the emulator's endpoint with an id no portal carries",
            LOCAL_ENDPOINT.to_string(),
            "/_aws/portal/zzzzzzzzzz/".to_string(),
        ),
        (
            // A bare first path segment belongs to whatever service the request is for,
            // so a portal is never claimed on one.
            "the emulator's endpoint with the portal id as a bare path segment",
            LOCAL_ENDPOINT.to_string(),
            format!("/{}", published.portal_id),
        ),
    ];
    for (what, host, path) in foreign {
        let page = fetch(&emulator, &host, &path).await;
        assert_ne!(
            page.status, 200,
            "case {case:?}: {what} must not serve this portal's page, got {}",
            page.summary()
        );
    }

    // And the control plane is still reachable on the portal's own domain: a `/v2/…` path
    // is a control-plane call on whatever host it arrives at.
    let (status, listed) = call(&emulator, "GET", "/v2/portals", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPortals must answer 200, got {status} {listed}"
    );
}
