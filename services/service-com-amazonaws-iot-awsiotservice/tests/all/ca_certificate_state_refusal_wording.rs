// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the SENTENCE the certificate planes refuse a delete of a
//! still-`ACTIVE` certificate with, on both planes at once.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: a refusal names the RESOURCE the request addressed, so the CA
//! plane (`DeleteCACertificate`, `DELETE /cacertificate/{certificateId}`) states its
//! `CertificateStateException` (HTTP 406) as "CA Certificate must be deactivated (not
//! ACTIVE) before deletion. Id: <certificateId>" while the DEVICE plane
//! (`DeleteCertificate`, `DELETE /certificates/{certificateId}`) states the same
//! precondition without the qualifier — the whole sentence up to the id is fixed text and
//! the id is the only variable part, taken from the request URI. A client that branches on
//! the message can therefore tell which plane refused it, because the two are remediated
//! differently (`UpdateCACertificate` against `UpdateCertificate`).
//!
//! The table spans what that rule generalises over rather than one request: the CA
//! registrations that reach the `ACTIVE` status in every form the registration takes — both
//! `CertificateMode` values (`SNI_ONLY`, which carries no verification certificate, and
//! `DEFAULT`, which must), `allowAutoRegistration` absent and explicitly `true`, a
//! `registrationConfig` present and absent, and a CA that a device certificate was
//! registered under — each under its own CA subject and therefore its own minted
//! `certificateId`, which is what shows the sentence tracks the id the request carried.
//! The statuses and ids come from the operations themselves (`RegisterCACertificate` /
//! `DescribeCACertificate`, `CreateKeysAndCertificate` / `DescribeCertificate`), never from
//! a literal in this file.
//!
//! The refusal names its own remediation, so what REMEDIATES it is pinned here beside it:
//! `UpdateCACertificate` (`PUT /cacertificate/{certificateId}`) writes the members the
//! request names onto the stored record — `newStatus`, `newAutoRegistrationStatus`,
//! `registrationConfig`, and `removeAutoRegistration`, which takes the record out of auto
//! registration — leaves every member it does not name standing, advances the record's
//! `customerVersion` when it changed something and leaves it alone when it did not, and
//! refuses an id the registry holds nothing under with `ResourceNotFoundException` (404).
//! A CA deactivated through it is then deleted, which is the sequence the refusal's sentence
//! tells a caller to follow. The same wording rule covers that miss: every operation
//! addressing ONE registered CA certificate states it in the CA plane's own sentence, whose
//! fixed text is the whole sentence and carries no part of the requested id.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    AUTO_REGISTRATION_DISABLE, AUTO_REGISTRATION_ENABLE, CA_CERTIFICATE_STATUS_ACTIVE,
    CA_CERTIFICATE_STATUS_INACTIVE, CERTIFICATE_MODE_DEFAULT, CERTIFICATE_MODE_SNI_ONLY,
    CERTIFICATE_STATUS_ACTIVE,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite signs for.
const REGION: &str = "eu-west-2";

/// The fixed text of the CA plane's state refusal, up to the id the request carried.
const CA_STATE_REFUSAL: &str = "CA Certificate must be deactivated (not ACTIVE) before deletion. \
                                Id: ";

/// The fixed text of the DEVICE plane's state refusal — the same sentence without the CA
/// qualifier, which is what keeps the two planes' refusals distinguishable.
const DEVICE_STATE_REFUSAL: &str =
    "Certificate must be deactivated (not ACTIVE) before deletion. Id: ";

