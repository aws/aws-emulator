// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ADDRESS an exported definition publishes, and for the
//! refusal of a `Specification` this operation does not serve.
//!
//! Two rules, each one level above the recorded case (an HTTP api created through a client
//! configured with this emulator's own endpoint and then exported, and an export asking for
//! a specification nothing serves):
//!
//! > The one Server Object of an exported definition names the address THIS EMULATOR SERVES
//! > the api's data plane at FOR THE CLIENT THAT ASKED FOR THE DOCUMENT, followed by the
//! > `{basePath}` template it declares as a server variable. A Server Object is an address a
//! > reader DIALS: the document is handed to code generators, importers and mock servers, so
//! > the authority it states is one this emulator answers on — the endpoint the export request
//! > reached, with the api behind the reserved data-plane path served there, and the
//! > `{api-id}.execute-api.{region}.amazonaws.com` host `Api$ApiEndpoint` documents for a
//! > client that resolves AWS's own hostnames, which this emulator claims host-style. Whichever
//! > it is, the address NAMES the api, substituting the variable yields an address that SERVES
//! > the exported configuration, and the document declares the import/export algorithm version
//! > it was written under.
//!
//! > A request naming a `Specification` outside the one value this operation serves is
//! > refused `400 BadRequestException` with API Gateway's own sentence for that refusal,
//! > which names the one specification version an export is written in and quotes nothing
//! > out of the request — so every unsupported spelling is refused with the same text.
//!
//! The first table's rows span what that rule generalises over: the endpoint the EXPORT was
//! addressed at and the endpoint the CREATE was addressed at (an IPv4 loopback literal with a
//! port, a loopback NAME on another port, a bracketed IPv6 literal, and this service's own AWS
//! host, in either position — the document follows the client that asked for it, whichever
//! endpoint minted the api), and the Region the api was created in (including the emulator's
//! own default, so no row passes on a hardcoded Region). The second table's rows span the SHAPE
//! of the rejected value: a later specification version, the served value in another case,
//! another specification family, the served value with a suffix, and a long value — in both
//! output types.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_export::UNSUPPORTED_SPECIFICATION_MESSAGE;
use crate::api_lifecycle::{as_str, call_at_host, emulator, error_code};
use crate::websocket_handshake_data_plane::{send, Answer};

/// The specification an export asks for: "OAS30, for OpenAPI 3.0, is the only supported
/// value" (`ExportApi$Specification`).
const OAS30: &str = "OAS30";

/// The extension an exported definition declares the import/export algorithm version under,
/// and the version this service exports under: "Currently, the only supported version is
/// 1.0" (`ExportApi$ExportVersion`).
const IMPORTEXPORT_VERSION_EXTENSION: &str = "x-amazon-apigateway-importexport-version";
const IMPORTEXPORT_VERSION: &str = "1.0";

/// The route every row's api carries, the path within the api that matches it, and the
/// upstream its integration fronts. An `HTTP_PROXY` route answers `307` with that upstream in
/// `Location`, so reaching it is how a case proves the advertised address SERVES the exported
/// configuration.
const ROUTE_KEY: &str = "GET /orders";
const ROUTE_PATH: &str = "/orders";
const UPSTREAM: &str = "https://upstream.example.com/orders";
const SERVED: u16 = 307;

/// The reserved stage "that serves an api's root", reached with no stage segment at all —
/// which is the base path the export of the api's latest configuration templates.
const DEFAULT_STAGE: &str = "$default";

/// The front door's envelope for a request no service claimed: what an address this emulator
/// does not answer on is answered with, and never a legitimate answer for an address an
/// artifact advertised.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The reserved path segment an api's data plane is served under on an address of this
/// emulator — `/_aws/execute-api/{api-id}`, the addressing `crate::service::invoke` serves.
const LOCAL_MARKER: &str = "/_aws/execute-api";

/// The DEFAULT execute-api address of `api_id` in `region`, in the form `Api$ApiEndpoint`
/// documents: "The URI of the API, of the form `{api-id}.execute-api.{region}.amazonaws.com`".
/// Written out here rather than taken from the service, so the table asserts the rule rather
/// than the implementation.
fn execute_api_url(api_id: &str, region: &str) -> String {
    format!("https://{api_id}.execute-api.{region}.amazonaws.com")
}

/// WHERE the client that asked for a row's document reached this emulator, which is what the
/// address that document advertises follows.
#[derive(Debug, PartialEq, Eq)]
enum ReachedAt {
    /// A client configured with one local endpoint URL, which resolves the emulator's own
    /// address and nothing else: the api is served on that same address, with its own id
    /// behind the reserved data-plane segment.
    TheEmulatorsOwnEndpoint,
    /// A client that resolves AWS's own hosts: the api is named on its own documented
    /// `execute-api` host, which this emulator claims host-style.
    AwsOwnHost,
}

