// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN an app's hosting is announced under: `App$defaultDomain`, and whether the
//! emulator answers at the address a client composes from it.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `amplify` signing name, then a plain unsigned `GET` of the site
//! — so what is asserted is what a client that was told where its site is sees.
//!
//! The rule under test: `App$defaultDomain` is "the default domain for the Amplify app"
//! (`smithy_query(query="doc",shape="App",member="defaultDomain")`) and
//! `Branch$displayName` "is used as the default domain prefix"
//! (`(shape="Branch",member="displayName")`), so the member is an address a client FETCHES
//! a deployed site at — which makes it an address THIS emulator answers on, not a public
//! AWS name nothing here serves:
//!
//! * a create that reached the emulator's own endpoint is announced on that endpoint, the
//!   one address such a client resolves, and the site of one of its branches is served
//!   under the path that names the app and the branch;
//! * a create addressed to one of AWS's own hosts comes from a client that resolves them
//!   and is announced the app's id under Amplify's hosting zone, the host form in which a
//!   client prefixes the branch's display name.
//!
//! Either way the value is an address and nothing else — no scheme, no path — resolved ONCE
//! when the app is created, so every later read of that app reports the same string whatever
//! it is addressed at, and a `GET` of the address composed from it is answered by the site
//! that branch deployed.
//!
//! The table below spans the class that rule generalises over: every spelling of the
//! emulator's own endpoint a client is configured with (the IPv4 loopback address, the
//! loopback name, a bracketed IPv6 literal, each on its own port, and one naming no port),
//! and the service's own published host beside them. Each row asserts BOTH halves — the
//! domain reported, and that a `GET` of the address built from it is answered by the site
//! that branch deployed — because the two together are what a client told the domain can do.
//!
//! The harness (`Emulator`) and the deployment driver (`Hosted`) are the ones
//! `app_branch_lifecycle` and `hosted_branch_site` already drive.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, REGION};
use crate::hosted_branch_site::{Hosted, SUCCEED};

/// The root document the rows deploy, so a `GET` of the site has something to answer with.
const ROOT_DOCUMENT: &str = "index.html";

/// The zone Amplify hosts an app's default domain under, pinned here rather than read from
/// the constant the service itself carries.
const HOSTING_ZONE: &str = "amplifyapp.com";

/// The path prefix an app announced on this emulator's own endpoint carries: the reserved
/// data-plane marker and the label naming the hosting plane. Written here independently of
/// the service, so the suite pins the address and not the code.
const LOCAL_MARKER: &str = "/_aws/amplify";

/// One row: the endpoint a client was pointed at, and whether that endpoint is the
/// EMULATOR'S OWN — which is what decides the form the app's hosting is announced in.
struct Row {
    /// What the row is, named in every assertion.
    case: &'static str,
    /// The `Host` the control-plane calls address.
    endpoint: &'static str,
    /// Whether the endpoint names no service, i.e. is this emulator's own address.
    own_endpoint: bool,
}

/// The rows: every addressing a client of this emulator uses, and the service's own
/// published host beside them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the IPv4 loopback address with a port",
            endpoint: "127.0.0.1:44285",
            own_endpoint: true,
        },
        Row {
            case: "the loopback name with another port",
            endpoint: "localhost:9297",
            own_endpoint: true,
        },
        Row {
            case: "a bracketed IPv6 loopback",
            endpoint: "[::1]:38199",
            own_endpoint: true,
        },
        Row {
            case: "the loopback address naming no port",
            endpoint: "127.0.0.1",
            own_endpoint: true,
        },
        Row {
            case: "this service's own published host",
            endpoint: "amplify.us-east-1.amazonaws.com",
            own_endpoint: false,
        },
    ]
}

/// The `app` record `GetApp` reports, addressed at `endpoint`.
async fn read_app(emulator: &Emulator, endpoint: &str, app_id: &str) -> Value {
    let reply = emulator
        .call_at(endpoint, "GET", &format!("/apps/{app_id}"), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "app {app_id:?}: expected HTTP 200 from GetApp, got {reply:?}"
    );
    reply.member("app")
}

