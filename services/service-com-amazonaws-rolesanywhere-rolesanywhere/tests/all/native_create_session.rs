// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the native session plane: `POST /sessions`, the call IAM
//! Roles Anywhere exists to serve.
//!
//! The rule under test: a request to this plane that presents an X.509 certificate issued
//! by an ENABLED trust anchor's own authority, proves it holds that certificate's private
//! key by signing the request with it, and names an ENABLED profile listing a role whose
//! trust policy lets this service assume it, is answered 201 with exactly one credential
//! set — a real session of that role, under the name the certificate's serial number gives
//! it — and is recorded against the subject that certificate identifies; a request failing
//! any of those is refused with the error class that gate belongs to (403
//! `AccessDeniedException` for an identity or a grant, 404 `ResourceNotFoundException` for
//! an ARN this account does not hold, 400 `ValidationException` for a request outside the
//! published shape) and vends nothing.
//!
//! Each table's rows differ along what the rule generalises over: the key family and curve
//! of the certificate and of the authority that issued it (so the signature scheme and the
//! chain algorithm both vary), the certificate's serial number (including one whose DER
//! encoding carries a leading zero byte), the host the request is addressed to (this
//! emulator's own address and the AWS one, which resolve this service by different means),
//! the session name the request asks for and whether the profile accepts one, the duration
//! asked for (each published bound and one past each), which gate refuses the request, and
//! how the signature is wrong.
//!
//! The certificates are written by `rcgen` and the request signatures by `aws-lc-rs` /
//! `ring`, so the bytes under test are produced by something other than the code under
//! test, and the string-to-sign is built here the way a client builds it rather than by
//! calling into the implementation.
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::sync::Arc;

use aws_lc_rs::encoding::AsDer as _;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The role a profile names, and the id IAM holds it under.
const ROLE: &str = "rolesanywhere-workload";
/// See [`ROLE`].
const ROLE_ID: &str = "AROAEXAMPLEWORKLOADROLE1";

/// A second role, for the row whose profile does not list the role the request asks for.
const OTHER_ROLE: &str = "rolesanywhere-other";
/// See [`OTHER_ROLE`].
const OTHER_ROLE_ID: &str = "AROAEXAMPLEOTHERROLE0001";

/// The service principal a role's trust policy names to let IAM Roles Anywhere assume it.
const SERVICE_PRINCIPAL: &str = "rolesanywhere.amazonaws.com";

/// This service's AWS host, which the front door resolves a route key from.
const AWS_HOST: &str = "rolesanywhere.us-west-2.amazonaws.com";

/// This emulator's own address, which resolves no route key at all — the address a workload
/// wired to the emulator actually calls.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The instant the requests here are signed at, and its date stamp. Nothing on this plane
/// is decided by it: it is a value the signature covers.
const SIGNED_AT: &str = "20260102T030405Z";
/// See [`SIGNED_AT`].
const SIGNED_DATE: &str = "20260102";

/// The headers every signature here covers, in the canonical (sorted) order.
const SIGNED_HEADERS: &str = "content-type;host;x-amz-date;x-amz-x509";

/// The shortest and the longest session this plane vends (AWS's published range for
/// `CreateSession`'s `durationSeconds`).
const MIN_DURATION: i64 = 900;
/// See [`MIN_DURATION`].
const MAX_DURATION: i64 = 3600;

/// Which key family and curve a certificate carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyKind {
    Rsa2048,
    EcdsaP256,
    EcdsaP384,
}

/// A private key, as the signer of a request and as `rcgen` writes certificates with it.
struct Signer {
    kind: KeyKind,
    /// The private key, PKCS#8 DER.
    pkcs8: Vec<u8>,
}

impl Signer {
    /// The `AWS4-X509-*` scheme a request signed with this key carries, which is the scheme
    /// AWS's credential helper signs with for each key family.
    fn scheme(&self) -> &'static str {
        match self.kind {
            KeyKind::Rsa2048 => "AWS4-X509-RSA-SHA256",
            KeyKind::EcdsaP256 | KeyKind::EcdsaP384 => "AWS4-X509-ECDSA-SHA256",
        }
    }

    /// `message` signed by this key: RSASSA-PKCS1-v1_5 over SHA-256 for an RSA key, ECDSA
    /// over SHA-256 (DER-encoded) for an elliptic-curve one.
    fn sign(&self, message: &[u8]) -> Vec<u8> {
        match self.kind {
            KeyKind::Rsa2048 => {
                let pair = aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&self.pkcs8)
                    .expect("the generated RSA key is readable");
                let mut signature = vec![0u8; pair.public_modulus_len()];
                pair.sign(
                    &aws_lc_rs::signature::RSA_PKCS1_SHA256,
                    &aws_lc_rs::rand::SystemRandom::new(),
                    message,
                    &mut signature,
                )
                .expect("the RSA key signs");
                signature
            }
            KeyKind::EcdsaP256 | KeyKind::EcdsaP384 => {
                let random = ring::rand::SystemRandom::new();
                let pair = ring::signature::EcdsaKeyPair::from_pkcs8(
                    &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                    &self.pkcs8,
                    &random,
                )
                .expect("the generated elliptic-curve key is readable");
                pair.sign(&random, message)
                    .expect("the elliptic-curve key signs")
                    .as_ref()
                    .to_vec()
            }
        }
    }
}

