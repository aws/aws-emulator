// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ADDRESSING `Api$ApiEndpoint` documents: the api's own
//! `execute-api` authority, with the stage name and route path appended.
//!
//! The rule, one level above any single api:
//!
//! > An `execute-api` front door answers for the api named by the FIRST DNS LABEL of the
//! > authority a request was addressed to — `{api-id}.execute-api.{region}.amazonaws.com`,
//! > the form `Api$ApiEndpoint` documents ("The URI of the API, of the form
//! > `{api-id}.execute-api.{region}.amazonaws.com`. The stage name is typically appended to
//! > this URI to form a complete path to a deployed API stage") — at EVERY address this
//! > emulator serves that plane on, so a client that resolves that name to this emulator and
//! > appends `/{stageName}/{routePath}` reaches the api's route. An authority whose leading
//! > label names no api of the account reaches no api, and the `execute-api` endpoint space
//! > answers it itself; so does one that names the api in a zone neither AWS publishes it in
//! > nor this emulator announces it in, because a name any other resolver could point at this
//! > address is the one the core's `Host` allowlist refuses.
//!
//! The rows span what the rule generalises over: the address the request is made to (the
//! `host:port` the api's own data-plane socket is published at, and this emulator's own front
//! door), the endpoint label (`execute-api` and the variant spelling AWS publishes beside it),
//! whether the client wrote the port it connected on, the stage the path names (the reserved
//! `$default` stage that serves the api's root, and a named one), and the authorities that
//! reach no api (an api id the account does not hold, a host that is no `execute-api` endpoint
//! at all, the api's name in a zone neither AWS publishes it in nor this emulator announces it
//! in — which only somebody else's resolver could point at this address — and a path that
//! would resolve out of the api); and
//! the operation that MINTED the api (`CreateApi`, `ImportApi`, `ReimportApi`).
//!
//! What an api's endpoint member ANNOUNCES is a different question — an announced address is
//! one the caller DIALS, so it is an address OF THIS EMULATOR
//! (`http_api_published_invoke_address`, `api_endpoint_documented_form`) — and this suite
//! asserts that the published address serves the documented addressing in AWS's own zone as
//! well, which is what a client that resolves the documented name needs.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

use crate::api_lifecycle::{as_str, call_at_host};
use crate::published_invoke_socket_front_door::{request, Answered};
use crate::websocket_handshake_data_plane::send;

/// The Region every api below is created in, and the Region label its documented authority
/// therefore carries.
const REGION: &str = "us-west-2";

/// The endpoint the control-plane client is configured with: this emulator's own address,
/// which is what a client that resolves nothing else sends.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The base URL this emulator reports as its own (`OperationContext::endpoint`), which is
/// what makes it an emulator that publishes dialable addresses at all.
const ENDPOINT_BASE: &str = "https://127.0.0.1:9297";

/// The upstream every row's `HTTP_PROXY` route fronts, and the status that integration
/// answers: reaching it is what proves the request resolved all the way to the route.
const UPSTREAM: &str = "https://upstream.example.com/items";
const SERVED: u16 = 307;

/// The route every api below deploys, and the path within a stage that matches it.
const ROUTE_KEY: &str = "GET /items";
const ROUTE_PATH: &str = "/items";

/// The reserved stage "that serves an api's root", reached with no stage segment at all, and
/// the named stage beside it.
const DEFAULT_STAGE: &str = "$default";
const NAMED_STAGE: &str = "prod";

/// What an `execute-api` front door answers a request it can place no `(api, stage)` for.
const FORBIDDEN: u16 = 403;

/// One emulator served at [`ENDPOINT_BASE`], so an api's data plane is published on a socket
/// of its own.
fn emulator_served_on_a_port() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.set_url_base(
        EndpointUrl::parse(ENDPOINT_BASE).expect("the endpoint base is a URL this emulator serves"),
    );
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine),
    );
    router
}

/// One deployed api: the id the service minted and the endpoint it published.
struct Deployed {
    api_id: String,
    api_endpoint: String,
}

