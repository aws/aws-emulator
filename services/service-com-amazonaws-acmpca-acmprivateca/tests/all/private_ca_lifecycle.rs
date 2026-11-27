// SPDX-License-Identifier: CC0-1.0

//! The private-CA lifecycle, driven over the wire the way a client drives it:
//! `POST /` with `X-Amz-Target: ACMPrivateCA.<Op>` and an awsJson1_1 body, through the
//! same multi-service router the emulator boots.
//!
//! Every suite here builds its own engine, so one test's state is invisible to another.
//!
//! The harness below ([`Emulator`] and the helpers under it) is `pub(crate)`, so a
//! sibling suite in `tests/all/` drives the same router through the same wire rather
//! than growing a second copy of it.
//!
//! The model declares more than one wire protocol for this service, so the last section of
//! this file ([`Wire`] and the tests under it) drives the SAME operations over each of
//! them: the protocol a request is served in is the one that request names.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::SETTLE_AFTER;
use service_com_amazonaws_acmpca_acmprivateca::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::{DecodePem, Encode};
use x509_cert::name::Name;
use x509_cert::request::CertReq;
use x509_cert::time::Time;
use x509_cert::Certificate;

use crate::ca_certificate_chain_import::Authority;
use crate::resource_policy::error_message;

/// A SigV4 `Authorization` header whose credential scope names this service and the
/// region every ARN the request mints must carry. The router verifies no signature here
/// (`set_request_authentication(false)`); the scope is read for routing and the region.
pub(crate) const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
                             Credential=AKIDTESTTESTTEST/20260101/eu-west-1/acm-pca/aws4_request, \
                             SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region of [`AUTHORIZATION`]'s credential scope.
pub(crate) const REGION: &str = "eu-west-1";

/// The account id every generated emulator presents.
pub(crate) const ACCOUNT: &str = "000000000000";

/// Long enough that a transition the clock owed at `SETTLE_AFTER` has come due.
pub(crate) fn wait_for_the_clock() {
    // A test may wait for a due instant; a handler may not. `tokio`'s timer is not in
    // this workspace's feature set, so the current-thread test runtime parks instead.
    std::thread::sleep(SETTLE_AFTER + Duration::from_millis(250));
}

/// One emulator with this service mounted.
///
/// `pub(crate)` so every suite in this test target drives the emulator through ONE
/// harness: a sibling suite reaches it as `crate::private_ca_lifecycle::Emulator`.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The ONE engine every operation of this emulator reads and writes, so a suite can
    /// seed a resource into a state only an unimplemented operation would put it in.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The same emulator with CloudWatch mounted beside this service on the ONE engine,
    /// exactly as `core/router` assembles them — which is what makes the metric store a CA
    /// publishes its issuance series into the store CloudWatch's own readers answer from.
    pub(crate) fn with_cloudwatch() -> Self {
        let mut emulator = Self::new();
        emulator.router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                Arc::clone(&emulator.engine),
            ),
        );
        emulator
    }

    /// The router every service mounted on this emulator is reached through, so a suite that
    /// mounts a second service beside this one ([`Self::with_cloudwatch`]) drives that
    /// service over its OWN wire protocol rather than through this service's.
    pub(crate) fn router(&self) -> &MultiServiceRouter {
        &self.router
    }

    /// This service's store inside the engine the router hands every handler — the same
    /// store the operations read and write, reached the way a handler reaches it.
    pub(crate) fn state(
        &self,
    ) -> engine::ServiceState<service_com_amazonaws_acmpca_acmprivateca::state::ComAmazonawsAcmpcaAcmprivatecaState>
    {
        service_com_amazonaws_acmpca_acmprivateca::state::service_state(&*self.engine)
            .expect("the private CA store is reachable through the shared engine")
    }

    /// Call `operation` and return the HTTP status with the parsed JSON body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_raw(
            operation,
            serde_json::to_vec(body).expect("the request body serializes"),
        )
        .await
    }

    /// Call `operation` with the request body EXACTLY as `body` spells it, and return the
    /// HTTP status with the parsed JSON response.
    ///
    /// `call` writes the body with `serde_json`, which emits every character of a string
    /// as itself; a client is free to spell the same string with `\uXXXX` escapes instead
    /// (Python's `json.dumps` escapes every non-ASCII character by default, so botocore
    /// does), and a suite that cares which spelling reaches the service builds the bytes
    /// itself through here.
    pub(crate) async fn call_raw(&self, operation: &str, body: Vec<u8>) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://acm-pca.eu-west-1.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("host", "acm-pca.eu-west-1.amazonaws.com")
            .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` as a client of `region`: the SigV4 credential scope names that
    /// region, which is where the resources the call mints live.
    ///
    /// A suite that cares which region a request arrives from \u2014 a CA belongs to the region
    /// its ARN names \u2014 drives the same emulator through here.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let host = format!("acm-pca.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("host", &host)
            .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDTESTTESTTEST/20260101/{region}/acm-pca/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation`, requiring success.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {parsed:?}"
        );
        parsed
    }

    /// Call `operation`, requiring the modelled error `code` with HTTP 400.
    pub(crate) async fn refused(
        &self,
        case: &str,
        operation: &str,
        body: &Value,
        code: &str,
    ) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to refuse with 400 {code}, got {status} {parsed:?}"
        );
        let wire_code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            wire_code.contains(code),
            "case {case:?}: expected {operation} to answer {code}, got {wire_code:?} ({parsed:?})"
        );
        parsed
    }
}

/// The smallest configuration `CreateCertificateAuthority` accepts, with `common_name`
/// as the subject.
pub(crate) fn root_ca_request(common_name: &str) -> Value {
    json!({
        "CertificateAuthorityConfiguration": {
            "KeyAlgorithm": "RSA_2048",
            "SigningAlgorithm": "SHA256WITHRSA",
            "Subject": {"CommonName": common_name}
        },
        "CertificateAuthorityType": "ROOT"
    })
}

/// A PEM block of `label` around `content`, base64 in 64-character lines (RFC 7468).
///
/// The framing is well formed and the body is base64 — and the body is NOT DER, which is
/// exactly what makes it a malformed certificate or certification request.
pub(crate) fn pem(label: &str, content: &str) -> String {
    pem_of_bytes(label, content.as_bytes())
}

/// A PEM block of `label` around raw `body` bytes.
pub(crate) fn pem_of_bytes(label: &str, body: &[u8]) -> String {
    let encoded = aws_smithy_types::base64::encode(body);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// The distinguished name `rcgen` puts in the objects below: one CommonName.
///
/// `pub(crate)` so the chain suite builds the authorities it presents with the same name.
pub(crate) fn rcgen_subject(common_name: &str) -> rcgen::DistinguishedName {
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, common_name);
    subject
}

/// A real self-signed X.509 certificate for `common_name`, PEM-encoded.
///
/// Written by `rcgen`, so the bytes the service is fed are a certificate built by
/// something other than the code under test.
pub(crate) fn certificate_pem(common_name: &str) -> String {
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("parameters");
    params.distinguished_name = rcgen_subject(common_name);
    params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem()
}

/// A real PKCS#10 certification request for `common_name`, PEM-encoded, likewise from
/// `rcgen`.
///
/// `pub(crate)` so a sibling suite feeds the service a leaf certification request built
/// outside the code under test.
pub(crate) fn csr_pem(common_name: &str) -> String {
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("parameters");
    params.distinguished_name = rcgen_subject(common_name);
    params
        .serialize_request(&key)
        .expect("a certification request")
        .pem()
        .expect("the request is PEM-encodable")
}

/// The CA certificate the certification request `csr` asks for: a certificate over the
/// subject and the public key that request published, which is what makes it the
/// certificate of the CA that published it.
///
/// Assembled here, out of `x509-cert`, rather than taken from the service: the bytes a
/// suite feeds `ImportCertificateAuthorityCertificate` are written by something other than
/// the code under test, exactly as [`certificate_pem`]'s are — what this adds is the
/// identity the import checks, the subject and the `SubjectPublicKeyInfo` of the CA's own
/// request. The signature is bits of the right shape: nothing verifies a CA certificate's
/// signature, which is signed by a key no caller of this API holds.
pub(crate) fn ca_certificate_for(csr: &str) -> String {
    let request = request_of(csr, "a CA certification request");
    certificate_over(
        &request.info.subject,
        &request.info.public_key,
        &request.algorithm,
    )
}

/// A certificate over `subject` and `key`, signed under `algorithm`, PEM-encoded.
///
/// The three members `ImportCertificateAuthorityCertificate` judges a CA certificate by are
/// the caller's to choose here, so a suite can present a certificate that diverges from the
/// CA's own request in exactly one of them.
///
/// Its validity window reaches a century ahead, because a CA activated with it reports that
/// window as its own and a certificate issued under it may not outlive it
/// (`IssueCertificateRequest$Validity`: "The end of the validity period configured on a
/// certificate must not exceed the limit set on its parents in the CA hierarchy."), so the
/// window has to hold every validity the suites ask a CA for — the furthest of them being
/// the 2050 `ABSOLUTE` sample of the validity-resolution table.
pub(crate) fn certificate_over(
    subject: &Name,
    key: &x509_cert::spki::SubjectPublicKeyInfoOwned,
    algorithm: &x509_cert::spki::AlgorithmIdentifierOwned,
) -> String {
    certificate_over_with_serial(subject, key, algorithm, &next_test_serial())
}

/// The same certificate, carrying the serial number whose value is the octets `serial`.
///
/// The serial is the caller's to choose here, so a suite can present a certificate whose
/// serial number is a value of any width — which is what a CA activated with it reports as
/// its own serial number.
pub(crate) fn certificate_over_with_serial(
    subject: &Name,
    key: &x509_cert::spki::SubjectPublicKeyInfoOwned,
    algorithm: &x509_cert::spki::AlgorithmIdentifierOwned,
    serial: &[u8],
) -> String {
    let not_before = SystemTime::now() - Duration::from_secs(3600);
    let not_after = not_before + Duration::from_secs(100 * 365 * 24 * 3600);
    let tbs_certificate = x509_cert::TbsCertificate {
        version: x509_cert::Version::V3,
        serial_number: x509_cert::serial_number::SerialNumber::new(serial)
            .expect("a serial number"),
        signature: algorithm.clone(),
        // A CA certificate for a root CA is self-signed, so the issuer is the subject.
        issuer: subject.clone(),
        validity: x509_cert::time::Validity {
            not_before: Time::try_from(not_before).expect("a representable date"),
            not_after: Time::try_from(not_after).expect("a representable date"),
        },
        subject: subject.clone(),
        subject_public_key_info: key.clone(),
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: None,
    };
    let certificate = Certificate {
        signature_algorithm: tbs_certificate.signature.clone(),
        signature: x509_cert::der::asn1::BitString::from_bytes(&next_test_serial())
            .expect("signature bits"),
        tbs_certificate,
    };
    pem_of_bytes(
        "CERTIFICATE",
        &certificate.to_der().expect("the certificate encodes"),
    )
}

/// A distinct eight-byte serial for every certificate a suite builds, so two of them are
/// never the same certificate.
fn next_test_serial() -> [u8; 8] {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .to_be_bytes()
}

/// The CA certificate the CA `arn` published its certification request for, read through
/// the service: `GetCertificateAuthorityCsr` and then [`ca_certificate_for`].
///
/// This is what a caller of this API does — take the CA's request to a signer and bring
/// back the certificate it asks for — so it is how every suite here activates a CA.
pub(crate) async fn own_ca_certificate(
    emulator: &Emulator,
    case: &str,
    arn: &str,
) -> String {
    let reported = emulator
        .ok(
            case,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    ca_certificate_for(&string(&reported, "Csr", case))
}

/// The X.509 certificate a PEM body carries, or a panic naming the case.
pub(crate) fn certificate_of(text: &str, case: &str) -> Certificate {
    Certificate::from_pem(text)
        .unwrap_or_else(|error| panic!("case {case:?}: expected an X.509 certificate: {error}"))
}

/// The DER of the certificate a PEM body carries — the identity of the certificate,
/// whatever framing it arrived in.
pub(crate) fn certificate_der(text: &str, case: &str) -> Vec<u8> {
    certificate_of(text, case)
        .to_der()
        .expect("a decoded certificate re-encodes")
}

/// The certification request a PEM body carries, or a panic naming the case.
pub(crate) fn request_of(text: &str, case: &str) -> CertReq {
    CertReq::from_pem(text).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a PKCS#10 certification request: {error}")
    })
}

