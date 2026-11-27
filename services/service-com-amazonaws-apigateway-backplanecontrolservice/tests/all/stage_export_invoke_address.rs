// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the INVOKE ADDRESS the artifacts of a deployed stage advertise: the
//! `host` of a Swagger 2.0 export, the `servers[].url` of an OpenAPI 3.0 export, and the
//! endpoint a generated client SDK is bound to.
//!
//! Two rules, each where an address is stated:
//!
//! > An address an artifact advertises for a client to DIAL — the `host` a Swagger 2.0
//! > document states beside the stage's `basePath`, and the endpoint a generated client SDK
//! > is bound to — is an address this emulator itself answers on: the endpoint the request
//! > reached, and the api's own `execute-api` host for a client that resolves AWS's own
//! > hosts.
//!
//! > The ONE Server Object of an OpenAPI 3.0 export states the endpoint of the API ITSELF —
//! > the api id as the leading DNS label of `{api-id}.execute-api.{region}.amazonaws.com`
//! > and the templated `{basePath}` as the whole path — whatever address the export request
//! > reached, because that document is the API's published contract and is read by client
//! > generators, contract tests and documentation pipelines.
//!
//! Both addresses are ones this data plane claims: it answers an invoke host-style on
//! `{api-id}.execute-api.{region}.amazonaws.com` as well as on the emulator's own endpoint
//! behind the reserved `/_aws/execute-api/{api-id}` prefix.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service — and then CALLS the address the artifact advertised, so what is
//! asserted is that the advertised endpoint serves the api rather than how a handler spelled
//! it.
//!
//! The rows span what the rules generalise over: the endpoint the request was addressed at
//! (a loopback literal with a port, the same endpoint by its loopback NAME on another port,
//! and one of AWS's own service hosts, signed for two different Regions), the stage name the
//! artifact is of, and the artifact that advertises the address (both `exportType` dialects
//! and a generated SDK). The recorded case — one export read off a stage reached at the
//! emulator's own endpoint — is ONE row. The unit test beside the reading itself
//! (`src/service/invoke/announced_address.rs`) carries the endpoint FORMS this suite cannot
//! drive end to end, a bracketed IPv6 address among them: the emulator's front door resolves
//! no target service from such a host, so an unsigned data-plane request to one reaches no
//! service to claim it.

use bytes::Bytes;
use serde_json::Value;

use crate::execute_api_data_plane::{deploy, invoke_on_host, json_template, mock_resource};
use crate::rest_api_lifecycle::{create_api, Gateway};

/// The scheme every advertised invoke address carries: this emulator serves HTTPS, and
/// "clients can invoke your API with the default
/// https://{api_id}.execute-api.{region}.amazonaws.com endpoint"
/// (`RestApi$disableExecuteApiEndpoint`).
const SCHEME: &str = "https";

/// The resource path the deployed `MOCK` method answers on, and the body it answers with —
/// reaching it is what "the advertised address is served" means.
const RESOURCE_PATH: &str = "items";
const SERVED_BODY: &str = r#"{"at":"items"}"#;

/// The Region this crate's wire harness creates a REST API in: the Region the shared
/// `create_api` request names in its host. An api's own `execute-api` endpoint lives in the
/// Region the api was CREATED in, so that is the Region an exported 3.0 document states
/// whatever Region a later request is signed for.
const API_CREATION_REGION: &str = "us-east-1";

/// The api's OWN default invoke host: the api id as the leading DNS label of
/// `{api-id}.execute-api.{region}.amazonaws.com`, in [`API_CREATION_REGION`].
fn api_endpoint_authority(api: &str) -> String {
    format!("{api}.execute-api.{API_CREATION_REGION}.amazonaws.com")
}

/// Which host a row's artifacts must advertise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Advertised {
    /// The emulator's own endpoint, where this data plane resolves a request by the stage
    /// segment of its path.
    EmulatorEndpoint,
    /// The api's own `{api-id}.execute-api.{region}.amazonaws.com` host, which this data
    /// plane claims host-style.
    ExecuteApiHost,
}

