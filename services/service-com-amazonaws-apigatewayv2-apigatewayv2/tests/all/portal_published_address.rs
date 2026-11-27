// SPDX-License-Identifier: CC0-1.0

//! The address a portal's site is SERVED at, dialled over a real connection, and the NAME it
//! publishes for it.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue (one
//! portal created with `Authorization={None:{}}` and API Gateway's default domain, published,
//! probed, disabled and probed again):
//!
//! > `EndpointConfigurationResponse$PortalDefaultDomainName` is a DOMAIN NAME this service
//! > generates for the portal — "generated and managed by API Gateway" — so it carries host
//! > labels and nothing else, and it is a name THIS emulator answers for: a request
//! > addressed to it is served the build of the portal's last settled publish at its root,
//! > is refused while the portal has nothing live, and reaches nothing at all once the portal
//! > is deleted. The ADDRESS a client dials is the `host:port` of the socket this emulator
//! > serves that site on, which the portal announces through its URL-typed members
//! > (`Preview$PreviewUrl`), because a domain name carries no port.
//!
//! The sibling suites (`portal_data_plane`, `portal_publication_revocation`) assert WHAT that
//! name answers through this emulator's own front door. What is asserted HERE is the half
//! those cannot see: that the portal is reached over a REAL connection, addressed by the name
//! the service itself published, and that the connection stops being accepted when the portal
//! goes — because a site nothing on this machine serves passes every in-process check and is
//! still unreachable.
//!
//! The rows span what the rule generalises over: the endpoint the CREATE was addressed at
//! (this service's own AWS host, the emulator's own endpoint as an IP literal with a port, a
//! loopback name with a port), the portal's `EndpointConfiguration` alternative (API
//! Gateway's default domain and a domain the account brought with its certificate), and the
//! `Authorization` alternative (`None` and a `CognitoConfig`). The publication states each
//! row walks are the flow itself: nothing published, published, a path with no page,
//! disabled, re-probed, and deleted.
//!
//! The rows span what the rule generalises over: the endpoint the CREATE was addressed at
//! (this service's own AWS host, the emulator's own endpoint as an IP literal with a port, a
//! loopback name with a port), the portal's `EndpointConfiguration` alternative (API
//! Gateway's default domain and a domain the account brought with its certificate), and the
//! `Authorization` alternative (`None` and a `CognitoConfig`). The publication states each
//! row walks are the flow itself: nothing published, published, a path with no page,
//! disabled, re-probed, and deleted.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::net::TcpStream;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::PORTAL_PREVIEW_SEGMENT;

use crate::api_lifecycle::{as_str, call_at_host, emulator};
use crate::custom_domain_mapping_invoke::{request_as, Answer};
use crate::portal_lifecycle::{custom_colors, portal_content};

/// The Region every control-plane call here is signed for.
const REGION: &str = "eu-west-1";

/// The statuses a probe of the published address earns: the portal's page, the refusal a
/// portal with nothing live answers, and the answer for a path the portal has no page at.
const SERVED: u16 = 200;
const FORBIDDEN: u16 = 403;
const NOT_FOUND: u16 = 404;

/// How long a wait for a CONDITION may run before a case gives up, and how long a probe of a
/// deleted address may take to be refused: both far beyond the span a portal's build is owed,
/// so a loaded host reaches the same outcome as an idle one. Nothing here is timed — the
/// deadline is only what keeps a case that will never hold from hanging the suite.
const CONDITION_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits before it asks the address again: short next to the span a build is
/// owed, so a case observes the condition soon after it holds, and long enough that polling
/// does not spin on connections while it waits. It bounds nothing the case asserts —
/// [`CONDITION_DEADLINE`] is the only bound.
const PROBE_INTERVAL: Duration = Duration::from_millis(50);

/// One row: the endpoint the create was addressed at, and the portal it creates.
struct Row {
    case: &'static str,
    /// The `Host` the control-plane calls carry — the endpoint this client reached the
    /// emulator at.
    endpoint_host: &'static str,
    /// The request `CreatePortal` is given.
    request: fn() -> Value,
}