/// The CommonName (RFC 4519 section 2.3, OID 2.5.4.3) of a DER distinguished name.
pub(crate) fn subject_common_name(name: &Name) -> String {
    name.0
        .iter()
        .flat_map(|rdn| rdn.0.iter())
        .find(|attribute| attribute.oid.to_string() == "2.5.4.3")
        .map(|attribute| String::from_utf8_lossy(attribute.value.value()).to_string())
        .unwrap_or_default()
}

/// A serial number as the value it denotes: a DER integer carries no leading zero, so
/// two encodings of one number compare equal only once they are stripped.
fn serial_value(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().skip_while(|byte| *byte == 0).collect()
}

/// The serial number of `certificate` as the integer it is: a DER INTEGER's octets are a
/// base-256 big-endian number (the leading zero octet that keeps it positive changes no
/// value), so this is that number in base ten — which is how Amazon Web Services Private CA
/// states the serial number of a private CA.
pub(crate) fn serial_as_integer(certificate: &Certificate) -> String {
    num_bigint::BigUint::from_bytes_be(certificate.tbs_certificate.serial_number.as_bytes())
        .to_str_radix(10)
}

/// The serial a certificate ARN carries, spelled as the ARN carries it: the hexadecimal
/// value the issuing CA allocated.
///
/// `pub(crate)` so every suite that reaches for a certificate by the serial in its ARN
/// splits one ARN the same way.
pub(crate) fn serial_in(certificate_arn: &str, case: &str) -> String {
    certificate_arn
        .rsplit_once("/certificate/")
        .map(|(_, serial)| serial.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: no serial in {certificate_arn:?}"))
}

/// The serial a certificate ARN names, as the integer the certificate carries: a DER
/// integer carries no leading zero, and `SerialNumber` adds one only to keep the value
/// positive, so the two compare equal once stripped.
fn serial_from_arn(certificate_arn: &str) -> Vec<u8> {
    let serial = serial_in(certificate_arn, "a certificate ARN");
    serial_value(&hex::decode(serial).expect("a certificate ARN carries a hex serial"))
}

/// An X.509 validity bound as the RFC 3339 instant it names.
fn timestamp(time: &Time) -> String {
    let seconds = i64::try_from(time.to_unix_duration().as_secs()).expect("a representable date");
    aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a representable date")
}

/// A blob member as awsJson1_1 carries it.
pub(crate) fn blob(text: &str) -> Value {
    Value::String(aws_smithy_types::base64::encode(text.as_bytes()))
}

/// `Value` member as a `&str`, or a panic naming the case.
pub(crate) fn string(value: &Value, member: &str, case: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member:?} in {value:?}"))
        .to_string()
}

/// Create a CA and drive it all the way to ACTIVE, returning its ARN and the CA
/// certificate imported into it.
///
/// The certificate is the one the CA's own certification request asks for
/// ([`own_ca_certificate`]): a private CA signs with the key pair it holds, so that is the
/// only certificate it takes.
pub(crate) async fn active_ca(emulator: &Emulator, common_name: &str) -> (String, String) {
    let created = emulator
        .ok(common_name, "CreateCertificateAuthority", &root_ca_request(common_name))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", common_name);
    wait_for_the_clock();
    let certificate = own_ca_certificate(emulator, common_name, &arn).await;
    emulator
        .ok(
            common_name,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": blob(&certificate)}),
        )
        .await;
    (arn, certificate)
}

/// Steps 1 to 6 of the lifecycle, in the order a client script runs them: create,
/// read the CSR, import the signed CA certificate, issue an end-entity certificate and
/// read it back. Every answer comes from what an earlier call persisted.
#[tokio::test]
async fn the_private_ca_lifecycle_runs_end_to_end() {
    let emulator = Emulator::new();
    let case = "lifecycle";

    // 1. CreateCertificateAuthority mints an ARN in the signed region and account.
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request("example-root"))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    let prefix = format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a CA ARN under {prefix:?}, got {arn:?}"
    );
    let id = arn[prefix.len()..].to_string();
    assert_eq!(
        id.len(),
        36,
        "case {case:?}: expected a 36-character uuid CA id, got {id:?}"
    );

    // A CA is returned in CREATING and its CSR is not a product of creation yet.
    let describing = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    assert_eq!(
        string(&describing["CertificateAuthority"], "Status", case),
        "CREATING",
        "case {case:?}: a CA reads CREATING until creation settles, got {describing:?}"
    );
    emulator
        .refused(
            case,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": arn}),
            "RequestInProgressException",
        )
        .await;
    emulator
        .refused(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Certificate": blob(&certificate_pem("too-early")),
            }),
            "RequestInProgressException",
        )
        .await;

    wait_for_the_clock();

    // 4. The describe between create and import reports PENDING_CERTIFICATE, with the
    // configuration the create call supplied read back out of state.
    let pending = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    let authority = &pending["CertificateAuthority"];
    assert_eq!(
        string(authority, "Status", case),
        "PENDING_CERTIFICATE",
        "case {case:?}: a settled CA waits for its certificate, got {authority:?}"
    );
    assert_eq!(string(authority, "Arn", case), arn);
    assert_eq!(string(authority, "OwnerAccount", case), ACCOUNT);
    assert_eq!(string(authority, "Type", case), "ROOT");
    assert_eq!(
        string(authority, "UsageMode", case),
        "GENERAL_PURPOSE",
        "case {case:?}: UsageMode defaults to GENERAL_PURPOSE, got {authority:?}"
    );
    assert_eq!(
        string(authority, "KeyStorageSecurityStandard", case),
        "FIPS_140_2_LEVEL_3_OR_HIGHER",
        "case {case:?}: KeyStorageSecurityStandard defaults to FIPS_140_2_LEVEL_3_OR_HIGHER, \
         got {authority:?}"
    );
    assert_eq!(
        authority["CertificateAuthorityConfiguration"]["Subject"]["CommonName"],
        json!("example-root"),
        "case {case:?}: the describe echoes the stored subject, got {authority:?}"
    );
    assert_eq!(
        authority["CertificateAuthorityConfiguration"]["KeyAlgorithm"],
        json!("RSA_2048"),
        "case {case:?}: the describe echoes the stored key algorithm, got {authority:?}"
    );
    assert!(
        authority.get("Serial").is_none(),
        "case {case:?}: a CA reports the serial number of the certificate it holds, and this \
         one holds none yet, got {authority:?}"
    );
    assert!(
        authority.get("NotBefore").is_none() && authority.get("NotAfter").is_none(),
        "case {case:?}: a CA has no validity window before its certificate is imported, \
         got {authority:?}"
    );

    // 2. The CSR is stable: it was derived once, at create time.
    let csr = string(
        &emulator
            .ok(case, "GetCertificateAuthorityCsr", &json!({"CertificateAuthorityArn": arn}))
            .await,
        "Csr",
        case,
    );
    let csr_again = string(
        &emulator
            .ok(case, "GetCertificateAuthorityCsr", &json!({"CertificateAuthorityArn": arn}))
            .await,
        "Csr",
        case,
    );
    assert_eq!(
        csr, csr_again,
        "case {case:?}: repeated GetCertificateAuthorityCsr must return the same CSR"
    );
    assert!(
        csr.starts_with("-----BEGIN CERTIFICATE REQUEST-----"),
        "case {case:?}: expected a PEM certification request, got {csr:?}"
    );
    // The CSR is a real PKCS#10 request for the CA's own subject.
    let request = request_of(&csr, case);
    assert_eq!(
        subject_common_name(&request.info.subject),
        "example-root",
        "case {case:?}: the CSR carries the CA's own subject, got {csr:?}"
    );

    // 3. Importing the certificate the CA's own request asks for activates the CA — with
    // the chain that signed it beside it, which is the path the certificate chains up to
    // (`crate::ca_certificate_chain_import`).
    let signer = Authority::new("example-signer");
    let ca_certificate = signer.certificate_for(&csr);
    let ca_chain = signer.chain();
    let imported = emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Certificate": blob(&ca_certificate),
                "CertificateChain": blob(&ca_chain),
            }),
        )
        .await;
    assert_eq!(
        imported,
        Value::Object(serde_json::Map::new()),
        "case {case:?}: ImportCertificateAuthorityCertificate answers an empty body"
    );

    let active = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    let authority = &active["CertificateAuthority"];
    assert_eq!(
        string(authority, "Status", case),
        "ACTIVE",
        "case {case:?}: importing a certificate activates the CA, got {authority:?}"
    );
    let not_before = authority["NotBefore"].as_f64().unwrap_or_default();
    let not_after = authority["NotAfter"].as_f64().unwrap_or_default();
    assert!(
        not_after > not_before && not_before > 0.0,
        "case {case:?}: an ACTIVE CA carries its certificate's validity window, \
         got {authority:?}"
    );
    let expected_serial = serial_as_integer(&certificate_of(&ca_certificate, case));
    assert_eq!(
        string(authority, "Serial", case),
        expected_serial,
        "case {case:?}: an activated CA reports the serial number of the certificate it \
         holds, got {authority:?}"
    );

    // GetCertificateAuthorityCertificate is the read of exactly those bytes.
    let read_back = emulator
        .ok(
            case,
            "GetCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    assert_eq!(
        certificate_der(&string(&read_back, "Certificate", case), case),
        certificate_der(&ca_certificate, case),
        "case {case:?}: the CA certificate reads back as the certificate that was imported"
    );
    assert_eq!(
        certificate_der(&string(&read_back, "CertificateChain", case), case),
        certificate_der(&ca_chain, case),
        "case {case:?}: the imported chain reads back as the certificate it carried"
    );

    // 5. IssueCertificate allocates a serial under the CA and is idempotent per token.
    let csr_to_sign = csr_pem("an-end-entity");
    let issue = json!({
        "CertificateAuthorityArn": arn,
        "Csr": blob(&csr_to_sign),
        "SigningAlgorithm": "SHA256WITHRSA",
        "Validity": {"Value": 365, "Type": "DAYS"},
        "IdempotencyToken": "tok-1",
    });
    let certificate_arn = string(
        &emulator.ok(case, "IssueCertificate", &issue).await,
        "CertificateArn",
        case,
    );
    let serial_prefix = format!("{arn}/certificate/");
    assert!(
        certificate_arn.starts_with(&serial_prefix),
        "case {case:?}: expected a certificate ARN under {serial_prefix:?}, \
         got {certificate_arn:?}"
    );
    let replayed = string(
        &emulator.ok(case, "IssueCertificate", &issue).await,
        "CertificateArn",
        case,
    );
    assert_eq!(
        replayed, certificate_arn,
        "case {case:?}: the same IdempotencyToken must not mint a second certificate"
    );

    // Issuance is asynchronous: the certificate is not retrievable in the same instant.
    emulator
        .refused(
            case,
            "GetCertificate",
            &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
            "RequestInProgressException",
        )
        .await;
    wait_for_the_clock();

    // 6. GetCertificate returns the PEM issued for that ARN, chained to the CA.
    let got = emulator
        .ok(
            case,
            "GetCertificate",
            &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
        )
        .await;
    let certificate = string(&got, "Certificate", case);
    assert!(
        certificate.starts_with("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: expected a PEM certificate, got {certificate:?}"
    );
    let issued = certificate_of(&certificate, case);
    assert_eq!(
        serial_value(issued.tbs_certificate.serial_number.as_bytes()),
        serial_from_arn(&certificate_arn),
        "case {case:?}: the certificate carries the serial its ARN names"
    );
    assert_eq!(
        subject_common_name(&issued.tbs_certificate.issuer),
        "example-root",
        "case {case:?}: the certificate is issued by the CA's own subject"
    );
    assert_eq!(
        subject_common_name(&issued.tbs_certificate.subject),
        "an-end-entity",
        "case {case:?}: the certificate is issued for the subject of the request it answers"
    );
    assert_eq!(
        issued.tbs_certificate.subject_public_key_info,
        request_of(&csr_to_sign, case).info.public_key,
        "case {case:?}: the certificate carries the public key of the request it answers"
    );
    assert_eq!(
        Certificate::load_pem_chain(string(&got, "CertificateChain", case).as_bytes())
            .expect("the chain is a PEM chain of certificates")
            .iter()
            .map(|certificate| certificate.to_der().expect("it re-encodes"))
            .collect::<Vec<_>>(),
        vec![
            certificate_der(&ca_certificate, case),
            certificate_der(&ca_chain, case)
        ],
        "case {case:?}: an issued certificate chains through the CA certificate and its chain"
    );

    // A certificate ARN this CA never issued does not resolve.
    emulator
        .refused(
            case,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "CertificateArn": format!("{arn}/certificate/0123456789abcdef"),
            }),
            "ResourceNotFoundException",
        )
        .await;

    // Nor does one issued by a different CA, even presented with its own CA's ARN.
    let (other_arn, _) = active_ca(&emulator, "other-root").await;
    let other_certificate_arn = string(
        &emulator
            .ok(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": other_arn,
                    "Csr": blob(&csr_to_sign),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": 30, "Type": "DAYS"},
                }),
            )
            .await,
        "CertificateArn",
        case,
    );
    emulator
        .refused(
            case,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "CertificateArn": other_certificate_arn,
            }),
            "ResourceNotFoundException",
        )
        .await;
}