/// One row: the endpoint the control-plane request is addressed at, the Region it is signed
/// for, the stage the artifacts are of, and the host they must advertise.
struct Row {
    case: &'static str,
    /// The `Host` the control-plane request carries — the endpoint the client is configured
    /// with.
    host: &'static str,
    region: &'static str,
    stage: &'static str,
    advertised: Advertised,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a client configured with the emulator's loopback \
                   endpoint",
            host: "127.0.0.1:39461",
            region: "us-west-2",
            stage: "prod",
            advertised: Advertised::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint by its loopback NAME on another port, another stage",
            host: "localhost:9297",
            region: "ap-south-1",
            stage: "beta-2",
            advertised: Advertised::EmulatorEndpoint,
        },
        Row {
            case: "a client that resolves AWS's own hosts: the api's own execute-api host",
            host: "apigateway.us-east-1.amazonaws.com",
            region: "us-east-1",
            stage: "prod",
            advertised: Advertised::ExecuteApiHost,
        },
        Row {
            case: "the same client in another Region, whose host names that Region",
            host: "apigateway.eu-west-1.amazonaws.com",
            region: "eu-west-1",
            stage: "v1",
            advertised: Advertised::ExecuteApiHost,
        },
    ]
}

impl Row {
    /// The api's own default `execute-api` host for the Region the export was asked for —
    /// the host advertised to a client that resolves AWS's own hosts, and the one no
    /// artifact of a client configured with this emulator's endpoint may name.
    fn execute_api_authority(&self, api: &str) -> String {
        format!("{api}.execute-api.{}.amazonaws.com", self.region)
    }

    /// The host the row's artifacts must advertise.
    fn authority(&self, api: &str) -> String {
        match self.advertised {
            Advertised::EmulatorEndpoint => self.host.to_string(),
            Advertised::ExecuteApiHost => self.execute_api_authority(api),
        }
    }

    /// The path that names the api on the dialled host ahead of the stage: the reserved
    /// data-plane prefix on the emulator's own endpoint, and nothing on the api's own
    /// `execute-api` host, whose whole path space is one api's invoke path.
    fn path_prefix(&self, api: &str) -> String {
        match self.advertised {
            Advertised::EmulatorEndpoint => format!("/_aws/execute-api/{api}"),
            Advertised::ExecuteApiHost => String::new(),
        }
    }

    /// The base path of the row's stage, which a 2.0 `basePath` states ("MUST start with a
    /// leading slash"): the stage's own segment, which this emulator resolves on its own
    /// endpoint by the stage the path names.
    fn base_path(&self) -> String {
        format!("/{}", self.stage)
    }

    /// The whole invoke URL of the row's stage as a generated client calls it.
    fn invoke_url(&self, api: &str) -> String {
        format!(
            "{SCHEME}://{}{}{}",
            self.authority(api),
            self.path_prefix(api),
            self.base_path()
        )
    }

    /// The invoke URL a reader of the exported 3.0 document resolves once it fills the
    /// `{basePath}` variable in: the API's OWN endpoint and the stage, which this data plane
    /// claims host-style whichever endpoint the export request reached.
    fn exported_invoke_url(&self, api: &str) -> String {
        format!(
            "{SCHEME}://{}{}",
            api_endpoint_authority(api),
            self.base_path()
        )
    }
}

/// Send one request to the control plane at the row's own endpoint, signed for its Region,
/// answering the status and the body bytes.
///
/// Built here rather than through the shared `call` helper because what every case varies is
/// exactly the endpoint the request is ADDRESSED at, which that helper fixes.
async fn call_at(row: &Row, gateway: &Gateway, uri: &str) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", row.host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{}/apigateway/aws4_request, \
                 SignedHeaders=host, Signature=0",
                row.region
            ),
        )
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    (status, response.into_body().to_vec())
}

