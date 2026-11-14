// SPDX-License-Identifier: CC0-1.0

//! The certificate a custom domain is created with has to be able to SERVE the name the
//! domain is created under.
//!
//! A custom domain answers HTTPS on the name `CreateDomainName` states, and the
//! certificate the same call names is what terminates that handshake — so the rule
//! asserted here is: the create is refused, with 400 `BadRequestException` and AppSync's
//! fixed sentence for a certificate it will not serve with, exactly when the certificate
//! the account holds at that ARN does not cover the requested domain name, and nothing is
//! stored for a refused pair. Coverage is the match a TLS client applies to a
//! certificate's names (the subject name and its subject alternative names, with a
//! leading `*` label matching one label), so the table's rows differ along the
//! certificate's NAME SET (one name, several names, a wildcard) and along the FORM of the
//! requested name (the exact name, a sibling, a name one level deeper, the apex of a
//! wildcard, the wildcard itself, another letter case) rather than only along the one pair
//! that exposed the defect.
//!
//! Every certificate is a certificate Certificate Manager really holds: it is minted here
//! and imported through that service's own `ImportCertificate` on the same engine, and the
//! name set each one covers is read back with its own `DescribeCertificate`, so no row
//! rests on a record this file wrote. An ARN that service resolves nothing for is the
//! row that pins the other half of the rule: a name is never refused on evidence that is
//! not there.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region a custom domain's certificate is read from, which is the Region every
/// certificate here is imported at. Source: the service's own
/// `state::CERTIFICATE_REGION`.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// AppSync's fixed sentence for a certificate it will not serve a custom domain with:
/// recorded behavior, interpolating no value of the request.
const CERTIFICATE_INVALID: &str = "Certificate is invalid.";

/// AppSync's fixed text for a custom domain this account does not hold, on the operations
/// that READ one — what a refused create leaves behind.
const DOMAIN_NOT_FOUND_ON_READ: &str = "Domain name not found";

/// `CreateDomainName`'s path.
const DOMAIN_NAMES_PATH: &str = "/v1/domainnames";