/// `Validity`/`ValidityNotBefore` resolution, over every `ValidityPeriodType` member.
///
/// The rule is the one `Validity$Type` documents: `DAYS`/`MONTHS`/`YEARS` are relative
/// to the start of the validity period, `ABSOLUTE` is seconds since the Unix epoch, and
/// `END_DATE` is UTCTime `YYMMDDHHMMSS` (a year field of 50 or more means 19YY) or
/// GeneralizedTime `YYYYMMDDHHMMSS`. The `ABSOLUTE` 2524608000 and `END_DATE`
/// 491231235959 rows are the sample values that documentation publishes.
#[tokio::test]
async fn validity_resolves_the_way_each_validity_period_type_is_documented() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "validity-root").await;
    let csr = blob(&csr_pem("a-validity-end-entity"));

    // 1767225600 = 2026-01-01T00:00:00Z, 1769817600 = 2026-01-31T00:00:00Z,
    // 1835395200 = 2028-02-29T00:00:00Z.
    let rows: &[(&str, i64, &str, Option<i64>, &str)] = &[
        ("one day", 1, "DAYS", Some(1_767_225_600), "2026-01-02T00:00:00Z"),
        ("a year of days", 365, "DAYS", Some(1_767_225_600), "2027-01-01T00:00:00Z"),
        ("one month", 1, "MONTHS", Some(1_767_225_600), "2026-02-01T00:00:00Z"),
        ("thirteen months", 13, "MONTHS", Some(1_767_225_600), "2027-02-01T00:00:00Z"),
        ("one year", 1, "YEARS", Some(1_767_225_600), "2027-01-01T00:00:00Z"),
        ("four years", 4, "YEARS", Some(1_767_225_600), "2030-01-01T00:00:00Z"),
        // A month shorter than the starting day of month clamps to its last day.
        ("month end clamp", 1, "MONTHS", Some(1_769_817_600), "2026-02-28T00:00:00Z"),
        ("leap day clamp", 1, "YEARS", Some(1_835_395_200), "2029-02-28T00:00:00Z"),
        ("absolute sample", 2_524_608_000, "ABSOLUTE", None, "2050-01-01T00:00:00Z"),
        ("utctime sample", 491_231_235_959, "END_DATE", None, "2049-12-31T23:59:59Z"),
        (
            "generalized time",
            20_351_231_235_959,
            "END_DATE",
            None,
            "2035-12-31T23:59:59Z",
        ),
    ];

    // Issue every row first and wait for the clock ONCE, rather than once per row.
    let mut issued: Vec<(&str, i64, &str, &str, String)> = Vec::with_capacity(rows.len());
    for (case, value, period, not_before, expected) in rows {
        let mut body = json!({
            "CertificateAuthorityArn": arn,
            "Csr": csr,
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": value, "Type": period},
        });
        if let Some(seconds) = not_before {
            body["ValidityNotBefore"] = json!({"Value": seconds, "Type": "ABSOLUTE"});
        }
        let certificate_arn = string(
            &emulator.ok(case, "IssueCertificate", &body).await,
            "CertificateArn",
            case,
        );
        issued.push((case, *value, period, expected, certificate_arn));
    }
    wait_for_the_clock();

    for (case, value, period, expected, certificate_arn) in issued {
        let certificate = string(
            &emulator
                .ok(
                    case,
                    "GetCertificate",
                    &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
                )
                .await,
            "Certificate",
            case,
        );
        assert_eq!(
            timestamp(&certificate_of(&certificate, case).tbs_certificate.validity.not_after),
            *expected,
            "case {case:?}: Validity {value} {period} must expire at {expected}"
        );
    }

    // A validity that resolves to the start of the window or earlier is not a validity,
    // and neither is one that resolves outside the range of dates a calendar holds.
    let refusals: &[(&str, i64, &str)] = &[
        // UTCTime year 50 means 1950, which is behind any issuance instant.
        ("utctime nineteen fifty", 501_231_235_959, "END_DATE"),
        ("thirteen digits", 1_234_567_890_123, "END_DATE"),
        ("not a calendar date", 20_351_331_235_959, "END_DATE"),
        ("february thirtieth", 20_360_230_000_000, "END_DATE"),
        ("hour twenty-five", 20_351_231_250_000, "END_DATE"),
        ("absolute in the past", 1, "ABSOLUTE"),
        // Magnitudes no calendar represents: refused, never wrapped or panicked on.
        ("days past the calendar", i64::MAX, "DAYS"),
        ("months past the calendar", i64::MAX, "MONTHS"),
        ("years past the calendar", i64::MAX, "YEARS"),
        ("absolute past the clock", i64::MAX, "ABSOLUTE"),
        ("end date past the clock", i64::MAX, "END_DATE"),
    ];
    for (case, value, period) in refusals {
        emulator
            .refused(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": csr,
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": value, "Type": period},
                }),
                "InvalidArgsException",
            )
            .await;
    }
}

/// The revocation rules `CreateCertificateAuthority`'s `RevocationConfiguration` member
/// documents: "A configuration disabling CRLs or OCSP must contain only the
/// Enabled=False parameter, and will fail if other parameters such as CustomCname or
/// ExpirationInDays are included.", and the one rule
/// `CrlDistributionPointExtensionConfiguration$OmitExtension` documents beside it: "This
/// configuration cannot be enabled with a custom CNAME set."
///
/// Each row states the error code its configuration is refused with, because the two
/// stages that refuse one answer differently: a member an enabled CRL configuration
/// requires but the model cannot mark `@required` is the request-validation stage's
/// (`ValidationException`, the member named — `crate::validation_stage_refusals`), while
/// members that cannot hold together are the semantic argument fault this operation models.
#[tokio::test]
async fn revocation_configurations_are_accepted_only_where_documented() {
    let emulator = Emulator::new();

    // The code each row is refused with, or `None` for a configuration that is accepted.
    let rows: &[(&str, Value, Option<&str>)] = &[
        ("crl disabled alone", json!({"CrlConfiguration": {"Enabled": false}}), None),
        (
            "crl disabled with a cname",
            json!({"CrlConfiguration": {"Enabled": false, "CustomCname": "crl.example.com"}}),
            Some("InvalidArgsException"),
        ),
        (
            "crl disabled with an expiration",
            json!({"CrlConfiguration": {"Enabled": false, "ExpirationInDays": 7}}),
            Some("InvalidArgsException"),
        ),
        (
            "crl disabled with a bucket",
            json!({"CrlConfiguration": {"Enabled": false, "S3BucketName": "crl-bucket"}}),
            Some("InvalidArgsException"),
        ),
        (
            "crl enabled without a bucket",
            json!({"CrlConfiguration": {"Enabled": true, "ExpirationInDays": 7}}),
            Some("ValidationException"),
        ),
        (
            "crl enabled without an expiration",
            json!({"CrlConfiguration": {"Enabled": true, "S3BucketName": "crl-bucket"}}),
            Some("InvalidArgsException"),
        ),
        (
            "crl enabled with both",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 7,
                    "S3BucketName": "crl-bucket",
                }
            }),
            None,
        ),
        // "Configures whether the CRL Distribution Point extension should be populated with
        // the default URL to the CRL ... This configuration cannot be enabled with a custom
        // CNAME set." (`CrlDistributionPointExtensionConfiguration$OmitExtension`.)
        (
            "crl enabled with the distribution point omitted",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 7,
                    "S3BucketName": "crl-bucket",
                    "CrlDistributionPointExtensionConfiguration": {"OmitExtension": true},
                }
            }),
            None,
        ),
        (
            "crl enabled with the distribution point published behind a cname",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 7,
                    "S3BucketName": "crl-bucket",
                    "CustomCname": "crl.example.com",
                    "CrlDistributionPointExtensionConfiguration": {"OmitExtension": false},
                }
            }),
            None,
        ),
        (
            "crl enabled with the distribution point omitted behind a cname",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 7,
                    "S3BucketName": "crl-bucket",
                    "CustomCname": "crl.example.com",
                    "CrlDistributionPointExtensionConfiguration": {"OmitExtension": true},
                }
            }),
            Some("ValidationException"),
        ),
        ("ocsp disabled alone", json!({"OcspConfiguration": {"Enabled": false}}), None),
        (
            "ocsp disabled with a cname",
            json!({"OcspConfiguration": {"Enabled": false, "OcspCustomCname": "ocsp.example.com"}}),
            Some("InvalidArgsException"),
        ),
        (
            "ocsp enabled with a cname",
            json!({"OcspConfiguration": {"Enabled": true, "OcspCustomCname": "ocsp.example.com"}}),
            None,
        ),
    ];

    // Create (or refuse) every row first, then wait for the clock ONCE before reading
    // the accepted ones back.
    let mut created: Vec<(&str, &Value, String)> = Vec::new();
    for (case, revocation, refused_with) in rows {
        let mut body = root_ca_request("revocation-root");
        body["RevocationConfiguration"] = revocation.clone();
        match refused_with {
            None => {
                let response = emulator.ok(case, "CreateCertificateAuthority", &body).await;
                let arn = string(&response, "CertificateAuthorityArn", case);
                created.push((case, revocation, arn));
            }
            Some(code) => {
                emulator
                    .refused(case, "CreateCertificateAuthority", &body, code)
                    .await;
            }
        }
    }
    wait_for_the_clock();

    for (case, revocation, arn) in created {
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        let reported = &described["CertificateAuthority"]["RevocationConfiguration"];
        // The configuration the create call supplied is echoed; the mechanism it said
        // nothing about is reported disabled (see
        // `a_described_ca_always_carries_both_revocation_configurations`).
        for mechanism in ["CrlConfiguration", "OcspConfiguration"] {
            let expected = revocation
                .get(mechanism)
                .cloned()
                .unwrap_or_else(|| json!({"Enabled": false}));
            assert_eq!(
                reported[mechanism], expected,
                "case {case:?}: expected {mechanism} {expected:?}, got {reported:?}"
            );
        }
    }
}