/// The export document of the row's stage in `dialect`, as the JSON file a caller downloads.
async fn export_document(row: &Row, gateway: &Gateway, api: &str, dialect: &str) -> Value {
    let (status, body) = call_at(
        row,
        gateway,
        &format!("/restapis/{api}/stages/{}/exports/{dialect}", row.stage),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?} as {dialect}: expected 200 from GetExport, got {status} with {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "case {:?} as {dialect}: expected a JSON export, got {error} for {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        )
    })
}

/// Every file of the SDK generated for the row's stage, as one text — what a
/// language-independent assertion about the generated sources reads.
async fn sdk_project(row: &Row, gateway: &Gateway, api: &str, sdk_type: &str) -> String {
    let (status, body) = call_at(
        row,
        gateway,
        &format!("/restapis/{api}/stages/{}/sdks/{sdk_type}", row.stage),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?} as {sdk_type}: expected 200 from GetSdk, got {status} with {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(body)).unwrap_or_else(|error| {
        panic!(
            "case {:?} as {sdk_type}: expected a zip archive, got {error}",
            row.case
        )
    });
    let mut text = String::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap_or_else(|error| {
            panic!(
                "case {:?} as {sdk_type}: expected entry {index}, got {error}",
                row.case
            )
        });
        std::io::Read::read_to_string(&mut entry, &mut text).unwrap_or_else(|error| {
            panic!(
                "case {:?} as {sdk_type}: expected the text of entry {index}, got {error}",
                row.case
            )
        });
        text.push('\n');
    }
    text
}

/// Stand the row's api up: one `MOCK` method under [`RESOURCE_PATH`], deployed to the row's
/// stage. Answers the api id.
async fn deployed_api(row: &Row, gateway: &Gateway) -> String {
    let (api, root) = create_api(gateway, row.case).await;
    mock_resource(
        gateway,
        &api,
        &root,
        RESOURCE_PATH,
        "GET",
        "200",
        json_template(SERVED_BODY),
    )
    .await;
    deploy(gateway, &api, row.stage).await;
    api
}

/// Assert the address an artifact advertised is one this emulator SERVES: a request to
/// `invoke_path` on `authority` reaches the method the stage deployed.
async fn assert_served(
    row: &Row,
    gateway: &Gateway,
    authority: &str,
    artifact: &str,
    invoke_path: &str,
) {
    let path = format!("{invoke_path}/{RESOURCE_PATH}");
    let answered = invoke_on_host(gateway, authority, "GET", &path, &[]).await;
    assert_eq!(
        answered.status, 200,
        "case {:?}: the address {artifact} advertised ({SCHEME}://{authority}{path}) must \
         serve the api, got {} with {:?}",
        row.case, answered.status, answered.body
    );
    assert_eq!(
        answered.body, SERVED_BODY,
        "case {:?}: expected the deployed method's body at the address {artifact} \
         advertised, got {:?}",
        row.case, answered.body
    );
}

/// Assert an advertised host names no AWS host for a client that resolves none.
fn assert_reachable_authority(row: &Row, authority: &str, artifact: &str) {
    if row.advertised == Advertised::EmulatorEndpoint {
        assert!(
            !authority.ends_with("amazonaws.com"),
            "case {:?}: {artifact} must not advertise an AWS host no client of this emulator \
             resolves, got {authority:?}",
            row.case
        );
    }
}