/// A key pair `rcgen` can write certificates with, and the same key as a [`Signer`].
fn key_pair(kind: KeyKind) -> (rcgen::KeyPair, Signer) {
    match kind {
        KeyKind::Rsa2048 => {
            // `rcgen`'s backend generates no RSA key, so the key comes from `aws-lc-rs`
            // and is handed to `rcgen` as the PKCS#8 PEM it reads one from.
            let private =
                aws_lc_rs::rsa::PrivateDecryptingKey::generate(aws_lc_rs::rsa::KeySize::Rsa2048)
                    .expect("an RSA-2048 key pair");
            let pkcs8 = private
                .as_der()
                .expect("the PKCS#8 encoding of the RSA key")
                .as_ref()
                .to_vec();
            let pem = x509_cert::der::pem::encode_string(
                "PRIVATE KEY",
                x509_cert::der::pem::LineEnding::LF,
                &pkcs8,
            )
            .expect("the PKCS#8 key is PEM-encodable");
            let pair = rcgen::KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &rcgen::PKCS_RSA_SHA256)
                .expect("rcgen reads the RSA key");
            (pair, Signer { kind, pkcs8 })
        }
        KeyKind::EcdsaP256 => {
            let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .expect("a P-256 key pair");
            let pkcs8 = pair.serialize_der();
            (pair, Signer { kind, pkcs8 })
        }
        KeyKind::EcdsaP384 => {
            let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384)
                .expect("a P-384 key pair");
            let pkcs8 = pair.serialize_der();
            (pair, Signer { kind, pkcs8 })
        }
    }
}

/// The certificate parameters this suite writes: one distinguished name, an explicit
/// validity window and an explicit serial number, so what the response must report is
/// decided here rather than read back out of the implementation.
fn params(common_name: &str, serial: &[u8], validity: (i32, i32), is_ca: bool) -> rcgen::CertificateParams {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, common_name);
    subject.push(rcgen::DnType::OrganizationName, "Example Workloads");
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(validity.0, 1, 1);
    params.not_after = rcgen::date_time_ymd(validity.1, 1, 1);
    params.serial_number = Some(rcgen::SerialNumber::from(serial.to_vec()));
    params.is_ca = if is_ca {
        rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained)
    } else {
        rcgen::IsCa::ExplicitNoCa
    };
    params
}

/// One certificate authority: the PEM a trust anchor's bundle holds, and the key it issues
/// with.
struct Authority {
    bundle: String,
    certificate: rcgen::Certificate,
    key: rcgen::KeyPair,
}

/// An authority of `kind`, named `common_name`.
fn new_authority(kind: KeyKind, common_name: &str) -> Authority {
    let (key, _) = key_pair(kind);
    let certificate = params(common_name, &[0x51, 0x42], (2020, 2100), true)
        .self_signed(&key)
        .expect("a self-signed authority certificate");
    Authority {
        bundle: certificate.pem(),
        certificate,
        key,
    }
}

/// One end-entity identity: what the request presents, and the key it signs with.
struct Identity {
    /// The DER certificate the `X-Amz-X509` header carries.
    der: Vec<u8>,
    /// The serial number in hex with no leading zero, which is the default session name.
    serial_hex: String,
    /// The common name the certificate's subject carries.
    common_name: String,
    signer: Signer,
}

/// An end-entity certificate of `kind` issued by `authority`, carrying `serial`.
fn new_identity(authority: &Authority, kind: KeyKind, common_name: &str, serial: &[u8]) -> Identity {
    issued_identity(authority, kind, common_name, serial, (2020, 2100))
}

/// See [`new_identity`]; `validity` is the certificate's own window, so a row can present
/// an expired certificate.
fn issued_identity(
    authority: &Authority,
    kind: KeyKind,
    common_name: &str,
    serial: &[u8],
    validity: (i32, i32),
) -> Identity {
    let (key, signer) = key_pair(kind);
    let certificate = params(common_name, serial, validity, false)
        .signed_by(&key, &authority.certificate, &authority.key)
        .expect("a certificate issued by the authority");
    Identity {
        der: certificate.der().to_vec(),
        serial_hex: expected_serial_hex(serial),
        common_name: common_name.to_string(),
        signer,
    }
}

/// The hex the default session name carries for a certificate whose serial is `serial`:
/// the number itself, lowercase, with no leading zero — computed here from the bytes this
/// suite chose rather than from the implementation.
fn expected_serial_hex(serial: &[u8]) -> String {
    let encoded: String = serial.iter().map(|byte| format!("{byte:02x}")).collect();
    let trimmed = encoded.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The lowercase hex SHA-256 of `bytes`, the digest SigV4 is defined over.
fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// One native session request, built and signed the way a client builds one.
struct SessionCall<'c> {
    identity: &'c Identity,
    host: &'c str,
    query: &'c str,
    body: String,
    /// What the signature covers — the body as sent, unless a row tampers with it.
    signed_body: String,
    /// The key that signs — the certificate's own, unless a row signs with another.
    signer: &'c Signer,
    /// Whether the certificate header is sent at all.
    presents_certificate: bool,
}