/// One row: which endpoint the create and the export were addressed at, and the Region the
/// api was created in.
struct Row {
    case: &'static str,
    /// The `Host` of the control-plane calls that MINT the api.
    create_host: &'static str,
    /// The `Host` of the export request itself, and where that client reached this emulator.
    export_host: &'static str,
    export_reached: ReachedAt,
    /// The Region every call of the row is signed for, and therefore the api's own.
    region: &'static str,
}

impl Row {
    /// The authority the row's exported document must advertise, and the path that names the
    /// api on it ahead of the base path.
    fn advertised(&self, api_id: &str) -> (String, String) {
        match self.export_reached {
            ReachedAt::TheEmulatorsOwnEndpoint => (
                self.export_host.to_string(),
                format!("{LOCAL_MARKER}/{api_id}"),
            ),
            ReachedAt::AwsOwnHost => (
                format!("{api_id}.execute-api.{}.amazonaws.com", self.region),
                String::new(),
            ),
        }
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: created and exported through the emulator's loopback \
                   endpoint",
            create_host: "127.0.0.1:35807",
            export_host: "127.0.0.1:35807",
            export_reached: ReachedAt::TheEmulatorsOwnEndpoint,
            region: "us-west-2",
        },
        Row {
            case: "created on the loopback NAME on another port, exported through AWS's own \
                   host",
            create_host: "localhost:9297",
            export_host: "apigateway.eu-west-1.amazonaws.com",
            export_reached: ReachedAt::AwsOwnHost,
            region: "eu-west-1",
        },
        Row {
            case: "created through AWS's own host, exported through a bracketed IPv6 loopback \
                   literal",
            create_host: "apigateway.ap-southeast-2.amazonaws.com",
            export_host: "[::1]:9297",
            export_reached: ReachedAt::TheEmulatorsOwnEndpoint,
            region: "ap-southeast-2",
        },
        Row {
            case: "the emulator's own default Region, signed for explicitly, exported at \
                   another local endpoint than the one that minted the api",
            create_host: "127.0.0.1:9297",
            export_host: "localhost:4566",
            export_reached: ReachedAt::TheEmulatorsOwnEndpoint,
            region: "us-east-1",
        },
        Row {
            case: "created and exported through AWS's own host",
            create_host: "apigateway.eu-north-1.amazonaws.com",
            export_host: "apigateway.eu-north-1.amazonaws.com",
            export_reached: ReachedAt::AwsOwnHost,
            region: "eu-north-1",
        },
    ]
}

/// Send one GET to `host`, signed for `region`, and answer `(status, body text)`.
///
/// An exported definition is a `@httpPayload` blob, so the answer is read as bytes rather
/// than through the JSON helper the rest of the suite's calls use.
async fn get_text(
    emulator: &MultiServiceRouter,
    host: &str,
    region: &str,
    uri: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{region}/apigateway/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
            ),
        )
        .body(Bytes::new())
        .expect("build the request");

    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    (status, String::from_utf8_lossy(&body).to_string())
}

/// Create one HTTP api with a route and a released `$default` stage for `row`, through the
/// endpoint that row's client reaches, and answer `(apiId, the apiEndpoint the create
/// published)`.
async fn created_api(emulator: &MultiServiceRouter, row: &Row) -> (String, String) {
    let (status, api) = call_at_host(
        emulator,
        row.create_host,
        row.region,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "published-export-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: expected 201 from CreateApi, got {status} {api}",
        row.case
    );
    let api_id = as_str(&api, "apiId");

    let (status, integration) = call_at_host(
        emulator,
        row.create_host,
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
        "case {:?}: expected 201 from CreateIntegration, got {status} {integration}",
        row.case
    );
    let target = format!("integrations/{}", as_str(&integration, "integrationId"));

    let (status, route) = call_at_host(
        emulator,
        row.create_host,
        row.region,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({ "routeKey": ROUTE_KEY, "target": target })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: expected 201 from CreateRoute, got {status} {route}",
        row.case
    );

    // The stage that serves the api's root, which is what makes the route servable at the
    // advertised address with no stage segment at all.
    let (status, stage) = call_at_host(
        emulator,
        row.create_host,
        row.region,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": DEFAULT_STAGE, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: expected 201 from CreateStage, got {status} {stage}",
        row.case
    );

    (api_id, as_str(&api, "apiEndpoint"))
}

/// The authority of `url` and the path it carries, with the `{basePath}` template of the one
/// Server Object already substituted with `base_path` — what a reader of the document DIALS.
///
/// The split itself is the one every suite that dials an address this service published uses
/// ([`crate::product_rest_endpoint_page_published_endpoint::dialed_parts`]); only the
/// substitution of the server variable belongs to this suite.
fn dialed(url: &str, base_path: &str) -> (String, String) {
    let resolved = url.replace("/{basePath}", &format!("/{base_path}"));
    crate::product_rest_endpoint_page_published_endpoint::dialed_parts(&resolved)
}

