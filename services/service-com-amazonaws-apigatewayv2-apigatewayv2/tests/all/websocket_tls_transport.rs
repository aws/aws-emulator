// SPDX-License-Identifier: CC0-1.0

//! The announced `wss://` socket driven the way a client of AWS dials it: TLS first, then
//! the RFC 6455 opening handshake and the frames over the encrypted stream.
//!
//! The rule these cases are a slice of:
//!
//! > The address a WEBSOCKET api announces as its `ApiEndpoint` is one a WebSocket client
//! > library can DIAL as it stands — the scheme it carries included. So the socket it names
//! > completes the opening handshake for a client that negotiated TLS first, dispatches the
//! > frames that follow to the routes the api deployed, and carries the backend's own reply
//! > back: a handler that posts to the `@connections` endpoint ITS OWN event names reaches
//! > this api's connection, and the payload arrives on the socket the client holds.
//!
//! Every other suite of this data plane dials the announced socket in the clear, which the
//! same socket also serves (the encryption is negotiated in band); these two drive the half a
//! client of AWS uses, with a real TLS client, and the leg a WebSocket application exists for
//! — the reply the BACKEND pushes.
//!
//! The harness mounts four services on one engine, because that reply crosses all of them:
//! this service's socket, the function AWS Lambda runs, the `@connections` endpoint
//! ApiGatewayManagementApi serves and the log group CloudWatch Logs holds. The control-plane
//! requests go through `api_lifecycle`'s one signed caller and the log reads through the
//! access-log suite's one reader, so nothing about either wire is restated here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::net::TcpStream;

use crate::api_lifecycle::call_in_region;
use crate::http_api_access_logging::delivered_to;
use crate::http_api_backend_dispatch::given_function;

/// The Region every request below is signed for — the one the Lambda harness creates its
/// functions in, so the api and the function it names live in the same Region.
const REGION: &str = "us-east-1";

/// The stage every case deploys.
const STAGE: &str = "prod";

/// The four services this suite spans, over ONE engine. The access-log suite's harness mounts
/// three of them ([`crate::http_api_access_logging::emulator`]); the `@connections` endpoint a
/// backend pushes its reply through is the fourth, which is why this one is its own.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// One control-plane request that must answer `201`, signed by the one caller this crate's
/// suites share.
async fn created(emulator: &MultiServiceRouter, what: &str, uri: &str, body: Value) -> Value {
    let (status, answered) = call_in_region(emulator, REGION, "POST", uri, Some(body)).await;
    assert_eq!(status, 201, "{what} must answer 201, got {status} {answered}");
    answered
}

/// A handler that answers the event it was given.
fn echoing_handler() -> String {
    "import json\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   print(\"handled a call\")\n\
     \x20   return {\"statusCode\": 200, \"body\": json.dumps({\"event\": event})}\n"
        .to_string()
}

/// A handler that pushes its answer BACK to the connection through the `@connections`
/// endpoint the event names, the way a WebSocket backend replies.
fn pushing_handler() -> String {
    "import json\n\
     import boto3\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   ctx = event[\"requestContext\"]\n\
     \x20   endpoint = \"https://\" + ctx[\"domainName\"] + \"/\" + ctx[\"stage\"]\n\
     \x20   print(\"pushing to \" + endpoint)\n\
     \x20   client = boto3.client(\"apigatewaymanagementapi\", endpoint_url=endpoint)\n\
     \x20   client.post_to_connection(\n\
     \x20       ConnectionId=ctx[\"connectionId\"],\n\
     \x20       Data=json.dumps({\"pushed\": True}).encode(),\n\
     \x20   )\n\
     \x20   return {\"statusCode\": 200}\n"
        .to_string()
}

/// The messages the function's own log group holds, read through the one reader this crate's
/// suites share ([`crate::http_api_access_logging::delivered_to`]).
async fn log_messages(emulator: &MultiServiceRouter, group: &str) -> Vec<String> {
    delivered_to(emulator, group).await.unwrap_or_default()
}

/// Create a deployed WEBSOCKET api whose `$connect` and `$default` routes front
/// `function`, answering the `host:port` its `ApiEndpoint` announces.
async fn given_socket(
    emulator: &MultiServiceRouter,
    name: &str,
    connect_function: &str,
    message_function: &str,
    two_way: bool,
) -> String {
    let api = created(
        emulator,
        "CreateApi",
        "/v2/apis",
        json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;
    let api_id = api
        .get("apiId")
        .and_then(Value::as_str)
        .expect("CreateApi answers an apiId")
        .to_string();
    let announced = api
        .get("apiEndpoint")
        .and_then(Value::as_str)
        .expect("CreateApi answers an apiEndpoint")
        .to_string();
    let authority = announced
        .strip_prefix("wss://")
        .unwrap_or_else(|| panic!("expected a wss:// endpoint, got {announced:?}"))
        .to_string();

    for (route_key, function) in [
        ("$connect", connect_function),
        ("$default", message_function),
    ] {
        let integration = created(
            emulator,
            "CreateIntegration",
            &format!("/v2/apis/{api_id}/integrations"),
            json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": function,
                "integrationMethod": "POST"
            }),
        )
        .await;
        let integration_id = integration
            .get("integrationId")
            .and_then(Value::as_str)
            .expect("CreateIntegration answers an integrationId")
            .to_string();
        let route = created(
            emulator,
            "CreateRoute",
            &format!("/v2/apis/{api_id}/routes"),
            json!({ "routeKey": route_key, "target": format!("integrations/{integration_id}") }),
        )
        .await;
        if route_key == "$default" && two_way {
            let route_id = route
                .get("routeId")
                .and_then(Value::as_str)
                .expect("CreateRoute answers a routeId")
                .to_string();
            created(
                emulator,
                "CreateRouteResponse",
                &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
                json!({ "routeResponseKey": "$default" }),
            )
            .await;
        }
    }
    created(
        emulator,
        "CreateStage",
        &format!("/v2/apis/{api_id}/stages"),
        json!({ "stageName": STAGE }),
    )
    .await;
    created(
        emulator,
        "CreateDeployment",
        &format!("/v2/apis/{api_id}/deployments"),
        json!({ "stageName": STAGE }),
    )
    .await;
    authority
}