/// One answer from the emulator: the parts a case branches on.
#[derive(Debug, Clone)]
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` header the error shape is resolved from.
    error_type: String,
    body: Value,
}

/// A router with this service and Certificate Manager mounted over ONE engine, so the
/// certificate a create names is the certificate that service holds.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(engine),
    );
    router
}

/// Read one answer off the router.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<Bytes>) -> Answer {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Call this service the way an SDK does: restJson1 over its own paths.
async fn appsync_call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");
    dispatch(router, request).await
}

/// Call `operation` of the Certificate Manager service mounted on the same router, the way
/// an SDK drives it: `POST /` with `X-Amz-Target: CertificateManager.<Op>`, signed for that
/// service in the Region its certificates live in.
async fn acm_call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("acm.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CertificateManager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/acm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    dispatch(router, request).await
}

/// A real self-signed certificate covering `names`, as the PEM pair `ImportCertificate`
/// takes: the leaf and the private key that matches it. The first name is also the subject
/// common name, which is the name Certificate Manager reports as the certificate's own.
fn minted(names: &[&str]) -> (String, String) {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
    let mut params = rcgen::CertificateParams::new(
        names
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<String>>(),
    )
    .expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(
        rcgen::DnType::CommonName,
        *names.first().expect("a certificate covers at least one name"),
    );
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2100, 1, 1);
    let certificate = params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem();
    (certificate, key.serialize_pem())
}

/// Import a certificate covering `names` into Certificate Manager through that service's
/// own `ImportCertificate`, and answer the ARN it was stored under.
///
/// The stored certificate is read back with that service's own `DescribeCertificate` and
/// its `SubjectAlternativeNames` asserted to be the names handed in, so a row's
/// expectation rests on the name set Certificate Manager holds.
async fn imported(router: &MultiServiceRouter, names: &[&str]) -> String {
    let (certificate, private_key) = minted(names);
    let answer = acm_call(
        router,
        "ImportCertificate",
        json!({
            "Certificate": ::aws_smithy_types::base64::encode(certificate.as_bytes()),
            "PrivateKey": ::aws_smithy_types::base64::encode(private_key.as_bytes()),
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case ImportCertificate({names:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let arn = answer.body["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case ImportCertificate({names:?}): expected a CertificateArn, got {:?}",
                answer.body
            )
        })
        .to_string();

    let read = acm_call(router, "DescribeCertificate", json!({"CertificateArn": arn})).await;
    assert_eq!(
        read.status, 200,
        "case DescribeCertificate({arn}): expected 200, got {} with {:?}",
        read.status, read.body
    );
    let held: Vec<String> = read.body["Certificate"]["SubjectAlternativeNames"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case DescribeCertificate({arn}): expected SubjectAlternativeNames, got {:?}",
                read.body
            )
        })
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    let expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        held, expected,
        "case DescribeCertificate({arn}): expected the imported name set {expected:?}, got \
         {held:?}"
    );
    arn
}

/// Which certificate a row names.
#[derive(Debug, Clone, Copy)]
enum CertificateChoice {
    /// One covering a single name.
    OneName,
    /// One covering two names, so a row can ask for the one that is NOT the subject.
    TwoNames,
    /// One covering `*.<apex>` and nothing else.
    Wildcard,
    /// A well-formed certificate ARN of this account and Region that nothing resolves for.
    Unresolvable,
    /// An Identity and Access Management server certificate, the other form
    /// `certificateArn` admits and one Certificate Manager holds nothing for.
    IamServerCertificate,
}

/// The one name [`CertificateChoice::OneName`] covers.
const ONE_NAME: &str = "single.appsync-domain-test.com";

/// The subject name of [`CertificateChoice::TwoNames`].
const TWO_NAMES_SUBJECT: &str = "subject.appsync-domain-test.com";

/// The second name of [`CertificateChoice::TwoNames`] — a subject alternative name, which is
/// covered exactly as the subject is.
const TWO_NAMES_ALTERNATIVE: &str = "alternative.appsync-domain-test.com";

/// The apex [`CertificateChoice::Wildcard`]'s single wildcard name sits over.
const WILDCARD_APEX: &str = "wild.appsync-domain-test.com";

/// A certificate id no import mints (Certificate Manager mints a random one), so the ARN
/// built from it resolves to nothing in the account.
const NIL_CERTIFICATE_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Every certificate the rows name, as the ARN each is addressed by.
struct Certificates {
    one_name: String,
    two_names: String,
    wildcard: String,
    unresolvable: String,
    server_certificate: String,
}

impl Certificates {
    /// Import the three real certificates and build the two ARNs that resolve to nothing.
    async fn imported(router: &MultiServiceRouter) -> Self {
        Self {
            one_name: imported(router, &[ONE_NAME]).await,
            two_names: imported(router, &[TWO_NAMES_SUBJECT, TWO_NAMES_ALTERNATIVE]).await,
            wildcard: imported(router, &[&format!("*.{WILDCARD_APEX}")]).await,
            unresolvable: format!(
                "arn:aws:acm:{REGION}:{ACCOUNT}:certificate/{NIL_CERTIFICATE_ID}"
            ),
            server_certificate: format!(
                "arn:aws:iam::{ACCOUNT}:server-certificate/appsync_domain_test"
            ),
        }
    }

    /// The ARN `certificate` names.
    fn arn(&self, certificate: CertificateChoice) -> &str {
        match certificate {
            CertificateChoice::OneName => &self.one_name,
            CertificateChoice::TwoNames => &self.two_names,
            CertificateChoice::Wildcard => &self.wildcard,
            CertificateChoice::Unresolvable => &self.unresolvable,
            CertificateChoice::IamServerCertificate => &self.server_certificate,
        }
    }
}

/// One row: the name a create asks for, the certificate it names, and whether that
/// certificate can serve the name.
struct Row {
    label: &'static str,
    domain_name: String,
    certificate: CertificateChoice,
    /// Whether the create is accepted; `false` means the modelled 400 refusal.
    accepted: bool,
}

/// The table. Each row differs from the others along the certificate's name set or along
/// the form of the requested name, which is what the coverage rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "the certificate's only name",
            domain_name: ONE_NAME.to_string(),
            certificate: CertificateChoice::OneName,
            accepted: true,
        },
        Row {
            label: "that same name in another letter case",
            domain_name: ONE_NAME.to_uppercase(),
            certificate: CertificateChoice::OneName,
            accepted: true,
        },
        Row {
            label: "a sibling of the certificate's only name",
            domain_name: "sibling.appsync-domain-test.com".to_string(),
            certificate: CertificateChoice::OneName,
            accepted: false,
        },
        Row {
            label: "a name one label deeper than the certificate's only name",
            domain_name: format!("deeper.{ONE_NAME}"),
            certificate: CertificateChoice::OneName,
            accepted: false,
        },
        Row {
            label: "the subject name of a certificate carrying two names",
            domain_name: TWO_NAMES_SUBJECT.to_string(),
            certificate: CertificateChoice::TwoNames,
            accepted: true,
        },
        Row {
            label: "the subject ALTERNATIVE name of that same certificate",
            domain_name: TWO_NAMES_ALTERNATIVE.to_string(),
            certificate: CertificateChoice::TwoNames,
            accepted: true,
        },
        Row {
            label: "a third name that certificate does not carry",
            domain_name: "third.appsync-domain-test.com".to_string(),
            certificate: CertificateChoice::TwoNames,
            accepted: false,
        },
        Row {
            label: "one label under a wildcard certificate",
            domain_name: format!("api.{WILDCARD_APEX}"),
            certificate: CertificateChoice::Wildcard,
            accepted: true,
        },
        Row {
            label: "the wildcard name itself",
            domain_name: format!("*.{WILDCARD_APEX}"),
            certificate: CertificateChoice::Wildcard,
            accepted: true,
        },
        Row {
            label: "the apex a wildcard certificate sits over, which the wildcard label \
                    does not match",
            domain_name: WILDCARD_APEX.to_string(),
            certificate: CertificateChoice::Wildcard,
            accepted: false,
        },
        Row {
            label: "two labels under a wildcard certificate, which one wildcard label \
                    does not match",
            domain_name: format!("a.b.{WILDCARD_APEX}"),
            certificate: CertificateChoice::Wildcard,
            accepted: false,
        },
        Row {
            label: "a certificate ARN the account resolves nothing for",
            domain_name: "unresolvable.appsync-domain-test.com".to_string(),
            certificate: CertificateChoice::Unresolvable,
            accepted: true,
        },
        Row {
            label: "an IAM server certificate, which Certificate Manager holds none of",
            domain_name: "server-cert.appsync-domain-test.com".to_string(),
            certificate: CertificateChoice::IamServerCertificate,
            accepted: true,
        },
    ]
}

/// `CreateDomainName` for `domain_name` with `certificate_arn`.
async fn create_domain(
    router: &MultiServiceRouter,
    domain_name: &str,
    certificate_arn: &str,
) -> Answer {
    appsync_call(
        router,
        "POST",
        DOMAIN_NAMES_PATH,
        Some(json!({"domainName": domain_name, "certificateArn": certificate_arn})),
    )
    .await
}

/// `GetDomainName` for `domain_name`.
async fn read_domain(router: &MultiServiceRouter, domain_name: &str) -> Answer {
    appsync_call(
        router,
        "GET",
        &format!("{DOMAIN_NAMES_PATH}/{domain_name}"),
        None,
    )
    .await
}

/// The `message` member of an answer's body, or `""` when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// The whole table: a create is accepted exactly when the certificate the account holds
/// covers the requested name, a refused one is the modelled 400 with AppSync's own
/// sentence, and a refused one leaves no domain behind.
#[tokio::test]
async fn create_domain_name_requires_a_certificate_that_covers_the_domain() {
    let router = router();
    let certificates = Certificates::imported(&router).await;

    for row in rows() {
        let arn = certificates.arn(row.certificate);
        let answer = create_domain(&router, &row.domain_name, arn).await;
        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {}: expected the create to be accepted with 200, got {} with {:?}",
                row.label, answer.status, answer.body
            );
            assert_eq!(
                answer.body["domainNameConfig"]["certificateArn"].as_str(),
                Some(arn),
                "case {}: expected the created domain to report the certificate it was \
                 created with, got {:?}",
                row.label,
                answer.body
            );
            // Read it back through the service's own reader: what the create reported is
            // what the store holds.
            let read = read_domain(&router, &row.domain_name).await;
            assert_eq!(
                read.status, 200,
                "case {}: expected GetDomainName to report the created domain with 200, \
                 got {} with {:?}",
                row.label, read.status, read.body
            );
            assert_eq!(
                read.body["domainNameConfig"]["domainName"].as_str(),
                Some(row.domain_name.as_str()),
                "case {}: expected GetDomainName to report the created name, got {:?}",
                row.label,
                read.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {}: expected the modelled 400 refusal, got {} with {:?}",
            row.label, answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {}: expected BadRequestException, got {:?} with {:?}",
            row.label, answer.error_type, answer.body
        );
        assert_eq!(
            message(&answer),
            CERTIFICATE_INVALID,
            "case {}: expected AppSync's certificate refusal, got {:?}",
            row.label,
            answer.body
        );
        // A refused create writes nothing, so the name is one the account does not hold.
        let read = read_domain(&router, &row.domain_name).await;
        assert_eq!(
            read.status, 404,
            "case {}: expected the refused create to leave no domain behind (404), got {} \
             with {:?}",
            row.label, read.status, read.body
        );
        assert_eq!(
            message(&read),
            DOMAIN_NOT_FOUND_ON_READ,
            "case {}: expected AppSync's absent-domain text, got {:?}",
            row.label,
            read.body
        );
    }
}

/// The association read of a domain that exists and was never linked to an API: the
/// modelled 404 with the sentence AppSync states the condition in, which is about the
/// DOMAIN rather than about the association.
#[tokio::test]
async fn get_api_association_reports_an_unassociated_domain() {
    let router = router();
    let certificates = Certificates::imported(&router).await;

    let created = create_domain(&router, ONE_NAME, &certificates.one_name).await;
    assert_eq!(
        created.status, 200,
        "case the positive control: expected 200, got {} with {:?}",
        created.status, created.body
    );

    let answer = appsync_call(
        &router,
        "GET",
        &format!("{DOMAIN_NAMES_PATH}/{ONE_NAME}/apiassociation"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "case an unassociated domain: expected 404, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "NotFoundException",
        "case an unassociated domain: expected NotFoundException, got {:?} with {:?}",
        answer.error_type, answer.body
    );
    assert_eq!(
        message(&answer),
        "Domain name is not associated.",
        "case an unassociated domain: expected AppSync's unassociated-domain text, got {:?}",
        answer.body
    );
}
