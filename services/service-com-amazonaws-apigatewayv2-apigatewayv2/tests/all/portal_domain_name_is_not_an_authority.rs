// SPDX-License-Identifier: CC0-1.0

//! The FORM of the default domain a portal publishes, on every operation that reports a
//! portal.
//!
//! The rule, one level above the recorded cases that filed the issue (a `GetPortal` and an
//! `UpdatePortal` whose `endpointConfiguration.portalDefaultDomainName` was asserted by shape
//! rather than by value):
//!
//! > `EndpointConfigurationResponse$PortalDefaultDomainName` is "the portal default domain
//! > name. This domain name is generated and managed by API Gateway", typed
//! > `__stringMin3Max256`
//! > (`smithy_query(query="shape",shape="EndpointConfigurationResponse")`), and what a client
//! > does with the value is FETCH it — so this emulator publishes the address it SERVES that
//! > portal's site at: the `host:port` of the socket the portal was given, or its own endpoint
//! > with the portal's reserved site path for a portal that got no socket. AWS's generated
//! > name resolves nowhere near this process, and neither does one this emulator could mint in
//! > a zone of its own, so neither is ever announced. The value is settled ONCE, by the
//! > create, so every operation that reports the portal reports that one address, no later
//! > caller's endpoint moves it; and it is an address this service ANSWERS at, so a request
//! > addressed to it is refused while the portal has nothing live and is served the portal's
//! > page once a publish has settled.
//!
//! The rows span what the rule generalises over: the endpoint the create was addressed at
//! (this service's own AWS host, the emulator's own endpoint as an IP literal with and without
//! a port, a loopback name with a port), the Region the calls are signed for, the
//! `EndpointConfigurationRequest` alternative the portal is created with (API Gateway's
//! default domain and a domain the account brought with its certificate), and the patch an
//! `UpdatePortal` carries (content only, an endpoint configuration rewritten to the default
//! domain, and one that brings a domain of the account's own) — the patches being the paths
//! that rebuild the member.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_at_host, emulator};
use crate::portal_data_plane::{
    fetch, fetch_path_until_served, refused, served, FORBIDDEN, FORBIDDEN_MESSAGE,
};
use crate::portal_lifecycle::{assert_published_domain_name, portal_content};

/// One row: where the calls are addressed, the Region they are signed for, and the domain
/// configuration the portal is created with.
struct Row {
    case: &'static str,
    /// The `Host` every control-plane call of the row carries — the endpoint this client
    /// reached the emulator at.
    host: &'static str,
    region: &'static str,
    /// The request's `endpointConfiguration`.
    endpoint_configuration: fn(&str) -> Value,
}

/// API Gateway's own generated domain: the alternative that brings nothing.
fn default_domain(_region: &str) -> Value {
    json!({ "none": {} })
}

/// A domain the account brought with the certificate it is served under: the other
/// alternative, whose two `AcmManaged` members are both required.
fn brought_domain(region: &str) -> Value {
    json!({
        "acmManaged": {
            "domainName": "developers.example.com",
            "certificateArn": format!(
                "arn:aws:acm:{region}:000000000000:certificate/\
                 11111111-2222-3333-4444-555555555555"
            )
        }
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a client configured with the emulator's loopback endpoint, default domain",
            host: "127.0.0.1:43511",
            region: "us-west-2",
            endpoint_configuration: default_domain,
        },
        Row {
            case: "the same endpoint named by its loopback NAME, and a brought domain",
            host: "localhost:9297",
            region: "eu-west-1",
            endpoint_configuration: brought_domain,
        },
        Row {
            case: "the emulator's own endpoint as an IP literal with no port",
            host: "127.0.0.1",
            region: "ap-southeast-2",
            endpoint_configuration: default_domain,
        },
        Row {
            case: "a client that resolves AWS's own hosts: this service's endpoint host",
            host: "apigateway.eu-north-1.amazonaws.com",
            region: "eu-north-1",
            endpoint_configuration: brought_domain,
        },
    ]
}

/// The `portalDefaultDomainName` a body reporting a portal carries.
fn published_name(case: &str, source: &str, body: &Value) -> String {
    let endpoint = body
        .get("endpointConfiguration")
        .cloned()
        .unwrap_or(Value::Null);
    let name = as_str(&endpoint, "portalDefaultDomainName");
    assert!(
        !name.is_empty(),
        "case {case:?}: {source} must report a portalDefaultDomainName, got {body}"
    );
    name
}

/// Create one portal for `row` and answer `(portal id, the created body)`.
async fn portal_of_row(emulator: &MultiServiceRouter, row: &Row) -> (String, Value) {
    let (status, created) = call_at_host(
        emulator,
        row.host,
        row.region,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": (row.endpoint_configuration)(row.region),
            "portalContent": portal_content("Domain Form Portal")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: expected 201 from CreatePortal, got {status} {created}",
        row.case
    );
    (as_str(&created, "portalId"), created)
}