/// Assert an exported document's host is the one the row advertises, and that a client
/// configured with this emulator's own endpoint is handed no AWS host it cannot resolve.
fn assert_exported_authority(row: &Row, api: &str, authority: &str, artifact: &str) {
    assert_eq!(
        authority,
        row.authority(api),
        "case {:?}: expected {artifact} to name the address the stage is served at, got \
         {authority:?}",
        row.case
    );
    assert_reachable_authority(row, authority, artifact);
    if row.advertised == Advertised::ExecuteApiHost {
        assert_eq!(
            authority.split('.').next(),
            Some(api),
            "case {:?}: expected the api id as the leading DNS label of {artifact}, got \
             {authority:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_swagger_export_advertises_the_host_the_emulator_serves_the_stage_on() {
    for row in rows() {
        let gateway = Gateway::new();
        let api = deployed_api(&row, &gateway).await;
        let document = export_document(&row, &gateway, &api, "swagger").await;

        let host = document["host"].as_str().unwrap_or_default();
        assert_exported_authority(&row, &api, host, "the swagger export's host");
        assert_eq!(
            document["basePath"].as_str().unwrap_or_default(),
            row.base_path(),
            "case {:?}: expected the stage as the basePath, got {document}",
            row.case
        );
        assert_eq!(
            document["schemes"],
            serde_json::json!([SCHEME]),
            "case {:?}: expected the https invoke scheme, got {document}",
            row.case
        );
        assert_served(
            &row,
            &gateway,
            &row.authority(&api),
            "the swagger export",
            &row.base_path(),
        )
        .await;
    }
}

#[tokio::test]
async fn an_oas30_export_names_the_apis_own_endpoint_whatever_address_it_was_asked_at() {
    for row in rows() {
        let gateway = Gateway::new();
        let api = deployed_api(&row, &gateway).await;
        let document = export_document(&row, &gateway, &api, "oas30").await;

        let server = &document["servers"][0];
        let url = server["url"].as_str().unwrap_or_default();
        // The `{basePath}` variable stands where the stage goes, and carries the stage name
        // as its default — so the URL a reader resolves is the stage's own invoke URL on the
        // API's own endpoint, with the api id as the host's leading label and no path ahead
        // of the variable.
        assert_eq!(
            url,
            format!("{SCHEME}://{}/{{basePath}}", api_endpoint_authority(&api)),
            "case {:?}: expected the oas30 export to name the API's own endpoint, got \
             {document}",
            row.case
        );
        assert_eq!(
            server["variables"]["basePath"]["default"]
                .as_str()
                .unwrap_or_default(),
            row.stage,
            "case {:?}: expected the stage as the basePath variable's default, got {document}",
            row.case
        );
        let resolved = url.replace("{basePath}", row.stage);
        assert_eq!(
            resolved,
            row.exported_invoke_url(&api),
            // The resolved URL is the stage's invoke URL on the api's own `execute-api`
            // host, so a reader of the document dials the API's published address.
            "case {:?}: expected the resolved server URL to be the stage's invoke URL on the \
             API's own endpoint, got {resolved:?}",
            row.case
        );
        let authority = url
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or_default();
        assert_eq!(
            authority.split('.').next(),
            Some(api.as_str()),
            "case {:?}: expected the api id as the leading DNS label of the oas30 export's \
             server URL, got {authority:?}",
            row.case
        );
        // The endpoint the document names is one this emulator answers for: this data plane
        // claims the api's own `execute-api` host, so the resolved URL reaches the deployed
        // method.
        assert_served(
            &row,
            &gateway,
            &api_endpoint_authority(&api),
            "the oas30 export",
            &row.base_path(),
        )
        .await;
    }
}

#[tokio::test]
async fn a_generated_sdk_is_bound_to_the_host_the_emulator_serves_the_stage_on() {
    for row in rows() {
        let gateway = Gateway::new();
        let api = deployed_api(&row, &gateway).await;
        // One generator is enough for the address: every generator is bound to the same
        // endpoint, and `stage_sdk` holds the table over the generators themselves.
        let project = sdk_project(&row, &gateway, &api, "javascript").await;

        let expected = row.invoke_url(&api);
        assert!(
            project.contains(&expected),
            "case {:?}: expected the generated client to call {expected:?}, got {project}",
            row.case
        );
        assert_reachable_authority(&row, &row.authority(&api), "the generated SDK");
        if row.advertised == Advertised::EmulatorEndpoint {
            let unreachable = format!("{api}.execute-api.{}.amazonaws.com", row.region);
            assert!(
                !project.contains(&unreachable),
                "case {:?}: expected no AWS host this emulator does not serve in the \
                 generated client, got {unreachable:?} in {project}",
                row.case
            );
        }
        assert_served(
            &row,
            &gateway,
            &row.authority(&api),
            "the generated SDK",
            &format!("{}{}", row.path_prefix(&api), row.base_path()),
        )
        .await;
    }
}