/// The line endings a row presents a well-formed certificate in: the framing the service
/// writes, that framing with the line endings RFC 7468 also allows, and that framing with
/// its final break left off.
#[derive(Debug, Clone, Copy)]
enum Framing {
    Canonical,
    Crlf,
    NoTrailingNewline,
}

impl Framing {
    /// `body` as this framing presents it.
    fn apply(self, body: &str) -> String {
        match self {
            Self::Canonical => body.to_string(),
            Self::Crlf => body.replace('\n', "\r\n"),
            Self::NoTrailingNewline => body.trim_end().to_string(),
        }
    }
}

/// A certificate body or certification request the service cannot read is malformed; one
/// it can read is accepted whatever its line endings and block count.
///
/// The rule: a blob the service reads as a certificate must be a PEM-framed DER X.509
/// certificate, and one it reads as a certification request a PEM-framed DER PKCS#10
/// request — well-formed PEM framing around a body that is not that structure is
/// malformed, which is what the modelled malformed-certificate and malformed-CSR errors
/// answer. A body that is not even of the PEM SHAPE the certificate member is published
/// with is refused one stage earlier, by request validation
/// (`crate::ca_certificate_import`).
#[tokio::test]
async fn pem_bodies_are_accepted_only_where_they_are_well_formed() {
    let emulator = Emulator::new();

    let request = csr_pem("a-request");
    let foreign = certificate_pem("a-good-ca");
    let foreign_body = certificate_der(&foreign, "a good certificate");
    // Every row here is of the published PEM shape — base64 in 64-character lines inside
    // the certificate armour — and carries something no X.509 parser can read, which is
    // what the modelled malformed-certificate failure is for.
    let malformed: &[(&str, String)] = &[
        ("base64 that is not der", pem("CERTIFICATE", "a certificate")),
        (
            "a truncated certificate",
            pem_of_bytes("CERTIFICATE", &foreign_body[..foreign_body.len() / 2]),
        ),
        (
            "a certificate with trailing bytes",
            pem_of_bytes(
                "CERTIFICATE",
                &[foreign_body.clone(), vec![0x00, 0x01]].concat(),
            ),
        ),
        (
            "a certification request under the certificate label",
            pem_of_bytes(
                "CERTIFICATE",
                &request_of(&request, "a request")
                    .to_der()
                    .expect("it re-encodes"),
            ),
        ),
    ];
    for (case, body) in malformed {
        let created = emulator
            .ok(case, "CreateCertificateAuthority", &root_ca_request("pem-root"))
            .await;
        let arn = string(&created, "CertificateAuthorityArn", case);
        wait_for_the_clock();
        emulator
            .refused(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn, "Certificate": blob(body)}),
                "MalformedCertificateException",
            )
            .await;
        // The CA is untouched by a refused import, so it still takes its own certificate.
        let own = own_ca_certificate(&emulator, case, &arn).await;
        emulator
            .ok(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn, "Certificate": blob(&own)}),
            )
            .await;
    }

    // The chain is held to the same rule, block by block: a chain whose second block is
    // not a certificate is refused even though its first one is.
    let malformed_chains: &[(&str, String)] = &[
        (
            "a chain whose second block is not der",
            format!(
                "{}{}",
                certificate_pem("an-intermediate"),
                pem("CERTIFICATE", "a root")
            ),
        ),
        (
            "a chain carrying a certification request",
            format!(
                "{}{}",
                certificate_pem("an-intermediate"),
                pem_of_bytes(
                    "CERTIFICATE",
                    &request_of(&request, "a request")
                        .to_der()
                        .expect("it re-encodes"),
                )
            ),
        ),
    ];
    for (case, chain) in malformed_chains {
        let created = emulator
            .ok(case, "CreateCertificateAuthority", &root_ca_request("pem-root"))
            .await;
        let arn = string(&created, "CertificateAuthorityArn", case);
        wait_for_the_clock();
        let own = own_ca_certificate(&emulator, case, &arn).await;
        emulator
            .refused(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Certificate": blob(&own),
                    "CertificateChain": blob(chain),
                }),
                "MalformedCertificateException",
            )
            .await;
        // The CA took nothing from the refused import, so it still waits for one.
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        assert_eq!(
            string(&described["CertificateAuthority"], "Status", case),
            "PENDING_CERTIFICATE",
            "case {case:?}: a refused import must leave the CA waiting for its certificate"
        );
    }

    // Accepted bodies: CRLF line endings, no trailing newline, and a chain of several
    // blocks. What each row varies is the framing the certificate arrives in and the chain
    // beside it — and a row that presents a chain presents the certificate that chains up
    // to that chain, which is the rule the chain suite covers in full
    // (`crate::ca_certificate_chain_import`).
    let chain_root = Authority::new("pem-root-authority");
    let chain_intermediate = Authority::issued_by(&chain_root, "pem-intermediate-authority");
    let accepted: &[(&str, Framing, Option<&Authority>, Option<String>)] = &[
        ("crlf", Framing::Crlf, None, None),
        ("no trailing newline", Framing::NoTrailingNewline, None, None),
        (
            "a two-certificate chain",
            Framing::Canonical,
            Some(&chain_intermediate),
            Some(format!(
                "{}{}",
                chain_intermediate.chain(),
                chain_root.chain()
            )),
        ),
        ("an empty chain", Framing::Canonical, None, Some(String::new())),
    ];
    for (case, framing, signer, chain) in accepted {
        let created = emulator
            .ok(case, "CreateCertificateAuthority", &root_ca_request("pem-root"))
            .await;
        let arn = string(&created, "CertificateAuthorityArn", case);
        wait_for_the_clock();
        let published = emulator
            .ok(
                case,
                "GetCertificateAuthorityCsr",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let published = string(&published, "Csr", case);
        // A row with no chain presents the self-signed certificate of the CA's own request;
        // a row with one presents the certificate the authority above that chain issued for
        // the same request.
        let own = match signer {
            Some(signer) => signer.certificate_for(&published),
            None => ca_certificate_for(&published),
        };
        let own_body = certificate_der(&own, case);
        let body = framing.apply(&own);
        let mut import = json!({"CertificateAuthorityArn": arn, "Certificate": blob(&body)});
        if let Some(chain) = chain {
            import["CertificateChain"] = blob(chain);
        }
        emulator
            .ok(case, "ImportCertificateAuthorityCertificate", &import)
            .await;
        let read_back = emulator
            .ok(
                case,
                "GetCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        assert_eq!(
            certificate_der(&string(&read_back, "Certificate", case), case),
            own_body,
            "case {case:?}: a well-formed body reads back as the certificate it carried"
        );
        // And in the service's canonical framing, whatever framing it arrived in: base64
        // in 64-character LF lines with a trailing newline (RFC 7468 section 2).
        assert_eq!(
            string(&read_back, "Certificate", case),
            pem_of_bytes("CERTIFICATE", &own_body),
            "case {case:?}: a well-formed body reads back in canonical PEM framing"
        );
        let expected_chain: Vec<Vec<u8>> = chain
            .as_deref()
            .filter(|chain| !chain.is_empty())
            .map(|chain| {
                Certificate::load_pem_chain(chain.as_bytes())
                    .expect("the chain under test is a PEM chain")
                    .iter()
                    .map(|certificate| certificate.to_der().expect("it re-encodes"))
                    .collect()
            })
            .unwrap_or_default();
        let read_back_chain: Vec<Vec<u8>> = read_back
            .get("CertificateChain")
            .and_then(Value::as_str)
            .map(|chain| {
                Certificate::load_pem_chain(chain.as_bytes())
                    .expect("the chain reads back as a PEM chain")
                    .iter()
                    .map(|certificate| certificate.to_der().expect("it re-encodes"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            read_back_chain, expected_chain,
            "case {case:?}: the chain reads back as the certificates imported"
        );
    }

    // A certification request is held to the same rule, under its own error.
    let (arn, _) = active_ca(&emulator, "csr-root").await;
    for (case, body) in [
        ("not pem at all", "a request".to_string()),
        ("the wrong label", foreign.clone()),
        ("base64 that is not der", pem("CERTIFICATE REQUEST", "a request")),
        (
            "a certificate under the request label",
            pem_of_bytes("CERTIFICATE REQUEST", &foreign_body),
        ),
    ] {
        emulator
            .refused(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&body),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": 7, "Type": "DAYS"},
                }),
                "MalformedCSRException",
            )
            .await;
    }
}

/// An ARN that is not this service's refers to no resource; a well-formed one that
/// names nothing stored is not found. The rule holds for every operation that takes a
/// CA ARN.
#[tokio::test]
async fn ca_arns_are_refused_by_form_and_then_by_existence() {
    let emulator = Emulator::new();
    let certificate = blob(&certificate_pem("an-arn-ca"));

    let malformed = [
        ("no ca id", format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority")),
        (
            "an extra resource segment",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/abc/extra"),
        ),
        (
            "another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:private-ca/abc"),
        ),
    ];
    let unknown = format!(
        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
         00000000-0000-4000-8000-000000000000"
    );

    for (operation, extra) in [
        ("DescribeCertificateAuthority", json!({})),
        ("GetCertificateAuthorityCsr", json!({})),
        ("GetCertificateAuthorityCertificate", json!({})),
        ("DeleteCertificateAuthority", json!({})),
        ("ListTags", json!({})),
        (
            "ImportCertificateAuthorityCertificate",
            json!({"Certificate": certificate}),
        ),
        (
            "IssueCertificate",
            json!({
                "Csr": blob(&csr_pem("an-arn-request")),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Value": 7, "Type": "DAYS"},
            }),
        ),
        ("RestoreCertificateAuthority", json!({})),
        ("DeleteCertificateAuthority", json!({})),
    ] {
        for (case, arn) in &malformed {
            let case = format!("{operation}: {case}");
            let mut body = extra.clone();
            body["CertificateAuthorityArn"] = json!(arn);
            emulator
                .refused(&case, operation, &body, "InvalidArnException")
                .await;
        }
        let case = format!("{operation}: a CA that was never created");
        let mut body = extra.clone();
        body["CertificateAuthorityArn"] = json!(unknown);
        emulator
            .refused(&case, operation, &body, "ResourceNotFoundException")
            .await;
    }

    // GetCertificate reads two ARNs, and holds the certificate ARN to the same rule.
    let (arn, _) = active_ca(&emulator, "arn-root").await;
    for (case, certificate_arn) in [
        ("not a certificate ARN", arn.clone()),
        (
            "a certificate of no CA",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate/abc"),
        ),
    ] {
        emulator
            .refused(
                case,
                "GetCertificate",
                &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
                "InvalidArnException",
            )
            .await;
    }
}

/// `CreateCertificateAuthority` refuses a configuration or a tag set the service cannot
/// accept, and only those.
#[tokio::test]
async fn create_refuses_an_unusable_configuration_or_tag_set() {
    let emulator = Emulator::new();

    // A subject with no relative distinguished name names nobody.
    let mut body = root_ca_request("ignored");
    body["CertificateAuthorityConfiguration"]["Subject"] = json!({});
    emulator
        .refused(
            "an empty subject",
            "CreateCertificateAuthority",
            &body,
            "InvalidArgsException",
        )
        .await;

    // Any single X.500 attribute is enough, not only CommonName.
    for (case, subject) in [
        ("common name", json!({"CommonName": "example"})),
        ("organization", json!({"Organization": "Example Inc"})),
        ("country", json!({"Country": "GB"})),
        (
            "a custom attribute",
            json!({"CustomAttributes": [{"ObjectIdentifier": "2.5.4.3", "Value": "example"}]}),
        ),
    ] {
        let mut body = root_ca_request("ignored");
        body["CertificateAuthorityConfiguration"]["Subject"] = subject;
        emulator.ok(case, "CreateCertificateAuthority", &body).await;
    }

    let tag_rows: &[(&str, Value, bool)] = &[
        ("one tag", json!([{"Key": "owner", "Value": "pki"}]), true),
        ("a valueless tag", json!([{"Key": "owner"}]), true),
        (
            "two distinct keys",
            json!([{"Key": "owner", "Value": "pki"}, {"Key": "tier", "Value": "root"}]),
            true,
        ),
        (
            "a repeated key",
            json!([{"Key": "owner", "Value": "pki"}, {"Key": "owner", "Value": "sre"}]),
            false,
        ),
        ("a reserved key", json!([{"Key": "aws:created-by", "Value": "me"}]), false),
    ];
    for (case, tags, accepted) in tag_rows {
        let mut body = root_ca_request("tagged-root");
        body["Tags"] = tags.clone();
        if *accepted {
            emulator.ok(case, "CreateCertificateAuthority", &body).await;
        } else {
            emulator
                .refused(case, "CreateCertificateAuthority", &body, "InvalidTagException")
                .await;
        }
    }
}