/// The sentence every operation addressing ONE registered CA certificate refuses an id the
/// account's CA registry holds none under with — fixed text, with no variable part.
const MISSING_CA_CERTIFICATE: &str = "Requested CA Certificate does not exist.";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The error shape the answer names, as a client reads it: the `x-amzn-errortype`
    /// header up to the `:` that introduces the shape's documentation URI.
    fn code(&self) -> String {
        self.error_code
            .as_deref()
            .map(|value| value.split(':').next().unwrap_or(value).to_string())
            .unwrap_or_default()
    }

    /// The `message` of an error body.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The whole body, for an assertion's failure text.
    fn text(&self) -> String {
        self.body.to_string()
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(value.to_string())
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .body(payload)
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
    /// when the case names none — and everything else in the body. Answers the minted
    /// `certificateId`.
    async fn register_ca(&self, case: &Case, authority: &Authority) -> String {
        let mut query = vec![format!("setAsActive={}", case.set_as_active)];
        if let Some(allow_auto_registration) = case.allow_auto_registration {
            query.push(format!("allowAutoRegistration={allow_auto_registration}"));
        }
        let mut body = json!({
            "caCertificate": authority.pem(),
            "certificateMode": case.certificate_mode,
        });
        if case.certificate_mode == CERTIFICATE_MODE_DEFAULT {
            // "If certificateMode is DEFAULT or not provided, the verificationCertificate
            // field must not be empty" (`RegisterCACertificate$verificationCertificate`).
            body["verificationCertificate"] = json!(end_entity_certificate(case.subject));
        }
        if let Some(configuration) = &case.registration_config {
            body["registrationConfig"] = configuration.clone();
        }
        let registered = self
            .call(
                "POST",
                &format!("/cacertificate?{}", query.join("&")),
                Some(&body),
            )
            .await;
        assert_eq!(
            registered.status,
            200,
            "case {}: expected the CA registration to answer 200, got {} body {}",
            case.label,
            registered.status,
            registered.text()
        );
        registered
            .body
            .get("certificateId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// `DescribeCACertificate`: `GET /cacertificate/{certificateId}`.
    async fn describe_ca(&self, certificate_id: &str) -> Reply {
        self.call("GET", &format!("/cacertificate/{certificate_id}"), None)
            .await
    }

    /// `DeleteCACertificate`: `DELETE /cacertificate/{certificateId}`.
    async fn delete_ca(&self, certificate_id: &str) -> Reply {
        self.call("DELETE", &format!("/cacertificate/{certificate_id}"), None)
            .await
    }

    /// `UpdateCACertificate`: `PUT /cacertificate/{certificateId}`, with `newStatus` and
    /// `newAutoRegistrationStatus` bound as `@httpQuery` parameters — absent from the query
    /// when the update names none — and `registrationConfig` and `removeAutoRegistration`
    /// in the body.
    async fn update_ca(&self, certificate_id: &str, update: &CaUpdate) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(new_status) = update.new_status {
            query.push(format!("newStatus={new_status}"));
        }
        if let Some(status) = update.new_auto_registration_status {
            query.push(format!("newAutoRegistrationStatus={status}"));
        }
        let path = if query.is_empty() {
            format!("/cacertificate/{certificate_id}")
        } else {
            format!("/cacertificate/{certificate_id}?{}", query.join("&"))
        };
        let mut body = json!({});
        if update.remove_auto_registration {
            body["removeAutoRegistration"] = json!(true);
        }
        if let Some(configuration) = &update.registration_config {
            body["registrationConfig"] = configuration.clone();
        }
        self.call("PUT", &path, Some(&body)).await
    }

    /// `RegisterCertificate`: `POST /certificate/register`, the leaf a CA signed — one of
    /// the forms a CA reaches the `ACTIVE` status in.
    async fn register_device_certificate(&self, case: &str, authority: &Authority) -> String {
        let registered = self
            .call(
                "POST",
                "/certificate/register?setAsActive=true",
                Some(&json!({
                    "certificatePem": authority.signed_device_certificate("leaf.example.com"),
                    "caCertificatePem": authority.pem(),
                })),
            )
            .await;
        assert_eq!(
            registered.status,
            200,
            "case {case}: expected the leaf registration to answer 200, got {} body {}",
            registered.status,
            registered.text()
        );
        registered
            .body
            .get("certificateId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, with `setAsActive` bound
    /// as an `@httpQuery` parameter — how the DEVICE plane gets an `ACTIVE` certificate.
    async fn create_device_certificate(&self) -> String {
        let created = self
            .call("POST", "/keys-and-certificate?setAsActive=true", None)
            .await;
        assert_eq!(
            created.status,
            200,
            "case a device certificate: expected 200, got {} body {}",
            created.status,
            created.text()
        );
        created
            .body
            .get("certificateId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// `DescribeCertificate`: `GET /certificates/{certificateId}`.
    async fn describe_certificate(&self, certificate_id: &str) -> Reply {
        self.call("GET", &format!("/certificates/{certificate_id}"), None)
            .await
    }

    /// `DeleteCertificate`: `DELETE /certificates/{certificateId}`.
    async fn delete_certificate(&self, certificate_id: &str) -> Reply {
        self.call("DELETE", &format!("/certificates/{certificate_id}"), None)
            .await
    }
}

/// One `UpdateCACertificate` request, as a case names it: every member is absent unless
/// the case names it, which is what leaves the record's own value standing.
#[derive(Default)]
struct CaUpdate {
    /// `newStatus` — `CACertificateStatus` is `ACTIVE | INACTIVE`.
    new_status: Option<&'static str>,
    /// `newAutoRegistrationStatus` — `AutoRegistrationStatus` is `ENABLE | DISABLE`.
    new_auto_registration_status: Option<&'static str>,
    /// `registrationConfig`.
    registration_config: Option<Value>,
    /// `removeAutoRegistration`, modelled `default false`.
    remove_auto_registration: bool,
}

impl CaUpdate {
    fn status(new_status: &'static str) -> Self {
        Self {
            new_status: Some(new_status),
            ..Self::default()
        }
    }

    fn auto_registration(new_auto_registration_status: &'static str) -> Self {
        Self {
            new_auto_registration_status: Some(new_auto_registration_status),
            ..Self::default()
        }
    }

    fn registration_config(configuration: Value) -> Self {
        Self {
            registration_config: Some(configuration),
            ..Self::default()
        }
    }

    fn removing_auto_registration() -> Self {
        Self {
            remove_auto_registration: true,
            ..Self::default()
        }
    }
}

/// One registration a case makes, as the case names it.
struct Case {    /// The label every assertion of the case names it by.
    label: &'static str,
    /// The CA's common name, which is the subject the material certifies under: a different
    /// one per case, so each case registers its own CA and gets its own `certificateId`.
    subject: &'static str,
    /// `certificateMode` — `SNI_ONLY` carries no verification certificate, `DEFAULT` one.
    certificate_mode: &'static str,
    /// `setAsActive` — every case of this table registers a CA that is to be `ACTIVE`, and
    /// the last one registers an `INACTIVE` CA to show the status is the precondition.
    set_as_active: bool,
    /// `allowAutoRegistration`, absent from the query when the case names none.
    allow_auto_registration: Option<bool>,
    /// `registrationConfig`, absent from the body when the case names none.
    registration_config: Option<Value>,
    /// Whether a device certificate signed by this CA is registered under it before the
    /// delete, as the recorded client's sequence does.
    with_registered_leaf: bool,
}

impl Case {
    /// A registration in `SNI_ONLY` mode that sets the CA `ACTIVE` and names nothing
    /// further: the shortest request that reaches the refused state, which the cases vary
    /// one member of.
    fn active(label: &'static str, subject: &'static str) -> Self {
        Self {
            label,
            subject,
            certificate_mode: CERTIFICATE_MODE_SNI_ONLY,
            set_as_active: true,
            allow_auto_registration: None,
            registration_config: None,
            with_registered_leaf: false,
        }
    }

    fn mode(mut self, certificate_mode: &'static str) -> Self {
        self.certificate_mode = certificate_mode;
        self
    }

    fn auto_registration(mut self, allow_auto_registration: bool) -> Self {
        self.allow_auto_registration = Some(allow_auto_registration);
        self
    }

    fn registration_config(mut self, configuration: Value) -> Self {
        self.registration_config = Some(configuration);
        self
    }

    fn with_registered_leaf(mut self) -> Self {
        self.with_registered_leaf = true;
        self
    }

    fn inactive(mut self) -> Self {
        self.set_as_active = false;
        self
    }
}

/// A certificate AUTHORITY of the caller's own: the key pair and the self-signed CA
/// certificate a case registers, plus the leaf material it signs.
struct Authority {
    key: ::rcgen::KeyPair,
    certificate: ::rcgen::Certificate,
}

impl Authority {
    fn new(common_name: &str) -> Self {
        let key = ::rcgen::KeyPair::generate().expect("a key pair is generated");
        let mut params = certificate_params(common_name);
        params.is_ca = ::rcgen::IsCa::Ca(::rcgen::BasicConstraints::Unconstrained);
        let certificate = params.self_signed(&key).expect("a self-signed authority");
        Self { key, certificate }
    }

    fn pem(&self) -> String {
        self.certificate.pem()
    }

    /// An END-ENTITY certificate signed BY this authority: a device certificate whose
    /// issuer is this authority's subject field.
    fn signed_device_certificate(&self, common_name: &str) -> String {
        let device_key = ::rcgen::KeyPair::generate().expect("a key pair is generated");
        let mut params = certificate_params(common_name);
        params.is_ca = ::rcgen::IsCa::ExplicitNoCa;
        params
            .signed_by(&device_key, &self.certificate, &self.key)
            .expect("a signed device certificate")
            .pem()
    }
}

/// An END-ENTITY certificate under `common_name`: readable material that is no authority,
/// which is what a `DEFAULT` registration's verification certificate is.
fn end_entity_certificate(common_name: &str) -> String {
    let key = ::rcgen::KeyPair::generate().expect("a key pair is generated");
    let mut params = certificate_params(common_name);
    params.is_ca = ::rcgen::IsCa::ExplicitNoCa;
    params.self_signed(&key).expect("a certificate").pem()
}

/// The parameters every piece of material in this suite is built from.
fn certificate_params(common_name: &str) -> ::rcgen::CertificateParams {
    let mut params =
        ::rcgen::CertificateParams::new(Vec::<String>::new()).expect("certificate parameters");
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(::rcgen::DnType::CommonName, common_name);
    params.distinguished_name = subject;
    params.not_before = ::rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = ::rcgen::date_time_ymd(2040, 1, 1);
    params
}

/// The `status` a `DescribeCACertificate` answer reports.
fn described_ca_status(reply: &Reply) -> String {
    described_member(reply, "status")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A member of the `certificateDescription` a `DescribeCACertificate` answer reports.
fn described_member(reply: &Reply, name: &str) -> Value {
    reply
        .body
        .get("certificateDescription")
        .and_then(|description| description.get(name))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `registrationConfig` a `DescribeCACertificate` answer reports, which the modelled
/// output shape carries OUTSIDE the description.
fn described_registration_config(reply: &Reply) -> Value {
    reply
        .body
        .get("registrationConfig")
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULE: `DeleteCACertificate` refuses a CA certificate whose status is `ACTIVE` with
/// `CertificateStateException` (HTTP 406) worded after the CA certificate the request
/// addressed — "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: " is
/// the whole fixed text and the `certificateId` from the request URI is the only variable
/// part — whatever form the registration that made it `ACTIVE` took.
#[tokio::test]
async fn an_active_ca_certificate_delete_is_refused_with_the_ca_plane_sentence() {
    let emulator = Emulator::new();

    let cases = [
        Case::active("an SNI_ONLY CA", "sni-only-ca.example.com"),
        Case::active("a DEFAULT-mode CA", "default-mode-ca.example.com")
            .mode(CERTIFICATE_MODE_DEFAULT),
        Case::active("a CA that auto-registers", "auto-registering-ca.example.com")
            .auto_registration(true),
        Case::active("a CA that does not auto-register", "no-auto-ca.example.com")
            .auto_registration(false),
        Case::active("a CA carrying a registrationConfig", "configured-ca.example.com")
            .registration_config(json!({ "templateBody": "{\"Resources\":{}}" })),
        Case::active("a CA with a registered leaf", "signing-ca.example.com")
            .with_registered_leaf(),
    ];

    let mut refused_ids: Vec<String> = Vec::new();
    for case in &cases {
        let authority = Authority::new(case.subject);
        let certificate_id = emulator.register_ca(case, &authority).await;
        if case.with_registered_leaf {
            emulator
                .register_device_certificate(case.label, &authority)
                .await;
        }

        let described = emulator.describe_ca(&certificate_id).await;
        assert_eq!(
            described_ca_status(&described),
            CA_CERTIFICATE_STATUS_ACTIVE,
            "case {}: expected the registered CA to be reported {CA_CERTIFICATE_STATUS_ACTIVE}, \
             got body {}",
            case.label,
            described.text()
        );

        let refused = emulator.delete_ca(&certificate_id).await;
        assert_eq!(
            (refused.status, refused.code(), refused.message()),
            (
                406,
                "CertificateStateException".to_string(),
                format!("{CA_STATE_REFUSAL}{certificate_id}")
            ),
            "case {}: expected 406 CertificateStateException worded \
             {CA_STATE_REFUSAL}{certificate_id}, got {} {} body {}",
            case.label,
            refused.status,
            refused.code(),
            refused.text()
        );

        // Nothing was removed: the refusal is the whole answer.
        assert_eq!(
            emulator.describe_ca(&certificate_id).await.status,
            200,
            "case {}: a refused delete must leave the CA certificate readable",
            case.label
        );
        refused_ids.push(certificate_id);
    }

    // The id is the sentence's only variable part, so a second CA is refused with its OWN
    // id rather than with any other case's.
    for (index, certificate_id) in refused_ids.iter().enumerate() {
        assert!(
            refused_ids
                .iter()
                .enumerate()
                .all(|(other, candidate)| other == index || candidate != certificate_id),
            "case {}: every case must be refused under its own minted id, got {certificate_id:?} \
             twice in {refused_ids:?}",
            cases[index].label
        );
    }
}

/// RULE (the other side of the same rule): the DEVICE certificate plane states the same
/// precondition WITHOUT the CA qualifier, so the two refusals — remediated by
/// `UpdateCACertificate` and by `UpdateCertificate` respectively — are distinguishable by
/// their message alone.
#[tokio::test]
async fn the_two_certificate_planes_word_the_state_refusal_differently() {
    let emulator = Emulator::new();

    let authority = Authority::new("both-planes-ca.example.com");
    let ca_id = emulator
        .register_ca(
            &Case::active("both planes", "both-planes-ca.example.com"),
            &authority,
        )
        .await;
    let certificate_id = emulator.create_device_certificate().await;
    assert_eq!(
        emulator
            .describe_certificate(&certificate_id)
            .await
            .body
            .get("certificateDescription")
            .and_then(|description| description.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        CERTIFICATE_STATUS_ACTIVE,
        "case both planes: the device certificate must be {CERTIFICATE_STATUS_ACTIVE} for its \
         delete to be refused"
    );

    let ca_refusal = emulator.delete_ca(&ca_id).await;
    let device_refusal = emulator.delete_certificate(&certificate_id).await;

    assert_eq!(
        (
            ca_refusal.status,
            ca_refusal.code(),
            ca_refusal.message()
        ),
        (
            406,
            "CertificateStateException".to_string(),
            format!("{CA_STATE_REFUSAL}{ca_id}")
        ),
        "case the CA plane: expected the CA-worded refusal, got {} {} body {}",
        ca_refusal.status,
        ca_refusal.code(),
        ca_refusal.text()
    );
    assert_eq!(
        (
            device_refusal.status,
            device_refusal.code(),
            device_refusal.message()
        ),
        (
            406,
            "CertificateStateException".to_string(),
            format!("{DEVICE_STATE_REFUSAL}{certificate_id}")
        ),
        "case the device plane: expected the device-worded refusal, got {} {} body {}",
        device_refusal.status,
        device_refusal.code(),
        device_refusal.text()
    );
    assert_ne!(
        ca_refusal.message().replace(&ca_id, ""),
        device_refusal.message().replace(&certificate_id, ""),
        "case both planes: the two planes' refusals must differ in their fixed text, got {:?} \
         and {:?}",
        ca_refusal.message(),
        device_refusal.message()
    );
}

/// RULE: the status is the precondition the sentence states, so a CA certificate that was
/// never made `ACTIVE` is deleted — the refusal is about the state and not about the
/// operation.
#[tokio::test]
async fn an_inactive_ca_certificate_is_deleted_rather_than_refused() {
    let emulator = Emulator::new();

    for case in [
        Case::active("an inactive SNI_ONLY CA", "inactive-sni-ca.example.com").inactive(),
        Case::active("an inactive DEFAULT-mode CA", "inactive-default-ca.example.com")
            .mode(CERTIFICATE_MODE_DEFAULT)
            .inactive(),
    ] {
        let authority = Authority::new(case.subject);
        let certificate_id = emulator.register_ca(&case, &authority).await;
        let deleted = emulator.delete_ca(&certificate_id).await;
        assert_eq!(
            deleted.status,
            200,
            "case {}: expected a delete of a non-ACTIVE CA to answer 200, got {} body {}",
            case.label,
            deleted.status,
            deleted.text()
        );
        assert_eq!(
            emulator.describe_ca(&certificate_id).await.status,
            404,
            "case {}: the deleted CA certificate must no longer be readable",
            case.label
        );
    }
}

/// RULE: the refusal's remediation is the CA plane's own update — `UpdateCACertificate`
/// writes the `newStatus` it names onto the stored record, so the CA a refused delete left
/// `ACTIVE` is deactivated and then deleted, and the write is a customer-initiated change
/// to the record: it moves the record to its next `customerVersion`.
#[tokio::test]
async fn deactivating_a_ca_certificate_remediates_the_refused_delete() {
    let emulator = Emulator::new();

    let case = Case::active("a remediated CA", "remediated-ca.example.com");
    let authority = Authority::new(case.subject);
    let certificate_id = emulator.register_ca(&case, &authority).await;

    let registered_version = described_member(&emulator.describe_ca(&certificate_id).await, "customerVersion")
        .as_i64()
        .unwrap_or_default();
    let refused = emulator.delete_ca(&certificate_id).await;
    assert_eq!(
        (refused.status, refused.message()),
        (406, format!("{CA_STATE_REFUSAL}{certificate_id}")),
        "case a remediated CA: expected the ACTIVE refusal first, got {} body {}",
        refused.status,
        refused.text()
    );

    let deactivated = emulator
        .update_ca(&certificate_id, &CaUpdate::status(CA_CERTIFICATE_STATUS_INACTIVE))
        .await;
    assert_eq!(
        (deactivated.status, deactivated.message()),
        (200, String::new()),
        "case a remediated CA: expected a 200 carrying no member from the update, got {} body \
         {}",
        deactivated.status,
        deactivated.text()
    );

    // The write is observable through the plane's own reader, not assumed.
    let described = emulator.describe_ca(&certificate_id).await;
    assert_eq!(
        described_ca_status(&described),
        CA_CERTIFICATE_STATUS_INACTIVE,
        "case a remediated CA: expected the stored status to be \
         {CA_CERTIFICATE_STATUS_INACTIVE}, got body {}",
        described.text()
    );
    let updated_version = described_member(&described, "customerVersion")
        .as_i64()
        .unwrap_or_default();
    assert_eq!(
        updated_version,
        registered_version + 1,
        "case a remediated CA: expected the customer version to advance from \
         {registered_version} to {}, got {updated_version}",
        registered_version + 1
    );

    let deleted = emulator.delete_ca(&certificate_id).await;
    assert_eq!(
        deleted.status,
        200,
        "case a remediated CA: the deactivated CA must now delete, got {} body {}",
        deleted.status,
        deleted.text()
    );
    assert_eq!(
        emulator.describe_ca(&certificate_id).await.status,
        404,
        "case a remediated CA: the deleted CA certificate must no longer be readable"
    );
}

/// RULE: `UpdateCACertificate` writes the members the request names onto the stored record
/// and leaves every member it does not name standing, and a call that changes nothing
/// leaves the record — its `customerVersion` included — exactly as it found it.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_leaves_the_rest() {
    let emulator = Emulator::new();

    // The configuration a case replaces or removes: `RegistrationConfig` members, from
    // `smithy_query(query="shape",shape="RegistrationConfig")`.
    let configured = json!({ "templateName": "registered-template" });
    let replacement = json!({ "templateName": "replacement-template" });

    // Each row: the label, the CA's own subject (one per case, so one case's write is
    // never read by another), the update, and what the record must report afterwards —
    // status, autoRegistrationStatus, registrationConfig, and whether the write advanced
    // the customer version.
    let cases: Vec<(&'static str, &'static str, CaUpdate, &str, &str, Value, bool)> = vec![
        (
            "deactivating the CA",
            "deactivated-ca.example.com",
            CaUpdate::status(CA_CERTIFICATE_STATUS_INACTIVE),
            CA_CERTIFICATE_STATUS_INACTIVE,
            AUTO_REGISTRATION_ENABLE,
            configured.clone(),
            true,
        ),
        (
            "re-stating the status the CA already carries",
            "restated-status-ca.example.com",
            CaUpdate::status(CA_CERTIFICATE_STATUS_ACTIVE),
            CA_CERTIFICATE_STATUS_ACTIVE,
            AUTO_REGISTRATION_ENABLE,
            configured.clone(),
            false,
        ),
        (
            "disabling auto registration",
            "disabled-auto-ca.example.com",
            CaUpdate::auto_registration(AUTO_REGISTRATION_DISABLE),
            CA_CERTIFICATE_STATUS_ACTIVE,
            AUTO_REGISTRATION_DISABLE,
            configured.clone(),
            true,
        ),
        (
            "re-stating the auto registration status the CA already carries",
            "restated-auto-ca.example.com",
            CaUpdate::auto_registration(AUTO_REGISTRATION_ENABLE),
            CA_CERTIFICATE_STATUS_ACTIVE,
            AUTO_REGISTRATION_ENABLE,
            configured.clone(),
            false,
        ),
        (
            "replacing the registration configuration",
            "replaced-config-ca.example.com",
            CaUpdate::registration_config(replacement.clone()),
            CA_CERTIFICATE_STATUS_ACTIVE,
            AUTO_REGISTRATION_ENABLE,
            replacement.clone(),
            true,
        ),
        (
            "removing auto registration",
            "removed-auto-ca.example.com",
            CaUpdate::removing_auto_registration(),
            CA_CERTIFICATE_STATUS_ACTIVE,
            AUTO_REGISTRATION_DISABLE,
            Value::Null,
            true,
        ),
        (
            "an update naming nothing",
            "untouched-ca.example.com",
            CaUpdate::default(),
            CA_CERTIFICATE_STATUS_ACTIVE,
            AUTO_REGISTRATION_ENABLE,
            configured.clone(),
            false,
        ),
    ];

    for (label, subject, update, status, auto_registration, registration_config, advances) in cases
    {
        // Every case starts from the same record: ACTIVE, auto-registering, configured.
        let authority = Authority::new(subject);
        let certificate_id = emulator
            .register_ca(
                &Case::active(label, subject)
                    .auto_registration(true)
                    .registration_config(configured.clone()),
                &authority,
            )
            .await;
        let before =
            described_member(&emulator.describe_ca(&certificate_id).await, "customerVersion")
                .as_i64()
                .unwrap_or_default();

        let answered = emulator.update_ca(&certificate_id, &update).await;
        assert_eq!(
            answered.status,
            200,
            "case {label}: expected the update to answer 200, got {} body {}",
            answered.status,
            answered.text()
        );

        let described = emulator.describe_ca(&certificate_id).await;
        assert_eq!(
            (
                described_ca_status(&described),
                described_member(&described, "autoRegistrationStatus"),
                described_registration_config(&described),
            ),
            (
                status.to_string(),
                json!(auto_registration),
                registration_config.clone(),
            ),
            "case {label}: expected {status} / {auto_registration} / \
             {registration_config:?}, got body {}",
            described.text()
        );
        let after = described_member(&described, "customerVersion")
            .as_i64()
            .unwrap_or_default();
        assert_eq!(
            after,
            if advances { before + 1 } else { before },
            "case {label}: expected the customer version to go from {before} to {}, got \
             {after}",
            if advances { before + 1 } else { before }
        );
    }
}

/// RULE: every operation addressing ONE registered CA certificate words a miss after the
/// resource the request addressed — `ResourceNotFoundException` (HTTP 404) with the CA
/// plane's whole-sentence fixed text, which carries no part of the requested id — and
/// writes nothing.
#[tokio::test]
async fn the_ca_plane_words_a_miss_after_the_ca_certificate_for_every_operation() {
    let emulator = Emulator::new();

    // One CA is registered, so the refusals below are about the id and not about an empty
    // registry.
    let authority = Authority::new("held-ca.example.com");
    let held = emulator
        .register_ca(&Case::active("a held CA", "held-ca.example.com"), &authority)
        .await;

    // Ids of the modelled form (`CertificateId`, len 64..64, pattern ^(0x)?[a-fA-F0-9]+$)
    // that the registry holds nothing under: lower and upper hex digits, and the `0x`
    // prefix the pattern admits.
    for unknown in [
        "0".repeat(64),
        "ABCDEF0123456789".repeat(4),
        format!("0x{}", "a".repeat(62)),
    ] {
        for (operation, refused) in [
            ("DescribeCACertificate", emulator.describe_ca(&unknown).await),
            ("DeleteCACertificate", emulator.delete_ca(&unknown).await),
            (
                "UpdateCACertificate",
                emulator
                    .update_ca(&unknown, &CaUpdate::status(CA_CERTIFICATE_STATUS_INACTIVE))
                    .await,
            ),
        ] {
            assert_eq!(
                (refused.status, refused.code(), refused.message()),
                (
                    404,
                    "ResourceNotFoundException".to_string(),
                    MISSING_CA_CERTIFICATE.to_string()
                ),
                "case {operation} on {unknown}: expected 404 ResourceNotFoundException worded \
                 {MISSING_CA_CERTIFICATE}, got {} {} body {}",
                refused.status,
                refused.code(),
                refused.text()
            );
        }
        assert_eq!(
            emulator.describe_ca(&unknown).await.status,
            404,
            "case {unknown}: a refused call must create no record"
        );
    }

    // The registered CA is untouched by every refusal above.
    assert_eq!(
        emulator.describe_ca(&held).await.status,
        200,
        "case a held CA: the registered CA certificate must still be readable"
    );
}