#[tokio::test]
async fn a_backend_pushes_its_reply_back_through_the_announced_endpoint() {
    let case = "a backend that replies through @connections";
    let (emulator, engine) = emulator();
    let admitting =
        given_function(&emulator, &engine, case, "socket-admit", &echoing_handler()).await;
    let pushing =
        given_function(&emulator, &engine, case, "socket-push", &pushing_handler()).await;
    let authority =
        given_socket(&emulator, "push-socket-api", &admitting, &pushing, false).await;
    let mut socket = dial(&authority).await;

    use futures_util::{SinkExt, StreamExt};
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({ "action": "register" }).to_string().into(),
        ))
        .await
        .expect("the frame is sent");
    let answered = tokio::time::timeout(std::time::Duration::from_secs(30), socket.next())
        .await
        .unwrap_or_else(|_| {
            panic!("case {case:?}: the backend's push must arrive over the same socket")
        });
    let text = match answered {
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => text.as_str().to_string(),
        other => panic!("case {case:?}: expected the pushed frame, got {other:?}"),
    };
    assert!(
        text.contains("pushed"),
        "case {case:?}: expected the backend's own payload, got {text:?}"
    );
    let messages = log_messages(&emulator, "/aws/lambda/socket-push").await;
    assert!(
        messages.iter().any(|line| line.contains("pushing to")),
        "case {case:?}: the function's log group must hold what the run logged, got {messages:?}"
    );
}

/// Dial `authority` over TLS and complete the opening handshake on `/{STAGE}`.
async fn dial(authority: &str) -> tokio_tungstenite::WebSocketStream<ClientStream> {
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("expected a host:port, got {authority:?}"));
    assert_eq!(host, "127.0.0.1", "the announced socket must be loopback");
    let port: u16 = port.parse().expect("the announced port parses");
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the announced socket accepts a connection");
    let tls = upgrade_to_tls(stream).await;
    let url = format!("wss://{authority}/{STAGE}");
    let (socket, response) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio_tungstenite::client_async(&url, tls),
    )
    .await
    .expect("the handshake must be answered")
    .unwrap_or_else(|error| panic!("the handshake must complete, got {error}"));
    assert_eq!(
        response.status().as_u16(),
        101,
        "a deployed stage must answer 101"
    );
    socket
}

/// The stream a case's client holds once its TLS handshake has completed.
type ClientStream = tokio_rustls::client::TlsStream<TcpStream>;

#[tokio::test]
async fn the_announced_socket_completes_a_tls_handshake_and_carries_frames() {
    let case = "a client that dials the announced wss:// address";
    let (emulator, engine) = emulator();
    let function =
        given_function(&emulator, &engine, case, "socket-echo", &echoing_handler()).await;
    let authority =
        given_socket(&emulator, "tls-socket-api", &function, &function, true).await;
    let mut socket = dial(&authority).await;

    use futures_util::{SinkExt, StreamExt};
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({ "action": "ping" }).to_string().into(),
        ))
        .await
        .expect("the frame is sent");
    let answered = tokio::time::timeout(std::time::Duration::from_secs(30), socket.next())
        .await
        .expect("a frame must come back");
    assert!(
        matches!(
            answered,
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
        ),
        "expected the route's reply, got {answered:?}"
    );

    // What a client debugging the backend reads: the function's own log group.
    let group = "/aws/lambda/socket-echo";
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut messages = log_messages(&emulator, group).await;
    while messages.is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        messages = log_messages(&emulator, group).await;
    }
    assert!(
        !messages.is_empty(),
        "the function's log group must hold what the run logged, got {messages:?}"
    );
}

/// Run the client side of a TLS handshake on `conn`, accepting the emulator's own
/// self-signed listener certificate.
async fn upgrade_to_tls(conn: TcpStream) -> tokio_rustls::client::TlsStream<TcpStream> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("the provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptTheEmulatorsCertificate(provider)))
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let server_name =
        rustls::pki_types::ServerName::try_from("localhost").expect("localhost is a server name");
    connector
        .connect(server_name, conn)
        .await
        .expect("the server must complete the in-band TLS handshake")
}

/// See [`upgrade_to_tls`].
#[derive(Debug)]
struct AcceptTheEmulatorsCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptTheEmulatorsCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