/// `CreateCertificateAuthority` is `@idempotent` on its token, and only on its token.
#[tokio::test]
async fn create_mints_one_ca_per_idempotency_token() {
    let emulator = Emulator::new();
    let case = "idempotency";

    let mut with_token = root_ca_request("token-root");
    with_token["IdempotencyToken"] = json!("tok-1");
    let first = string(
        &emulator.ok(case, "CreateCertificateAuthority", &with_token).await,
        "CertificateAuthorityArn",
        case,
    );
    let repeated = string(
        &emulator.ok(case, "CreateCertificateAuthority", &with_token).await,
        "CertificateAuthorityArn",
        case,
    );
    assert_eq!(
        first, repeated,
        "case {case:?}: the same IdempotencyToken must answer with the first CA"
    );

    let mut other_token = root_ca_request("token-root");
    other_token["IdempotencyToken"] = json!("tok-2");
    let second = string(
        &emulator.ok(case, "CreateCertificateAuthority", &other_token).await,
        "CertificateAuthorityArn",
        case,
    );
    assert_ne!(
        first, second,
        "case {case:?}: a different IdempotencyToken must mint a different CA"
    );

    let untokened = root_ca_request("token-root");
    let third = string(
        &emulator.ok(case, "CreateCertificateAuthority", &untokened).await,
        "CertificateAuthorityArn",
        case,
    );
    let fourth = string(
        &emulator.ok(case, "CreateCertificateAuthority", &untokened).await,
        "CertificateAuthorityArn",
        case,
    );
    assert_ne!(
        third, fourth,
        "case {case:?}: without a token every call mints its own CA"
    );
}

/// Issuing repeatedly from one CA: every certificate gets its own serial and ARN, and
/// each one reads back as the certificate issued for exactly that ARN. An invariant over
/// a run of issuances, not a single case.
#[tokio::test]
async fn every_issuance_from_one_ca_is_its_own_certificate() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "serial-root").await;
    const ISSUANCES: usize = 16;

    let mut arns: Vec<String> = Vec::with_capacity(ISSUANCES);
    let mut requests: Vec<String> = Vec::with_capacity(ISSUANCES);
    for index in 0..ISSUANCES {
        let case = format!("issuance {index}");
        // The request differs per issuance, so two certificates never derive from
        // identical input either.
        let csr = csr_pem(&format!("end-entity-{index}"));
        let issued = emulator
            .ok(
                &case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&csr),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": index as i64 + 1, "Type": "DAYS"},
                }),
            )
            .await;
        arns.push(string(&issued, "CertificateArn", &case));
        requests.push(csr);
    }
    wait_for_the_clock();

    let mut seen = std::collections::HashSet::new();
    for (index, certificate_arn) in arns.iter().enumerate() {
        let case = format!("issuance {index}");
        assert!(
            seen.insert(certificate_arn.clone()),
            "case {case:?}: {certificate_arn:?} was minted twice"
        );
        let got = emulator
            .ok(
                &case,
                "GetCertificate",
                &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
            )
            .await;
        let certificate = string(&got, "Certificate", &case);
        let issued = certificate_of(&certificate, &case);
        assert_eq!(
            serial_value(issued.tbs_certificate.serial_number.as_bytes()),
            serial_from_arn(certificate_arn),
            "case {case:?}: {certificate_arn:?} must answer with the certificate of its serial"
        );
        assert_eq!(
            issued.tbs_certificate.subject_public_key_info,
            request_of(&requests[index], &case).info.public_key,
            "case {case:?}: the certificate carries the public key of the request it answers"
        );
        assert_eq!(
            subject_common_name(&issued.tbs_certificate.subject),
            format!("end-entity-{index}"),
            "case {case:?}: the certificate is issued for the subject of its own request"
        );
    }
}

/// A CA that holds no certificate cannot sign with one, and cannot hand one out.
#[tokio::test]
async fn a_ca_without_its_certificate_can_neither_issue_nor_serve_one() {
    let emulator = Emulator::new();
    let case = "pending CA";
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request("pending-root"))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    wait_for_the_clock();

    emulator
        .refused(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Csr": blob(&csr_pem("a-pending-request")),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Value": 7, "Type": "DAYS"},
            }),
            "InvalidStateException",
        )
        .await;
    emulator
        .refused(
            case,
            "GetCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn}),
            "InvalidStateException",
        )
        .await;

    // And once it is ACTIVE it cannot take a second certificate.
    let certificate = blob(&own_ca_certificate(&emulator, case, &arn).await);
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": certificate}),
        )
        .await;
    emulator
        .refused(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": certificate}),
            "InvalidStateException",
        )
        .await;
}

/// A CA whose UsageMode is SHORT_LIVED_CERTIFICATE issues only inside the seven days
/// its documentation limits it to, and refuses a longer window as a validation failure
/// (the sentence itself is asserted in [`super::short_lived_ca_certificate_validity`]).
#[tokio::test]
async fn a_short_lived_ca_refuses_a_validity_beyond_seven_days() {
    let emulator = Emulator::new();
    let case = "short lived";
    let mut body = root_ca_request("short-lived-root");
    body["UsageMode"] = json!("SHORT_LIVED_CERTIFICATE");
    let arn = string(
        &emulator.ok(case, "CreateCertificateAuthority", &body).await,
        "CertificateAuthorityArn",
        case,
    );
    wait_for_the_clock();
    let certificate = own_ca_certificate(&emulator, case, &arn).await;
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Certificate": blob(&certificate),
            }),
        )
        .await;

    for (case, days, accepted) in [
        ("one day", 1, true),
        ("seven days", 7, true),
        ("eight days", 8, false),
        ("a year", 365, false),
    ] {
        let request = json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&csr_pem("a-short-lived-request")),
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": days, "Type": "DAYS"},
        });
        if accepted {
            emulator.ok(case, "IssueCertificate", &request).await;
        } else {
            emulator
                .refused(case, "IssueCertificate", &request, "ValidationException")
                .await;
        }
    }
}

/// The emulator's state survives between requests, which is what makes the lifecycle
/// above a lifecycle: the CA read in one call is the CA written by an earlier one.
#[tokio::test]
async fn a_created_ca_is_visible_to_every_later_request() {
    let emulator = Emulator::new();
    let case = "shared state";
    let mut arns = Vec::new();
    for index in 0..3 {
        let created = emulator
            .ok(
                case,
                "CreateCertificateAuthority",
                &root_ca_request(&format!("root-{index}")),
            )
            .await;
        arns.push(string(&created, "CertificateAuthorityArn", case));
    }
    wait_for_the_clock();
    for (index, arn) in arns.iter().enumerate() {
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        assert_eq!(
            described["CertificateAuthority"]["CertificateAuthorityConfiguration"]["Subject"]
                ["CommonName"],
            json!(format!("root-{index}")),
            "case {case:?}: {arn:?} must still be the CA that was created"
        );
        let created_at = described["CertificateAuthority"]["CreatedAt"]
            .as_f64()
            .unwrap_or_default();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        assert!(
            created_at > 0.0 && created_at <= now + 1.0,
            "case {case:?}: CreatedAt must be the instant the CA was created, got {created_at}"
        );
    }
}

/// "The specified signing algorithm family (RSA or ECDSA) must match the algorithm
/// family of the CA's secret key." The rule holds over every `KeyAlgorithm` /
/// `SigningAlgorithm` pair the model declares, not only the RSA one the issue cited.
#[tokio::test]
async fn a_ca_signs_only_with_its_own_key_family() {
    let emulator = Emulator::new();

    // Rows drawn from KEY_ALGORITHM_VALUES / SIGNING_ALGORITHM_VALUES in `ops::types`.
    let rows: &[(&str, &str, &str, bool)] = &[
        ("rsa with rsa", "RSA_2048", "SHA256WITHRSA", true),
        ("rsa with rsa-pss", "RSA_4096", "SHA512WITHRSA_PSS", true),
        ("rsa with ecdsa", "RSA_2048", "SHA256WITHECDSA", false),
        ("ec with ecdsa", "EC_prime256v1", "SHA384WITHECDSA", true),
        ("ec with rsa", "EC_secp521r1", "SHA256WITHRSA", false),
        ("lattice with lattice", "ML_DSA_44", "ML_DSA_44", true),
        ("lattice with rsa", "ML_DSA_87", "SHA384WITHRSA", false),
        ("sm2 with sm3", "SM2", "SM3WITHSM2", true),
        ("sm2 with ecdsa", "SM2", "SHA256WITHECDSA", false),
    ];

    // Create every CA, then wait for the clock ONCE, then activate them all.
    let mut authorities: Vec<String> = Vec::with_capacity(rows.len());
    for (case, key_algorithm, _, _) in rows {
        let mut body = root_ca_request("family-root");
        body["CertificateAuthorityConfiguration"]["KeyAlgorithm"] = json!(key_algorithm);
        // The CA's own CSR is signed with a signature of its key's family.
        body["CertificateAuthorityConfiguration"]["SigningAlgorithm"] = json!(match *key_algorithm {
            "RSA_2048" | "RSA_4096" => "SHA256WITHRSA",
            "EC_prime256v1" | "EC_secp521r1" => "SHA256WITHECDSA",
            "SM2" => "SM3WITHSM2",
            lattice => lattice,
        });
        authorities.push(string(
            &emulator.ok(case, "CreateCertificateAuthority", &body).await,
            "CertificateAuthorityArn",
            case,
        ));
    }
    wait_for_the_clock();

    for ((case, _key_algorithm, signing_algorithm, accepted), arn) in rows.iter().zip(&authorities) {
        let own = own_ca_certificate(&emulator, case, arn).await;
        emulator
            .ok(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Certificate": blob(&own),
                }),
            )
            .await;
        let request = json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&csr_pem("a-family-request")),
            "SigningAlgorithm": signing_algorithm,
            "Validity": {"Value": 7, "Type": "DAYS"},
        });
        if *accepted {
            emulator.ok(case, "IssueCertificate", &request).await;
        } else {
            emulator
                .refused(case, "IssueCertificate", &request, "InvalidArgsException")
                .await;
        }
    }
}