/// Create one HTTP api with an `HTTP_PROXY` route and two auto-deploying stages — the
/// reserved `$default` stage and a named one — so both the api's root and a stage path under
/// it serve that route.
async fn deployed_api(emulator: &MultiServiceRouter, case: &str, name: &str) -> Deployed {
    let (status, created) = call_at_host(
        emulator,
        EMULATOR_HOST,
        REGION,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {created}"
    );
    let api_id = as_str(&created, "apiId");
    let api_endpoint = as_str(&created, "apiEndpoint");

    let (status, integration) = call_at_host(
        emulator,
        EMULATOR_HOST,
        REGION,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationMethod": "GET",
            "integrationUri": UPSTREAM,
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    let (status, route) = call_at_host(
        emulator,
        EMULATOR_HOST,
        REGION,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": ROUTE_KEY,
            "target": format!("integrations/{integration_id}")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {status} {route}"
    );

    for stage_name in [DEFAULT_STAGE, NAMED_STAGE] {
        let (status, stage) = call_at_host(
            emulator,
            EMULATOR_HOST,
            REGION,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": stage_name, "autoDeploy": true })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateStage {stage_name:?} must answer 201, got {status} {stage}"
        );
    }

    Deployed {
        api_id,
        api_endpoint,
    }
}

/// The authority of an announced endpoint — what a client dials — and the path it carries
/// ahead of the stage, empty for an endpoint that is a bare host.
fn announced_parts(announced: &str) -> (String, String) {
    let without_scheme = announced
        .split_once("://")
        .map_or(announced, |(_scheme, rest)| rest);
    match without_scheme.find('/') {
        Some(at) => (
            without_scheme[..at].to_string(),
            without_scheme[at..].to_string(),
        ),
        None => (without_scheme.to_string(), String::new()),
    }
}

/// Dial `authority` — one TCP connection to this machine's own loopback address, which is
/// what a client that resolved a name to this emulator does — send `GET path` naming `host`,
/// and read the whole answer back.
///
/// The wire work is the ONE raw HTTP/1.1 client the suites that drive this service's
/// core-owned addresses share (`published_invoke_socket_front_door::request`), so none of them
/// can read an answer differently from another: one request per connection,
/// `Connection: close`, read to the end of the stream under that module's generous deadline.
/// Nothing here waits on a clock.
async fn dialled(case: &str, authority: &str, host: &str, path: &str) -> Answered {
    request(case, authority, "GET", path, host, &[]).await
}

/// RULE: the address an HTTP api's endpoint is PUBLISHED at serves the api addressed the way
/// `Api$ApiEndpoint` documents — the api's own `execute-api` authority in the `Host`, the
/// stage name and route path appended — as well as the address it publishes, and it reaches
/// no api for an authority that names none.
#[tokio::test]
async fn the_published_address_serves_the_documented_execute_api_authority() {
    let emulator = emulator_served_on_a_port();
    let case = "a client holding the published invoke address";
    let deployed = deployed_api(&emulator, case, "documented-authority").await;
    let (authority, published_path) = announced_parts(&deployed.api_endpoint);

    // The published address is an address OF THIS EMULATOR: the address the socket serving the
    // api's data plane landed on, carrying the port it answers on, with the api named behind
    // the reserved data-plane path — so a client dials it as it stands.
    assert_eq!(
        (authority.as_str(), published_path.as_str()),
        (
            format!(
                "127.0.0.1:{}",
                crate::published_invoke_socket_front_door::dialable_port(case, &authority)
            )
            .as_str(),
            format!("/_aws/execute-api/{}", deployed.api_id).as_str()
        ),
        "case {case:?}: the published endpoint must name an address of this emulator, got {:?}",
        deployed.api_endpoint
    );

    let api = deployed.api_id.as_str();
    let port = authority
        .rsplit_once(':')
        .map(|(_host, port)| port.to_string())
        .unwrap_or_default();
    // The rows: (case, the `Host` the client writes, the path it carries, the status).
    let rows: Vec<(String, String, String, u16)> = vec![
        (
            "the documented authority, the stage that serves the api's root".to_string(),
            format!("{api}.execute-api.{REGION}.amazonaws.com"),
            ROUTE_PATH.to_string(),
            SERVED,
        ),
        (
            "the documented authority with the stage name appended".to_string(),
            format!("{api}.execute-api.{REGION}.amazonaws.com"),
            format!("/{NAMED_STAGE}{ROUTE_PATH}"),
            SERVED,
        ),
        (
            // `execute-api-fips` is the variant spelling AWS publishes for the same
            // endpoint, which the data planes' one reading of such a host admits
            // (`service::data_plane::api_id_of_host`).
            "an endpoint VARIANT label in place of execute-api".to_string(),
            format!("{api}.execute-api-fips.{REGION}.amazonaws.com"),
            ROUTE_PATH.to_string(),
            SERVED,
        ),
        (
            // A name in any other zone is one only somebody else's resolver could point at
            // this address, which is the rebinding the core's own `Host` allowlist refuses;
            // this endpoint admits the api's name in the zone AWS publishes it in and no
            // other.
            "the same api named in a zone of somebody else's".to_string(),
            format!("{api}.execute-api.{REGION}.attacker.example.com"),
            ROUTE_PATH.to_string(),
            FORBIDDEN,
        ),
        (
            // The loopback zone is the one this emulator ANNOUNCES the api's endpoint in:
            // RFC 6761 reserves it and every name under it for the loopback interface the
            // core binds on, so no resolver somebody else controls can point it here either.
            "the same api named in the loopback zone".to_string(),
            format!("{api}.execute-api.{REGION}.localhost"),
            ROUTE_PATH.to_string(),
            SERVED,
        ),
        (
            "the same api named deeper inside the loopback zone".to_string(),
            format!("{api}.execute-api.{REGION}.sub.localhost"),
            ROUTE_PATH.to_string(),
            FORBIDDEN,
        ),
        (
            "the documented authority carrying the port the client connected on".to_string(),
            format!("{api}.execute-api.{REGION}.amazonaws.com:{port}"),
            format!("/{NAMED_STAGE}{ROUTE_PATH}"),
            SERVED,
        ),
        (
            "the address as published, which keeps serving what it served".to_string(),
            authority.clone(),
            format!("{published_path}{ROUTE_PATH}"),
            SERVED,
        ),
        (
            "an api id the account does not hold".to_string(),
            format!("notthisapi0.execute-api.{REGION}.amazonaws.com"),
            ROUTE_PATH.to_string(),
            FORBIDDEN,
        ),
        (
            "a host that is no execute-api endpoint at all".to_string(),
            "attacker.example.com".to_string(),
            ROUTE_PATH.to_string(),
            FORBIDDEN,
        ),
        (
            "a path under the documented authority that would resolve out of the api"
                .to_string(),
            format!("{api}.execute-api.{REGION}.amazonaws.com"),
            format!("{ROUTE_PATH}/../v2/apis"),
            FORBIDDEN,
        ),
    ];

    for (case, host, path, expected) in rows {
        let served = dialled(&case, &authority, &host, &path).await;
        assert_eq!(
            served.status, expected,
            "case {case:?}: GET {host}{path} must answer {expected}, got {served:?}"
        );
        if expected == SERVED {
            assert_eq!(
                served.field("location"),
                Some(UPSTREAM),
                "case {case:?}: GET {host}{path} must be sent to the upstream the route's \
                 integration names, got {served:?}"
            );
        }
    }
}

/// RULE, at this emulator's OWN front door: the same documented addressing reaches the same
/// api there, because the api is named by the authority's leading label wherever the plane is
/// served.
#[tokio::test]
async fn the_front_door_serves_the_documented_execute_api_authority() {
    let emulator = crate::api_lifecycle::emulator();
    let case = "a client that resolves the documented name to this emulator";
    let deployed = deployed_api(&emulator, case, "documented-authority-front-door").await;
    let api = deployed.api_id.as_str();

    // The rows span the same endpoint labels and stages as the socket's, plus the authority
    // that names no api of the account.
    let rows: Vec<(String, String, String, u16)> = vec![
        (
            "the documented authority, the stage that serves the api's root".to_string(),
            format!("{api}.execute-api.{REGION}.amazonaws.com"),
            ROUTE_PATH.to_string(),
            SERVED,
        ),
        (
            "the documented authority with the stage name appended".to_string(),
            format!("{api}.execute-api.{REGION}.amazonaws.com"),
            format!("/{NAMED_STAGE}{ROUTE_PATH}"),
            SERVED,
        ),
        (
            "an endpoint VARIANT label in place of execute-api".to_string(),
            format!("{api}.execute-api-fips.{REGION}.amazonaws.com"),
            ROUTE_PATH.to_string(),
            SERVED,
        ),
        (
            "an api id the account does not hold".to_string(),
            format!("notthisapi0.execute-api.{REGION}.amazonaws.com"),
            ROUTE_PATH.to_string(),
            FORBIDDEN,
        ),
    ];

    for (case, host, path, expected) in rows {
        let answer = send(&emulator, &host, "GET", &path, &[]).await;
        assert_eq!(
            answer.status,
            expected,
            "case {case:?}: GET {host}{path} must answer {expected}, got {}",
            answer.summary()
        );
        if expected == SERVED {
            assert_eq!(
                answer.location.as_deref(),
                Some(UPSTREAM),
                "case {case:?}: GET {host}{path} must be sent to the upstream the route's \
                 integration names, got {}",
                answer.summary()
            );
        }
    }
}

/// The OpenAPI 3 definition the import rows PUT: one route fronting the same upstream, so
/// what an import and a re-import are asserted on is the address they publish and serve.
fn definition(name: &str) -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": name, "version": "1.0" },
        "paths": {
            ROUTE_PATH: {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "GET",
                        "uri": UPSTREAM,
                        "payloadFormatVersion": "1.0"
                    }
                }
            }
        }
    })
    .to_string()
}

