// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of DOMAIN CONFIGURATIONS: `CreateDomainConfiguration`, the
//! `DescribeDomainConfiguration` reader that proves what it persisted, and
//! `DeleteDomainConfiguration`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * a configuration created without a `domainName` configures the AWS-managed domain
//!   the account's endpoint already serves, and one carrying its own domain name is
//!   the customer-managed kind;
//! * a `serverCertificateArn` may only name a certificate that EXISTS, and whether one
//!   exists is the holder's answer: this suite writes the certificate into the one
//!   shared record Certificate Manager keeps of a certificate's serving material and
//!   so exercises both answers. A certificate nothing is held for is refused
//!   `InvalidRequestException` (400) with the fixed `Certificate <arn> does not
//!   exist.` and nothing created, which is what makes the following read a 404;
//! * a created configuration is read back member for member, and the name is its
//!   identity, so a second create under it is a conflict.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_common::acm_certificate_key_material::{
    CertificateKeyMaterial, CertificateKeyMaterialDirectory,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region this suite signs for; the ARN assertions read it back.
const REGION: &str = "eu-central-1";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the domain-configuration request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn create_configuration(&self, name: &str, body: &Value) -> Reply {
        self.dispatch("POST", &format!("/domainConfigurations/{name}"), Some(body))
            .await
    }

    async fn describe_configuration(&self, name: &str) -> Reply {
        self.dispatch("GET", &format!("/domainConfigurations/{name}"), None)
            .await
    }

    async fn delete_configuration(&self, name: &str) -> Reply {
        self.dispatch("DELETE", &format!("/domainConfigurations/{name}"), None)
            .await
    }

    /// Record `certificate_arn` as a certificate the holder has serving material for —
    /// the write Certificate Manager makes when a caller brings a certificate into it,
    /// which is the record this service reads a certificate's existence from.
    fn hold_certificate(&self, certificate_arn: &str, domain_name: &str) {
        let directory = self.engine.state::<CertificateKeyMaterialDirectory>();
        directory.lock().record(
            certificate_arn,
            CertificateKeyMaterial {
                certificate_pem: "-----BEGIN CERTIFICATE-----\nheld\n-----END CERTIFICATE-----\n"
                    .to_string(),
                certificate_chain_pem: None,
                private_key_pem: "-----BEGIN PRIVATE KEY-----\nheld\n-----END PRIVATE KEY-----\n"
                    .to_string(),
                domain_names: vec![domain_name.to_string()],
            },
        );
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }
}

/// An ACM certificate ARN of this account, numbered.
fn certificate_arn(index: usize) -> String {
    format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/cert-{index:06}")
}

