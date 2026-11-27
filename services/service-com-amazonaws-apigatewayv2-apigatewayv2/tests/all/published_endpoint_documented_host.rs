// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ADDRESS an HTTP api publishes as its `ApiEndpoint`.
//!
//! The rule, one level above the recorded case (an HTTP api created through CloudFormation,
//! whose `ApiEndpoint` a client read out of the stack's outputs and dialled):
//!
//! > The URI an HTTP api publishes as its `ApiEndpoint` is one a client DIALS AS GIVEN — "The
//! > URI of the API, of the form `{api-id}.execute-api.{region}.amazonaws.com`. The stage name
//! > is typically appended to this URI to form a complete path to a deployed API stage"
//! > (`Api$ApiEndpoint`) — so its AUTHORITY is an address this emulator ANSWERS ON: the
//! > `host:port` of the socket that api's data plane is served on, which is an address literal
//! > carrying the port the core assigned. AWS's own form is a NAME and AWS answers for it; a
//! > name of this emulator's own minting is answered for by no resolver on the machine it runs
//! > on, so none is published. What names the api is then the PATH: its own id behind the
//! > reserved data-plane marker (`/_aws/execute-api/{api-id}`), the addressing this emulator
//! > serves an api's data plane at — so appending `/{stageName}/{routePath}` to the published
//! > value reaches the api's route.
//!
//! The rows span what the rule generalises over: the Region the api was created in (including
//! the emulator's own default, so no row can pass on a hardcoded Region), the operation that
//! MINTED the api (`CreateApi`, `ImportApi`) and the stage the invoke URL names (the reserved
//! `$default` stage that serves the api's root and a named one). The recorded case is ONE row.
//!
//! The other half of the same rule: `zz_temp_resolution_probe` holds that the published value
//! needs no resolution at all and that the api is SERVED under it. What the socket decides for
//! a request arriving there — the stage, the route, the gateway's own refusals — is
//! `published_invoke_socket_front_door`'s, and what the addressing AWS documents reaches at
//! every front door is `execute_api_documented_authority`'s; the DOCUMENTED host being answered
//! for at this very address, over these same rows, is `published_endpoint_dialable_address`'s.

use serde_json::{json, Value};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

use crate::api_lifecycle::{as_str, call_at_host};

/// The endpoint this emulator is taken to be served at, which is what makes it publish a
/// dialable address for an api's data plane at all: the api's plane binds a loopback port of
/// its own, and the authority here is never dialled.
const SERVED_AT: &str = "https://127.0.0.1:9297";

/// The endpoint the control-plane client is configured with — this emulator's own address,
/// which is what a client that resolves nothing else sends — and ANOTHER of its own addresses,
/// which a later read is made at.
pub(crate) const EMULATOR_HOST: &str = "127.0.0.1:9297";
const ANOTHER_EMULATOR_HOST: &str = "localhost:19999";

/// The marker the published path names an api's data plane behind, and the label behind it:
/// `/_aws/execute-api/{api-id}`, the addressing this emulator serves an api's data plane at.
pub(crate) const LOCAL_MARKER: &str = "/_aws/execute-api";

/// The upstream every row's `HTTP_PROXY` route fronts, and the status that integration answers:
/// reaching it is what proves a request resolved all the way to the route.
pub(crate) const UPSTREAM: &str = "https://upstream.example.com/items";
pub(crate) const SERVED: u16 = 307;

/// The route every api below carries, and the path within a stage that matches it.
const ROUTE_KEY: &str = "GET /items";
pub(crate) const ROUTE_PATH: &str = "/items";

/// The reserved stage "that serves an api's root", reached with no stage segment at all, and the
/// named stage beside it.
const DEFAULT_STAGE: &str = "$default";
pub(crate) const NAMED_STAGE: &str = "prod";

/// Which operation MINTS the api a row is about.
#[derive(Clone, Copy, Debug)]
pub(crate) enum MintedBy {
    /// `CreateApi`, over a name and a protocol type.
    Create,
    /// `ImportApi`, over an OpenAPI 3 definition carrying the same route.
    Import,
}

/// One row: the Region the api is created in and the operation that minted it.
pub(crate) struct Row {
    pub(crate) case: &'static str,
    pub(crate) region: &'static str,
    pub(crate) minted_by: MintedBy,
}

/// The rows both halves of this rule are asserted over.
pub(crate) fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: an api created in us-west-2",
            region: "us-west-2",
            minted_by: MintedBy::Create,
        },
        Row {
            case: "an api created in another Region",
            region: "eu-west-1",
            minted_by: MintedBy::Create,
        },
        Row {
            case: "an api created in the emulator's own default Region, signed for explicitly",
            region: "us-east-1",
            minted_by: MintedBy::Create,
        },
        Row {
            case: "an api minted by an import, in a Region of its own",
            region: "ap-southeast-2",
            minted_by: MintedBy::Import,
        },
    ]
}