impl<'c> SessionCall<'c> {
    /// A well-formed call for `identity` with `body`.
    fn new(identity: &'c Identity, host: &'c str, body: &Value) -> Self {
        let body = body.to_string();
        Self {
            identity,
            host,
            query: "",
            signed_body: body.clone(),
            body,
            signer: &identity.signer,
            presents_certificate: true,
        }
    }

    /// The request as an HTTP message: the headers the scheme requires and the
    /// `Authorization` header whose signature covers the canonical request.
    fn request(&self) -> http::Request<bytes::Bytes> {
        let certificate = aws_smithy_types::base64::encode(&self.identity.der);
        let canonical_headers = format!(
            "content-type:application/json\nhost:{host}\nx-amz-date:{SIGNED_AT}\n\
             x-amz-x509:{certificate}\n",
            host = self.host,
        );
        let canonical_request = format!(
            "POST\n/sessions\n{query}\n{canonical_headers}\n{SIGNED_HEADERS}\n{payload}",
            query = self.query,
            payload = hex_sha256(self.signed_body.as_bytes()),
        );
        let scope = format!("{SIGNED_DATE}/{REGION}/rolesanywhere/aws4_request");
        let string_to_sign = format!(
            "{scheme}\n{SIGNED_AT}\n{scope}\n{canonical}",
            scheme = self.signer.scheme(),
            canonical = hex_sha256(canonical_request.as_bytes()),
        );
        let signature = hex::encode(self.signer.sign(string_to_sign.as_bytes()));
        let uri = if self.query.is_empty() {
            format!("https://{}/sessions", self.host)
        } else {
            format!("https://{}/sessions?{}", self.host, self.query)
        };
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", self.host)
            .header("content-type", "application/json")
            .header("x-amz-date", SIGNED_AT)
            .header(
                "authorization",
                format!(
                    "{scheme} Credential={serial}/{scope}, SignedHeaders={SIGNED_HEADERS}, \
                     Signature={signature}",
                    scheme = self.signer.scheme(),
                    serial = self.identity.serial_hex,
                ),
            );
        if self.presents_certificate {
            builder = builder.header("x-amz-x509", certificate);
        }
        builder
            .body(bytes::Bytes::from(self.body.clone()))
            .expect("the request builds")
    }
}

/// One HTTP answer: the status, the parsed JSON body and the wire error code.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Answer {
    /// The one credential set a successful answer carries.
    fn credential_set(&self, case: &str) -> Value {
        let set = self
            .body
            .get("credentialSet")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a credentialSet member, got {self:?}")
            });
        assert_eq!(
            set.len(),
            1,
            "case {case:?}: expected exactly one credential set, got {set:?}"
        );
        set[0].clone()
    }

    /// One string member, by JSON pointer.
    fn string_at(&self, case: &str, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {self:?}"))
            .to_string()
    }
}

/// The emulator this suite drives: IAM Roles Anywhere beside the token service, over one
/// engine, so a vended credential is resolved by the service that owns that question.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// One control-plane call, signed the way a client signs this service's API.
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", AWS_HOST)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/{SIGNED_DATE}/{REGION}/rolesanywhere/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");
        self.answer(request).await
    }

    /// One native session call.
    async fn create_session(&self, call: &SessionCall<'_>) -> Answer {
        self.answer(call.request()).await
    }

    async fn answer(&self, request: http::Request<bytes::Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(':')
            .next()
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
            body,
            error_code,
        }
    }

    /// Record `role_name` in IAM with `trust_policy`, as `iam:CreateRole` records it, and
    /// answer its ARN.
    fn given_role(&self, role_name: &str, role_id: &str, trust_policy: Option<String>) -> String {
        let arn = format!(
            "arn:aws:iam::{}:role/{role_name}",
            engine::emulator_account_id()
        );
        let role = iam::ops::types::Role {
            path: "/".to_string(),
            role_name: role_name.to_string(),
            role_id: role_id.to_string(),
            arn: arn.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: trust_policy,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        };
        self.engine
            .state::<iam::state::ComAmazonawsIamAwsidentitymanagementv20100508State>()
            .lock()
            .roles
            .insert(iam::state::name_key(role_name), role);
        arn
    }

    /// `sts:GetCallerIdentity` under `access_key_id`, as an awsQuery POST a client makes.
    async fn caller_identity(&self, access_key_id: &str) -> (u16, String) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sts.amazonaws.com/")
            .header("host", "sts.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/{SIGNED_DATE}/{REGION}/sts/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                "Action=GetCallerIdentity&Version=2011-06-15",
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    /// How long the session minted under `access_key_id` lasts, as the token service's own
    /// registry records it.
    fn session_duration(&self, access_key_id: &str) -> i32 {
        sts::state::service_state(&*self.engine)
            .expect("the token service store")
            .lock()
            .sessions
            .get(access_key_id)
            .map(|session| session.duration_seconds)
            .unwrap_or_else(|| panic!("the registry holds no session for {access_key_id}"))
    }

    /// How many subjects this service's store holds — what a request that writes nothing
    /// leaves unchanged.
    fn recorded_subjects(&self) -> usize {
        service_com_amazonaws_rolesanywhere_rolesanywhere::state::service_state(&*self.engine)
            .expect("the IAM Roles Anywhere store")
            .lock()
            .subjects
            .len()
    }
}

