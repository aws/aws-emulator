// SPDX-License-Identifier: CC0-1.0

//! Every address this service ANNOUNCES for a caller to dial is an address this emulator is
//! REACHED at.
//!
//! The rule, one level above the recorded cases (an HTTP api whose `ApiEndpoint` and a portal
//! whose `PortalDefaultDomainName` named a host the client resolved nowhere):
//!
//! > A member that hands a caller an address to dial — `Api$ApiEndpoint`, "The URI of the API …
//! > The stage name is typically appended to this URI to form a complete path to a deployed API
//! > stage", and `EndpointConfigurationResponse$PortalDefaultDomainName`, the domain "generated
//! > and managed by API Gateway" that a portal's site is served on — is a value the caller
//! > DIALS. So this emulator fills it with an address it is reached at: the `host:port` of the
//! > listener serving that resource, or the endpoint the request arrived at, with whatever
//! > cannot be carried in the host (which resource of several a shared address serves) carried
//! > in the reserved data-plane path. A DNS name of the form AWS publishes is never announced:
//! > no resolver a client of this machine has answers for it — not in AWS's own zone, which
//! > leads away from this process, and not in a zone of this emulator's own — so announcing one
//! > announces an address nothing reaches.
//!
//! The rows span what the rule generalises over: the resource whose address is announced (an
//! HTTP api, a WEBSOCKET api, a portal), the operation that mints it (`CreateApi`, `ImportApi`,
//! `CreatePortal`) and whether this emulator is SERVED on a port at all — which is what decides
//! whether a resource gets a listener of its own or is named on the endpoint the request
//! reached. The recorded cases are two rows.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_at_host, emulator};
use crate::portal_lifecycle::portal_content;
use crate::published_endpoint_documented_host::{emulator_served_on_a_port, EMULATOR_HOST};

/// The Region every row signs for, and the one a second row signs for so no row can pass on a
/// hardcoded Region.
const REGION: &str = "us-west-2";
const OTHER_REGION: &str = "eu-west-1";

/// The longest a dial of an announced address waits for the connection to be accepted.
///
/// Generous on purpose: what is asserted is that the address ACCEPTS a connection at all, so a
/// slow or loaded host only takes longer to prove it.
const DIAL_DEADLINE: Duration = Duration::from_secs(30);

/// WHICH member of WHICH operation a row reads the announced address off.
#[derive(Clone, Copy, Debug)]
enum Announced {
    /// `CreateApi` with `ProtocolType=HTTP` -> `apiEndpoint`.
    HttpApi,
    /// `ImportApi` -> `apiEndpoint`, the same member minted by an import.
    ImportedApi,
    /// `CreateApi` with `ProtocolType=WEBSOCKET` -> `apiEndpoint`, whose data plane is a socket
    /// of the api's own.
    WebSocketApi,
    /// `CreatePortal` -> `endpointConfiguration.portalDefaultDomainName`.
    Portal,
}

/// One row: the announced member, the Region the calls are signed for, and the scheme the
/// announced value carries (none for a member that is not a URL).
struct Row {
    case: &'static str,
    announced: Announced,
    region: &'static str,
    scheme: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: an HTTP api's ApiEndpoint",
            announced: Announced::HttpApi,
            region: REGION,
            scheme: Some("https"),
        },
        Row {
            case: "the same member on an api minted by an import, in another Region",
            announced: Announced::ImportedApi,
            region: OTHER_REGION,
            scheme: Some("https"),
        },
        Row {
            case: "a WEBSOCKET api, whose data plane is a socket of its own",
            announced: Announced::WebSocketApi,
            region: REGION,
            scheme: Some("wss"),
        },
        Row {
            case: "the recorded case: a portal's default domain",
            announced: Announced::Portal,
            region: OTHER_REGION,
            scheme: None,
        },
    ]
}

/// The OpenAPI 3 definition the import row PUTs.
fn definition() -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": "announced-address", "version": "1.0" },
        "paths": {
            "/items": {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "GET",
                        "uri": "https://upstream.example.com/items",
                        "payloadFormatVersion": "1.0"
                    }
                }
            }
        }
    })
    .to_string()
}

/// The body a portal row creates its portal with.
fn portal_request() -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content("Announced Address Portal")
    })
}

