// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `RegisterCACertificate` and the `DescribeCACertificate`
//! reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: an operation that REGISTERS material a caller owns reads every
//! member it reports out of that material, records it under the identity the material
//! itself determines, and refuses material it cannot read instead of recording a resource
//! it cannot describe. Here that means `RegisterCACertificate` keys the CA registry by the
//! SHA-256 digest of the certificate's own DER (the modelled `CertificateId`, `length
//! 64..64, pattern ^(0x)?[a-fA-F0-9]+$`), mints `arn:aws:iot:<region>:<account>:cacert/<id>`
//! over that id, resolves the record's status from `setAsActive` and its
//! `autoRegistrationStatus` from `allowAutoRegistration`, takes the reported `validity`
//! from the certificate's own validity period, and `DescribeCACertificate` reports that
//! one record back; material that is not a readable X.509 certificate authority is
//! `CertificateValidationException` (400), a mode and a verification certificate that
//! contradict each other are `InvalidRequestException` (400), material already registered
//! is `ResourceAlreadyExistsException` (409), and one CA subject field registered past the
//! documented limit is `LimitExceededException` (410) — and a refusal records nothing.
//!
//! The tables span what that rule generalises over rather than one request: the three
//! forms each of `setAsActive` and `allowAutoRegistration` takes on the wire (absent — the
//! modelled `default false` — explicitly `false`, explicitly `true`), both
//! `CertificateMode` values plus the mode left out altogether, several CA subjects and
//! validity periods, metadata present and absent, the region the call is signed for, and
//! several shapes of material that is not a CA certificate.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    AUTO_REGISTRATION_DISABLE, AUTO_REGISTRATION_ENABLE, CA_CERTIFICATE_STATUS_ACTIVE,
    CA_CERTIFICATE_STATUS_INACTIVE, CERTIFICATE_MODE_DEFAULT, CERTIFICATE_MODE_SNI_ONLY,
    FIRST_CERTIFICATE_CUSTOMER_VERSION, MAX_CA_CERTIFICATES, MAX_CA_CERTIFICATES_PER_SUBJECT,
    MAX_POLICY_TAGS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::{DecodePem, Encode};