/// Every store this service grows per request is bounded, and the bound is answered with
/// the quota error the operation models rather than by growing without limit.
#[tokio::test]
async fn the_per_request_stores_are_bounded() {
    use service_com_amazonaws_acmpca_acmprivateca::state::{
        MAX_CERTIFICATES_PER_CA, MAX_CERTIFICATE_AUTHORITIES, MAX_IDEMPOTENCY_TOKENS,
    };

    let emulator = Emulator::new();

    // Certificates per CA: the last one inside the bound is accepted, the next refused.
    let (arn, _) = active_ca(&emulator, "bounded-root").await;
    let issue = |index: usize| {
        json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&csr_pem(&format!("end-entity-{index}"))),
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": 7, "Type": "DAYS"},
        })
    };
    // One issuance for every slot the bound allows; the next one is past it.
    for index in 0..MAX_CERTIFICATES_PER_CA {
        emulator
            .ok(&format!("certificate {index}"), "IssueCertificate", &issue(index))
            .await;
    }
    emulator
        .refused(
            "one certificate past the quota",
            "IssueCertificate",
            &issue(MAX_CERTIFICATES_PER_CA),
            "LimitExceededException",
        )
        .await;

    // Idempotency tokens: beyond the bound the oldest record is evicted, so the token it
    // held no longer replays while the newest one still does.
    //
    // Each request is kept as it was sent, because a replay is the SAME request sent again:
    // a token identifies retries of one call, and the call is identified by its own content
    // (its certification request above all), so a body built afresh — with a fresh key pair
    // in its CSR — is another call and gets a certificate of its own.
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "token-bound-root").await;
    let mut requests: Vec<Value> = Vec::new();
    let mut minted: Vec<String> = Vec::new();
    for index in 0..=MAX_IDEMPOTENCY_TOKENS {
        let case = format!("token {index}");
        let mut body = json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&csr_pem(&format!("end-entity-{index}"))),
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": 7, "Type": "DAYS"},
        });
        body["IdempotencyToken"] = json!(format!("tok-{index}"));
        minted.push(string(
            &emulator.ok(&case, "IssueCertificate", &body).await,
            "CertificateArn",
            &case,
        ));
        requests.push(body);
    }
    let replay = |index: usize| requests[index].clone();
    let newest = MAX_IDEMPOTENCY_TOKENS;
    assert_eq!(
        string(
            &emulator.ok("newest token", "IssueCertificate", &replay(newest)).await,
            "CertificateArn",
            "newest token",
        ),
        minted[newest],
        "case {:?}: a live idempotency record must still replay",
        "newest token"
    );
    assert_ne!(
        string(
            &emulator.ok("evicted token", "IssueCertificate", &replay(0)).await,
            "CertificateArn",
            "evicted token",
        ),
        minted[0],
        "case {:?}: the oldest record is evicted at the bound, so its token mints anew",
        "evicted token"
    );

    // Certificate authorities: likewise bounded.
    let emulator = Emulator::new();
    for index in 0..MAX_CERTIFICATE_AUTHORITIES {
        emulator
            .ok(
                &format!("ca {index}"),
                "CreateCertificateAuthority",
                &root_ca_request(&format!("root-{index}")),
            )
            .await;
    }
    emulator
        .refused(
            "one CA past the quota",
            "CreateCertificateAuthority",
            &root_ca_request("one-too-many"),
            "LimitExceededException",
        )
        .await;
}

/// The algorithm catalogs this suite and the service read, pinned against the model.
///
/// `state::SIGNING_ALGORITHMS_IN_MODEL_ORDER` is the `SigningAlgorithm` enum in
/// declaration order (the order the service lists valid algorithms in); the generated
/// `ops::types::SIGNING_ALGORITHM_VALUES` is the same set, sorted. If the model gains a
/// member, the two stop agreeing and this fails instead of the service silently omitting
/// it.
#[test]
fn the_signing_algorithm_catalog_holds_every_member_the_model_declares() {
    use service_com_amazonaws_acmpca_acmprivateca::ops::types::SIGNING_ALGORITHM_VALUES;
    use service_com_amazonaws_acmpca_acmprivateca::state::SIGNING_ALGORITHMS_IN_MODEL_ORDER;

    let mut declared = SIGNING_ALGORITHMS_IN_MODEL_ORDER.to_vec();
    declared.sort_unstable();
    assert_eq!(
        declared,
        SIGNING_ALGORITHM_VALUES.to_vec(),
        "case {:?}: the declaration-order catalog must hold exactly the modelled members",
        "signing algorithms"
    );
}

/// A CA is created only with a `SigningAlgorithm` its `KeyAlgorithm` can sign with.
///
/// The rule is the one the CA's key imposes: "The specified signing algorithm family
/// (RSA or ECDSA) must match the algorithm family of the CA's secret key"
/// (`IssueCertificateRequest` docs) — a CA signs certificate requests with the key it was
/// created with, so a `CertificateAuthorityConfiguration` pairing a key with a signature
/// of another family is refused with `InvalidArgsException` and no CA is created.
///
/// The table spans the pairing of EVERY `KeyAlgorithm` family with EVERY
/// `SigningAlgorithm` family the model declares (the value lists in `ops::types`), not
/// only the RSA key / ECDSA signature pair the issue cited.
#[tokio::test]
async fn create_refuses_a_signing_algorithm_the_configured_key_cannot_sign_with() {
    let emulator = Emulator::new();

    // One key per family, from KEY_ALGORITHM_VALUES, against one signing algorithm per
    // family, from SIGNING_ALGORITHM_VALUES.
    let keys: &[(&str, &str)] = &[
        ("RSA_2048", "RSA"),
        ("RSA_3072", "RSA"),
        ("RSA_4096", "RSA"),
        ("EC_prime256v1", "EC"),
        ("EC_secp384r1", "EC"),
        ("EC_secp521r1", "EC"),
        ("ML_DSA_44", "ML_DSA"),
        ("ML_DSA_65", "ML_DSA"),
        ("ML_DSA_87", "ML_DSA"),
        ("SM2", "SM2"),
    ];
    let signatures: &[(&str, &str)] = &[
        ("SHA256WITHRSA", "RSA"),
        ("SHA384WITHRSA", "RSA"),
        ("SHA512WITHRSA", "RSA"),
        ("SHA256WITHRSA_PSS", "RSA"),
        ("SHA384WITHRSA_PSS", "RSA"),
        ("SHA512WITHRSA_PSS", "RSA"),
        ("SHA256WITHECDSA", "EC"),
        ("SHA384WITHECDSA", "EC"),
        ("SHA512WITHECDSA", "EC"),
        ("ML_DSA_44", "ML_DSA"),
        ("ML_DSA_65", "ML_DSA"),
        ("ML_DSA_87", "ML_DSA"),
        ("SM3WITHSM2", "SM2"),
    ];

    // Every CA a usable pairing created is persisted with exactly that pairing.
    let mut usable: Vec<(String, String, &str, &str)> = Vec::new();
    for (key_algorithm, key_family) in keys {
        for (signing_algorithm, signing_family) in signatures {
            let case = format!("{key_algorithm} with {signing_algorithm}");
            let mut body = root_ca_request("algorithm-root");
            body["CertificateAuthorityConfiguration"]["KeyAlgorithm"] = json!(key_algorithm);
            body["CertificateAuthorityConfiguration"]["SigningAlgorithm"] =
                json!(signing_algorithm);
            if key_family == signing_family {
                let created = emulator.ok(&case, "CreateCertificateAuthority", &body).await;
                let arn = string(&created, "CertificateAuthorityArn", &case);
                assert!(
                    arn.contains(":certificate-authority/"),
                    "case {case:?}: a usable pairing creates a CA, got {arn:?}"
                );
                usable.push((case.clone(), arn, *key_algorithm, *signing_algorithm));
                continue;
            }
            let refused = emulator
                .refused(&case, "CreateCertificateAuthority", &body, "InvalidArgsException")
                .await;
            // The message is built from the request, so it names the algorithm that was
            // rejected, the key it cannot be used with, and the algorithms that key signs
            // with.
            let message = refused
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                message.contains(signing_algorithm) && message.contains(key_algorithm),
                "case {case:?}: expected a message naming {signing_algorithm} and \
                 {key_algorithm}, got {message:?}"
            );
            let valid = signatures
                .iter()
                .filter(|(_, family)| family == key_family)
                .map(|(algorithm, _)| *algorithm);
            for algorithm in valid {
                assert!(
                    message.contains(algorithm),
                    "case {case:?}: expected the message to list {algorithm} as valid for \
                     {key_algorithm}, got {message:?}"
                );
            }
        }
    }

    // And nothing a refused pairing asked for was persisted: every CA that exists is one
    // of the usable pairings, and reads back carrying exactly the algorithms it was
    // created with. (`ListCertificateAuthorities` is still scaffolded, so the read-back is
    // per CA.)
    wait_for_the_clock();
    for (case, arn, key_algorithm, signing_algorithm) in usable {
        let described = emulator
            .ok(&case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        let configuration =
            &described["CertificateAuthority"]["CertificateAuthorityConfiguration"];
        assert_eq!(
            configuration["KeyAlgorithm"],
            json!(key_algorithm),
            "case {case:?}: the CA keeps the key algorithm it was created with, \
             got {configuration:?}"
        );
        assert_eq!(
            configuration["SigningAlgorithm"],
            json!(signing_algorithm),
            "case {case:?}: the CA keeps the signing algorithm it was created with, \
             got {configuration:?}"
        );
    }
}

/// A reported `CertificateAuthority` always carries both revocation configurations.
///
/// The rule: `CrlConfiguration$Enabled` and `OcspConfiguration$Enabled` are `@required`
/// booleans and "By default, both certificate validation mechanisms are disabled"
/// (`CreateCertificateAuthorityRequest$RevocationConfiguration` docs), so every read of a
/// CA reports a `RevocationConfiguration` holding both — the configuration the CA was
/// created with, and the disabled configuration for the mechanism it was not.
///
/// The table spans a CA created with no revocation member at all, with one mechanism
/// only, and with both. Every operation that reports a `CertificateAuthority` reads the
/// same projection, so the assertion is on `DescribeCertificateAuthority`
/// (`ListCertificateAuthorities` is still scaffolded, under its own issue).
#[tokio::test]
async fn a_described_ca_always_carries_both_revocation_configurations() {
    let emulator = Emulator::new();

    let rows: &[(&str, Option<Value>, bool, bool)] = &[
        ("no revocation member", None, false, false),
        (
            "crl disabled only",
            Some(json!({"CrlConfiguration": {"Enabled": false}})),
            false,
            false,
        ),
        (
            "ocsp enabled only",
            Some(json!({"OcspConfiguration": {"Enabled": true}})),
            false,
            true,
        ),
        (
            "crl enabled only",
            Some(json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 7,
                    "S3BucketName": "crl-bucket",
                }
            })),
            true,
            false,
        ),
        (
            "both mechanisms on",
            Some(json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 30,
                    "S3BucketName": "crl-bucket",
                },
                "OcspConfiguration": {"Enabled": true, "OcspCustomCname": "ocsp.example.com"},
            })),
            true,
            true,
        ),
        (
            "both mechanisms off",
            Some(json!({
                "CrlConfiguration": {"Enabled": false},
                "OcspConfiguration": {"Enabled": false},
            })),
            false,
            false,
        ),
    ];

    let mut created: Vec<(&str, String, bool, bool)> = Vec::with_capacity(rows.len());
    for (case, revocation, crl, ocsp) in rows {
        let mut body = root_ca_request("revocation-report-root");
        if let Some(revocation) = revocation {
            body["RevocationConfiguration"] = revocation.clone();
        }
        let response = emulator.ok(case, "CreateCertificateAuthority", &body).await;
        created.push((
            case,
            string(&response, "CertificateAuthorityArn", case),
            *crl,
            *ocsp,
        ));
    }
    wait_for_the_clock();

    for (case, arn, crl, ocsp) in created {
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        let revocation = &described["CertificateAuthority"]["RevocationConfiguration"];
        assert_eq!(
            revocation["CrlConfiguration"]["Enabled"],
            json!(crl),
            "case {case:?}: the describe must report CrlConfiguration.Enabled={crl}, \
             got {revocation:?}"
        );
        assert_eq!(
            revocation["OcspConfiguration"]["Enabled"],
            json!(ocsp),
            "case {case:?}: the describe must report OcspConfiguration.Enabled={ocsp}, \
             got {revocation:?}"
        );
    }
}