/// One emulator served at [`SERVED_AT`], so an api's data plane is published on a socket of its
/// own rather than on the endpoint a control-plane request arrived at.
pub(crate) fn emulator_served_on_a_port() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.set_url_base(
        EndpointUrl::parse(SERVED_AT).expect("the endpoint base is a URL this emulator serves"),
    );
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine),
    );
    router
}

/// The OpenAPI 3 definition an import row PUTs: the same route over the same upstream, so what
/// the row is asserted on is the address the import publishes.
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

/// One minted api: the id the service assigned and the endpoint the minting operation reported.
pub(crate) struct Minted {
    pub(crate) api_id: String,
    pub(crate) api_endpoint: String,
}

/// Mint the row's api, give it the route every row serves and release both stages over it.
pub(crate) async fn minted(emulator: &MultiServiceRouter, row: &Row) -> Minted {
    let name = "published-endpoint-host-form";
    let (api_id, api_endpoint) = match row.minted_by {
        MintedBy::Create => {
            let (status, created) = call_at_host(
                emulator,
                EMULATOR_HOST,
                row.region,
                "POST",
                "/v2/apis",
                Some(json!({ "name": name, "protocolType": "HTTP" })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {:?}: CreateApi must answer 201, got {status} {created}",
                row.case
            );
            (as_str(&created, "apiId"), as_str(&created, "apiEndpoint"))
        }
        MintedBy::Import => {
            let (status, imported) = call_at_host(
                emulator,
                EMULATOR_HOST,
                row.region,
                "PUT",
                "/v2/apis",
                Some(json!({ "body": definition(name) })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: ImportApi must answer 200, got {status} {imported}",
                row.case
            );
            (as_str(&imported, "apiId"), as_str(&imported, "apiEndpoint"))
        }
    };

    // A created api still needs the route an import's definition carried.
    if matches!(row.minted_by, MintedBy::Create) {
        let (status, integration) = call_at_host(
            emulator,
            EMULATOR_HOST,
            row.region,
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
            "case {:?}: CreateIntegration must answer 201, got {status} {integration}",
            row.case
        );
        let integration_id = as_str(&integration, "integrationId");
        let (status, route) = call_at_host(
            emulator,
            EMULATOR_HOST,
            row.region,
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
            "case {:?}: CreateRoute must answer 201, got {status} {route}",
            row.case
        );
    }

    for stage_name in [DEFAULT_STAGE, NAMED_STAGE] {
        let (status, stage) = call_at_host(
            emulator,
            EMULATOR_HOST,
            row.region,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": stage_name, "autoDeploy": true })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateStage {stage_name:?} must answer 201, got {status} {stage}",
            row.case
        );
    }

    Minted {
        api_id,
        api_endpoint,
    }
}

/// The authority of a published endpoint — what a client resolves and dials — and the path it
/// carries ahead of the stage, empty for an endpoint that is a bare host.
pub(crate) fn published_parts(announced: &str) -> (String, String) {
    let rest = announced.split_once("://").map_or(announced, |(_scheme, rest)| rest);
    match rest.find('/') {
        Some(at) => (rest[..at].to_string(), rest[at..].to_string()),
        None => (rest.to_string(), String::new()),
    }
}

/// The host an authority names, without the port a published address carries.
pub(crate) fn host_of(authority: &str) -> &str {
    authority
        .rsplit_once(':')
        .map_or(authority, |(host, _port)| host)
}

/// RULE: the endpoint an HTTP api publishes carries, as its authority, an address this
/// emulator answers on — the address literal and port its data-plane socket landed on, which a
/// client dials as it stands — and names the api's own id behind the reserved data-plane path,
/// whichever Region the api was created in, whichever operation minted it and whichever
/// operation reports it.
#[tokio::test]
async fn a_published_endpoint_names_the_api_under_an_address_this_emulator_answers_on() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator_served_on_a_port();
        let api = minted(&emulator, &row).await;
        let (authority, path) = published_parts(&api.api_endpoint);
        let host = host_of(&authority);

        assert!(
            api.api_endpoint.starts_with("https://"),
            "case {case:?}: an HTTP api is invoked over https, got {:?}",
            api.api_endpoint
        );
        // The authority is an ADDRESS, not a name this service minted: a client resolves
        // nothing to reach it, and the interface is the one the core binds its listeners on.
        let address: std::net::IpAddr = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .unwrap_or_else(|_| {
                panic!(
                    "case {case:?}: expected the address the api's socket landed on, got {:?}",
                    api.api_endpoint
                )
            });
        assert!(
            address.is_loopback(),
            "case {case:?}: expected the interface the core binds its listeners on, got {:?}",
            api.api_endpoint
        );
        assert!(
            authority
                .rsplit_once(':')
                .is_some_and(|(_host, port)| port.parse::<u16>().is_ok_and(|port| port != 0)),
            "case {case:?}: a served emulator must publish the port its data plane is served \
             at, got {:?}",
            api.api_endpoint
        );
        assert_eq!(
            path,
            format!("{LOCAL_MARKER}/{}", api.api_id),
            "case {case:?}: expected the api's own id behind the reserved data-plane path, so \
             appending the stage name to the published value is the whole invoke path, got {:?}",
            api.api_endpoint
        );

        // Every reader reports the one value the minting operation published.
        let (status, fetched) = call_at_host(
            &emulator,
            EMULATOR_HOST,
            row.region,
            "GET",
            &format!("/v2/apis/{}", api.api_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApi must answer 200, got {status} {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "apiEndpoint"),
            api.api_endpoint,
            "case {case:?}: GetApi must report the published endpoint, got {fetched}"
        );
        let (status, listed) =
            call_at_host(&emulator, EMULATOR_HOST, row.region, "GET", "/v2/apis", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApis must answer 200, got {status} {listed}"
        );
        let listed_endpoint = listed
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("apiId").and_then(Value::as_str) == Some(&api.api_id))
            })
            .map(|item| as_str(item, "apiEndpoint"))
            .unwrap_or_default();
        assert_eq!(
            listed_endpoint, api.api_endpoint,
            "case {case:?}: GetApis must report the published endpoint, got {listed}"
        );
    }
}