use x509_cert::Certificate;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// `region`: the scope is how the router picks the claiming service, and how the service
/// picks the region its ARNs are scoped to. The signature itself is never checked here —
/// the harness turns request authentication off, exactly as an unauthenticated emulator
/// runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `message` of an error body.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// The error shape the answer names, as a client reads it: the `x-amzn-errortype`
    /// header up to the `:` that introduces the shape's documentation URI, which this
    /// protocol appends for some shapes and an SDK strips.
    fn code(&self) -> Option<&str> {
        self.error_code
            .as_deref()
            .map(|value| value.split(':').next().unwrap_or(value))
    }

    /// A member of the reported `CACertificateDescription`.
    fn described(&self, name: &str) -> Option<&Value> {
        self.body
            .get("certificateDescription")
            .and_then(|description| description.get(name))
    }

    /// A string member of the reported `CACertificateDescription`.
    fn described_text(&self, name: &str) -> Option<String> {
        self.described(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A member of the reported `validity`, in epoch seconds.
    fn validity_seconds(&self, name: &str) -> Option<f64> {
        self.described("validity")
            .and_then(|validity| validity.get(name))
            .and_then(Value::as_f64)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, region: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `RegisterCACertificate`: `POST /cacertificate`, with `setAsActive` and
    /// `allowAutoRegistration` bound as `@httpQuery` parameters — absent from the query
    /// when the case names none — and everything else in the body.
    async fn register(&self, request: &Registration) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(set_as_active) = request.set_as_active {
            query.push(format!("setAsActive={set_as_active}"));
        }
        if let Some(allow_auto_registration) = request.allow_auto_registration {
            query.push(format!("allowAutoRegistration={allow_auto_registration}"));
        }
        let path = if query.is_empty() {
            "/cacertificate".to_string()
        } else {
            format!("/cacertificate?{}", query.join("&"))
        };

        let mut body = json!({ "caCertificate": request.ca_certificate });
        if let Some(mode) = request.certificate_mode {
            body["certificateMode"] = json!(mode);
        }
        if let Some(verification) = &request.verification_certificate {
            body["verificationCertificate"] = json!(verification);
        }
        if let Some(configuration) = &request.registration_config {
            body["registrationConfig"] = configuration.clone();
        }
        if let Some(tags) = &request.tags {
            body["tags"] = tags.clone();
        }
        self.call("POST", request.region, &path, Some(&body)).await
    }

    /// `DescribeCACertificate`: `GET /cacertificate/{certificateId}`.
    async fn describe(&self, region: &str, certificate_id: &str) -> Reply {
        self.call("GET", region, &format!("/cacertificate/{certificate_id}"), None)
            .await
    }

    /// `DescribeCertificate`: `GET /certificates/{certificateId}` — the DEVICE certificate
    /// reader, which a registered CA certificate must not be visible through.
    async fn describe_device_certificate(&self, certificate_id: &str) -> Reply {
        self.call("GET", REGION, &format!("/certificates/{certificate_id}"), None)
            .await
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, which issues the
    /// END-ENTITY certificate one case offers as CA material.
    async fn create_device_certificate(&self) -> Reply {
        self.call("POST", REGION, "/keys-and-certificate?setAsActive=false", None)
            .await
    }
}

/// One `RegisterCACertificate` request, as a case names it.
struct Registration {
    ca_certificate: String,
    verification_certificate: Option<String>,
    certificate_mode: Option<&'static str>,
    set_as_active: Option<bool>,
    allow_auto_registration: Option<bool>,
    registration_config: Option<Value>,
    tags: Option<Value>,
    region: &'static str,
}

impl Registration {
    /// A registration of `ca_certificate` in `SNI_ONLY` mode, which is the mode that
    /// carries no verification certificate: the shortest legal request, which the cases
    /// about the CA material itself vary one member of.
    fn sni_only(ca_certificate: &str) -> Self {
        Self {
            ca_certificate: ca_certificate.to_string(),
            verification_certificate: None,
            certificate_mode: Some(CERTIFICATE_MODE_SNI_ONLY),
            set_as_active: None,
            allow_auto_registration: None,
            registration_config: None,
            tags: None,
            region: REGION,
        }
    }
}

/// A self-signed certificate AUTHORITY under `common_name`, valid over the period the case
/// names, as the PEM a caller registers.
///
/// Every call generates a fresh key pair, so two certificates under one common name are
/// different material with the same SUBJECT — which is what the documented per-subject
/// limit counts.
fn ca_certificate(common_name: &str, not_before_year: i32, not_after_year: i32) -> String {
    certificate(
        common_name,
        not_before_year,
        not_after_year,
        ::rcgen::IsCa::Ca(::rcgen::BasicConstraints::Unconstrained),
    )
}

/// An END-ENTITY certificate under `common_name`: a readable X.509 certificate whose
/// `basicConstraints` extension asserts `cA FALSE`, which is what RFC 5280 section 4.2.1.9
/// says a certificate that is not a certificate authority carries.
fn end_entity_certificate(common_name: &str) -> String {
    certificate(common_name, 2020, 2040, ::rcgen::IsCa::ExplicitNoCa)
}

/// The certificate the two helpers above differ only in the `basicConstraints` of.
fn certificate(
    common_name: &str,
    not_before_year: i32,
    not_after_year: i32,
    is_ca: ::rcgen::IsCa,
) -> String {
    let key = ::rcgen::KeyPair::generate().expect("a key pair is generated");
    let mut params =
        ::rcgen::CertificateParams::new(Vec::<String>::new()).expect("certificate parameters");
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(::rcgen::DnType::CommonName, common_name);
    params.distinguished_name = subject;
    params.not_before = ::rcgen::date_time_ymd(not_before_year, 1, 1);
    params.not_after = ::rcgen::date_time_ymd(not_after_year, 1, 1);
    params.is_ca = is_ca;
    params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem()
}

/// The `certificateId` a registration of `pem` must report: the SHA-256 digest of the
/// certificate's own DER, hex encoded — computed here from the material the case
/// registered, so the assertion does not read it back out of the answer it is checking.
fn expected_certificate_id(pem: &str) -> String {
    let der = Certificate::from_pem(pem.trim().as_bytes())
        .expect("the case's material is a certificate")
        .to_der()
        .expect("the certificate re-encodes");
    let mut hasher = Sha256::new();
    hasher.update(&der);
    ::hex::encode(hasher.finalize())
}

/// The ARN a registered CA certificate carries.
fn expected_certificate_arn(region: &str, certificate_id: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:cacert/{certificate_id}")
}

/// The epoch second `rcgen::date_time_ymd(year, 1, 1)` names, which is the bound the
/// registered certificate carries and therefore the bound the description must report.
fn january_first(year: i32) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    {
        ::rcgen::date_time_ymd(year, 1, 1).unix_timestamp() as f64
    }
}

/// The message AWS IoT refuses material it cannot read as a certificate with.
fn unreadable_message() -> String {
    "Invalid certificate: the caCertificate field is not a PEM-encoded X.509 certificate"
        .to_string()
}

/// The message it refuses a readable certificate that is not a certificate authority with.
fn not_a_ca_message() -> String {
    "Invalid certificate: the caCertificate field is not a certificate authority (its \
     basicConstraints extension does not assert cA TRUE)"
        .to_string()
}

/// The message a `DEFAULT`-mode registration carrying no verification certificate is
/// refused with: "If certificateMode is DEFAULT or not provided, the
/// verificationCertificate field must not be empty"
/// (`smithy_query(query="operation",operation="RegisterCACertificate")`).
fn verification_required_message() -> String {
    "The verificationCertificate field must not be empty when certificateMode is DEFAULT"
        .to_string()
}

/// The message an `SNI_ONLY`-mode registration carrying one is refused with: "If
/// certificateMode is SNI_ONLY, the verificationCertificate field must be empty".
fn verification_not_allowed_message() -> String {
    "The verificationCertificate field must be empty when certificateMode is SNI_ONLY".to_string()
}

/// The message the eleventh registration of one CA subject field is refused with: "You can
/// register up to 10 CA certificates with the same CA subject field per Amazon Web Services
/// account" (`smithy_query(query="operation",operation="RegisterCACertificate")`).
fn subject_limit_message() -> String {
    format!(
        "You can register up to {MAX_CA_CERTIFICATES_PER_SUBJECT} CA certificates with the same \
         CA subject field per Amazon Web Services account"
    )
}

/// The message a registration past the emulator's own capacity is refused with.
fn registry_full_message() -> String {
    format!("this emulator registers at most {MAX_CA_CERTIFICATES} CA certificates")
}

/// The message a registration of material the account already holds is refused with.
fn already_exists_message(certificate_id: &str) -> String {
    format!("Certificate already exists (certificateId: {certificate_id})")
}

/// The message every reader of ONE registered CA certificate states a miss with: the CA
/// plane words it after the resource the request addressed, so the whole sentence is fixed
/// text and the requested id is no part of it.
fn missing_message() -> String {
    "Requested CA Certificate does not exist.".to_string()
}

/// One case of the registration table: a request, and what the record it writes must look
/// like.
struct Case {
    label: &'static str,
    mode: Option<&'static str>,
    set_as_active: Option<bool>,
    allow_auto_registration: Option<bool>,
    subject: &'static str,
    not_before_year: i32,
    not_after_year: i32,
    registration_config: Option<Value>,
    tags: Option<Value>,
    region: &'static str,
}

/// The table: the forms the two boolean query members take on the wire, each
/// `CertificateMode` plus the mode left out, metadata present and absent, several subjects
/// and validity periods, and a region other than the harness default.
fn registration_cases() -> Vec<Case> {
    vec![
        Case {
            label: "no mode, no setAsActive, no allowAutoRegistration",
            mode: None,
            set_as_active: None,
            allow_auto_registration: None,
            subject: "no-mode-ca.example.com",
            not_before_year: 2020,
            not_after_year: 2040,
            registration_config: None,
            tags: None,
            region: REGION,
        },
        Case {
            label: "DEFAULT mode, setAsActive=false, allowAutoRegistration=false",
            mode: Some(CERTIFICATE_MODE_DEFAULT),
            set_as_active: Some(false),
            allow_auto_registration: Some(false),
            subject: "default-inactive-ca.example.com",
            not_before_year: 2021,
            not_after_year: 2031,
            registration_config: None,
            tags: None,
            region: REGION,
        },
        Case {
            label: "DEFAULT mode, setAsActive=true, allowAutoRegistration=true, tagged",
            mode: Some(CERTIFICATE_MODE_DEFAULT),
            set_as_active: Some(true),
            allow_auto_registration: Some(true),
            subject: "default-active-ca.example.com",
            not_before_year: 2019,
            not_after_year: 2049,
            registration_config: Some(json!({
                "templateName": "FleetProvisioning",
                "templateBody": "{\"Resources\":{}}",
                "roleArn": "arn:aws:iam::000000000000:role/IoTRegistration"
            })),
            tags: Some(json!([
                { "Key": "fleet", "Value": "north" },
                { "Key": "owner" }
            ])),
            region: REGION,
        },
        Case {
            label: "SNI_ONLY mode, setAsActive=true, no allowAutoRegistration",
            mode: Some(CERTIFICATE_MODE_SNI_ONLY),
            set_as_active: Some(true),
            allow_auto_registration: None,
            subject: "sni-only-ca.example.com",
            not_before_year: 2023,
            not_after_year: 2033,
            registration_config: None,
            tags: None,
            region: REGION,
        },
        Case {
            label: "SNI_ONLY mode signed for another region",
            mode: Some(CERTIFICATE_MODE_SNI_ONLY),
            set_as_active: None,
            allow_auto_registration: Some(true),
            subject: "eu-ca.example.com",
            not_before_year: 2022,
            not_after_year: 2032,
            registration_config: None,
            tags: None,
            region: "eu-west-1",
        },
    ]
}

#[tokio::test]
async fn registering_a_ca_certificate_records_what_the_material_says() {
    let emulator = Emulator::new();

    for case in registration_cases() {
        let label = case.label;
        let pem = ca_certificate(case.subject, case.not_before_year, case.not_after_year);
        // A `DEFAULT` registration carries a verification certificate and an `SNI_ONLY`
        // one carries none, which is the rule the next test pins; here each case sends the
        // member its own mode requires.
        let verification_certificate = match case.mode {
            Some(CERTIFICATE_MODE_SNI_ONLY) => None,
            _ => Some(ca_certificate("verification.example.com", 2020, 2040)),
        };
        let registration = Registration {
            ca_certificate: pem.clone(),
            verification_certificate,
            certificate_mode: case.mode,
            set_as_active: case.set_as_active,
            allow_auto_registration: case.allow_auto_registration,
            registration_config: case.registration_config.clone(),
            tags: case.tags.clone(),
            region: case.region,
        };

        let registered = emulator.register(&registration).await;
        assert_eq!(
            registered.status, 200,
            "case {label:?}: RegisterCACertificate must succeed, got {registered:?}"
        );

        let certificate_id = expected_certificate_id(&pem);
        assert_eq!(
            registered.text("certificateId"),
            Some(certificate_id.clone()),
            "case {label:?}: the id must be the SHA-256 of the certificate's DER, \
             got {registered:?}"
        );
        assert_eq!(
            registered.text("certificateArn"),
            Some(expected_certificate_arn(case.region, &certificate_id)),
            "case {label:?}: the ARN must be scoped to the signed region, got {registered:?}"
        );
        assert_eq!(
            certificate_id.len(),
            64,
            "case {label:?}: CertificateId is modelled length 64..64, got {certificate_id:?}"
        );
        assert!(
            certificate_id
                .chars()
                .all(|character| character.is_ascii_hexdigit()),
            "case {label:?}: CertificateId is modelled ^(0x)?[a-fA-F0-9]+$, got {certificate_id:?}"
        );

        // The reader reports the record the registration wrote, member for member.
        let read = emulator.describe(case.region, &certificate_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the registered CA certificate must be readable, got {read:?}"
        );
        assert_eq!(
            read.described_text("certificateId"),
            Some(certificate_id.clone()),
            "case {label:?}: expected the registered id, got {read:?}"
        );
        assert_eq!(
            read.described_text("certificateArn"),
            Some(expected_certificate_arn(case.region, &certificate_id)),
            "case {label:?}: expected the registered ARN, got {read:?}"
        );
        assert_eq!(
            read.described_text("certificatePem").as_deref(),
            Some(pem.as_str()),
            "case {label:?}: the reader must hand back the registered material, got {read:?}"
        );
        assert_eq!(
            read.described_text("ownedBy").as_deref(),
            Some(ACCOUNT_ID),
            "case {label:?}: expected the owning account, got {read:?}"
        );
        let expected_status = if case.set_as_active == Some(true) {
            CA_CERTIFICATE_STATUS_ACTIVE
        } else {
            CA_CERTIFICATE_STATUS_INACTIVE
        };
        assert_eq!(
            read.described_text("status").as_deref(),
            Some(expected_status),
            "case {label:?}: expected status {expected_status}, got {read:?}"
        );
        let expected_auto_registration = if case.allow_auto_registration == Some(true) {
            AUTO_REGISTRATION_ENABLE
        } else {
            AUTO_REGISTRATION_DISABLE
        };
        assert_eq!(
            read.described_text("autoRegistrationStatus").as_deref(),
            Some(expected_auto_registration),
            "case {label:?}: expected autoRegistrationStatus {expected_auto_registration}, \
             got {read:?}"
        );
        let expected_mode = case.mode.unwrap_or(CERTIFICATE_MODE_DEFAULT);
        assert_eq!(
            read.described_text("certificateMode").as_deref(),
            Some(expected_mode),
            "case {label:?}: expected certificateMode {expected_mode}, got {read:?}"
        );
        assert_eq!(
            read.described("customerVersion").and_then(Value::as_i64),
            Some(i64::from(FIRST_CERTIFICATE_CUSTOMER_VERSION)),
            "case {label:?}: a registered CA certificate is at its first customer version, \
             got {read:?}"
        );
        assert!(
            read.described_text("generationId")
                .is_some_and(|generation| !generation.is_empty()),
            "case {label:?}: the record must carry a generationId, got {read:?}"
        );
        assert_eq!(
            read.validity_seconds("notBefore"),
            Some(january_first(case.not_before_year)),
            "case {label:?}: the validity must be the certificate's own, got {read:?}"
        );
        assert_eq!(
            read.validity_seconds("notAfter"),
            Some(january_first(case.not_after_year)),
            "case {label:?}: the validity must be the certificate's own, got {read:?}"
        );
        let creation_date = read.described("creationDate").and_then(Value::as_f64);
        assert!(
            creation_date.is_some(),
            "case {label:?}: the record must carry a creationDate, got {read:?}"
        );
        assert_eq!(
            read.described("lastModifiedDate").and_then(Value::as_f64),
            creation_date,
            "case {label:?}: nothing has modified the record since it was registered, \
             got {read:?}"
        );

        // `registrationConfig` is reported outside the description, exactly as the
        // registration named it, and is absent when it named none.
        assert_eq!(
            read.body.get("registrationConfig").cloned(),
            case.registration_config.clone(),
            "case {label:?}: expected the registered registrationConfig, got {read:?}"
        );

        // The CA registry is not the DEVICE certificate store: the id of a registered CA
        // certificate names nothing for the reader of a device certificate.
        let device = emulator.describe_device_certificate(&certificate_id).await;
        assert_eq!(
            device.status, 404,
            "case {label:?}: a CA certificate must not be visible as a device certificate, \
             got {device:?}"
        );
    }
}

#[tokio::test]
async fn registering_the_same_material_twice_is_refused_already_exists() {
    let emulator = Emulator::new();

    // The id is the material's own fingerprint, so a second registration of the same
    // certificate is a collision however the rest of the request differs.
    let pem = ca_certificate("repeat-ca.example.com", 2020, 2040);
    let certificate_id = expected_certificate_id(&pem);
    let first = emulator.register(&Registration::sni_only(&pem)).await;
    assert_eq!(
        first.status, 200,
        "the first registration must succeed, got {first:?}"
    );
    let described = emulator.describe(REGION, &certificate_id).await;

    for (label, set_as_active) in [
        ("the same request again", None),
        ("the same material, setAsActive=true", Some(true)),
    ] {
        let mut repeat = Registration::sni_only(&pem);
        repeat.set_as_active = set_as_active;
        let refused = emulator.register(&repeat).await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: registering held material must be refused 409, got {refused:?}"
        );
        assert_eq!(
            refused.code(),
            Some("ResourceAlreadyExistsException"),
            "case {label:?}: expected ResourceAlreadyExistsException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            already_exists_message(&certificate_id),
            "case {label:?}: expected {:?}, got {refused:?}",
            already_exists_message(&certificate_id)
        );
        assert_eq!(
            refused.text("resourceId"),
            Some(certificate_id.clone()),
            "case {label:?}: the refusal must name the id that exists, got {refused:?}"
        );
        assert_eq!(
            refused.text("resourceArn"),
            Some(expected_certificate_arn(REGION, &certificate_id)),
            "case {label:?}: the refusal must name the ARN that exists, got {refused:?}"
        );

        // The refusal wrote nothing: the record is exactly what the first registration
        // left, its status included.
        let read = emulator.describe(REGION, &certificate_id).await;
        assert_eq!(
            read.body, described.body,
            "case {label:?}: a refused registration must change nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_mode_and_a_verification_certificate_that_disagree_are_refused() {
    let emulator = Emulator::new();

    // "If certificateMode is SNI_ONLY, the verificationCertificate field must be empty. If
    // certificateMode is DEFAULT or not provided, the verificationCertificate field must
    // not be empty."
    let cases: Vec<(&str, Option<&'static str>, Option<String>, String)> = vec![
        (
            "no mode and no verification certificate",
            None,
            None,
            verification_required_message(),
        ),
        (
            "DEFAULT mode and no verification certificate",
            Some(CERTIFICATE_MODE_DEFAULT),
            None,
            verification_required_message(),
        ),
        (
            "DEFAULT mode and an empty verification certificate",
            Some(CERTIFICATE_MODE_DEFAULT),
            Some("   ".to_string()),
            verification_required_message(),
        ),
        (
            "SNI_ONLY mode and a verification certificate",
            Some(CERTIFICATE_MODE_SNI_ONLY),
            Some(ca_certificate("verification.example.com", 2020, 2040)),
            verification_not_allowed_message(),
        ),
    ];

    for (label, mode, verification_certificate, expected) in cases {
        let pem = ca_certificate("disagreeing-ca.example.com", 2020, 2040);
        let registration = Registration {
            certificate_mode: mode,
            verification_certificate,
            ..Registration::sni_only(&pem)
        };

        let refused = emulator.register(&registration).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: the request must be refused 400, got {refused:?}"
        );
        assert_eq!(
            refused.code(),
            Some("InvalidRequestException"),
            "case {label:?}: expected InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {label:?}: expected {expected:?}, got {refused:?}"
        );

        // The refusal recorded nothing: the id the material would have been held under
        // names no CA certificate.
        let read = emulator
            .describe(REGION, &expected_certificate_id(&pem))
            .await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: a refused registration must record nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn material_that_is_not_a_ca_certificate_is_refused_certificate_validation() {
    let emulator = Emulator::new();

    // The certificate this service itself issues is an end-entity certificate, so it is
    // one case of the rule rather than a special one.
    let issued = emulator.create_device_certificate().await;
    assert_eq!(
        issued.status, 200,
        "CreateKeysAndCertificate must succeed, got {issued:?}"
    );
    let device_certificate_pem = issued
        .text("certificatePem")
        .expect("the issue reports a certificatePem");

    let cases: Vec<(&str, String, String)> = vec![
        (
            "plain text",
            "this is not a certificate".to_string(),
            unreadable_message(),
        ),
        (
            "a PEM block whose body is not DER",
            "-----BEGIN CERTIFICATE-----\nZm9vYmFy\n-----END CERTIFICATE-----\n".to_string(),
            unreadable_message(),
        ),
        (
            "a PEM header with no footer",
            "-----BEGIN CERTIFICATE-----\n".to_string(),
            unreadable_message(),
        ),
        (
            "whitespace only",
            " \n ".to_string(),
            unreadable_message(),
        ),
        (
            "a certificate signing request, not a certificate",
            "-----BEGIN CERTIFICATE REQUEST-----\nZm9vYmFy\n-----END CERTIFICATE REQUEST-----\n"
                .to_string(),
            unreadable_message(),
        ),
        (
            "an end-entity certificate",
            end_entity_certificate("leaf.example.com"),
            not_a_ca_message(),
        ),
        (
            "the device certificate this service issues",
            device_certificate_pem,
            not_a_ca_message(),
        ),
    ];

    for (label, material, expected) in cases {
        let refused = emulator
            .register(&Registration::sni_only(&material))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: material that is not a CA certificate must be refused 400, \
             got {refused:?}"
        );
        assert_eq!(
            refused.code(),
            Some("CertificateValidationException"),
            "case {label:?}: expected CertificateValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {label:?}: expected {expected:?}, got {refused:?}"
        );
    }

    // A verification certificate is certificate material too: a `DEFAULT` registration
    // whose verification certificate cannot be read is refused the same way, and the CA
    // material it carried is recorded nowhere.
    let pem = ca_certificate("verified-ca.example.com", 2020, 2040);
    let registration = Registration {
        certificate_mode: Some(CERTIFICATE_MODE_DEFAULT),
        verification_certificate: Some("not a certificate either".to_string()),
        ..Registration::sni_only(&pem)
    };
    let refused = emulator.register(&registration).await;
    assert_eq!(
        refused.status, 400,
        "an unreadable verification certificate must be refused 400, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        Some("CertificateValidationException"),
        "expected CertificateValidationException, got {refused:?}"
    );
    let read = emulator
        .describe(REGION, &expected_certificate_id(&pem))
        .await;
    assert_eq!(
        read.status, 404,
        "a refused registration must record nothing, got {read:?}"
    );
}

#[tokio::test]
async fn one_ca_subject_field_may_be_registered_up_to_the_documented_limit() {
    let emulator = Emulator::new();

    // "You can register up to 10 CA certificates with the same CA subject field per Amazon
    // Web Services account": the limit counts the SUBJECT the certificates carry, so each
    // registration below is different material under one subject.
    let subject = "shared-subject-ca.example.com";
    let mut registered_ids: Vec<String> = Vec::new();
    for index in 0..MAX_CA_CERTIFICATES_PER_SUBJECT {
        let pem = ca_certificate(subject, 2020, 2040);
        let accepted = emulator.register(&Registration::sni_only(&pem)).await;
        assert_eq!(
            accepted.status, 200,
            "registration {index} under one subject must succeed, got {accepted:?}"
        );
        registered_ids.push(expected_certificate_id(&pem));
    }

    let past_the_limit = ca_certificate(subject, 2020, 2040);
    let refused = emulator
        .register(&Registration::sni_only(&past_the_limit))
        .await;
    assert_eq!(
        refused.status, 410,
        "the registration past the limit must be refused 410, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        Some("LimitExceededException"),
        "expected LimitExceededException, got {refused:?}"
    );
    assert_eq!(
        refused.message(),
        subject_limit_message(),
        "expected {:?}, got {refused:?}",
        subject_limit_message()
    );
    let read = emulator
        .describe(REGION, &expected_certificate_id(&past_the_limit))
        .await;
    assert_eq!(
        read.status, 404,
        "the refused registration must record nothing, got {read:?}"
    );

    // Every certificate the limit admitted is still readable, and the limit is per
    // SUBJECT: another subject registers afterwards.
    for certificate_id in &registered_ids {
        let held = emulator.describe(REGION, certificate_id).await;
        assert_eq!(
            held.status, 200,
            "the CA certificate {certificate_id} must still be readable, got {held:?}"
        );
    }
    let other_subject = ca_certificate("other-subject-ca.example.com", 2020, 2040);
    let accepted = emulator
        .register(&Registration::sni_only(&other_subject))
        .await;
    assert_eq!(
        accepted.status, 200,
        "another CA subject must still register, got {accepted:?}"
    );
}

#[tokio::test]
async fn a_registration_carrying_more_tags_than_a_resource_may_hold_is_refused() {
    let emulator = Emulator::new();

    // AWS IoT's tagging limit is 50 tags on a resource, and `TagList` carries no modelled
    // maximum, so the request past it is refused rather than recorded.
    let pem = ca_certificate("tagged-ca.example.com", 2020, 2040);
    let tags: Vec<Value> = (0..=MAX_POLICY_TAGS)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": format!("value-{index}") }))
        .collect();
    let registration = Registration {
        tags: Some(Value::Array(tags)),
        ..Registration::sni_only(&pem)
    };

    let refused = emulator.register(&registration).await;
    assert_eq!(
        refused.status, 400,
        "a registration past the tag limit must be refused 400, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        Some("InvalidRequestException"),
        "expected InvalidRequestException, got {refused:?}"
    );
    let read = emulator
        .describe(REGION, &expected_certificate_id(&pem))
        .await;
    assert_eq!(
        read.status, 404,
        "the refused registration must record nothing, got {read:?}"
    );

    // Exactly the limit is accepted, so what was refused is the count past it.
    let at_the_limit = ca_certificate("tagged-ca.example.com", 2020, 2040);
    let tags: Vec<Value> = (0..MAX_POLICY_TAGS)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": format!("value-{index}") }))
        .collect();
    let registration = Registration {
        tags: Some(Value::Array(tags)),
        ..Registration::sni_only(&at_the_limit)
    };
    let accepted = emulator.register(&registration).await;
    assert_eq!(
        accepted.status, 200,
        "a registration at the tag limit must succeed, got {accepted:?}"
    );
}

#[tokio::test]
async fn the_registry_stops_growing_at_the_emulators_own_ceiling() {
    let emulator = Emulator::new();

    // The registry is held in memory and a CALLER grows it, one registration at a time, so
    // the account's whole registry is bounded as well as each subject: the registration
    // past the ceiling is refused rather than recorded. Each subject takes its documented
    // ten certificates, so what the last registration below meets is the registry ceiling
    // and not the per-subject one.
    let mut last_id = String::new();
    for index in 0..MAX_CA_CERTIFICATES {
        let subject = format!(
            "bulk-{}-ca.example.com",
            index / MAX_CA_CERTIFICATES_PER_SUBJECT
        );
        let pem = ca_certificate(&subject, 2020, 2040);
        let accepted = emulator.register(&Registration::sni_only(&pem)).await;
        assert_eq!(
            accepted.status, 200,
            "registration {index} up to the ceiling must succeed, got {accepted:?}"
        );
        last_id = expected_certificate_id(&pem);
    }

    // A subject nothing is registered under yet, so what refuses it is the registry
    // ceiling.
    let past_the_ceiling = ca_certificate("overflow-ca.example.com", 2020, 2040);
    let refused = emulator
        .register(&Registration::sni_only(&past_the_ceiling))
        .await;
    assert_eq!(
        refused.status, 410,
        "the registration past the ceiling must be refused 410, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        Some("LimitExceededException"),
        "expected LimitExceededException, got {refused:?}"
    );
    assert_eq!(
        refused.message(),
        registry_full_message(),
        "expected {:?}, got {refused:?}",
        registry_full_message()
    );
    let read = emulator
        .describe(REGION, &expected_certificate_id(&past_the_ceiling))
        .await;
    assert_eq!(
        read.status, 404,
        "the refused registration must record nothing, got {read:?}"
    );

    // What the ceiling admitted is untouched.
    let held = emulator.describe(REGION, &last_id).await;
    assert_eq!(
        held.status, 200,
        "the last admitted CA certificate must still be readable, got {held:?}"
    );
}

#[tokio::test]
async fn describing_an_unregistered_ca_certificate_is_refused_not_found() {
    let emulator = Emulator::new();

    // One CA certificate is registered, so the refusals below are about the id and not
    // about an empty registry.
    let pem = ca_certificate("held-ca.example.com", 2020, 2040);
    let registered = emulator.register(&Registration::sni_only(&pem)).await;
    assert_eq!(
        registered.status, 200,
        "the registration must succeed, got {registered:?}"
    );
    let held = expected_certificate_id(&pem);

    // `CertificateId` is modelled `length 64..64, pattern ^(0x)?[a-fA-F0-9]+$`
    // (`smithy_query(query="operation",operation="DescribeCACertificate")`), so every id
    // below is a legal one the registry simply does not hold: the lower and upper hex
    // digits, the `0x` prefix the pattern allows, and the held id with its last character
    // changed.
    let mut neighbour = held.clone();
    neighbour.pop();
    neighbour.push(if held.ends_with('0') { '1' } else { '0' });
    for certificate_id in [
        "0".repeat(64),
        "f".repeat(64),
        "ABCDEF".repeat(10) + "abcd",
        format!("0x{}", "a".repeat(62)),
        neighbour,
    ] {
        let refused = emulator.describe(REGION, &certificate_id).await;
        assert_eq!(
            refused.status, 404,
            "case {certificate_id:?}: an unheld id must be refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.code(),
            Some("ResourceNotFoundException"),
            "case {certificate_id:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            missing_message(),
            "case {certificate_id:?}: expected {:?}, got {refused:?}",
            missing_message()
        );
    }

    let read = emulator.describe(REGION, &held).await;
    assert_eq!(
        read.status, 200,
        "the registered CA certificate must still be readable, got {read:?}"
    );
}