/// The octets of the serial numbers the certificates of the table below carry.
///
/// The rows span what the rule generalises over — the WIDTH and the OCTETS of the number a
/// certificate carries, which is all a serial number is: `CertificateAuthority$Serial` is a
/// plain string with no length, pattern or format trait
/// (`smithy_query(query="shape",shape="CertificateAuthority")`), so nothing about the value
/// is constrained and every width an X.509 serial number may hold has to read back as the
/// number the certificate holds. One octet, a leading nibble of zero, a leading octet whose
/// high bit is set (which DER prefixes with a zero octet that is no part of the value),
/// interior zero octets, the sixteen octets this service allocates for the certificates it
/// issues, and the twenty octets RFC 5280 section 4.1.2.2 sets as the widest a conforming CA
/// may use.
const SERIAL_OCTETS: &[(&str, &[u8])] = &[
    ("a one-octet serial number", &[0x01]),
    ("a leading nibble of zero", &[0x0f, 0xdb]),
    ("a leading octet with its high bit set", &[0xc8, 0x5c, 0xfc]),
    ("interior zero octets", &[0x10, 0x00, 0x00, 0x05]),
    (
        "sixteen octets, the width this service allocates",
        &[
            0x0f, 0xdb, 0xfb, 0x98, 0x3c, 0xc4, 0x8f, 0xca, 0xde, 0xbf, 0x03, 0xcf, 0x32, 0x83,
            0xba, 0xac,
        ],
    ),
    (
        "twenty octets, the widest RFC 5280 section 4.1.2.2 allows",
        &[
            0x7f, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13,
        ],
    ),
];

/// `octets` as the integer they are: the number a DER INTEGER of those octets denotes, in
/// base ten, read out with the big-integer library rather than by this suite.
fn as_integer(octets: &[u8]) -> String {
    num_bigint::BigUint::from_bytes_be(octets).to_str_radix(10)
}

/// A catalog of hand-computed values, which is what pins the BASE the number is stated in: a
/// rendering in any other base would satisfy every assertion below that only reads the number
/// back out again. The widest is 2^128 - 1, the largest sixteen-octet serial number there is.
const SERIAL_INTEGERS: &[(&[u8], &str)] = &[
    (&[0x01], "1"),
    (&[0x0f, 0xdb], "4059"),
    (&[0xc8, 0x5c, 0xfc], "13131004"),
    (&[0x10, 0x00, 0x00, 0x05], "268435461"),
    (
        &[0xff; 16],
        "340282366920938463463374607431768211455",
    ),
];

#[test]
fn a_serial_number_is_stated_in_base_ten() {
    for (octets, expected) in SERIAL_INTEGERS {
        let stated = as_integer(octets);
        assert_eq!(
            &stated, expected,
            "case {octets:02x?}: expected the number those octets are in base ten, \
             got {stated:?}"
        );
    }
}

/// A private CA reports as its `Serial` the serial number of the X.509 certificate it was
/// activated with, stated as the integer that number is — and reports none at all while it
/// holds no certificate of its own.
///
/// Read through every projection of a `CertificateAuthority`: `DescribeCertificateAuthority`
/// and `ListCertificateAuthorities` report the same structure, so both have to state the
/// same number. The expectation of a row is the number the certificate the CA was activated
/// with carries, read with the same X.509 library a client reads it with — including off the
/// certificate this service hands back from `GetCertificateAuthorityCertificate`, which is
/// what makes the value a client can join.
#[tokio::test]
async fn a_ca_reports_the_serial_number_of_the_certificate_it_holds() {
    let emulator = Emulator::new();

    // Every CA is created first, so one wait covers the transition all of them owe.
    let mut authorities: Vec<String> = Vec::with_capacity(SERIAL_OCTETS.len());
    for (index, (case, _)) in SERIAL_OCTETS.iter().enumerate() {
        let created = emulator
            .ok(
                case,
                "CreateCertificateAuthority",
                &root_ca_request(&format!("serial-report-{index}")),
            )
            .await;
        authorities.push(string(&created, "CertificateAuthorityArn", case));
    }

    // A CA that holds no certificate has no serial number to report — in CREATING, and
    // still once it has settled into PENDING_CERTIFICATE.
    for ((case, _), arn) in SERIAL_OCTETS.iter().zip(&authorities) {
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        let authority = &described["CertificateAuthority"];
        assert!(
            authority.get("Serial").is_none(),
            "case {case:?}: a CA holding no certificate has no serial number to report, \
             got {authority:?}"
        );
    }
    wait_for_the_clock();

    for ((case, octets), arn) in SERIAL_OCTETS.iter().zip(&authorities) {
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        let authority = &described["CertificateAuthority"];
        assert!(
            authority.get("Serial").is_none(),
            "case {case:?}: a CA still waiting for its certificate has no serial number to \
             report, got {authority:?}"
        );

        // The certificate the CA's own certification request asks for, carrying this row's
        // serial number, built by the X.509 library rather than by this service.
        let csr = string(
            &emulator
                .ok(case, "GetCertificateAuthorityCsr", &json!({"CertificateAuthorityArn": arn}))
                .await,
            "Csr",
            case,
        );
        let request = request_of(&csr, case);
        let ca_certificate = certificate_over_with_serial(
            &request.info.subject,
            &request.info.public_key,
            &request.algorithm,
            octets,
        );
        // The certificate really does carry this row's number: a DER INTEGER drops nothing
        // and adds only the zero octet that keeps the value positive.
        let expected = as_integer(octets);
        assert_eq!(
            serial_as_integer(&certificate_of(&ca_certificate, case)),
            expected,
            "case {case:?}: the certificate under test must carry the row's serial number"
        );

        emulator
            .ok(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Certificate": blob(&ca_certificate),
                }),
            )
            .await;

        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        let authority = &described["CertificateAuthority"];
        let reported = string(authority, "Serial", case);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the serial number of the certificate the CA holds, \
             got {reported:?}"
        );
        // And it is stated as a number: decimal digits, with no separator and no leading
        // zero, which is the one spelling of a value in base ten.
        assert!(
            reported.chars().all(|digit| digit.is_ascii_digit())
                && (reported == "0" || !reported.starts_with('0')),
            "case {case:?}: expected the number in base ten, got {reported:?}"
        );

        // The same number the certificate this service hands back carries, which is the
        // join a client makes.
        let read_back = emulator
            .ok(
                case,
                "GetCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let held = certificate_of(&string(&read_back, "Certificate", case), case);
        assert_eq!(
            serial_as_integer(&held),
            reported,
            "case {case:?}: expected the serial number a client reads off the CA certificate, \
             got {reported:?}"
        );

        // Reporting it changed nothing: a second read states the same number.
        let again = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        assert_eq!(
            string(&again["CertificateAuthority"], "Serial", case),
            reported,
            "case {case:?}: a serial number is stable across reads"
        );
    }

    // Every projection of a `CertificateAuthority` states the same number: the listing
    // reports the structure the describe does.
    let listed = emulator
        .ok("the listing", "ListCertificateAuthorities", &json!({}))
        .await;
    let reported = listed
        .get("CertificateAuthorities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a CertificateAuthorities list, got {listed:?}"))
        .clone();
    for ((case, octets), arn) in SERIAL_OCTETS.iter().zip(&authorities) {
        let authority = reported
            .iter()
            .find(|authority| string(authority, "Arn", case) == *arn)
            .unwrap_or_else(|| panic!("case {case:?}: the listing omits {arn}"));
        assert_eq!(
            string(authority, "Serial", case),
            as_integer(octets),
            "case {case:?}: the listing must state the serial the describe states, \
             got {authority:?}"
        );
    }
}

// --- the wire protocols this service declares -------------------------------------------

/// The name this service's shape carries in the model, which both of its protocols spell
/// into a request: into `X-Amz-Target: <shape>.<Operation>` on the JSON wire, and into the
/// path `/service/<shape>/operation/<Operation>` on the CBOR one.
pub(crate) const SERVICE_SHAPE: &str = "ACMPrivateCA";

/// One wire protocol this service's model declares, as the marker a request names it by.
///
/// The model declares two of them — `smithy_query(query="service_info")` reports
/// `awsJson1_1` and `rpcv2Cbor` — and a client may drive every operation over either, so a
/// request is served, and answered, in the protocol it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wire {
    /// `awsJson1_1`: `POST /` under `X-Amz-Target`, with a JSON document as the body.
    AwsJson,
    /// `rpcv2Cbor`: `POST /service/{Service}/operation/{Operation}` under
    /// `smithy-protocol: rpc-v2-cbor`, with a CBOR document as the body.
    RpcV2Cbor,
}

/// Every protocol the model declares, so a table below runs its rows over all of them
/// rather than over whichever one a suite happened to be written against.
pub(crate) const DECLARED_WIRES: &[Wire] = &[Wire::AwsJson, Wire::RpcV2Cbor];

impl Wire {
    /// The request this protocol carries `operation` and `body` in: its routing (a header
    /// on the JSON wire, the URI path on the CBOR one) and its encoding of the body.
    fn request(self, operation: &str, body: &Value) -> http::Request<bytes::Bytes> {
        let host = format!("acm-pca.{REGION}.amazonaws.com");
        let common = http::Request::builder()
            .method("POST")
            .header("host", &host)
            .header("authorization", AUTHORIZATION);
        let (builder, encoded) = match self {
            Wire::AwsJson => (
                common
                    .uri(format!("https://{host}/"))
                    .header("content-type", "application/x-amz-json-1.1")
                    .header("x-amz-target", format!("{SERVICE_SHAPE}.{operation}")),
                serde_json::to_vec(body).expect("the request body serializes"),
            ),
            Wire::RpcV2Cbor => (
                common
                    .uri(format!(
                        "https://{host}/service/{SERVICE_SHAPE}/operation/{operation}"
                    ))
                    .header("content-type", "application/cbor")
                    .header("accept", "application/cbor")
                    .header("smithy-protocol", "rpc-v2-cbor"),
                cbor_document(body),
            ),
        };
        builder
            .body(bytes::Bytes::from(encoded))
            .expect("the request builds")
    }

    /// The members of a response body this protocol carries, read back as the same JSON
    /// document whichever protocol answered — so one assertion reads either answer.
    fn members(self, body: &[u8]) -> Value {
        if body.is_empty() {
            return Value::Null;
        }
        match self {
            Wire::AwsJson => serde_json::from_slice(body).unwrap_or(Value::Null),
            Wire::RpcV2Cbor => cbor_members(body),
        }
    }

    /// The `content-type` a response of this protocol carries.
    fn content_type(self) -> &'static str {
        match self {
            Wire::AwsJson => "application/x-amz-json-1.1",
            Wire::RpcV2Cbor => "application/cbor",
        }
    }

    /// The `smithy-protocol` header this protocol tags a response with, where it has one:
    /// the CBOR protocol names itself in every response, and the JSON wire sends no such
    /// header.
    fn smithy_protocol(self) -> Option<&'static str> {
        match self {
            Wire::AwsJson => None,
            Wire::RpcV2Cbor => Some("rpc-v2-cbor"),
        }
    }

    /// A short name for a case, so two rows of one table build resources of their own.
    fn label(self) -> &'static str {
        match self {
            Wire::AwsJson => "json",
            Wire::RpcV2Cbor => "cbor",
        }
    }
}

impl Emulator {
    /// Call `operation` over `wire` and return the HTTP status with the response members.
    pub(crate) async fn call_on(&self, wire: Wire, operation: &str, body: &Value) -> (u16, Value) {
        let response = self.dispatch_on(wire, operation, body).await;
        let status = response.status().as_u16();
        (status, wire.members(response.body()))
    }

    /// The whole HTTP response `wire` answers `operation` with, so a suite can read the
    /// headers that say which protocol the answer is in.
    pub(crate) async fn dispatch_on(
        &self,
        wire: Wire,
        operation: &str,
        body: &Value,
    ) -> http::Response<bytes::Bytes> {
        self.router.dispatch(wire.request(operation, body)).await
    }