#[tokio::test]
async fn an_exported_document_states_the_address_this_emulator_serves_the_api_at() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case;
        let (api_id, published) = created_api(&emulator, &row).await;

        let (status, text) = get_text(
            &emulator,
            row.export_host,
            row.region,
            &format!("/v2/apis/{api_id}/exports/{OAS30}?outputType=JSON&includeExtensions=true"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ExportApi, got {status} {text}"
        );
        let document: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!("case {case:?}: the JSON output type must parse as JSON, got {error}: {text}")
        });

        // One server, naming the address this emulator serves the api at for the client that
        // asked for the document, with the base path templated.
        let servers = document
            .get("servers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            servers.len(),
            1,
            "case {case:?}: an exported document names one server, got {document}"
        );
        let (expected_authority, expected_api_path) = row.advertised(&api_id);
        let expected = format!("https://{expected_authority}{expected_api_path}/{{basePath}}");
        let url = document
            .pointer("/servers/0/url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            url, expected,
            "case {case:?}: servers[0].url must be the address the api is served at under the \
             templated base path, expected {expected:?}, got {url:?}"
        );

        // That address is the one the row's client can DIAL, and it NAMES the api: in the
        // host's first label on the api's own documented `execute-api` host, and in the
        // reserved data-plane path on an address of this emulator.
        let (authority, path) = dialed(url, "");
        assert_eq!(
            authority, expected_authority,
            "case {case:?}: the advertised authority must be one this emulator answers on, \
             expected {expected_authority:?}, got {url:?}"
        );
        assert_eq!(
            path.trim_end_matches('/'),
            expected_api_path,
            "case {case:?}: the advertised address must name the api, expected \
             {expected_api_path:?}, got {url:?}"
        );
        if row.export_reached == ReachedAt::TheEmulatorsOwnEndpoint {
            // A client that resolves nothing but this emulator's own endpoint is never sent
            // to a host AWS publishes its own endpoints at: it reaches none of them.
            assert!(
                !url.contains("amazonaws.com"),
                "case {case:?}: a document for a client of this emulator's own endpoint must \
                 not advertise one of AWS's own hosts, got {url:?}"
            );
            assert_ne!(
                url,
                format!("{}/{{basePath}}", execute_api_url(&api_id, row.region)),
                "case {case:?}: this row exists to separate the address this emulator serves \
                 the api at from the documented one, got the documented one in {url:?}"
            );
        }

        // The template the url carries is DECLARED, so a reader that resolves server
        // variables substitutes it; the api's latest configuration is served under no base
        // path at all, so its default is the empty string.
        assert_eq!(
            document
                .pointer("/servers/0/variables/basePath/default")
                .and_then(Value::as_str),
            Some(""),
            "case {case:?}: the server must declare the basePath variable the url templates, \
             got {document}"
        );

        // And the advertised address SERVES the exported configuration: the url a reader
        // resolves, with the route's own path appended, reaches that route's integration.
        let invoke_path = format!("{}{ROUTE_PATH}", path.trim_end_matches('/'));
        let answered: Answer = send(&emulator, &authority, "GET", &invoke_path, &[]).await;
        assert_ne!(
            answered.error_type.as_deref(),
            Some(UNKNOWN_SERVICE),
            "case {case:?}: the advertised address must be one this emulator claims, but \
             {authority}{invoke_path} is claimed by no service: {}",
            answered.summary()
        );
        assert_eq!(
            answered.status,
            SERVED,
            "case {case:?}: {authority}{invoke_path} must reach the route's integration, got {}",
            answered.summary()
        );
        assert_eq!(
            answered.location.as_deref(),
            Some(UPSTREAM),
            "case {case:?}: {authority}{invoke_path} must be sent to the upstream the \
             integration names, got {}",
            answered.summary()
        );

        // And the document says which algorithm wrote it.
        assert_eq!(
            document
                .get(IMPORTEXPORT_VERSION_EXTENSION)
                .and_then(Value::as_str),
            Some(IMPORTEXPORT_VERSION),
            "case {case:?}: the document must declare the import/export version, got \
             {document}"
        );

        // Exporting a definition REPORTS: the address the api itself publishes is unchanged
        // by it, so a client reading the api after the export is told what it was told before.
        let (status, api) = call_at_host(
            &emulator,
            row.create_host,
            row.region,
            "GET",
            &format!("/v2/apis/{api_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetApi, got {status} {api}"
        );
        assert_eq!(
            as_str(&api, "apiEndpoint"),
            published,
            "case {case:?}: GetApi must report the address the create published, got {api}"
        );
    }
}