/// The trust policy that lets `principal` assume a role, as `iam:CreateRole` stores one.
fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": ["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"],
        }],
    })
    .to_string()
}

/// The text of the one `<name>…</name>` element of an awsQuery response.
fn element(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open).map(|at| at + open.len());
    let end = body.find(&close);
    match (start, end) {
        (Some(start), Some(end)) if start <= end => body[start..end].to_string(),
        _ => String::new(),
    }
}

/// The account the emulator serves.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// A trust anchor holding `bundle`, enabled unless `enabled` says otherwise, and the ARN
/// and id it was created under.
async fn given_trust_anchor(
    emulator: &Emulator,
    case: &str,
    bundle: &str,
    enabled: bool,
) -> (String, String) {
    let answer = emulator
        .send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": "workload-authority",
                "enabled": enabled,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": { "x509CertificateData": bundle },
                },
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreateTrustAnchor to answer 201, got {answer:?}"
    );
    (
        answer.string_at(case, "/trustAnchor/trustAnchorArn"),
        answer.string_at(case, "/trustAnchor/trustAnchorId"),
    )
}

/// A profile naming `role_arns`, and the ARN and id it was created under.
async fn given_profile(
    emulator: &Emulator,
    case: &str,
    role_arns: &[&str],
    extra: Value,
) -> (String, String) {
    let mut body = json!({
        "name": "workload-profile",
        "enabled": true,
        "roleArns": role_arns,
    });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    let answer = emulator.send("POST", "/profiles", Some(body)).await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreateProfile to answer 201, got {answer:?}"
    );
    (
        answer.string_at(case, "/profile/profileArn"),
        answer.string_at(case, "/profile/profileId"),
    )
}

/// The body of a well-formed session request.
fn session_body(profile_arn: &str, role_arn: &str, trust_anchor_arn: &str) -> Value {
    json!({
        "profileArn": profile_arn,
        "roleArn": role_arn,
        "trustAnchorArn": trust_anchor_arn,
    })
}