    /// Call `operation` over `wire`, requiring success.
    pub(crate) async fn ok_on(
        &self,
        wire: Wire,
        case: &str,
        operation: &str,
        body: &Value,
    ) -> Value {
        let (status, parsed) = self.call_on(wire, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} over {wire:?} to answer 200, \
             got {status} {parsed:?}"
        );
        parsed
    }
}

/// `body` as the CBOR document the `rpcv2Cbor` wire carries it as, written by the CBOR
/// library rather than by the encoder the emulator answers with.
fn cbor_document(body: &Value) -> Vec<u8> {
    let mut encoded = Vec::new();
    ciborium::ser::into_writer(&cbor_of_json(body), &mut encoded)
        .expect("the request body encodes as CBOR");
    encoded
}

/// One JSON value as the CBOR item of the same kind: an object is a map keyed by text, an
/// array a list, a string text, an integral number an integer and any other number a
/// float. Those are the modelled types the rows below send.
fn cbor_of_json(value: &Value) -> ciborium::value::Value {
    use ciborium::value::Value as Item;
    match value {
        Value::Null => Item::Null,
        Value::Bool(flag) => Item::Bool(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => Item::Integer(integer.into()),
            None => Item::Float(number.as_f64().expect("a JSON number is an i64 or an f64")),
        },
        Value::String(text) => Item::Text(text.clone()),
        Value::Array(items) => Item::Array(items.iter().map(cbor_of_json).collect()),
        Value::Object(members) => Item::Map(
            members
                .iter()
                .map(|(name, value)| (Item::Text(name.clone()), cbor_of_json(value)))
                .collect(),
        ),
    }
}

/// The members of a CBOR response body as the JSON document of the same values, or
/// `Value::Null` for a body that is not CBOR at all.
///
/// Two CBOR items have no JSON spelling of their own, and each is read as the JSON wire
/// states that modelled type: a byte string (a `blob`) as its base64, and a tagged item (a
/// `timestamp`, tag 1) as the value under the tag.
fn cbor_members(body: &[u8]) -> Value {
    match ciborium::de::from_reader::<ciborium::value::Value, _>(body) {
        Ok(item) => json_of_cbor(&item),
        Err(_) => Value::Null,
    }
}

/// One CBOR item as the JSON value of the same kind, see [`cbor_members`].
fn json_of_cbor(item: &ciborium::value::Value) -> Value {
    use ciborium::value::Value as Item;
    match item {
        Item::Null => Value::Null,
        Item::Bool(flag) => Value::Bool(*flag),
        Item::Integer(integer) => i64::try_from(i128::from(*integer))
            .map(Value::from)
            .unwrap_or(Value::Null),
        Item::Float(number) => serde_json::Number::from_f64(*number)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Item::Bytes(octets) => Value::String(aws_smithy_types::base64::encode(octets)),
        Item::Text(text) => Value::String(text.clone()),
        Item::Array(items) => Value::Array(items.iter().map(json_of_cbor).collect()),
        Item::Map(members) => Value::Object(
            members
                .iter()
                .map(|(name, value)| (cbor_key(name), json_of_cbor(value)))
                .collect(),
        ),
        Item::Tag(_, tagged) => json_of_cbor(tagged),
        other => panic!("a response carried the CBOR item {other:?}, which names no member"),
    }
}

/// A CBOR map key as the member name it is: a member name is text on both protocols.
fn cbor_key(item: &ciborium::value::Value) -> String {
    match item {
        ciborium::value::Value::Text(text) => text.clone(),
        other => panic!("a response keyed a member by {other:?}, which is not a member name"),
    }
}

/// The wire error code an error body carries, which both of this service's protocols
/// convey as the body's `__type` member.
pub(crate) fn error_code(body: &Value, case: &str) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no error type in {body:?}"))
        .to_string()
}

/// Every operation is served over each protocol the model declares, and the answer is in
/// the protocol the request named.
///
/// The rows differ along what the rule generalises over: the protocol the request names,
/// and the shape of the call — a create (a nested structure and an enumerated member in,
/// one string out), a read of what that create persisted (a structure out), a write of a
/// list member, the read of that list back, and a listing asked for with no member at all.
#[tokio::test]
async fn each_declared_protocol_serves_every_operation_and_answers_in_itself() {
    let emulator = Emulator::new();

    for wire in DECLARED_WIRES.iter().copied() {
        let case = format!("a request naming {wire:?}");
        let common_name = format!("two-wires-{}", wire.label());

        // A create: the minted ARN comes back in the protocol the request named.
        let created = emulator
            .ok_on(
                wire,
                &case,
                "CreateCertificateAuthority",
                &root_ca_request(&common_name),
            )
            .await;
        let arn = string(&created, "CertificateAuthorityArn", &case);
        let prefix = format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/");
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected an ARN under {prefix:?}, got {arn:?}"
        );

        // A read of what that create persisted.
        let described = emulator
            .ok_on(
                wire,
                &case,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let authority = &described["CertificateAuthority"];
        assert_eq!(
            authority.get("Arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the CA the create minted, got {described:?}"
        );
        let subject = authority
            .pointer("/CertificateAuthorityConfiguration/Subject/CommonName")
            .and_then(Value::as_str);
        assert_eq!(
            subject,
            Some(common_name.as_str()),
            "case {case:?}: expected the subject the create carried, got {described:?}"
        );

        // A write of a list member, and the read of that list back.
        emulator
            .ok_on(
                wire,
                &case,
                "TagCertificateAuthority",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Tags": [{"Key": "wire", "Value": wire.label()}],
                }),
            )
            .await;
        let listed_tags = emulator
            .ok_on(wire, &case, "ListTags", &json!({"CertificateAuthorityArn": arn}))
            .await;
        assert_eq!(
            listed_tags.pointer("/Tags/0/Value").and_then(Value::as_str),
            Some(wire.label()),
            "case {case:?}: expected the tag the write persisted, got {listed_tags:?}"
        );

        // A listing, asked for with no member at all — an empty document on either wire.
        let listed = emulator
            .ok_on(wire, &case, "ListCertificateAuthorities", &json!({}))
            .await;
        let arns: Vec<&str> = listed
            .get("CertificateAuthorities")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no listing in {listed:?}"))
            .iter()
            .filter_map(|authority| authority.get("Arn").and_then(Value::as_str))
            .collect();
        assert!(
            arns.contains(&arn.as_str()),
            "case {case:?}: expected the listing to hold {arn:?}, got {arns:?}"
        );

        // And the answer says which protocol it is in.
        let response = emulator
            .dispatch_on(
                wire,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .map(|value| value.to_str().expect("a header of ASCII").to_string())
        };
        assert_eq!(
            header("content-type").as_deref(),
            Some(wire.content_type()),
            "case {case:?}: expected the content type of the protocol the request named"
        );
        assert_eq!(
            header("smithy-protocol").as_deref(),
            wire.smithy_protocol(),
            "case {case:?}: expected the protocol header of the protocol the request named"
        );
    }
}

/// One private CA is one resource, whichever protocol it was created over: the state a call
/// on one wire persisted is what a call on the other reports.
///
/// The rows differ along the protocol the resource was created over, and each is read back
/// over BOTH — so the answer asserted is the whole document, member for member, rather than
/// the one member a single row happens to look at.
#[tokio::test]
async fn a_private_ca_created_over_one_protocol_is_read_back_over_the_other() {
    let emulator = Emulator::new();

    for created_over in DECLARED_WIRES.iter().copied() {
        let case = format!("a CA created over {created_over:?}");
        let common_name = format!("either-wire-{}", created_over.label());
        let created = emulator
            .ok_on(
                created_over,
                &case,
                "CreateCertificateAuthority",
                &root_ca_request(&common_name),
            )
            .await;
        let arn = string(&created, "CertificateAuthorityArn", &case);

        let mut answers: Vec<(Wire, Value)> = Vec::with_capacity(DECLARED_WIRES.len());
        for read_over in DECLARED_WIRES.iter().copied() {
            answers.push((
                read_over,
                emulator
                    .ok_on(
                        read_over,
                        &case,
                        "DescribeCertificateAuthority",
                        &json!({"CertificateAuthorityArn": arn}),
                    )
                    .await,
            ));
        }
        let (first_wire, first) = &answers[0];
        for (read_over, answer) in &answers[1..] {
            assert_eq!(
                answer, first,
                "case {case:?}: expected the read over {read_over:?} to report what the read \
                 over {first_wire:?} reports, got {answer:?} against {first:?}"
            );
        }
        assert_eq!(
            first
                .pointer("/CertificateAuthority/Arn")
                .and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the CA the create minted, got {first:?}"
        );
    }
}

/// A refusal is rendered in the protocol of the request, and says the same thing either
/// way: the status and the error code are the service's, so only the envelope differs.
///
/// The rows differ along the refusal class — an ARN naming no private CA resource, a
/// well-formed ARN naming no authority this service holds, and a request the
/// request-validation stage refuses before any handler runs — and each runs over every
/// protocol the model declares.
#[tokio::test]
async fn a_refusal_is_rendered_in_the_protocol_of_the_request() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an ARN that names no private CA resource",
            "DescribeCertificateAuthority",
            json!({
                "CertificateAuthorityArn":
                    format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate/0123456789abcdef"),
            }),
            "InvalidArnException",
        ),
        (
            "a well-formed ARN naming no authority",
            "GetPolicy",
            json!({
                "ResourceArn":
                    format!(
                        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                         7b9f4c1e-0d2a-4f6b-9c3e-5a8d1f2b4c60"
                    ),
            }),
            "ResourceNotFoundException",
        ),
        (
            "a request carrying no value for a required member",
            "DescribeCertificateAuthority",
            json!({}),
            "ValidationException",
        ),
    ];

    for (case, operation, body, expected) in &rows {
        let mut answers: Vec<(Wire, u16, Value)> = Vec::with_capacity(DECLARED_WIRES.len());
        for wire in DECLARED_WIRES.iter().copied() {
            let (status, parsed) = emulator.call_on(wire, operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} over {wire:?} to refuse with 400, \
                 got {status} {parsed:?}"
            );
            let code = error_code(&parsed, case);
            assert!(
                code.contains(expected),
                "case {case:?}: expected {operation} over {wire:?} to answer {expected}, \
                 got {code:?}"
            );
            answers.push((wire, status, parsed));
        }
        let (first_wire, first_status, first) = &answers[0];
        for (wire, status, parsed) in &answers[1..] {
            assert_eq!(
                (status, error_message(parsed, case)),
                (first_status, error_message(first, case)),
                "case {case:?}: expected the refusal over {wire:?} to say what the refusal \
                 over {first_wire:?} says, got {parsed:?} against {first:?}"
            );
        }
    }
}

/// A request that names NONE of the protocols the model declares is not served at all: a
/// service declaring more than one cannot guess which envelope such a caller speaks, so the
/// call is refused before any handler runs and performs nothing.
#[tokio::test]
async fn a_request_naming_none_of_the_declared_protocols_performs_nothing() {
    let emulator = Emulator::new();
    let case = "a request naming no protocol";

    // The CBOR wire's routing, with no protocol marker and a content type neither protocol
    // claims: the request names an operation, so what refuses it is the absent protocol.
    let host = format!("acm-pca.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", "application/octet-stream")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(&root_ca_request("no-protocol")).expect("the body serializes"),
        ))
        .expect("the request builds");
    let response = emulator.router().dispatch(request).await;
    let status = response.status().as_u16();
    assert_ne!(
        status, 200,
        "case {case:?}: a request naming none of the declared protocols must not be served"
    );

    // And nothing was created: the refusal came before any handler.
    let listed = emulator
        .ok_on(
            Wire::AwsJson,
            case,
            "ListCertificateAuthorities",
            &json!({}),
        )
        .await;
    assert_eq!(
        listed.get("CertificateAuthorities").and_then(Value::as_array),
        Some(&Vec::new()),
        "case {case:?}: expected no CA to have been created, got {listed:?}"
    );
}