/// A portal on API Gateway's default domain, with no authorization.
fn default_domain_portal() -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content("Published Portal")
    })
}

/// A Cognito-authorized portal on a domain the account brought with its certificate: the
/// other alternative of both unions, which changes nothing about the address the portal's
/// site is published at.
fn brought_domain_portal() -> Value {
    json!({
        "authorization": {
            "cognitoConfig": {
                "appClientId": "7bd0s2k9rr1cv9m0lq3ucaf9bk",
                "userPoolArn": format!(
                    "arn:aws:cognito-idp:{REGION}:000000000000:userpool/{REGION}_pooL1"
                ),
                "userPoolDomain": "https://portal-users.auth.eu-west-1.amazoncognito.com"
            }
        },
        "endpointConfiguration": {
            "acmManaged": {
                "domainName": "developers.example.com",
                "certificateArn": format!(
                    "arn:aws:acm:{REGION}:000000000000:certificate/\
                     11111111-2222-3333-4444-555555555555"
                )
            }
        },
        "portalContent": {
            "displayName": "Brought Domain Portal",
            "description": "the catalogue of our public APIs",
            "theme": { "customColors": custom_colors() }
        }
    })
}

fn rows() -> &'static [Row] {
    &[
        Row {
            case: "created through this service's own AWS host, on the default domain",
            endpoint_host: "apigateway.eu-west-1.amazonaws.com",
            request: default_domain_portal,
        },
        Row {
            case: "created through the emulator's own endpoint, on a domain the account \
                   brought",
            endpoint_host: "127.0.0.1:9297",
            request: brought_domain_portal,
        },
        Row {
            case: "created through a loopback name with a port, on the default domain",
            endpoint_host: "localhost:9297",
            request: default_domain_portal,
        },
    ]
}

/// One control-plane call of this row, addressed at the endpoint the row names.
async fn control(
    emulator: &MultiServiceRouter,
    row: &Row,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_at_host(emulator, row.endpoint_host, REGION, method, uri, body).await
}

/// Assert `address` is an address on THIS machine that a client can open a connection to:
/// what the portal's site is served at has to be dialable.
fn assert_dialable(case: &str, address: &str) {
    let parsed = address
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected the portal's site to be served at an address a \
                 client can dial, got {address:?}: {error}"
            )
        });
    assert!(
        parsed.ip().is_loopback(),
        "case {case:?}: expected an address of this machine, got {address:?}"
    );
    assert_ne!(
        parsed.port(),
        0,
        "case {case:?}: expected a port a client can dial, got {address:?}"
    );
}

/// The `host:port` the portal's own site is served at, read off the URL the portal announces
/// its preview at (`Preview$PreviewUrl`, the one URL-typed member that carries that address).
fn site_address(case: &str, preview_url: &str) -> String {
    let authority = preview_url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(authority, _path)| authority.to_string())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an https preview URL with a path, got \
                 {preview_url:?}"
            )
        });
    assert_dialable(case, &authority);
    authority
}