/// RULE: a certificate an enabled trust anchor's authority issued, proving possession of
/// its key, is exchanged for exactly one credential set — a real session of the role the
/// request names, under the certificate's own serial number — and the request is recorded
/// against that certificate's subject.
#[tokio::test]
async fn native_session_vends_one_usable_role_session() {
    // The rows vary the key family of the certificate AND of the authority that issued it
    // (so the request's signature scheme and the chain's signature algorithm both change),
    // the serial number (including one whose DER encoding carries a leading zero byte), and
    // the host the request is addressed to.
    let rows: &[(&str, KeyKind, KeyKind, &[u8], &str)] = &[
        (
            "an RSA-2048 certificate from an RSA authority, at this emulator's address",
            KeyKind::Rsa2048,
            KeyKind::Rsa2048,
            &[0x4d, 0x3c, 0x2b, 0x1a],
            EMULATOR_HOST,
        ),
        (
            "a P-256 certificate from a P-256 authority, at the AWS host",
            KeyKind::EcdsaP256,
            KeyKind::EcdsaP256,
            &[0x01],
            AWS_HOST,
        ),
        (
            "a P-256 certificate from a P-384 authority, serial with a high first byte",
            KeyKind::EcdsaP256,
            KeyKind::EcdsaP384,
            &[0xab, 0xcd, 0xef, 0x12, 0x34],
            EMULATOR_HOST,
        ),
    ];

    for (case, leaf_kind, authority_kind, serial, host) in rows {
        let emulator = Emulator::new();
        let role_arn = emulator.given_role(ROLE, ROLE_ID, Some(trust_policy(SERVICE_PRINCIPAL)));
        let authority = new_authority(*authority_kind, "workload-authority");
        let identity = new_identity(&authority, *leaf_kind, "workload-leaf", serial);
        let (trust_anchor_arn, _) = given_trust_anchor(&emulator, case, &authority.bundle, true).await;
        let (profile_arn, _) = given_profile(&emulator, case, &[&role_arn], json!({})).await;

        let body = session_body(&profile_arn, &role_arn, &trust_anchor_arn);
        let answer = emulator
            .create_session(&SessionCall::new(&identity, host, &body))
            .await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected 201 from POST /sessions, got {answer:?}"
        );

        let vended = answer.credential_set(case);
        assert_eq!(
            vended.get("roleArn").and_then(Value::as_str),
            Some(role_arn.as_str()),
            "case {case:?}: expected the credential set to echo the role, got {vended:?}"
        );
        let assumed = vended
            .pointer("/assumedRoleUser/arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            assumed,
            format!(
                "arn:aws:sts::{}:assumed-role/{ROLE}/{}",
                account(),
                identity.serial_hex
            ),
            "case {case:?}: expected the session to be named by the certificate's serial, \
             got {assumed:?}"
        );
        assert_eq!(
            vended
                .pointer("/assumedRoleUser/assumedRoleId")
                .and_then(Value::as_str),
            Some(format!("{ROLE_ID}:{}", identity.serial_hex).as_str()),
            "case {case:?}: expected the assumed role id to name the role and the session, \
             got {vended:?}"
        );

        let access_key_id = vended
            .pointer("/credentials/accessKeyId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for member in ["accessKeyId", "secretAccessKey", "sessionToken", "expiration"] {
            let value = vended
                .pointer(&format!("/credentials/{member}"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !value.is_empty(),
                "case {case:?}: expected a {member} in the vended credentials, got {vended:?}"
            );
        }
        let expiration = vended
            .pointer("/credentials/expiration")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            aws_smithy_types::DateTime::from_str(
                expiration,
                aws_smithy_types::date_time::Format::DateTime
            )
            .is_ok(),
            "case {case:?}: expected an ISO-8601 expiration, got {expiration:?}"
        );

        // The credential is a REAL session: the service that owns the question answers it.
        let (status, identity_document) = emulator.caller_identity(&access_key_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetCallerIdentity to answer 200 under the vended \
             credential, got {status} with {identity_document}"
        );
        assert_eq!(
            element(&identity_document, "Arn"),
            assumed,
            "case {case:?}: expected GetCallerIdentity to report the assumed-role identity, \
             got {identity_document}"
        );

        // And the request is recorded against the subject the certificate identifies,
        // readable through the operation that owns subjects.
        let subject_arn = answer.string_at(case, "/subjectArn");
        let subject_id = subject_arn
            .rsplit('/')
            .next()
            .expect("the subject ARN names an id")
            .to_string();
        let subject = emulator
            .send("GET", &format!("/subject/{subject_id}"), None)
            .await;
        assert_eq!(
            subject.status, 200,
            "case {case:?}: expected GetSubject to read the recorded subject back, got \
             {subject:?}"
        );
        let x509_subject = subject.string_at(case, "/subject/x509Subject");
        assert!(
            x509_subject.contains(&format!("CN={}", identity.common_name)),
            "case {case:?}: expected the subject to carry the certificate's own name, got \
             {x509_subject:?}"
        );
        assert_eq!(
            subject
                .body
                .pointer("/subject/credentials/0/serialNumber")
                .and_then(Value::as_str),
            Some(identity.serial_hex.as_str()),
            "case {case:?}: expected the recorded credential to name the certificate's \
             serial, got {subject:?}"
        );
        assert_eq!(
            subject
                .body
                .pointer("/subject/credentials/0/failed")
                .and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected the recorded attempt to be a success, got {subject:?}"
        );
        assert_eq!(
            subject
                .body
                .pointer("/subject/enabled")
                .and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected the recorded subject to be enabled, got {subject:?}"
        );

        // A second call by the same identity is the same subject, seen again.
        let again = emulator
            .create_session(&SessionCall::new(&identity, host, &body))
            .await;
        assert_eq!(
            again.status, 201,
            "case {case:?}: expected a second call to be served, got {again:?}"
        );
        assert_eq!(
            again.string_at(case, "/subjectArn"),
            subject_arn,
            "case {case:?}: expected one subject per certificate identity, got {again:?}"
        );
    }
}