/// RULE: an api MINTED BY AN IMPORT publishes and serves the same addressing as one created
/// through `CreateApi`, and a RE-IMPORT keeps the api's identity — so the address published
/// before the re-import is the address published after it, and the documented authority of
/// that api serves its route throughout.
#[tokio::test]
async fn an_imported_and_reimported_api_keeps_one_served_invoke_address() {
    let emulator = emulator_served_on_a_port();
    let case = "an api minted by ImportApi";

    let (status, imported) = call_at_host(
        &emulator,
        EMULATOR_HOST,
        REGION,
        "PUT",
        "/v2/apis",
        Some(json!({ "body": definition("documented-authority-import") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ImportApi must answer 200, got {status} {imported}"
    );
    let api_id = as_str(&imported, "apiId");
    let imported_endpoint = as_str(&imported, "apiEndpoint");

    // An imported api carries no stage, so one is released over it.
    let (status, stage) = call_at_host(
        &emulator,
        EMULATOR_HOST,
        REGION,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": DEFAULT_STAGE, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );

    let (authority, _published_path) = announced_parts(&imported_endpoint);
    let documented = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
    let served = dialled(case, &authority, &documented, ROUTE_PATH).await;
    assert_eq!(
        served.status, SERVED,
        "case {case:?}: GET {documented}{ROUTE_PATH} must reach the imported route, got \
         {served:?}"
    );

    // A RE-IMPORT keeps the api's identity, so it keeps the one published address.
    let case = "the same api after a re-import";
    let (status, reimported) = call_at_host(
        &emulator,
        EMULATOR_HOST,
        REGION,
        "PUT",
        &format!("/v2/apis/{api_id}"),
        Some(json!({ "body": definition("documented-authority-reimport") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReimportApi must answer 200, got {status} {reimported}"
    );
    assert_eq!(
        as_str(&reimported, "apiId"),
        api_id,
        "case {case:?}: a re-import keeps the api it re-imported into, got {reimported}"
    );
    assert_eq!(
        as_str(&reimported, "apiEndpoint"),
        imported_endpoint,
        "case {case:?}: a re-import must keep the invoke address the import published, got \
         {reimported}"
    );

    // And that address still serves the api at its documented authority.
    let served = dialled(case, &authority, &documented, ROUTE_PATH).await;
    assert_eq!(
        served.status, SERVED,
        "case {case:?}: GET {documented}{ROUTE_PATH} must reach the re-imported route, got \
         {served:?}"
    );
}

/// The api the published endpoint names is the one a reader reports, so what this suite
/// dialled is the api the control plane holds: `GetApi` reports the same endpoint value.
#[tokio::test]
async fn the_published_endpoint_is_the_one_every_reader_reports() {
    let emulator = emulator_served_on_a_port();
    let case = "the endpoint read back through GetApi";
    let deployed = deployed_api(&emulator, case, "documented-authority-read-back").await;

    let (status, fetched) = call_at_host(
        &emulator,
        EMULATOR_HOST,
        REGION,
        "GET",
        &format!("/v2/apis/{}", deployed.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetApi must answer 200, got {status} {fetched}"
    );
    assert_eq!(
        as_str(&fetched, "apiEndpoint"),
        deployed.api_endpoint,
        "case {case:?}: GetApi must report the endpoint the create published, got {fetched}"
    );
    assert_eq!(
        as_str(&fetched, "apiId"),
        deployed.api_id,
        "case {case:?}: GetApi must report the api the create minted, got {fetched}"
    );
    // And the value is a URL whose authority a client can dial as it stands, with the api
    // NAMED in the path under it, which is what makes the addressing above reachable at all.
    let (authority, path) = announced_parts(&deployed.api_endpoint);
    assert!(
        authority.starts_with("127.0.0.1:") && authority.contains(':'),
        "case {case:?}: the published endpoint must name an address this emulator answers on, \
         got {:?}",
        deployed.api_endpoint
    );
    assert_eq!(
        path,
        format!("/_aws/execute-api/{}", deployed.api_id),
        "case {case:?}: the published endpoint names the api in its path, so appending the \
         stage name to it is the whole invoke path, got {:?}",
        deployed.api_endpoint
    );
}