/// RULE: a created configuration is read back member for member, and which KIND of
/// domain it configures follows from whether it carries a domain name of its own.
#[tokio::test]
async fn a_created_domain_configuration_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    emulator.hold_certificate(&certificate_arn(1), "iot.example.com");

    // The rows span which optional members the request carries, and both kinds of
    // domain: one without a `domainName` (the AWS-managed domain, for which
    // `serverCertificateArns` is "not required") and one with its own name and the
    // certificate the service presents for it.
    let rows: Vec<(&str, &str, Value, &str, usize)> = vec![
        (
            "the AWS-managed domain, nothing optional",
            "managed.config",
            json!({}),
            "AWS_MANAGED",
            0,
        ),
        (
            "the AWS-managed domain with an authorizer and a service type",
            "managed-with-authorizer",
            json!({
                "serviceType": "DATA",
                "authorizerConfig": { "defaultAuthorizerName": "my-authorizer",
                                      "allowAuthorizerOverride": true },
                "tags": [{ "Key": "team", "Value": "fleet" }],
            }),
            "AWS_MANAGED",
            0,
        ),
        (
            "a customer-managed domain presenting a certificate the holder has",
            "customer_managed-1",
            json!({
                "domainName": "iot.example.com",
                "serverCertificateArns": [certificate_arn(1)],
                "applicationProtocol": "SECURE_MQTT",
                "authenticationType": "AWS_X509",
                "tlsConfig": { "securityPolicy": "IoTSecurityPolicy_TLS13_1_2_2022_10" },
                "serverCertificateConfig": { "enableOCSPCheck": false },
                "clientCertificateConfig": {
                    "clientCertificateCallbackArn":
                        "arn:aws:lambda:eu-central-1:000000000000:function:client-check"
                },
            }),
            "CUSTOMER_MANAGED",
            1,
        ),
    ];

    for (case, name, body, domain_type, certificate_count) in &rows {
        let created = emulator.create_configuration(name, body).await;
        assert_eq!(
            created.status,
            200,
            "case {case}: expected 200, got {} body {}",
            created.status,
            created.text()
        );
        let expected_arn =
            format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:domainconfiguration/{name}");
        assert_eq!(
            created.json()["domainConfigurationArn"],
            json!(expected_arn),
            "case {case}: expected the minted ARN, got {}",
            created.text()
        );

        let read = emulator.describe_configuration(name).await;
        assert_eq!(
            read.status,
            200,
            "case {case}: expected the read to answer 200, got {} body {}",
            read.status,
            read.text()
        );
        let described = read.json();
        assert_eq!(
            described["domainConfigurationName"],
            json!(name),
            "case {case}: the read must name the configuration, got {described}"
        );
        assert_eq!(
            described["domainConfigurationArn"], created.json()["domainConfigurationArn"],
            "case {case}: the read must report the ARN the create minted, got {described}"
        );
        assert_eq!(
            described["domainConfigurationStatus"],
            json!("ENABLED"),
            "case {case}: a configuration serves from the moment it is created, got {described}"
        );
        assert_eq!(
            described["serviceType"],
            json!(body
                .get("serviceType")
                .and_then(Value::as_str)
                .unwrap_or("DATA")),
            "case {case}: the read must report the stored service type, got {described}"
        );
        assert_eq!(
            described["domainType"],
            json!(domain_type),
            "case {case}: expected a {domain_type} domain, got {described}"
        );
        assert_eq!(
            described["domainName"],
            body.get("domainName").cloned().unwrap_or(Value::Null),
            "case {case}: the read must report the stored domain name, got {described}"
        );
        let certificates = described["serverCertificates"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no serverCertificates in {described}"));
        assert_eq!(
            certificates.len(),
            *certificate_count,
            "case {case}: expected {certificate_count} certificate(s), got {described}"
        );
        if let Some(certificate) = certificates.first() {
            assert_eq!(
                certificate["serverCertificateArn"],
                json!(certificate_arn(1)),
                "case {case}: the read must report the resolved certificate, got {described}"
            );
            assert_eq!(
                certificate["serverCertificateStatus"],
                json!("VALID"),
                "case {case}: a certificate the service could resolve is valid, got {described}"
            );
        }
        for member in [
            "authorizerConfig",
            "tlsConfig",
            "serverCertificateConfig",
            "clientCertificateConfig",
            "applicationProtocol",
            "authenticationType",
        ] {
            assert_eq!(
                described[member],
                body.get(member).cloned().unwrap_or(Value::Null),
                "case {case}: the read must report the stored {member}, got {described}"
            );
        }
        assert!(
            described["lastStatusChangeDate"].is_number(),
            "case {case}: the read must report when the status last changed, got {described}"
        );
    }

    // The name is the configuration's identity, so a second create under it is a
    // conflict and the stored record is left alone.
    let conflict = emulator
        .create_configuration("managed.config", &json!({ "serviceType": "DATA" }))
        .await;
    assert_eq!(
        conflict.status,
        409,
        "case a name already taken: expected 409, got {} body {}",
        conflict.status,
        conflict.text()
    );
    assert_eq!(
        conflict.error_code(),
        "ResourceAlreadyExistsException",
        "case a name already taken: got {}",
        conflict.text()
    );

    // The delete removes the configuration, and a second delete is the same 404 a read
    // of a name the account holds nothing under is.
    assert_eq!(
        emulator.delete_configuration("managed.config").await.status,
        200,
        "case the delete: expected 200"
    );
    for (case, reply) in [
        (
            "a read of the deleted configuration",
            emulator.describe_configuration("managed.config").await,
        ),
        (
            "a second delete",
            emulator.delete_configuration("managed.config").await,
        ),
    ] {
        assert_eq!(
            reply.status,
            404,
            "case {case}: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case}: got {}",
            reply.text()
        );
    }
}

/// RULE: a `serverCertificateArn` the holder has no certificate for cannot be
/// presented to a device, so the create is refused and nothing is written.
#[tokio::test]
async fn a_certificate_the_holder_does_not_have_is_refused() {
    let emulator = Emulator::new();
    emulator.hold_certificate(&certificate_arn(1), "iot.example.com");

    // The rows span the forms a certificate the holder does not have takes: another
    // account's certificate, another region's, one whose id nothing was recorded
    // under, and a value that is no certificate ARN at all.
    let unknown = [
        certificate_arn(2),
        format!("arn:aws:acm:{REGION}:000000000001:certificate/cert-000001"),
        format!("arn:aws:acm:us-east-1:{ACCOUNT_ID}:certificate/cert-000001"),
        format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/00000000-0000-0000-0000-000000000000"),
    ];
    for (index, certificate) in unknown.iter().enumerate() {
        let name = format!("refused-{index}");
        let refused = emulator
            .create_configuration(
                &name,
                &json!({
                    "domainName": "iot.example.com",
                    "serverCertificateArns": [certificate],
                }),
            )
            .await;
        assert_eq!(
            refused.status,
            400,
            "case {certificate}: expected 400, got {} body {}",
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.error_code(),
            "InvalidRequestException",
            "case {certificate}: got {}",
            refused.text()
        );
        assert_eq!(
            refused.error_message(),
            format!("Certificate {certificate} does not exist."),
            "case {certificate}: the refusal must name the certificate, got {}",
            refused.text()
        );
        // Nothing was created: the read of the name the create asked for is a 404.
        let described = emulator.describe_configuration(&name).await;
        assert_eq!(
            described.status,
            404,
            "case {certificate}: a refused create must leave no configuration behind, got {} \
             body {}",
            described.status,
            described.text()
        );
    }

    // The certificate the holder DOES have is accepted, so the refusals above are
    // about the certificate and not about the request's shape.
    let accepted = emulator
        .create_configuration(
            "accepted",
            &json!({
                "domainName": "iot.example.com",
                "serverCertificateArns": [certificate_arn(1)],
            }),
        )
        .await;
    assert_eq!(
        accepted.status,
        200,
        "case the certificate the holder has: expected 200, got {} body {}",
        accepted.status,
        accepted.text()
    );
}