/// RULE: a profile's `enabled` flag gates the data plane on every request — a disabled
/// profile vends nothing and is refused 403 `AccessDeniedException` with no credential set,
/// and enabling it again serves the identical request. The same holds for a trust anchor.
#[tokio::test]
async fn disabling_a_profile_or_trust_anchor_stops_vending_and_enabling_restores_it() {
    let rows: &[(&str, bool)] = &[
        ("the profile is the switch", true),
        ("the trust anchor is the switch", false),
    ];

    for (case, switch_is_the_profile) in rows {
        let emulator = Emulator::new();
        let role_arn = emulator.given_role(ROLE, ROLE_ID, Some(trust_policy(SERVICE_PRINCIPAL)));
        let authority = new_authority(KeyKind::Rsa2048, "workload-authority");
        let identity = new_identity(
            &authority,
            KeyKind::Rsa2048,
            "workload-leaf",
            &[0x7f, 0x01],
        );
        let (trust_anchor_arn, trust_anchor_id) =
            given_trust_anchor(&emulator, case, &authority.bundle, true).await;
        let (profile_arn, profile_id) =
            given_profile(&emulator, case, &[&role_arn], json!({})).await;
        let body = session_body(&profile_arn, &role_arn, &trust_anchor_arn);
        let call = SessionCall::new(&identity, EMULATOR_HOST, &body);

        let control = emulator.create_session(&call).await;
        assert_eq!(
            control.status, 201,
            "case {case:?}: expected the positive control to be served, got {control:?}"
        );

        let (disable, enable, read) = if *switch_is_the_profile {
            (
                format!("/profile/{profile_id}/disable"),
                format!("/profile/{profile_id}/enable"),
                format!("/profile/{profile_id}"),
            )
        } else {
            (
                format!("/trustanchor/{trust_anchor_id}/disable"),
                format!("/trustanchor/{trust_anchor_id}/enable"),
                format!("/trustanchor/{trust_anchor_id}"),
            )
        };
        let member = if *switch_is_the_profile {
            "/profile/enabled"
        } else {
            "/trustAnchor/enabled"
        };

        let disabled = emulator.send("POST", &disable, None).await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: expected the disable call to answer 200, got {disabled:?}"
        );
        let after = emulator.send("GET", &read, None).await;
        assert_eq!(
            after.body.pointer(member).and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected the read to report it disabled, got {after:?}"
        );

        let refused = emulator.create_session(&call).await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected the identical signed request to be refused 403, got \
             {refused:?}"
        );
        assert_eq!(
            refused.error_code, "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {refused:?}"
        );
        assert!(
            refused.body.get("credentialSet").is_none(),
            "case {case:?}: expected no credential set in the refusal, got {refused:?}"
        );

        // An identity the account still ADMITS — the trust anchor issued it and is enabled
        // — leaves a record of the refused attempt, as the subject's `failed` member is
        // documented to report. An identity it no longer admits at all (the anchor itself
        // is the switch) is refused before anything is written, so the subject stands as
        // the last attempt the account admitted left it.
        let subject_id = control
            .string_at(case, "/subjectArn")
            .rsplit('/')
            .next()
            .expect("the subject ARN names an id")
            .to_string();
        let subject = emulator
            .send("GET", &format!("/subject/{subject_id}"), None)
            .await;
        assert_eq!(
            subject
                .body
                .pointer("/subject/credentials/0/failed")
                .and_then(Value::as_bool),
            Some(*switch_is_the_profile),
            "case {case:?}: expected the recorded attempt's failure to follow whether the \
             account still admits the identity, got {subject:?}"
        );

        let enabled = emulator.send("POST", &enable, None).await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: expected the enable call to answer 200, got {enabled:?}"
        );
        assert_eq!(
            enabled.body.pointer(member).and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected the enable call to report it enabled, got {enabled:?}"
        );

        let restored = emulator.create_session(&call).await;
        assert_eq!(
            restored.status, 201,
            "case {case:?}: expected the identical signed request to be served again, got \
             {restored:?}"
        );
        assert_eq!(
            restored.credential_set(case).get("roleArn"),
            Some(&json!(role_arn)),
            "case {case:?}: expected the restored session to name the role, got {restored:?}"
        );
    }
}

