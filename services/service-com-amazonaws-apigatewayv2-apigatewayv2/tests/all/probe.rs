// SPDX-License-Identifier: CC0-1.0

//! The READINESS PROBE a client runs against an endpoint this control plane announced:
//! the FIRST request to it already carries the status the endpoint's stored state implies.
//!
//! A client that creates an api, a stage or a portal and then calls the endpoint the
//! create reported polls that endpoint until it answers what it expects — that probe is how
//! every behavioural workflow begins. So the property these cases pin is the one a poll
//! cannot distinguish from a hang: the answer must be there on the first request, and it
//! must be the endpoint's OWN answer rather than the front door's `404 UnknownService` for
//! a request no service claimed.
//!
//! The rule these cases assert:
//!
//! > Every endpoint this service announces answers from the records the control plane
//! > stored, with no warm-up: an invoke URL whose route is `AWS_IAM` refuses an unsigned
//! > request `403` on the first probe, one whose route is `NONE` reaches that route's
//! > integration on the first probe, and a portal's domain serves its published build on
//! > the first probe once the publish the clock owed has settled. The same holds for the
//! > SECOND probe, with nothing called in between: a probe is a read.
//!
//! The rows span what the rule generalises over: which endpoint is probed (an invoke URL
//! and a portal domain), how the probe addresses it (the announced host, and the emulator's
//! own endpoint with the resource id in the path), the route's `AuthorizationType` (`NONE`
//! and `AWS_IAM`), whether the api carries a `CorsConfiguration`, and the form of the route
//! key (the greedy `ANY /{proxy+}` catch-all and a literal method and path).

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::PORTAL_BUILD;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::portal_lifecycle::portal_content;
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The emulator's own endpoint, the only address a client on this machine can reach.
const LOCAL_ENDPOINT: &str = "127.0.0.1:9297";

/// The status and sentence an `AWS_IAM` route answers an unsigned request with.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status a request whose authorizer refused it carries, which an ADMITTED request must
/// therefore not carry.
const UNAUTHORIZED: u16 = 401;

/// The status a request matching no route of the api carries — likewise not an admitted
/// request's.
const NOT_FOUND: u16 = 404;

/// The error code the front door answers when NO service claims a request: the answer a
/// poll can never converge on, and what every row here must not see.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// How long to wait so a portal build the service owes has certainly come due. A BLOCKING
/// wait on purpose — this workspace's `tokio` carries no `time` feature.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// What the FIRST probe of a route must observe.
///
/// A route whose authorization refuses the probe answers that refusal outright. For a route
/// that ADMITS it, this suite deliberately does not name a status: what an admitted request
/// is answered with is the integration's business (and is pinned, for an integration that
/// produced no response, by `http_api_integration_fault_status`) — what matters to a probe
/// is that the request got PAST authorization and was answered by the endpoint itself rather
/// than by the front door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// `403` `{"message":"Forbidden"}`, the refusal an `AWS_IAM` route owes an unsigned
    /// request.
    Forbidden,
    /// The route let the probe through to its integration.
    Admitted,
}

/// One probed invoke URL: the api as the client created it, and what the probe must observe.
struct Row {
    case: &'static str,
    /// The `CreateApi` body — whether the api carries a CORS configuration is part of it.
    api: Value,
    route_key: &'static str,
    /// The route's `AuthorizationType`, or `None` for a route that names none.
    authorization_type: Option<&'static str>,
    method: &'static str,
    route_path: &'static str,
    expected: Expected,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an AWS_IAM catch-all on an api with a CORS configuration, probed unsigned",
            api: json!({
                "name": "probe-cors-iam",
                "protocolType": "HTTP",
                "corsConfiguration": {
                    "allowOrigins": ["https://example.com"],
                    "allowMethods": ["GET", "OPTIONS"],
                    "allowHeaders": ["content-type"],
                    "maxAge": 600
                }
            }),
            route_key: "ANY /{proxy+}",
            authorization_type: Some("AWS_IAM"),
            method: "GET",
            route_path: "/orders",
            expected: Expected::Forbidden,
        },
        Row {
            case: "an AWS_IAM route on a literal method and path, probed unsigned",
            api: json!({ "name": "probe-iam-literal", "protocolType": "HTTP" }),
            route_key: "GET /open",
            authorization_type: Some("AWS_IAM"),
            method: "GET",
            route_path: "/open",
            expected: Expected::Forbidden,
        },
        Row {
            case: "a route open to everyone reaches its integration on the first probe",
            api: json!({ "name": "probe-open", "protocolType": "HTTP" }),
            route_key: "GET /open",
            authorization_type: Some("NONE"),
            method: "GET",
            route_path: "/open",
            expected: Expected::Admitted,
        },
        Row {
            case: "a route naming no authorization type at all is open too",
            api: json!({ "name": "probe-default-auth", "protocolType": "HTTP" }),
            route_key: "ANY /{proxy+}",
            authorization_type: Option::None,
            method: "POST",
            route_path: "/orders/7/items",
            expected: Expected::Admitted,
        },
    ]
}

/// Assert one probe observed what the api's records imply, and was answered by the endpoint
/// itself rather than by the front door.
fn probed(case: &str, answer: &Answer, expected: Expected) {
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the announced endpoint must be served by this service, got {}",
        answer.summary()
    );
    match expected {
        Expected::Forbidden => {
            assert_eq!(
                answer.status, FORBIDDEN,
                "case {case:?}: expected {FORBIDDEN} on the first probe, got {}",
                answer.summary()
            );
            assert_eq!(
                message(answer),
                FORBIDDEN_MESSAGE,
                "case {case:?}: expected the {FORBIDDEN_MESSAGE:?} body, got {}",
                answer.summary()
            );
        }
        Expected::Admitted => {
            // The probe got past the route's authorization and past route matching: what
            // the integration then answers is not this suite's subject, so only the
            // statuses that would mean it never got there are excluded.
            assert!(
                ![FORBIDDEN, UNAUTHORIZED, NOT_FOUND].contains(&answer.status),
                "case {case:?}: the probe must reach the route's integration on the first \
                 request, got {}",
                answer.summary()
            );
        }
    }
}

