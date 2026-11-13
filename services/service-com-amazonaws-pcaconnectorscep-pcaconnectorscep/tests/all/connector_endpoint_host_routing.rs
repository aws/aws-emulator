// SPDX-License-Identifier: CC0-1.0

//! The HOST a connector's `Endpoint` names belongs to this service, whatever its leading
//! label is.
//!
//! `Connector$Endpoint` is "the connector's HTTPS public SCEP URL" (smithy_query shape
//! Connector): a URL whose host carries the connector's own id as its leading label, and
//! which a SCEP-enabled device or a mobile device management system is then pointed at. The
//! emulator's front door decides which mounted service a request belongs to by asking each of
//! them, and answers a request no service claims with a routing error of its own — so a host
//! this service PUBLISHED has to be claimed by this service, or the device holding that URL
//! is told no such service exists and never reaches the SCEP plane behind it.
//!
//! The RULE these cases assert, over a table rather than one recorded request: a request
//! addressed to a host this service published in a connector's `Endpoint` is this service's
//! to answer, and is served by that connector's SCEP data plane — 200 with the DER
//! certificate of the private CA the connector names while the connector is `ACTIVE`, and
//! this service's own non-2xx otherwise — never by the front door's routing error, which
//! names no AWS error for this surface.
//!
//! Rows differ along what the rule generalises over: which published host form is dialled
//! (the public SCEP host, and the endpoint-qualified PrivateLink one), the Region, the letter
//! case the host is written in, whether the host names a connector of that Region at all, the
//! connector's status, and the SCEP operation named.
//!
//! The OTHER addressing the same plane is published at — the reserved path on the emulator's
//! own endpoint — is pinned beside this in `connector_scep_endpoint`, which is also where the
//! choice between the two addressings is.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::{Decode, DecodePem};

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

use crate::harness::{self, label};

/// The template that gives a ROOT CA its own certificate, which is the step the documented
/// procedure for a root private CA signs with.
const ROOT_CA_TEMPLATE: &str = "arn:aws:acm-pca:::template/RootCACertificate/V1";

/// The media type a single DER X.509 certificate is served as, which is what RFC 8894 answers
/// `GetCACert` with for a CA holding one certificate.
const CA_CERTIFICATE_CONTENT_TYPE: &str = "application/x-x509-ca-cert";

/// The error code the front door answers a request NO mounted service claims with. It names
/// no AWS error of this surface, so a client dialling a URL this service published must never
/// be answered with it.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The status a private CA waiting for its own certificate reports
/// (`CertificateAuthorityStatus`), which is where its CSR can be read.
const PENDING_CERTIFICATE: &str = "PENDING_CERTIFICATE";

/// How long a poll waits for a transition another service's clock owes before giving up.
/// Generous on purpose: the loop ends as soon as the condition holds, so only a genuinely
/// stuck emulator reaches the deadline and no case is decided by how fast the host runs.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll parks between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// What one call answered: the HTTP status, the media type, the error code the envelope
/// carries, the raw body and the parsed body (`Value::Null` when it is not JSON).
#[derive(Debug)]
struct Fetched {
    status: u16,
    content_type: String,
    error_type: String,
    body: Vec<u8>,
    json: Value,
}

impl Fetched {
    /// One member of the parsed body, as a string.
    fn text(&self, member: &str, case: &str) -> String {
        self.json
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a string {member:?} in {:?}", self.json))
            .to_string()
    }
}

/// The router this suite drives: this service beside Amazon Web Services Private CA over ONE
/// shared engine, which is what the assembled emulator does — the certificate a connector's
/// SCEP plane answers with is the one that service holds for the CA the connector names.
fn router_with_private_ca() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere; these
    // cases are about which service a request belongs to and what it is answered.
    router.set_request_authentication(false);
    router.register_service(scep::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One request this router dispatches, exactly as it arrived.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<bytes::Bytes>) -> Fetched {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let error_type = header("x-amzn-errortype")
        .split(['/', ':'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let body = response.into_body().to_vec();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    Fetched {
        status,
        content_type,
        error_type,
        body,
        json,
    }
}

/// One UNSIGNED fetch of a published URL, as a SCEP-enabled device sends it: the URL is
/// dialled exactly as it was published, with the `Host` its own authority names and no AWS
/// credential at all.
async fn fetch(router: &MultiServiceRouter, url: &str) -> Fetched {
    let authority = url
        .split_once("://")
        .map(|(_scheme, rest)| rest.split(['/', '?']).next().unwrap_or(rest))
        .expect("a published URL carries a scheme");
    let request = http::Request::builder()
        .method("GET")
        .uri(url)
        .header("host", authority)
        .body(bytes::Bytes::new())
        .expect("build request");
    dispatch(router, request).await
}

/// One SIGNED Amazon Web Services Private CA call over that service's awsJson1_1 wire.
async fn private_ca_call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Fetched {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://acm-pca.{region}.amazonaws.com/"))
        .header("host", format!("acm-pca.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/acm-pca/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize body"),
        ))
        .expect("build request");
    dispatch(router, request).await
}

/// One private-CA call that must answer 200.
async fn private_ca_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Fetched {
    let answer = private_ca_call(router, region, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} to answer 200, got {} / {:?}",
        answer.status, answer.json
    );
    answer
}