/// RULE: an identity or a grant the account does not admit vends nothing — each refusal
/// carries the error class its gate belongs to and no credential set.
#[tokio::test]
async fn an_identity_the_account_does_not_admit_vends_nothing() {
    /// What one row changes about an otherwise well-formed request.
    enum Row {
        /// A certificate issued by an authority the trust anchor does not hold.
        ForeignAuthority,
        /// A certificate outside its own validity window.
        Expired,
        /// A body changed after it was signed.
        TamperedBody,
        /// A signature made by a key other than the certificate's.
        ForeignKey,
        /// No certificate presented at all.
        NoCertificate,
        /// A role the profile does not list.
        UnlistedRole,
        /// A role whose trust policy admits another service.
        UntrustingRole,
        /// A role this account does not hold.
        AbsentRole,
        /// A profile ARN this account does not hold.
        UnknownProfile,
        /// A trust anchor ARN this account does not hold.
        UnknownTrustAnchor,
        /// A request missing a required member.
        MissingMember,
    }

    let rows: &[(&str, Row, u16, &str)] = &[
        (
            "a certificate from another authority",
            Row::ForeignAuthority,
            403,
            "AccessDeniedException",
        ),
        (
            "an expired certificate",
            Row::Expired,
            403,
            "AccessDeniedException",
        ),
        (
            "a body changed after signing",
            Row::TamperedBody,
            403,
            "AccessDeniedException",
        ),
        (
            "a signature made with another key",
            Row::ForeignKey,
            403,
            "AccessDeniedException",
        ),
        (
            "no certificate presented",
            Row::NoCertificate,
            403,
            "AccessDeniedException",
        ),
        (
            "a role the profile does not list",
            Row::UnlistedRole,
            403,
            "AccessDeniedException",
        ),
        (
            "a role trusting another service",
            Row::UntrustingRole,
            403,
            "AccessDeniedException",
        ),
        (
            "a role this account does not hold",
            Row::AbsentRole,
            403,
            "AccessDeniedException",
        ),
        (
            "a profile this account does not hold",
            Row::UnknownProfile,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a trust anchor this account does not hold",
            Row::UnknownTrustAnchor,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a request with no profileArn",
            Row::MissingMember,
            400,
            "ValidationException",
        ),
    ];

    for (case, row, status, code) in rows {
        let emulator = Emulator::new();
        let role_arn = emulator.given_role(ROLE, ROLE_ID, Some(trust_policy(SERVICE_PRINCIPAL)));
        let other_role_arn = emulator.given_role(
            OTHER_ROLE,
            OTHER_ROLE_ID,
            Some(trust_policy("ec2.amazonaws.com")),
        );
        let authority = new_authority(KeyKind::EcdsaP256, "workload-authority");
        let identity = new_identity(&authority, KeyKind::EcdsaP256, "workload-leaf", &[0x22, 0x33]);
        let (trust_anchor_arn, _) =
            given_trust_anchor(&emulator, case, &authority.bundle, true).await;
        let (profile_arn, _) = given_profile(&emulator, case, &[&role_arn], json!({})).await;
        let absent_role = format!("arn:aws:iam::{}:role/not-created", account());

        // Material only some rows need.
        let foreign = new_authority(KeyKind::EcdsaP256, "other-authority");
        let foreign_identity = new_identity(&foreign, KeyKind::EcdsaP256, "foreign-leaf", &[0x44]);
        let expired_identity = issued_identity(
            &authority,
            KeyKind::EcdsaP256,
            "expired-leaf",
            &[0x55],
            (2020, 2021),
        );

        let body = match row {
            Row::UnlistedRole => session_body(&profile_arn, &other_role_arn, &trust_anchor_arn),
            Row::UntrustingRole => {
                // The profile lists it, so the refusal is the trust policy's and nothing
                // else's.
                let (listed, _) =
                    given_profile(&emulator, case, &[&other_role_arn], json!({})).await;
                session_body(&listed, &other_role_arn, &trust_anchor_arn)
            }
            Row::AbsentRole => {
                let (listed, _) = given_profile(&emulator, case, &[&absent_role], json!({})).await;
                session_body(&listed, &absent_role, &trust_anchor_arn)
            }
            Row::UnknownProfile => session_body(
                &format!(
                    "arn:aws:rolesanywhere:{REGION}:{}:profile/00000000-0000-0000-0000-000000000000",
                    account()
                ),
                &role_arn,
                &trust_anchor_arn,
            ),
            Row::UnknownTrustAnchor => session_body(
                &profile_arn,
                &role_arn,
                &format!(
                    "arn:aws:rolesanywhere:{REGION}:{}:trust-anchor/00000000-0000-0000-0000-000000000000",
                    account()
                ),
            ),
            Row::MissingMember => json!({
                "roleArn": role_arn,
                "trustAnchorArn": trust_anchor_arn,
            }),
            _ => session_body(&profile_arn, &role_arn, &trust_anchor_arn),
        };

        let presented = match row {
            Row::ForeignAuthority => &foreign_identity,
            Row::Expired => &expired_identity,
            _ => &identity,
        };
        let mut call = SessionCall::new(presented, EMULATOR_HOST, &body);
        match row {
            Row::TamperedBody => {
                call.signed_body = session_body(&profile_arn, &role_arn, &trust_anchor_arn)
                    .to_string()
                    .replace("profileArn", "profileARN");
            }
            Row::ForeignKey => call.signer = &foreign_identity.signer,
            Row::NoCertificate => call.presents_certificate = false,
            _ => {}
        }

        let answer = emulator.create_session(&call).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, *code,
            "case {case:?}: expected {code}, got {answer:?}"
        );
        assert!(
            answer.body.get("credentialSet").is_none(),
            "case {case:?}: expected no credential set in the refusal, got {answer:?}"
        );
        assert!(
            answer
                .body
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected the refusal to carry a message, got {answer:?}"
        );
        // A request whose certificate THIS ACCOUNT does not admit — no enabled trust
        // anchor of it issued the certificate, the request proves nothing about the
        // certificate it presents, or it names no resource of this account — writes
        // nothing at all: it cannot grow the store by minting its own certificate
        // identities. Only a request the account admits and then refuses on an
        // AUTHORIZATION gate leaves the recorded attempt the subject is for.
        let admitted_identity = matches!(
            row,
            Row::UnlistedRole | Row::UntrustingRole | Row::AbsentRole
        );
        assert_eq!(
            emulator.recorded_subjects(),
            usize::from(admitted_identity),
            "case {case:?}: expected the store to hold a subject only for an identity the \
             account admits, got {answer:?}"
        );
    }
}