/// The patches an `UpdatePortal` of a row carries, each named for the failure message: the
/// member is the service's, so none of them moves it.
fn patches(region: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "a patch that touches only the portal's content",
            json!({ "portalContent": portal_content("Renamed Domain Form Portal") }),
        ),
        (
            "a patch that rewrites the endpoint configuration to the default domain",
            json!({
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Default Domain Again")
            }),
        ),
        (
            "a patch that brings a domain of the account's own",
            json!({ "endpointConfiguration": brought_domain(region) }),
        ),
    ]
}

/// RULE: every reader of a portal reports the one address this emulator serves that portal's
/// site at, whatever endpoint the calls are addressed at and whatever a patch rewrites around
/// it.
#[tokio::test]
async fn every_portal_reader_reports_the_address_the_site_is_served_at() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        let (portal_id, created) = portal_of_row(&emulator, &row).await;
        let published = published_name(case, "CreatePortal", &created);
        assert_published_domain_name(
            &format!("{case}: CreatePortal"),
            &published,
            &portal_id,
        );
        // The precise defect this suite pins: the value is settled by the portal's own site,
        // never by the endpoint the CALL happened to reach this emulator at — two portals
        // created through one endpoint are published at two addresses, and the control-plane
        // endpoint is not a portal's site.
        assert_ne!(
            published, row.host,
            "case {case:?}: the endpoint this emulator is reached at is not the portal's own \
             site address, got {published:?}"
        );
        let (other_id, other_created) = portal_of_row(&emulator, &row).await;
        let other_published = published_name(case, "CreatePortal", &other_created);
        assert_ne!(
            published, other_published,
            "case {case:?}: expected each portal published at its own address, got \
             {published:?} for {portal_id} and {other_id}"
        );

        // The reader a client that did not make the create uses reports the same name.
        let (status, read) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal, got {status} {read}"
        );
        assert_eq!(
            published_name(case, "GetPortal", &read),
            published,
            "case {case:?}: expected GetPortal to report the generated name, got {read}"
        );

        // So does the mutation response of every patch, which is what a client that updates
        // and re-reads in one round trip stores.
        for (what, patch) in patches(row.region) {
            let (status, patched) = call_at_host(
                &emulator,
                row.host,
                row.region,
                "PATCH",
                &format!("/v2/portals/{portal_id}"),
                Some(patch),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from UpdatePortal with {what}, got {status} \
                 {patched}"
            );
            let patched_name = published_name(case, "UpdatePortal", &patched);
            assert_eq!(
                patched_name, published,
                "case {case:?}: {what} must carry the published address over, got {patched}"
            );
            assert_published_domain_name(
                &format!("{case}: UpdatePortal with {what}"),
                &patched_name,
                &portal_id,
            );
        }

        // And the summary the collection reports, read after those patches.
        let (status, listed) =
            call_at_host(&emulator, row.host, row.region, "GET", "/v2/portals", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListPortals, got {status} {listed}"
        );
        let summary = listed
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| as_str(item, "portalId") == portal_id)
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListPortals must report the portal, got {listed}")
            });
        assert_eq!(
            published_name(case, "ListPortals", &summary),
            published,
            "case {case:?}: expected the generated name in the portal's summary, got {listed}"
        );

        // A read addressed at ANOTHER endpoint reports it too: the name was settled by the
        // create and is not the reading request's to move.
        let (status, elsewhere) = call_at_host(
            &emulator,
            "127.0.0.1:34257",
            row.region,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal elsewhere, got {status} {elsewhere}"
        );
        assert_eq!(
            published_name(case, "GetPortal elsewhere", &elsewhere),
            published,
            "case {case:?}: the endpoint a later caller reaches this emulator at must not \
             move the name, got {elsewhere}"
        );
    }
}

/// RULE's other half: the address the portal publishes is one THIS service answers at — a
/// client that dials it is refused while the portal has nothing live and is served the
/// portal's page once a publish has settled.
#[tokio::test]
async fn the_published_address_is_one_this_service_answers_at() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        let (portal_id, created) = portal_of_row(&emulator, &row).await;
        let published = published_name(case, "CreatePortal", &created);

        // Nothing is published yet: the address is claimed by this service and answers the
        // refusal a site with nothing live earns, rather than a routing failure.
        let page = fetch(&emulator, &published, "/").await;
        refused(
            &format!("{case}: {published} before the publish"),
            &page,
            FORBIDDEN,
            FORBIDDEN_MESSAGE,
        );

        let (status, body) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "POST",
            &format!("/v2/portals/{portal_id}/publish"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 from PublishPortal, got {status} {body}"
        );

        // A publish only STARTS the build, so the wait is for the CONDITION: the page being
        // served at the address the service published.
        let page = fetch_path_until_served(&emulator, case, &published, "/").await;
        served(case, &page, &portal_id);
    }
}