#[tokio::test]
async fn an_exported_stage_resolves_to_the_address_that_stage_answers_on() {
    let emulator = emulator();

    // One api, exported for a NAMED stage: the base path the server variable defaults to is
    // that stage's own name, and resolving the url against it reaches the stage's route.
    let row = Row {
        case: "a named stage exported through the emulator's own endpoint",
        create_host: "127.0.0.1:41099",
        export_host: "127.0.0.1:41099",
        export_reached: ReachedAt::TheEmulatorsOwnEndpoint,
        region: "us-west-2",
    };
    let case = row.case;
    let (api_id, _published) = created_api(&emulator, &row).await;

    let stage_name = "prod";
    let (status, stage) = call_at_host(
        &emulator,
        row.create_host,
        row.region,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage_name, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {stage}"
    );

    let (status, text) = get_text(
        &emulator,
        row.export_host,
        row.region,
        &format!("/v2/apis/{api_id}/exports/{OAS30}?outputType=JSON&stageName={stage_name}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ExportApi, got {status} {text}"
    );
    let document: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("case {case:?}: the JSON output type must parse as JSON, got {error}: {text}")
    });

    let (expected_authority, expected_api_path) = row.advertised(&api_id);
    let url = document
        .pointer("/servers/0/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        url,
        format!("https://{expected_authority}{expected_api_path}/{{basePath}}"),
        "case {case:?}: servers[0].url must be the address the api is served at under the \
         templated base path, got {document}"
    );
    assert_eq!(
        document
            .pointer("/servers/0/variables/basePath/default")
            .and_then(Value::as_str),
        Some(stage_name),
        "case {case:?}: the basePath variable must default to the stage's own base path, got \
         {document}"
    );

    // Resolving the variable yields the stage's own invoke address, which serves the route.
    let (authority, path) = dialed(url, stage_name);
    let invoke_path = format!("{path}{ROUTE_PATH}");
    let answered: Answer = send(&emulator, &authority, "GET", &invoke_path, &[]).await;
    assert_eq!(
        answered.status,
        SERVED,
        "case {case:?}: {authority}{invoke_path} must reach the route's integration, got {}",
        answered.summary()
    );
    assert_eq!(
        answered.location.as_deref(),
        Some(UPSTREAM),
        "case {case:?}: {authority}{invoke_path} must be sent to the upstream the integration \
         names, got {}",
        answered.summary()
    );
}

#[tokio::test]
async fn a_specification_this_operation_does_not_serve_is_refused_with_one_sentence() {
    let emulator = emulator();

    let row = Row {
        case: "an api to export",
        create_host: "apigateway.eu-west-1.amazonaws.com",
        export_host: "apigateway.eu-west-1.amazonaws.com",
        export_reached: ReachedAt::AwsOwnHost,
        region: "eu-west-1",
    };
    let (api_id, _) = created_api(&emulator, &row).await;

    // The rows span the shape of the rejected value — a later specification version, the
    // served value in another case, another specification family, the served value with a
    // suffix, and a long value — and both output types, so the specification gate is what
    // answers whichever output type was asked for.
    for (case, specification, output_type) in [
        ("a later specification version", "OAS31".to_string(), "JSON"),
        (
            "the served value spelled in lower case",
            "oas30".to_string(),
            "JSON",
        ),
        (
            "another specification family",
            "SWAGGER".to_string(),
            "YAML",
        ),
        (
            "the served value with a suffix",
            "OAS30.1".to_string(),
            "JSON",
        ),
        ("a long value", "X".repeat(128), "YAML"),
    ] {
        let (status, text) = get_text(
            &emulator,
            row.export_host,
            row.region,
            &format!("/v2/apis/{api_id}/exports/{specification}?outputType={output_type}"),
        )
        .await;
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from ExportApi, got {status} {text}"
        );
        assert_eq!(
            error_code(&parsed),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {text}"
        );
        let message = as_str(&parsed, "message");
        assert_eq!(
            message, UNSUPPORTED_SPECIFICATION_MESSAGE,
            "case {case:?}: the refusal must carry the one sentence for an unsupported \
             specification, got {message:?}"
        );
        // Fixed text: nothing of the request is quoted back into it.
        assert!(
            !message.contains(specification.as_str()),
            "case {case:?}: the sentence quotes nothing out of the request, got {message:?}"
        );
    }

    // The one value it DOES serve is still served, so the gate refuses what is outside the
    // catalog and nothing else.
    let (status, text) = get_text(
        &emulator,
        row.export_host,
        row.region,
        &format!("/v2/apis/{api_id}/exports/{OAS30}?outputType=JSON"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the one served specification must still export, got {status} {text}"
    );
}