/// RULE: the session a request vends lasts what it asked for, bounded by the published
/// range and by the profile it was vended through, and is named by the certificate's serial
/// unless the profile accepts a name the request asks for.
#[tokio::test]
async fn the_session_duration_and_name_follow_the_request_and_the_profile() {
    /// One duration row: what the request asks for, what the profile was created with, and
    /// the status and the session length that follow.
    struct Duration {
        case: &'static str,
        asked: Option<i64>,
        profile: Option<i64>,
        status: u16,
        lasts: Option<i32>,
    }

    let durations = [
        Duration {
            case: "the shortest published duration",
            asked: Some(MIN_DURATION),
            profile: None,
            status: 201,
            lasts: Some(900),
        },
        Duration {
            case: "the longest published duration",
            asked: Some(MAX_DURATION),
            profile: None,
            status: 201,
            lasts: Some(3600),
        },
        Duration {
            case: "one second under the shortest",
            asked: Some(MIN_DURATION - 1),
            profile: None,
            status: 400,
            lasts: None,
        },
        Duration {
            case: "one second over the longest",
            asked: Some(MAX_DURATION + 1),
            profile: None,
            status: 400,
            lasts: None,
        },
        Duration {
            case: "no duration asked for, a profile shorter than the maximum",
            asked: None,
            profile: Some(1800),
            status: 201,
            lasts: Some(1800),
        },
        Duration {
            case: "a profile longer than this plane vends",
            asked: None,
            profile: Some(43_200),
            status: 201,
            lasts: Some(3600),
        },
        Duration {
            case: "a request longer than its profile admits",
            asked: Some(3600),
            profile: Some(1800),
            status: 201,
            lasts: Some(1800),
        },
    ];

    for row in &durations {
        let case = row.case;
        let emulator = Emulator::new();
        let role_arn = emulator.given_role(ROLE, ROLE_ID, Some(trust_policy(SERVICE_PRINCIPAL)));
        let authority = new_authority(KeyKind::EcdsaP256, "workload-authority");
        let identity = new_identity(&authority, KeyKind::EcdsaP256, "workload-leaf", &[0x66]);
        let (trust_anchor_arn, _) =
            given_trust_anchor(&emulator, case, &authority.bundle, true).await;
        let extra = match row.profile {
            Some(seconds) => json!({ "durationSeconds": seconds }),
            None => json!({}),
        };
        let (profile_arn, _) = given_profile(&emulator, case, &[&role_arn], extra).await;

        let mut body = session_body(&profile_arn, &role_arn, &trust_anchor_arn);
        if let Some(asked) = row.asked {
            body["durationSeconds"] = json!(asked);
        }
        let answer = emulator
            .create_session(&SessionCall::new(&identity, EMULATOR_HOST, &body))
            .await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected {}, got {answer:?}",
            row.status
        );
        match row.lasts {
            Some(lasts) => {
                let access_key_id = answer
                    .credential_set(case)
                    .pointer("/credentials/accessKeyId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    emulator.session_duration(&access_key_id),
                    lasts,
                    "case {case:?}: expected a session of {lasts} seconds"
                );
            }
            None => assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            ),
        }
    }

    /// One session-name row: whether the profile accepts a name, what the request asks
    /// for, and the name the session is vended under.
    struct Named {
        case: &'static str,
        accepts: bool,
        asked_in_query: Option<&'static str>,
        asked_in_body: Option<&'static str>,
        status: u16,
        /// `None` means the certificate's own serial names the session.
        expected: Option<&'static str>,
    }

    let names = [
        Named {
            case: "a profile that accepts a name, asked for in the query",
            accepts: true,
            asked_in_query: Some("audit-run-1"),
            asked_in_body: None,
            status: 201,
            expected: Some("audit-run-1"),
        },
        Named {
            case: "a profile that accepts a name, asked for in the body",
            accepts: true,
            asked_in_query: None,
            asked_in_body: Some("batch.worker@2"),
            status: 201,
            expected: Some("batch.worker@2"),
        },
        Named {
            case: "a profile that does not accept a name",
            accepts: false,
            asked_in_query: Some("audit-run-1"),
            asked_in_body: None,
            status: 201,
            expected: None,
        },
        Named {
            case: "no name asked for",
            accepts: true,
            asked_in_query: None,
            asked_in_body: None,
            status: 201,
            expected: None,
        },
        Named {
            case: "a name outside the published pattern",
            accepts: true,
            asked_in_query: None,
            asked_in_body: Some("not a session name"),
            status: 400,
            expected: None,
        },
        Named {
            case: "a name shorter than the published minimum",
            accepts: true,
            asked_in_query: Some("a"),
            asked_in_body: None,
            status: 400,
            expected: None,
        },
    ];

    for row in &names {
        let case = row.case;
        let emulator = Emulator::new();
        let role_arn = emulator.given_role(ROLE, ROLE_ID, Some(trust_policy(SERVICE_PRINCIPAL)));
        let authority = new_authority(KeyKind::EcdsaP256, "workload-authority");
        let identity = new_identity(&authority, KeyKind::EcdsaP256, "workload-leaf", &[0x77, 0x88]);
        let (trust_anchor_arn, _) =
            given_trust_anchor(&emulator, case, &authority.bundle, true).await;
        let (profile_arn, _) = given_profile(
            &emulator,
            case,
            &[&role_arn],
            json!({ "acceptRoleSessionName": row.accepts }),
        )
        .await;

        let mut body = session_body(&profile_arn, &role_arn, &trust_anchor_arn);
        if let Some(name) = row.asked_in_body {
            body["sessionName"] = json!(name);
        }
        let query = row
            .asked_in_query
            .map(|name| format!("roleSessionName={name}"))
            .unwrap_or_default();
        let mut call = SessionCall::new(&identity, EMULATOR_HOST, &body);
        call.query = &query;

        let answer = emulator.create_session(&call).await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected {}, got {answer:?}",
            row.status
        );
        if row.status != 201 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            );
            continue;
        }
        let expected = row.expected.unwrap_or(&identity.serial_hex);
        let assumed = answer
            .credential_set(case)
            .pointer("/assumedRoleUser/arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            assumed,
            format!("arn:aws:sts::{}:assumed-role/{ROLE}/{expected}", account()),
            "case {case:?}: expected the session to be named {expected:?}, got {assumed:?}"
        );
    }
}