/// Rule: an app's default domain is an address this emulator answers on — the endpoint the
/// create reached with the app's identity in the path, or the app's id under Amplify's
/// hosting zone for a client that resolves AWS's own hosts — and the site of a branch of
/// that app is served at the address a client composes from it.
#[tokio::test]
async fn an_app_announces_its_hosting_at_an_address_this_emulator_answers_on() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let created = emulator
            .call_at(
                row.endpoint,
                "POST",
                "/apps",
                Some(&json!({ "name": format!("hosting-{case}") })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateApp, got {created:?}"
        );
        let app = created.member("app");
        let app_id = string_member(&app, "appId");
        let reported = string_member(&app, "defaultDomain");

        let expected = if row.own_endpoint {
            row.endpoint.to_string()
        } else {
            format!("{app_id}.{HOSTING_ZONE}")
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the app's domain {expected:?}, got {reported:?}"
        );
        // The announced value is an ADDRESS and nothing else: it leads back to where the
        // caller is already talking (an own endpoint) or is a DNS name a branch label can be
        // prefixed to (AWS's own zone), and carries no scheme and no path either way.
        assert!(
            !reported.contains('/'),
            "case {case:?}: a domain carries no scheme and no path, got {reported:?}"
        );
        if !row.own_endpoint {
            assert!(
                reported.chars().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || "-.".contains(byte)),
                "case {case:?}: the domain must be a DNS name, got {reported:?}"
            );
        }

        // The same value is reported by every reader, and by one addressed somewhere
        // else: it is the app's record, resolved once by the create, not a function of the
        // reading request.
        let read_back = read_app(&emulator, row.endpoint, &app_id).await;
        assert_eq!(
            string_member(&read_back, "defaultDomain"),
            reported,
            "case {case:?}: GetApp must report the app's domain, got {read_back:?}"
        );
        let elsewhere = read_app(&emulator, &format!("amplify.{REGION}.amazonaws.com"), &app_id)
            .await;
        assert_eq!(
            string_member(&elsewhere, "defaultDomain"),
            reported,
            "case {case:?}: a read addressed elsewhere must report the same domain, got \
             {elsewhere:?}"
        );
        let listed = emulator.list_apps("").await;
        let first = list_member(&listed.body, "apps")
            .iter()
            .find(|listed| listed.get("appId").and_then(Value::as_str) == Some(app_id.as_str()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: ListApps must report the created app"));
        assert_eq!(
            string_member(&first, "defaultDomain"),
            reported,
            "case {case:?}: ListApps must report the app's domain, got {first:?}"
        );

        // The address a client composes from the two members is the address the hosting
        // edge serves: a branch of that app, deployed to, answers there.
        let display_name = "main";
        let hosted = Hosted::adopt(
            emulator,
            app_id.clone(),
            &json!({ "branchName": display_name }),
        )
        .await;
        let (visited_host, visited_path) = if row.own_endpoint {
            (
                row.endpoint.to_string(),
                format!("{LOCAL_MARKER}/{app_id}/{display_name}/"),
            )
        } else {
            (format!("{display_name}.{reported}"), "/".to_string())
        };
        let body = format!("<html>{case}</html>").into_bytes();
        let status = hosted
            .deploy(case, &[(ROOT_DOCUMENT, body.as_slice())])
            .await;
        assert_eq!(
            status, SUCCEED,
            "case {case:?}: the deployment must succeed, got {status}"
        );
        let visited = hosted
            .visit_host("GET", &visited_host, &visited_path)
            .await;
        assert_eq!(
            visited.status, 200,
            "case {case:?}: a GET of {visited_host:?}{visited_path:?} must be answered, got \
             {visited:?}"
        );
        assert_eq!(
            visited.body, body,
            "case {case:?}: expected the deployed document, got {:?}",
            String::from_utf8_lossy(&visited.body)
        );

        // And nothing else is served there: a branch label this app has no branch under is
        // a 404 at the same address, so the edge answers for the branch the address names
        // rather than for the app.
        let other_path = if row.own_endpoint {
            format!("{LOCAL_MARKER}/{app_id}/another-branch/")
        } else {
            "/".to_string()
        };
        let other_host = if row.own_endpoint {
            row.endpoint.to_string()
        } else {
            format!("another-branch.{reported}")
        };
        let missed = hosted.visit_host("GET", &other_host, &other_path).await;
        assert_eq!(
            missed.status, 404,
            "case {case:?}: a branch this app does not have must not be served, got \
             {missed:?}"
        );
    }
}