/// Start a preview on `portal_id` and answer the `Preview$PreviewUrl` the portal reports
/// once that build has SETTLED — the one URL-typed member that names the address this
/// emulator serves the portal's site at.
///
/// The wait is for the CONDITION (the preview reporting itself ready), never for a fixed
/// time, and it is waited out before anything else is asked of the portal, because a build in
/// flight is what a portal refuses another build for.
async fn preview_url_once_ready(emulator: &MultiServiceRouter, row: &Row, case: &str, portal_id: &str) -> String {
    let (status, body) = control(
        emulator,
        row,
        "POST",
        &format!("/v2/portals/{portal_id}/preview"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from PreviewPortal, got {status} {body}"
    );
    let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
    loop {
        let (status, read) = control(
            emulator,
            row,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal, got {status} {read}"
        );
        let preview = read.get("preview").cloned().unwrap_or(Value::Null);
        if as_str(&preview, "previewStatus") == PREVIEW_READY {
            return as_str(&preview, "previewUrl");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the preview of {portal_id} never settled, last read {read}"
        );
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

/// The `Preview$PreviewStatus` of a preview whose build has come due
/// (`smithy_query(query="shape", shape="PreviewStatus")`).
const PREVIEW_READY: &str = "PREVIEW_READY";

/// Dial `address` for `path`, addressed by `host`, until `holds` is true of the answer, and
/// answer that probe. `waited_for` names the condition in the failure message.
///
/// The wait is for the CONDITION, never for a fixed time: the address is asked again until the
/// condition holds or [`CONDITION_DEADLINE`] passes, so neither a fast nor a loaded host
/// changes what the case observes.
async fn probe_until(
    case: &str,
    address: &str,
    host: &str,
    path: &str,
    waited_for: &str,
    holds: impl Fn(&Answer) -> bool,
) -> Answer {
    let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
    loop {
        let answer = request_as(case, address, path, host).await;
        if holds(&answer) {
            return answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {host}{path} never answered {waited_for}, last answer \
             {answer:?}"
        );
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

/// Assert one probe is the portal's own page: the document, as the portal's site, naming the
/// portal it was built for.
fn assert_page(case: &str, answer: &Answer, portal_id: &str) {
    assert_eq!(
        answer.status, SERVED,
        "case {case:?}: expected the portal's page, got {answer:?}"
    );
    assert!(
        answer
            .body
            .contains(&format!("data-portal-id=\"{portal_id}\"")),
        "case {case:?}: the page must name the portal it was built for, got {answer:?}"
    );
}

/// Whether a client can open a connection to `address` right now.
///
/// The port is the one the portal's site is served at and the host is the loopback literal
/// this emulator's listeners bind, so nothing here dials off this machine. Answers as soon as
/// the connection is accepted or refused; [`CONDITION_DEADLINE`] is only the bound that keeps
/// a dial nothing ever answers from hanging the suite.
async fn connects(address: &str) -> bool {
    let Ok(address) = address.parse::<std::net::SocketAddr>() else {
        return false;
    };
    let dialed = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, address.port()));
    matches!(
        tokio::time::timeout(CONDITION_DEADLINE, dialed).await,
        Ok(Ok(_))
    )
}

/// Wait until `address` accepts no connection any more, and answer whether it got there.
///
/// Closing the socket is work the service carries out AFTER it has answered the call that
/// freed it, so the condition a case asserts is that the address STOPS being accepted — and it
/// is waited out by dialling again until that holds or [`CONDITION_DEADLINE`] passes, never by
/// dialling once at the instant the delete answered, which is what makes the same outcome
/// reached on a loaded host as on an idle one. Nothing here is timed.
async fn refuses_connections_eventually(address: &str) -> bool {
    let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
    loop {
        if !connects(address).await {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

/// The whole flow a consumer of a published portal drives, over real connections to the
/// address the service itself reported, addressed by the name it published.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_dials_the_published_address_is_served_the_portals_site() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();

        let (status, created) =
            control(&emulator, row, "POST", "/v2/portals", Some((row.request)())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let endpoint = created
            .get("endpointConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        let published = as_str(&endpoint, "portalDefaultDomainName");
        crate::portal_lifecycle::assert_published_domain_name(case, &published, &portal_id);

        // The reader reports the same name, so a client that goes through `GetPortal` —
        // which is what a client that did not make the create does — addresses what the
        // create published.
        let (status, read) = control(
            &emulator,
            row,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal, got {status} {read}"
        );
        let read_published = as_str(
            read.get("endpointConfiguration").unwrap_or(&Value::Null),
            "portalDefaultDomainName",
        );
        assert_eq!(
            read_published, published,
            "case {case:?}: expected GetPortal to report the published name, got {read}"
        );

        // The ADDRESS that name is served at is the portal's own socket, which the portal
        // announces through the one URL-typed member that carries it: a preview is built
        // here so the portal reports one, and the preview's own page is asserted below.
        let preview_url = preview_url_once_ready(&emulator, row, case, &portal_id).await;
        let address = site_address(case, &preview_url);
        assert_eq!(
            preview_url,
            format!("https://{address}/{PORTAL_PREVIEW_SEGMENT}"),
            "case {case:?}: expected the preview URL on the address the site is served at, \
             got {preview_url:?}"
        );

        // Nothing has published this portal, so the published NAME answers — a connection is
        // accepted and an HTTP answer comes back — with the refusal a site with nothing live
        // earns, rather than with the page or with nothing at all.
        let answer = request_as(case, &address, "/", &published).await;
        assert_eq!(
            answer.status, FORBIDDEN,
            "case {case:?}: expected {FORBIDDEN} from the published name before the first \
             publish, got {answer:?}"
        );

        // A publish only STARTS the build, so the page appears once that build has come due.
        let (status, body) = control(
            &emulator,
            row,
            "POST",
            &format!("/v2/portals/{portal_id}/publish"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 from PublishPortal, got {status} {body}"
        );
        let answer = probe_until(
            case,
            &address,
            &published,
            "/",
            "the portal's page",
            |answer| answer.status == SERVED,
        )
        .await;
        assert_page(case, &answer, &portal_id);

        // A path the portal has no page at is its own class on that same name.
        let answer = request_as(case, &address, "/nowhere", &published).await;
        assert_eq!(
            answer.status, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} for a path the portal has no page at, got \
             {answer:?}"
        );

        // The preview the portal announced is served at its own path, once its build has
        // come due.
        let answer = probe_until(
            case,
            &address,
            &published,
            &format!("/{PORTAL_PREVIEW_SEGMENT}"),
            "the portal's preview",
            |answer| answer.status == SERVED,
        )
        .await;
        assert_page(&format!("{case}: the preview"), &answer, &portal_id);

        // The take-down REVOKES public access, so the very first probe after its 204 finds
        // nothing served — and every later probe answers the same.
        let (status, body) = control(
            &emulator,
            row,
            "DELETE",
            &format!("/v2/portals/{portal_id}/publish"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DisablePortal, got {status} {body}"
        );
        for probe in 1..=3 {
            let answer = request_as(case, &address, "/", &published).await;
            assert_eq!(
                answer.status, FORBIDDEN,
                "case {case:?}: probe {probe} after the disable must find nothing served, got \
                 {answer:?}"
            );
        }

        // And the address itself goes with the portal: a deleted portal's site is reached by
        // nobody, so the connection is not even accepted any more.
        let (status, body) = control(
            &emulator,
            row,
            "DELETE",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeletePortal, got {status} {body}"
        );
        // The listener's accept loop is torn down WITH the handle the delete dropped, which
        // the runtime completes after the 204 has been answered, so the wait is for that
        // CONDITION — the port refusing a dial — and not for the instant the delete returned,
        // which is only how fast the host happens to be.
        assert!(
            refuses_connections_eventually(&address).await,
            "case {case:?}: expected {address} to accept no connection once the portal is \
             deleted"
        );
    }
}

/// What a row of [`a_deleted_portals_address_stops_accepting_connections`] leaves the portal's
/// publication in before the portal itself is taken away.
#[derive(Debug, Clone, Copy)]
enum Publication {
    /// Nothing was ever published: the address answers the refusal a site with nothing live
    /// earns.
    NothingLive,
    /// A publish was accepted and its build has settled: the address serves the portal's page.
    Live,
    /// A publish was taken down again: the address is accepted and serves nothing.
    TakenDown,
}

/// The address a portal's site is served at accepts no connection once the portal is DELETED,
/// whatever its publication was doing when the delete took it away — because the socket's life
/// is the portal RECORD's, not the publication's.
///
/// The rows span that publication state, which is what the rule generalises over here: a
/// portal nothing published, one serving a settled publish, and one whose publication was
/// taken down again.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_portals_address_stops_accepting_connections() {
    let row = &rows()[0];
    for publication in [
        Publication::NothingLive,
        Publication::Live,
        Publication::TakenDown,
    ] {
        let case = &format!("a portal deleted with its publication {publication:?}");
        let emulator = emulator();

        let (status, created) =
            control(&emulator, row, "POST", "/v2/portals", Some(default_domain_portal())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let published = as_str(
            created
                .get("endpointConfiguration")
                .unwrap_or(&Value::Null),
            "portalDefaultDomainName",
        );
        let preview_url = preview_url_once_ready(&emulator, row, case, &portal_id).await;
        let address = site_address(case, &preview_url);

        if matches!(publication, Publication::Live | Publication::TakenDown) {
            let (status, body) = control(
                &emulator,
                row,
                "POST",
                &format!("/v2/portals/{portal_id}/publish"),
                Some(json!({})),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected 202 from PublishPortal, got {status} {body}"
            );
            let answer = probe_until(
                case,
                &address,
                &published,
                "/",
                "the portal's page",
                |answer| answer.status == SERVED,
            )
            .await;
            assert_page(case, &answer, &portal_id);
        }
        if matches!(publication, Publication::TakenDown) {
            let (status, body) = control(
                &emulator,
                row,
                "DELETE",
                &format!("/v2/portals/{portal_id}/publish"),
                None,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected 204 from DisablePortal, got {status} {body}"
            );
        }

        // The address is reached while the portal is still held, whatever its publication is:
        // that is what makes the assertion below about the DELETE rather than about an address
        // nothing ever answered.
        let answer = request_as(case, &address, "/", &published).await;
        let expected = match publication {
            Publication::Live => SERVED,
            Publication::NothingLive | Publication::TakenDown => FORBIDDEN,
        };
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected {expected} from the published name while the portal is \
             held, got {answer:?}"
        );

        let (status, body) = control(
            &emulator,
            row,
            "DELETE",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeletePortal, got {status} {body}"
        );
        assert!(
            refuses_connections_eventually(&address).await,
            "case {case:?}: expected {address} to accept no connection once the portal is \
             deleted"
        );
    }
}

/// One portal, one name and one address: every portal of the account publishes a name of its
/// own and is served at an address of its own, and each serves ITS own site rather than
/// another portal's.
#[tokio::test(flavor = "multi_thread")]
async fn each_portals_published_address_serves_only_its_own_site() {
    let case = "two portals of one account, both published";
    let emulator = emulator();
    let row = &rows()[0];

    // (portal id, the name it published, the address its site is served at)
    let mut portals: Vec<(String, String, String)> = Vec::new();
    for index in 0..2 {
        let (status, created) = control(
            &emulator,
            row,
            "POST",
            "/v2/portals",
            Some(json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content(&format!("Portal {index}"))
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let published = as_str(
            created
                .get("endpointConfiguration")
                .unwrap_or(&Value::Null),
            "portalDefaultDomainName",
        );
        crate::portal_lifecycle::assert_published_domain_name(case, &published, &portal_id);
        // The address that name is served at, read off the portal's own preview URL.
        let preview_url = preview_url_once_ready(&emulator, row, case, &portal_id).await;
        let address = site_address(case, &preview_url);
        let (status, body) = control(
            &emulator,
            row,
            "POST",
            &format!("/v2/portals/{portal_id}/publish"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 from PublishPortal, got {status} {body}"
        );
        portals.push((portal_id, published, address));
    }

    assert_ne!(
        portals[0].1, portals[1].1,
        "case {case:?}: expected each portal its own published name, got {portals:?}"
    );
    assert_ne!(
        portals[0].2, portals[1].2,
        "case {case:?}: expected each portal its own address, got {portals:?}"
    );
    for (portal_id, published, address) in &portals {
        let answer = probe_until(
            case,
            address,
            published,
            "/",
            "the portal's page",
            |answer| answer.status == SERVED,
        )
        .await;
        assert_page(case, &answer, portal_id);
        let other = portals
            .iter()
            .find(|(other_id, _, _)| other_id != portal_id)
            .map(|(other_id, _, _)| other_id.clone())
            .unwrap_or_default();
        assert!(
            !answer
                .body
                .contains(&format!("data-portal-id=\"{other}\"")),
            "case {case:?}: {published} must serve only its own portal's page, got {answer:?}"
        );
    }
}