/// A private CA in `region` that EXISTS, created through that service's own
/// `CreateCertificateAuthority` and left where that call leaves it — holding no certificate
/// yet.
async fn private_ca(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    common_name: &str,
) -> String {
    private_ca_ok(
        router,
        case,
        region,
        "CreateCertificateAuthority",
        &json!({
            "CertificateAuthorityConfiguration": {
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": common_name},
            },
            "CertificateAuthorityType": "ROOT",
        }),
    )
    .await
    .text("CertificateAuthorityArn", case)
}

/// Look again until a private CA reports `status`, or the generous deadline passes: the
/// transition that service's clock owes is waited for as a CONDITION, never as a duration.
async fn wait_for_ca_status(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    ca_arn: &str,
    status: &str,
) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let described = private_ca_ok(
            router,
            case,
            region,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": ca_arn}),
        )
        .await;
        let reported = described.json["CertificateAuthority"]["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if reported == status {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the private CA {ca_arn:?} never reached {status:?}, last reported \
             {reported:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Look again until the certificate `IssueCertificate` accepted can be read, or the deadline
/// passes, and answer the PEM it holds.
async fn wait_for_certificate(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    ca_arn: &str,
    certificate_arn: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let answer = private_ca_call(
            router,
            region,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": ca_arn,
                "CertificateArn": certificate_arn,
            }),
        )
        .await;
        if answer.status == 200 {
            return answer.text("Certificate", case);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: GetCertificate for {certificate_arn:?} never answered 200, last \
             answered {} / {:?}",
            answer.status,
            answer.json
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// A root private CA in `region`, driven all the way to holding its own certificate through
/// that service's documented procedure, answering its ARN and the certificate it now holds.
///
/// The certificate is read back through that service's OWN
/// `GetCertificateAuthorityCertificate`, because that read is what the SCEP plane answers
/// from: nothing this suite seeded stands in for it.
async fn active_private_ca(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    common_name: &str,
) -> (String, String) {
    let ca_arn = private_ca(router, case, region, common_name).await;
    wait_for_ca_status(router, case, region, &ca_arn, PENDING_CERTIFICATE).await;

    let csr = private_ca_ok(
        router,
        case,
        region,
        "GetCertificateAuthorityCsr",
        &json!({"CertificateAuthorityArn": ca_arn}),
    )
    .await
    .text("Csr", case);
    let certificate_arn = private_ca_ok(
        router,
        case,
        region,
        "IssueCertificate",
        &json!({
            "CertificateAuthorityArn": ca_arn,
            "Csr": aws_smithy_types::base64::encode(csr.as_bytes()),
            "SigningAlgorithm": "SHA256WITHRSA",
            "TemplateArn": ROOT_CA_TEMPLATE,
            "Validity": {"Type": "YEARS", "Value": 10},
        }),
    )
    .await
    .text("CertificateArn", case);
    let certificate = wait_for_certificate(router, case, region, &ca_arn, &certificate_arn).await;
    private_ca_ok(
        router,
        case,
        region,
        "ImportCertificateAuthorityCertificate",
        &json!({
            "CertificateAuthorityArn": ca_arn,
            "Certificate": aws_smithy_types::base64::encode(certificate.as_bytes()),
        }),
    )
    .await;

    let held = private_ca_ok(
        router,
        case,
        region,
        "GetCertificateAuthorityCertificate",
        &json!({"CertificateAuthorityArn": ca_arn}),
    )
    .await
    .text("Certificate", case);
    (ca_arn, held)
}

/// Create a connector linking `ca_arn` and move the transition its creation owes, answering
/// its ARN. The create is addressed at this service's own regional endpoint, which is the
/// address a client that resolves AWS's own hostnames reaches it on.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    region: &str,
    ca_arn: &str,
    vpc_endpoint_id: Option<&str>,
) -> String {
    let arn = creating_connector(router, case, region, ca_arn, vpc_endpoint_id).await;
    harness::release_pending(engine, &arn);
    arn
}