/// Mint the row's resource through `host` and answer `(the resource's id, the announced
/// address)`.
async fn announced(emulator: &MultiServiceRouter, row: &Row, host: &str) -> (String, String) {
    let case = row.case;
    match row.announced {
        Announced::HttpApi | Announced::WebSocketApi => {
            let websocket = matches!(row.announced, Announced::WebSocketApi);
            let mut request = json!({
                "name": "announced-address",
                "protocolType": match websocket {
                    true => "WEBSOCKET",
                    false => "HTTP",
                }
            });
            if websocket {
                // A WEBSOCKET api selects its route from the frame it was sent, so the create
                // states the expression: "the route selection expression for the API. For HTTP
                // APIs, the routeSelectionExpression must be ${request.method} ${request.path}"
                // (`CreateApi$RouteSelectionExpression`), and a WebSocket api's is its own.
                request["routeSelectionExpression"] = json!("$request.body.action");
            }
            let (status, created) = call_at_host(
                emulator,
                host,
                row.region,
                "POST",
                "/v2/apis",
                Some(request),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateApi must answer 201, got {status} {created}"
            );
            (as_str(&created, "apiId"), as_str(&created, "apiEndpoint"))
        }
        Announced::ImportedApi => {
            let (status, imported) = call_at_host(
                emulator,
                host,
                row.region,
                "PUT",
                "/v2/apis",
                Some(json!({ "body": definition() })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: ImportApi must answer 200, got {status} {imported}"
            );
            (as_str(&imported, "apiId"), as_str(&imported, "apiEndpoint"))
        }
        Announced::Portal => {
            let (status, created) = call_at_host(
                emulator,
                host,
                row.region,
                "POST",
                "/v2/portals",
                Some(portal_request()),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreatePortal must answer 201, got {status} {created}"
            );
            let endpoint = created
                .get("endpointConfiguration")
                .cloned()
                .unwrap_or(Value::Null);
            (
                as_str(&created, "portalId"),
                as_str(&endpoint, "portalDefaultDomainName"),
            )
        }
    }
}

/// The announced value split into the authority a client dials and the path it carries — the
/// tree's one reading of a published endpoint
/// (`published_endpoint_documented_host::published_parts`), which every suite that drives such
/// a value splits it with.
fn parts(announced: &str) -> (String, String) {
    crate::published_endpoint_documented_host::published_parts(announced)
}

/// The host of `authority`, unbracketed and without the port a published address carries: the
/// tree's one split of a published authority (`published_endpoint_documented_host::host_of`)
/// with the brackets an IPv6 literal carries removed, so the result parses as the address it
/// names.
fn host_of(authority: &str) -> &str {
    crate::published_endpoint_documented_host::host_of(authority)
        .trim_matches(|character| character == '[' || character == ']')
}

/// Whether `host` is one a client running on this machine DIALS as it stands: an address
/// literal of this machine's own interfaces, or the one name RFC 6761 reserves for the loopback
/// interface, which every resolver answers. A name in any other zone is one such a client
/// resolves nowhere — which is what makes publishing it publishing nothing.
///
/// `host` comes unbracketed from [`host_of`], so an IPv6 literal parses as the address it is.
/// Wider than the reading the suites about a SOCKET-published address apply
/// (`published_endpoint_documented_host`, which admits an address literal alone), because the
/// rows below also cover the value announced on the endpoint a request REACHED: that authority
/// is the one the client itself wrote, which may be the loopback name.
fn dialable_on_this_machine(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok() || host.eq_ignore_ascii_case("localhost")
}

/// RULE: on an emulator served on a port, every announced address names a host a client of this
/// machine dials, carries the port of the listener that serves the resource, and ACCEPTS a
/// connection there.
#[tokio::test(flavor = "multi_thread")]
async fn every_announced_address_is_one_a_client_of_this_machine_can_dial() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator_served_on_a_port();
        let (resource_id, announced) = announced(&emulator, &row, EMULATOR_HOST).await;
        assert!(
            !announced.is_empty(),
            "case {case:?}: every announced address is a value, got {announced:?}"
        );
        if let Some(scheme) = row.scheme {
            assert!(
                announced.starts_with(&format!("{scheme}://")),
                "case {case:?}: expected the {scheme} scheme the resource is reached over, got \
                 {announced:?}"
            );
        }
        let (authority, path) = parts(&announced);
        let host = host_of(&authority);
        assert!(
            dialable_on_this_machine(host),
            "case {case:?}: expected a host a client of this machine dials, got {host:?} in \
             {announced:?}"
        );
        // The PORT a client dials, read off the published authority the way every suite that
        // drives one of this service's published addresses reads it — it panics, case-labelled,
        // for an authority naming no port or a zero one.
        let port = crate::published_invoke_socket_front_door::dialable_port(case, &authority);
        // A shared address names WHICH resource it serves in the reserved data-plane path; a
        // listener of the resource's own needs no such segment, and then the value carries no
        // path at all.
        assert!(
            path.is_empty() || path.contains(&resource_id),
            "case {case:?}: expected the path of an announced address to name the resource \
             {resource_id:?}, got {announced:?}"
        );
        // And the address ANSWERS: a connection to the port it publishes is accepted on the
        // loopback interface its host was just asserted to name, which is what makes the value
        // an address rather than a label.
        let dialled = tokio::time::timeout(
            DIAL_DEADLINE,
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
        )
        .await;
        assert!(
            dialled.is_ok_and(|connected| connected.is_ok()),
            "case {case:?}: expected the announced address {announced:?} to accept a \
             connection at {authority}"
        );
    }
}

/// RULE, on an emulator that is served on NO port of its own (one driven in process): an api's
/// announced address names the endpoint the request REACHED this emulator at, with the api in
/// the path — never a name in a zone nothing on this machine resolves.
///
/// A resource the core opens a listener OF ITS OWN for — a WEBSOCKET api, whose RFC 6455
/// handshake is completed nowhere else, and a portal's website — is named on that listener
/// whether or not this process is served on a port, so those rows are the other test's.
#[tokio::test]
async fn an_announced_address_names_the_endpoint_the_request_reached() {
    // The endpoints a client that resolves nothing but this emulator reaches it at: an address
    // literal with a port, a loopback name with another port, and a bracketed IPv6 literal.
    let endpoints: &[&str] = &["127.0.0.1:40705", "localhost:9297", "[::1]:9297"];
    for endpoint in endpoints {
        for row in rows() {
            let case = row.case;
            if !matches!(row.announced, Announced::HttpApi | Announced::ImportedApi) {
                continue;
            }
            let emulator = emulator();
            let (resource_id, announced) = announced(&emulator, &row, endpoint).await;
            let (authority, path) = parts(&announced);
            assert!(
                dialable_on_this_machine(host_of(&authority)),
                "case {case:?} at {endpoint:?}: expected a host a client of this machine \
                 dials, got {announced:?}"
            );
            assert_eq!(
                authority, *endpoint,
                "case {case:?} at {endpoint:?}: expected the endpoint this request reached, \
                 got {announced:?}"
            );
            assert!(
                path.contains(&resource_id),
                "case {case:?} at {endpoint:?}: expected the path to name the resource \
                 {resource_id:?}, got {announced:?}"
            );
        }
    }
}
