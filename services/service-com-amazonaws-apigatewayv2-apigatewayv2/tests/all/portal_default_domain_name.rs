// SPDX-License-Identifier: CC0-1.0

//! The default domain a portal publishes, and what this emulator answers at it.
//!
//! The rule, one level above the recorded case (one `CreatePortal` through a client
//! configured with this emulator's loopback endpoint):
//!
//! > `EndpointConfigurationResponse$PortalDefaultDomainName` is "the portal default domain
//! > name. This domain name is generated and managed by API Gateway", typed
//! > `__stringMin3Max256`
//! > (`smithy_query(query="shape",shape="EndpointConfigurationResponse")`), and a client
//! > FETCHES the value — so this emulator publishes the address it serves that portal's site
//! > at: the `host:port` of the socket the portal was given, or its own endpoint with the
//! > portal's reserved site path for a portal that got no socket. It is settled once, by the
//! > create, so every operation that reports the portal reports that one address; and it is an
//! > address this emulator ANSWERS at, so a request addressed to it is refused while the
//! > portal has nothing live and is answered the portal's page once a publish has settled.
//!
//! The rows span what the rule generalises over: the endpoint the create was addressed at
//! (this service's own AWS host, the emulator's own endpoint as an IP literal with and
//! without a port, a loopback name with a port) and the Region the calls are signed for,
//! including the emulator's own default — so no row can pass on a hardcoded Region. The
//! URL-typed members that send a client to the site carry the same address with the scheme
//! and path a URL needs, and are `portal_published_address`'s subject.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call_at_host, emulator};
use crate::portal_data_plane::{fetch, fetch_path_until_served, refused, served, FORBIDDEN, FORBIDDEN_MESSAGE};
use crate::portal_lifecycle::{assert_published_domain_name, portal_content};

/// One row: the endpoint the calls are addressed at, and the Region they are signed for.
struct Row {
    case: &'static str,
    host: &'static str,
    region: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a client configured with the emulator's loopback \
                   endpoint",
            host: "127.0.0.1:40683",
            region: "us-west-2",
        },
        Row {
            case: "the same endpoint named by its loopback NAME on another port",
            host: "localhost:9297",
            region: "eu-west-1",
        },
        Row {
            case: "the emulator's own endpoint as an IP literal with no port",
            host: "127.0.0.1",
            region: "ap-southeast-2",
        },
        Row {
            case: "a client that resolves AWS's own hosts: this service's own endpoint host",
            host: "apigateway.eu-north-1.amazonaws.com",
            region: "eu-north-1",
        },
        Row {
            case: "the emulator's own default Region, signed for explicitly",
            host: "127.0.0.1:9297",
            region: "us-east-1",
        },
    ]
}

/// The `portalDefaultDomainName` one portal body reports.
fn published_name(case: &str, body: &Value) -> String {
    let endpoint = body
        .get("endpointConfiguration")
        .cloned()
        .unwrap_or(Value::Null);
    let name = as_str(&endpoint, "portalDefaultDomainName");
    assert!(
        !name.is_empty(),
        "case {case:?}: every portal reports a default domain name, got {body}"
    );
    name
}

/// RULE: the member is the address this emulator serves THIS portal's site at, every
/// operation that reports the portal reports that one value, and no two portals share it.
#[tokio::test]
async fn every_portal_reports_the_one_address_its_site_is_served_at() {
    let emulator = emulator();
    let mut seen: Vec<(String, String)> = Vec::new();

    for row in rows() {
        let case = row.case;
        let (status, created) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "POST",
            "/v2/portals",
            Some(json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Named Portal")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let name = published_name(case, &created);
        assert_published_domain_name(
            &format!("{case}: CreatePortal"),
            &name,
            &portal_id,
        );

        // Every reader of the portal reports that one value: the name is the portal's, not
        // the reading request's.
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
            published_name(case, &read),
            name,
            "case {case:?}: expected GetPortal to report the generated name, got {read}"
        );

        let (status, patched) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "PATCH",
            &format!("/v2/portals/{portal_id}"),
            Some(json!({
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Renamed Named Portal")
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdatePortal, got {status} {patched}"
        );
        assert_eq!(
            published_name(case, &patched),
            name,
            "case {case:?}: expected UpdatePortal to carry the generated name over, got \
             {patched}"
        );

        // And a read addressed at ANOTHER endpoint reports it too: the value was settled by
        // the create.
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
            published_name(case, &elsewhere),
            name,
            "case {case:?}: the endpoint a later caller reaches this emulator at must not \
             move the name, got {elsewhere}"
        );

        assert!(
            !seen.iter().any(|(_, held)| held == &name),
            "case {case:?}: the generated name {name:?} is already held, by {seen:?}"
        );
        seen.push((portal_id, name));
    }

    assert_eq!(
        seen.len(),
        rows().len(),
        "expected one generated name per portal, got {seen:?}"
    );
}

/// RULE's other half: the published address is one this emulator ANSWERS at — the portal's
/// site is refused there while nothing is published and served there once a publish has
/// settled.
#[tokio::test]
async fn the_published_address_is_one_this_emulator_answers_at() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        let (status, created) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "POST",
            "/v2/portals",
            Some(json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Served Portal")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let name = published_name(case, &created);

        // Nothing is published yet, so the name answers the refusal a site with nothing live
        // earns — which is already an answer OF this emulator, not a routing failure.
        let page = fetch(&emulator, &name, "/").await;
        refused(
            &format!("{case}: before the publish"),
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
        // served at the name the service generated.
        let page = fetch_path_until_served(&emulator, case, &name, "/").await;
        served(case, &page, &portal_id);
    }
}