/// Create a connector and leave the transition its creation owes where it is, answering its
/// ARN: a connector whose status is still the one a create starts it in.
async fn creating_connector(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    ca_arn: &str,
    vpc_endpoint_id: Option<&str>,
) -> String {
    let mut request = json!({"CertificateAuthorityArn": ca_arn});
    if let Some(vpce) = vpc_endpoint_id {
        request["VpcEndpointId"] = json!(vpce);
    }
    let created = harness::call(router, region, "POST", "/connectors", Some(request)).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    created.text("ConnectorArn")
}

/// The `Endpoint` `GetConnector` reports for `arn` — the URL this service PUBLISHES for a
/// caller to dial, which is the one the cases below dial.
async fn reported_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    arn: &str,
) -> String {
    let read = harness::call(
        router,
        region,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from GetConnector, got {} / {:?}",
        read.status, read.body
    );
    read.body["Connector"]["Endpoint"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string Endpoint in {:?}", read.body))
        .to_string()
}

/// The connector id inside a connector ARN, which is also the leading label of the host its
/// `Endpoint` names.
fn connector_id_of(arn: &str) -> &str {
    arn.rsplit_once('/')
        .unwrap_or_else(|| panic!("expected a connector ARN ending `connector/<uuid>`, got {arn:?}"))
        .1
}

/// The same URL with its HOST upper-cased: a DNS name is case-insensitive, so a client that
/// pasted the published host in another case is addressing the same connector.
fn with_upper_case_host(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("a published URL carries a scheme");
    match rest.split_once(['/', '?']) {
        Some((authority, tail)) => {
            let separator = &rest[authority.len()..rest.len() - tail.len()];
            format!("{scheme}://{}{separator}{tail}", authority.to_uppercase())
        }
        None => format!("{scheme}://{}", rest.to_uppercase()),
    }
}

/// The subject and serial number of a DER X.509 certificate: the two facts that identify it
/// whatever framing it arrived in.
fn identity_of_der(der: &[u8], case: &str) -> (String, String) {
    let certificate = x509_cert::Certificate::from_der(der)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a DER X.509 certificate: {error}"));
    (
        certificate.tbs_certificate.subject.to_string(),
        hex::encode(certificate.tbs_certificate.serial_number.as_bytes()),
    )
}

/// The same two facts out of a PEM certificate.
fn identity_of_pem(text: &str, case: &str) -> (String, String) {
    let certificate = x509_cert::Certificate::from_pem(text)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a PEM X.509 certificate: {error}"));
    (
        certificate.tbs_certificate.subject.to_string(),
        hex::encode(certificate.tbs_certificate.serial_number.as_bytes()),
    )
}

/// Assert this answer came from a service and not from the front door's "no service claims
/// this request" routing error — the one answer a client dialling a published URL must never
/// get, whatever else is wrong with its request.
fn assert_not_the_front_doors_routing_error(answer: &Fetched, case: &str) {
    assert_ne!(
        answer.error_type, UNKNOWN_SERVICE,
        "case {case:?}: a published connector host must be claimed by this service, but the \
         front door answered {} with the {UNKNOWN_SERVICE} routing error: {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_ne!(
        answer.json.get("__type").and_then(Value::as_str),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: a published connector host must be claimed by this service, but the \
         front door answered {} with the {UNKNOWN_SERVICE} routing error: {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// One row of the round-trip table: the Region the CA and the connector live in, the subject
/// of the CA, and the access path the create chose — which is what decides which of the two
/// published host forms the `Endpoint` names.
#[derive(Debug)]
struct HostRow {
    name: &'static str,
    region: &'static str,
    common_name: &'static str,
    vpc_endpoint_id: Option<&'static str>,
}

/// The table: both published host forms, in different Regions.
fn host_rows() -> Vec<HostRow> {
    vec![
        HostRow {
            name: "the public SCEP host of a connector reachable over the internet",
            region: "eu-west-1",
            common_name: "scep-host-public",
            vpc_endpoint_id: None,
        },
        HostRow {
            name: "the endpoint-qualified host of a connector reachable through a VPC endpoint",
            region: "ap-south-1",
            common_name: "scep-host-private",
            vpc_endpoint_id: Some("vpce-0123abcd456789abc"),
        },
    ]
}

/// A `GetCACert` GET to the host a connector's `Endpoint` names reaches this service and is
/// answered 200 with the DER certificate of the private CA that connector links — the same
/// subject and serial number Amazon Web Services Private CA answers for that CA — rather than
/// with the front door's routing error over the connector id in the host's leading label.
#[tokio::test]
async fn get_ca_cert_at_a_published_scep_host_answers_the_connectors_ca_certificate() {
    for row in host_rows() {
        let (router, engine) = router_with_private_ca();
        let (ca_arn, held) = active_private_ca(&router, row.name, row.region, row.common_name).await;
        let arn = active_connector(
            &router,
            &engine,
            row.name,
            row.region,
            &ca_arn,
            row.vpc_endpoint_id,
        )
        .await;
        let endpoint = reported_endpoint(&router, row.name, row.region, &arn).await;
        let id = connector_id_of(&arn);
        assert!(
            endpoint.starts_with(&format!("https://{id}.")),
            "case {row:?}: the published Endpoint's host carries the connector id {id:?} as its \
             leading label, which is the label the front door would otherwise read as a service \
             name; got {endpoint:?}"
        );

        let url = format!("{endpoint}?operation=GetCACert");
        let answer = fetch(&router, &url).await;
        assert_not_the_front_doors_routing_error(&answer, row.name);
        assert_eq!(
            answer.status, 200,
            "case {row:?}: expected 200 from GetCACert at {url:?}, got {} / {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type, CA_CERTIFICATE_CONTENT_TYPE,
            "case {row:?}: expected the DER certificate media type, got {:?}",
            answer.content_type
        );
        let served = identity_of_der(&answer.body, row.name);
        let expected = identity_of_pem(&held, row.name);
        assert_eq!(
            served, expected,
            "case {row:?}: expected the certificate of {ca_arn:?} (subject, serial) {expected:?}, \
             got {served:?}"
        );
    }
}

/// Every request to a published connector host is answered by this service, including the
/// ones it cannot serve: a host in another letter case, a connector whose creation has not
/// settled, a host naming no connector of that Region, and an operation this plane does not
/// implement are each answered this service's own status — never the front door's routing
/// error, and never a certificate.
#[tokio::test]
async fn a_published_scep_host_is_answered_by_this_service_even_when_it_cannot_serve() {
    let region = "eu-west-1";
    let case = "the plane behind a published host";
    let (router, engine) = router_with_private_ca();
    // A CA that EXISTS and holds no certificate: the connectors below link a CA their create
    // resolves, and that CA has nothing to present to a SCEP client.
    let ca_arn = private_ca(&router, case, region, "scep-host-uncertificated").await;

    let active = active_connector(&router, &engine, case, region, &ca_arn, None).await;
    let active_host = reported_endpoint(&router, case, region, &active).await;
    let creating = creating_connector(&router, case, region, &ca_arn, None).await;
    let creating_host = reported_endpoint(&router, case, region, &creating).await;
    // A host of the same published form naming a connector this service does not hold, and one
    // naming this connector in a Region it does not live in.
    let absent_host = scep::state::connector_endpoint(
        region,
        "99999999-8888-7777-6666-555544443333",
        None,
    );
    let other_region_host =
        scep::state::connector_endpoint("us-east-2", connector_id_of(&active), None);

    // The rows: what the request is, the URL it is sent to, and the status this service
    // answers it. An operation this plane does not implement is a bad request; a host naming
    // no connector of that Region is not found; a connector whose plane is not up yet is
    // unavailable; and a connector whose private CA holds no certificate has none to present.
    let rows: &[(&str, String, u16)] = &[
        (
            "the published host, as published",
            format!("{active_host}?operation=GetCACert"),
            502,
        ),
        (
            "the published host in upper case, which names the same connector",
            with_upper_case_host(&format!("{active_host}?operation=GetCACert")),
            502,
        ),
        (
            "an operation this plane does not serve",
            format!("{active_host}?operation=GetCACaps"),
            400,
        ),
        (
            "an enrolment nothing here performs",
            format!("{active_host}?operation=PKIOperation"),
            400,
        ),
        ("no operation at all", active_host.clone(), 400),
        (
            "a connector whose creation has not settled",
            format!("{creating_host}?operation=GetCACert"),
            503,
        ),
        (
            "a published host naming no connector",
            format!("{absent_host}?operation=GetCACert"),
            404,
        ),
        (
            "this connector's id under another Region's host",
            format!("{other_region_host}?operation=GetCACert"),
            404,
        ),
    ];

    for (row, url, expected) in rows {
        let answer = fetch(&router, url).await;
        assert_not_the_front_doors_routing_error(&answer, row);
        assert_eq!(
            answer.status, *expected,
            "case {row:?}: expected {expected} from {url:?}, got {} / {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_ne!(
            answer.content_type, CA_CERTIFICATE_CONTENT_TYPE,
            "case {row:?}: a request this plane cannot serve carries no certificate, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}
