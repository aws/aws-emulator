// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN an app's hosting is reported under: `App$defaultDomain`, and whether the
//! emulator answers at the host a client composes from it.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `amplify` signing name, then a plain unsigned `GET` of the site
//! — so what is asserted is what a client that was told where its site is sees.
//!
//! The rule under test: `App$defaultDomain` is "the default domain for the Amplify app"
//! (`smithy_query(query="doc",shape="App",member="defaultDomain")`), a property of the APP
//! — the minted app id under the one zone Amplify hosts apps on — so it is a DNS name whose
//! first label is that id, it is the same string on every read of one app, and nothing
//! about the request that created the app (its `Host`, the endpoint it addressed, the port
//! this process is served on) takes part in it. `Branch$displayName` "is used as the
//! default domain prefix" (`(shape="Branch",member="displayName")`), so a client composes
//! `https://<displayName>.<defaultDomain>/`, and the hosting edge serves exactly that host
//! — addressed, like every other request, at this emulator's own endpoint.
//!
//! The table below spans the class that rule generalises over: every spelling of the
//! emulator's own endpoint a client is configured with (the IPv4 loopback address, the
//! loopback name, a bracketed IPv6 literal, each on its own port, and one naming no port),
//! and the service's own published host. Each row asserts BOTH halves — the domain
//! reported, and that a `GET` of the host built from it is answered by the site that branch
//! deployed — because the two together are what a client told the domain can do.
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

/// One row: the endpoint a client was pointed at, which the domain of the app it creates
/// there must not depend on.
struct Row {
    /// What the row is, named in every assertion.
    case: &'static str,
    /// The `Host` the control-plane calls address.
    endpoint: &'static str,
}

/// The rows: every addressing a client of this emulator uses, and the service's own
/// published host beside them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the IPv4 loopback address with a port",
            endpoint: "127.0.0.1:44285",
        },
        Row {
            case: "the loopback name with another port",
            endpoint: "localhost:9297",
        },
        Row {
            case: "a bracketed IPv6 loopback",
            endpoint: "[::1]:38199",
        },
        Row {
            case: "the loopback address naming no port",
            endpoint: "127.0.0.1",
        },
        Row {
            case: "this service's own published host",
            endpoint: "amplify.us-east-1.amazonaws.com",
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

/// Rule: an app's default domain is its own id under Amplify's hosting zone, whatever
/// endpoint the create addressed, and the site of a branch of that app is served at the
/// host a client composes from it.
#[tokio::test]
async fn an_app_reports_its_own_id_under_the_hosting_zone_as_its_default_domain() {
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

        let expected = format!("{app_id}.{HOSTING_ZONE}");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the app's domain {expected:?}, got {reported:?}"
        );
        // A DNS name and nothing else: no port, no userinfo, no path — a label a branch's
        // own label can be prefixed to.
        assert!(
            reported
                .chars()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || "-.".contains(byte)),
            "case {case:?}: the domain must be a DNS name, got {reported:?}"
        );

        // The same value is reported by every reader, and by one addressed somewhere
        // else: it is the app's record, not a function of the reading request.
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

        // The host a client composes from the two members is the host the hosting edge
        // serves: a branch of that app, deployed to, answers there — `Hosted` composes it
        // out of `defaultDomain` and the branch's `displayName` exactly as a client does.
        let hosted = Hosted::adopt(emulator, app_id, &json!({ "branchName": "main" })).await;
        assert_eq!(
            hosted.host,
            format!("main.{reported}"),
            "case {case:?}: the site's host is the branch's label under the app's domain, \
             got {:?}",
            hosted.host
        );
        let body = format!("<html>{case}</html>").into_bytes();
        let status = hosted
            .deploy(case, &[(ROOT_DOCUMENT, body.as_slice())])
            .await;
        assert_eq!(
            status, SUCCEED,
            "case {case:?}: the deployment must succeed, got {status}"
        );
        let visited = hosted.visit("/").await;
        assert_eq!(
            visited.status, 200,
            "case {case:?}: a GET of the site's host must be answered, got {visited:?}"
        );
        assert_eq!(
            visited.body, body,
            "case {case:?}: expected the deployed document, got {:?}",
            String::from_utf8_lossy(&visited.body)
        );
    }
}