/// RULE: the id the published path carries is the api's OWN, settled when the api was minted —
/// so two apis of one account are never published under one value, the only part that differs
/// between two of them is the id, the Region the api was created in moves no part of the
/// address a client dials, and neither a later request's Region and endpoint nor a rename moves
/// the value.
#[tokio::test]
async fn the_published_path_carries_the_apis_own_id() {
    let case = "two apis in one Region and one in another, on one emulator";
    let emulator = emulator_served_on_a_port();
    let first_region = rows()[0].region;
    let second_region = rows()[1].region;
    let reading_region = rows()[3].region;

    let first = minted(
        &emulator,
        &Row {
            case,
            region: first_region,
            minted_by: MintedBy::Create,
        },
    )
    .await;
    let second = minted(
        &emulator,
        &Row {
            case,
            region: first_region,
            minted_by: MintedBy::Create,
        },
    )
    .await;
    let other = minted(
        &emulator,
        &Row {
            case,
            region: second_region,
            minted_by: MintedBy::Create,
        },
    )
    .await;
    let host = |minted: &Minted| host_of(&published_parts(&minted.api_endpoint).0).to_string();

    // Two apis of one account are never published under one value, and the only part that
    // differs between two apis of the SAME Region is the one naming the api.
    assert_ne!(
        first.api_endpoint, second.api_endpoint,
        "case {case:?}: expected two apis published under two values, got {:?} for both",
        first.api_endpoint
    );
    assert_eq!(
        first.api_endpoint.replacen(&first.api_id, &second.api_id, 1),
        second.api_endpoint,
        "case {case:?}: expected the api's id to be the only difference, got {:?} and {:?}",
        first.api_endpoint,
        second.api_endpoint
    );
    // And the Region the api was created in moves no part of the published value: what a
    // client dials is the address this emulator serves that api's plane on, which belongs to
    // this process and not to a Region.
    assert_eq!(
        first.api_endpoint.replacen(&first.api_id, &other.api_id, 1),
        other.api_endpoint,
        "case {case:?}: expected the api's id to be the only difference across Regions, got \
         {:?} and {:?}",
        first.api_endpoint,
        other.api_endpoint
    );
    // The authority itself is one address of this emulator, dialable as it stands, for every
    // api it serves.
    for (which, minted) in [("first", &first), ("second", &second), ("other", &other)] {
        assert_eq!(
            host(minted),
            host(&first),
            "case {case:?}: expected the {which} api published on an address of this emulator, \
             got {:?}",
            minted.api_endpoint
        );
    }

    // A later read signed for a THIRD Region, addressed at ANOTHER endpoint of this emulator,
    // reports the value the create settled: the address is the api's, not the reader's.
    let (status, read) = call_at_host(
        &emulator,
        ANOTHER_EMULATOR_HOST,
        reading_region,
        "GET",
        &format!("/v2/apis/{}", first.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetApi must answer 200, got {status} {read}"
    );
    assert_eq!(
        as_str(&read, "apiEndpoint"),
        first.api_endpoint,
        "case {case:?}: a reader at another endpoint, in another Region, must report the \
         published endpoint, got {read}"
    );

    // Renaming the api moves nothing either: what the value carries is the api's id, never its
    // name.
    let renamed = "published-endpoint-renamed";
    let (status, updated) = call_at_host(
        &emulator,
        EMULATOR_HOST,
        first_region,
        "PATCH",
        &format!("/v2/apis/{}", first.api_id),
        Some(json!({ "name": renamed })),
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: UpdateApi must succeed, got {status} {updated}"
    );
    assert_eq!(
        updated.get("name").and_then(Value::as_str),
        Some(renamed),
        "case {case:?}: UpdateApi must rename the api, got {updated}"
    );
    assert_eq!(
        as_str(&updated, "apiEndpoint"),
        first.api_endpoint,
        "case {case:?}: a rename must keep the published endpoint, got {updated}"
    );
}