/// The first probe of an invoke URL already carries the answer the api's records imply, on
/// the announced host and on the emulator's own endpoint, and a second probe answers the
/// same.
#[tokio::test]
async fn the_first_probe_of_an_invoke_url_is_answered_from_the_apis_records() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();

        let (status, api) = call(&emulator, "POST", "/v2/apis", Some(row.api.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateApi must answer 201, got {status} {api}"
        );
        let api_id = as_str(&api, "apiId");
        let endpoint = as_str(&api, "apiEndpoint");
        let endpoint_host = endpoint
            .split_once("://")
            .map_or(endpoint.as_str(), |(_scheme, host)| host)
            .to_string();

        let (status, integration) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": format!(
                    "arn:aws:lambda:eu-west-1:000000000000:function:probe-{api_id}"
                ),
                "payloadFormatVersion": "2.0"
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
        );
        let integration_id = as_str(&integration, "integrationId");

        let mut route = json!({
            "routeKey": row.route_key,
            "target": format!("integrations/{integration_id}")
        });
        if let Some(authorization_type) = row.authorization_type {
            route["authorizationType"] = json!(authorization_type);
        }
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(route),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRoute must answer 201, got {status} {created}"
        );

        let (status, stage) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": "$default", "autoDeploy": true })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateStage must answer 201, got {status} {stage}"
        );

        // The announced host, and the same request at the emulator's own endpoint with the
        // api id carried in the path: the same records decide both.
        let probes = [
            ("the announced invoke URL", endpoint_host.clone(), row.route_path.to_string()),
            (
                "the emulator's own endpoint",
                LOCAL_ENDPOINT.to_string(),
                format!("/_aws/execute-api/{api_id}{}", row.route_path),
            ),
        ];
        for (where_, host, path) in probes {
            for attempt in 1..=2 {
                let answer = send(&emulator, &host, row.method, &path, &[]).await;
                probed(
                    &format!("{case} at {where_}, probe {attempt}"),
                    &answer,
                    row.expected,
                );
            }
        }
    }
}

/// The first probe of a portal's announced domain serves its published build, once the
/// publish the clock owed has settled — and nothing before that.
#[tokio::test]
async fn the_first_probe_of_a_portal_domain_is_answered_from_the_portals_records() {
    let case = "a portal published and then probed on its announced domain";
    let emulator = emulator();
    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("Probe Portal")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreatePortal must answer 201, got {status} {created}"
    );
    let portal_id = as_str(&created, "portalId");
    let domain = as_str(
        created
            .get("endpointConfiguration")
            .unwrap_or(&Value::Null),
        "portalDefaultDomainName",
    );

    // Before any publish the domain answers its own refusal rather than nothing at all.
    let answer = send(&emulator, &domain, "GET", "/", &[]).await;
    probed(
        &format!("{case}: before the publish"),
        &answer,
        Expected::Forbidden,
    );

    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/portals/{portal_id}/publish"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: PublishPortal must answer 202, got {status} {body}"
    );
    sleep(past_the_build());

    for attempt in 1..=2 {
        let answer = send(&emulator, &domain, "GET", "/", &[]).await;
        assert_ne!(
            answer.error_type.as_deref(),
            Some(UNKNOWN_SERVICE),
            "case {case:?}: probe {attempt} must be served by this service, got {}",
            answer.summary()
        );
        assert_eq!(
            answer.status, 200,
            "case {case:?}: probe {attempt} must serve the published portal, got {}",
            answer.summary()
        );
    }
}

/// A probe of an endpoint this service announced is never answered by the front door's
/// "no service claimed this" — the answer a poll cannot converge on — whatever state the
/// api is in.
#[tokio::test]
async fn an_announced_endpoint_is_claimed_in_every_state_of_the_api() {
    let emulator = emulator();
    let (status, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "probe-states", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(status, 201, "CreateApi must answer 201, got {status} {api}");
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    // Each step of the sequence a client runs, probed right after it: an api with no
    // stage, a stage with no deployment, and a deployed stage whose route matches nothing.
    let answer = send(&emulator, &host, "GET", "/orders", &[]).await;
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "an api with no stage still owns its announced endpoint, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.status, NOT_FOUND,
        "an api with no stage holds no stage the path could name, got {}",
        answer.summary()
    );

    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default" })),
    )
    .await;
    assert_eq!(status, 201, "CreateStage must answer 201, got {status} {stage}");
    let answer = send(&emulator, &host, "GET", "/orders", &[]).await;
    assert_eq!(
        answer.status, FORBIDDEN,
        "a stage with no deployment serves nothing, got {}",
        answer.summary()
    );

    let (status, deployment) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "stageName": "$default" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateDeployment must answer 201, got {status} {deployment}"
    );
    let answer = send(&emulator, &host, "GET", "/orders", &[]).await;
    assert_eq!(
        answer.status, 404,
        "a deployed stage whose routes match nothing answers its own 404, got {}",
        answer.summary()
    );
    assert_eq!(
        message(&answer),
        "Not Found",
        "that 404 is the endpoint's own, got {}",
        answer.summary()
    );
}
