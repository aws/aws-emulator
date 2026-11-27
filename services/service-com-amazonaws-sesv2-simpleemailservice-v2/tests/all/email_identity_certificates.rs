// SPDX-License-Identifier: CC0-1.0

//! The S/MIME certificate-association surface of an email identity, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reads the engine's own state where a response cannot show what was
//! persisted.
//!
//! The rules these cases pin:
//!
//! * an association belongs to ONE address — the identity's own when the identity is an
//!   address, the `FromAddress` within the domain when it is a domain — and a listing of
//!   the identity reports every association it holds, each with the address it applies
//!   to, the certificate it names and the status it currently carries;
//! * where the preparation of an association ENDS is what the service that holds the
//!   certificate says about it: `ACTIVE` for a certificate it holds that a certificate
//!   authority issued, that has been issued, is still valid and may be used to protect
//!   email, and `FAILED` for one it does not hold at all, holds in a state that cannot
//!   sign, or holds with nothing but its own key vouching for it;
//! * a created association starts out being prepared for signing and reaches that end
//!   when the clock says so — unless its certificate cannot be prepared at all, in which
//!   case it is created `FAILED`; a removed one stops signing at once and is cleaned up
//!   afterwards;
//! * a second association for an address that already has one is refused, and so is a
//!   `FromAddress` that does not name an address the identity covers;
//! * an identity that still signs with a certificate cannot be deleted;
//! * a paginated listing answers with at most `PageSize` associations and a `NextToken`
//!   only while associations remain behind that page, so a walk that follows the token
//!   terminates and visits every association exactly once.
//!
//! The tables below span what those rules generalise over: the kind of identity (address,
//! domain), the form of `FromAddress` (absent, the identity itself, a differently cased
//! spelling of it, an address in the domain, an address in a subdomain), what Certificate
//! Manager holds for the ARN the request names (nothing, a certificate it issued for TLS,
//! an imported certificate that may protect email, one that may not, one that asserts no
//! key purpose at all, one whose validity has run out, one that is not issued yet, one no
//! authority other than its own subject signed), the status an association is in, the
//! number of stored associations and the page size.
//!
//! Certificate Manager is mounted BESIDE this service on one engine, so every certificate
//! a case names is one its holder really holds: the material is built by `rcgen` —
//! something other than the code under test — and brought in through that service's own
//! `ImportCertificate`/`RequestCertificate`, and the ARN asserted on is the one it
//! answered rather than a value written into the test.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::IDENTITY_CERTIFICATE_STATUS_VALUES;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_arn, identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, IdentityCertificateRecord,
    CERTIFICATE_SETTLE, MAX_CERTIFICATES_PER_IDENTITY,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request in this suite is signed for, which is the Region the
/// certificate ARNs the holder mints carry too.
const HOME: &str = "us-east-1";

/// A well-formed ACM certificate ARN, as `CertificateArn`'s modelled pattern requires.
/// The id part only has to be an identifier the pattern accepts; which certificate it
/// names is ACM's business, not this service's.
fn certificate_arn(id: &str) -> String {
    format!("arn:aws:acm:us-east-1:123456789012:certificate/{id}")
}

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `Certificates` a listing reported, in the order it reported them.
    fn certificates(&self) -> Vec<Value> {
        self.body
            .get("Certificates")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `NextToken` a page handed back, if any.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// A string member of one listed certificate, or `""` when absent.
fn member(certificate: &Value, name: &str) -> String {
    certificate
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A real certificate and the private key that matches it.
struct Minted {
    /// The PEM-encoded certificate.
    certificate: String,
    /// The PEM-encoded private key.
    private_key: String,
}

/// The subject common name every leaf certificate in this suite carries.
const LEAF_COMMON_NAME: &str = "signing.example.com";

/// The subject common name the certificate authority that signs those leaves carries,
/// which is the name their issuer field then names.
const AUTHORITY_COMMON_NAME: &str = "signing-authority.example.com";

/// The parameters of a certificate for `common_name` asserting `purposes` as its Extended
/// Key Usage extension and valid until the first of January of `valid_until`.
///
/// An empty `purposes` leaves the extension OFF the certificate, which is the "asserts no
/// key purpose" case.
fn certificate_params(
    purposes: &[rcgen::ExtendedKeyUsagePurpose],
    valid_until: i32,
    common_name: &str,
) -> rcgen::CertificateParams {
    let mut params = rcgen::CertificateParams::new(vec![common_name.to_string()])
        .expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, common_name);
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(valid_until, 1, 1);
    params.extended_key_usages = purposes.to_vec();
    params
}

/// A certificate authority whose key signs the leaves a case brings into the holder, so a
/// leaf's issuer field names an authority other than the leaf itself.
///
/// Built by `rcgen` — something other than the code under test — like the leaves are, so
/// WHO issued an imported certificate is a genuine X.509 fact of its body rather than a
/// value written into the test.
struct Authority {
    certificate: rcgen::Certificate,
    key: rcgen::KeyPair,
}

impl Authority {
    fn new() -> Self {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
        let mut params = certificate_params(&[], 2100, AUTHORITY_COMMON_NAME);
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let certificate = params.self_signed(&key).expect("a certificate authority");
        Self { certificate, key }
    }

    /// A leaf asserting `purposes` and valid until the first of January of `valid_until`,
    /// signed by this authority.
    fn sign(&self, purposes: &[rcgen::ExtendedKeyUsagePurpose], valid_until: i32) -> Minted {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
        let certificate = certificate_params(purposes, valid_until, LEAF_COMMON_NAME)
            .signed_by(&key, &self.certificate, &self.key)
            .expect("a signed certificate")
            .pem();
        Minted {
            certificate,
            private_key: key.serialize_pem(),
        }
    }
}

/// A real SELF-SIGNED certificate asserting `purposes`: its issuer field names its own
/// subject, so no authority vouches for it.
fn mint_self_signed(purposes: &[rcgen::ExtendedKeyUsagePurpose], valid_until: i32) -> Minted {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
    let certificate = certificate_params(purposes, valid_until, LEAF_COMMON_NAME)
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem();
    Minted {
        certificate,
        private_key: key.serialize_pem(),
    }
}

/// A PEM body as the base64 a blob member carries on the wire.
fn blob(pem: &str) -> Value {
    json!(aws_smithy_types::base64::encode(pem.as_bytes()))
}

/// The emulated service behind its router, plus the shared engine so a test can read
/// what a handler persisted and move the instant a clock-owed transition falls due.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // The HOLDER of the certificates an association names, on the same engine: what
        // it answers about a certificate is what the association's preparation reads.
        router.register_service(
            service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
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
            error_type,
            body,
        }
    }

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn get_identity(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn delete_identity(&self, identity: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn associate(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identity/certificates", Some(body))
            .await
    }

    async fn disassociate(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identity/certificates/delete", Some(body))
            .await
    }

    async fn list_certificates(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identity/certificates/list", Some(body))
            .await
    }

    /// Call one Certificate Manager operation over the same router and return the parsed
    /// body, requiring success.
    ///
    /// That service speaks `awsJson1_1` (`POST /`, `X-Amz-Target:
    /// CertificateManager.<Op>`), signed for [`HOME`] so the ARNs it mints carry the same
    /// Region this suite's own requests do.
    async fn acm(&self, case: &str, operation: &str, body: Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://acm.us-east-1.amazonaws.com/")
            .header("host", "acm.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("CertificateManager.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/acm/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("serialize body"),
            ))
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {operation}, got {status} {parsed:?}"
        );
        parsed
    }

    /// Bring a certificate signed by a certificate authority, built from `purposes` and
    /// valid until `valid_until`, into the holder, and answer the ARN it stored it under.
    async fn imported_certificate(
        &self,
        case: &str,
        purposes: &[rcgen::ExtendedKeyUsagePurpose],
        valid_until: i32,
    ) -> String {
        self.import(case, Authority::new().sign(purposes, valid_until))
            .await
    }

    /// Bring a SELF-SIGNED certificate built from `purposes` into the holder, and answer
    /// the ARN it stored it under: a certificate no authority other than its own subject
    /// issued.
    async fn self_signed_certificate(
        &self,
        case: &str,
        purposes: &[rcgen::ExtendedKeyUsagePurpose],
        valid_until: i32,
    ) -> String {
        self.import(case, mint_self_signed(purposes, valid_until))
            .await
    }

    /// Import `minted` through the holder's own `ImportCertificate`, and answer the ARN it
    /// answered with.
    async fn import(&self, case: &str, minted: Minted) -> String {
        let imported = self
            .acm(
                case,
                "ImportCertificate",
                json!({
                    "Certificate": blob(&minted.certificate),
                    "PrivateKey": blob(&minted.private_key),
                }),
            )
            .await;
        imported["CertificateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {imported:?}"))
            .to_string()
    }

    /// An imported certificate that may be used to sign a message: one a certificate
    /// authority issued asserting the `emailProtection` key purpose, which is what an
    /// S/MIME signing certificate carries.
    async fn signing_certificate(&self, case: &str) -> String {
        self.imported_certificate(
            case,
            &[rcgen::ExtendedKeyUsagePurpose::EmailProtection],
            2100,
        )
        .await
    }

    /// A certificate the HOLDER could not issue: a request whose validation outcome is
    /// brought into the past, so the holder reports the state that outcome left it in.
    async fn unissued_certificate(&self, case: &str) -> String {
        let requested = self
            .acm(
                case,
                "RequestCertificate",
                json!({ "DomainName": "tls.example.com", "ValidationMethod": "DNS" }),
            )
            .await;
        let arn = requested["CertificateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {requested:?}"))
            .to_string();
        self.hold_acm_validation(&arn, SystemTime::now() - Duration::from_secs(1));
        let described = self
            .acm(case, "DescribeCertificate", json!({ "CertificateArn": arn }))
            .await;
        assert_eq!(
            described["Certificate"]["Status"].as_str(),
            Some("FAILED"),
            "case {case:?}: expected the holder to report a certificate it could not issue, got \
             {described:?}"
        );
        arn
    }

    /// A certificate the holder is still validating, so it has not been issued: the
    /// outcome it owes is pushed well past this case's own lifetime rather than waited on.
    async fn pending_certificate(&self, case: &str) -> String {
        let requested = self
            .acm(
                case,
                "RequestCertificate",
                json!({ "DomainName": "pending.example.com", "ValidationMethod": "DNS" }),
            )
            .await;
        let arn = requested["CertificateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {requested:?}"))
            .to_string();
        self.hold_acm_validation(&arn, SystemTime::now() + Duration::from_secs(3600));
        let described = self
            .acm(case, "DescribeCertificate", json!({ "CertificateArn": arn }))
            .await;
        assert_eq!(
            described["Certificate"]["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case:?}: expected the holder to report the certificate still pending, got \
             {described:?}"
        );
        arn
    }

    /// Move the instant at which the holder's validation outcome for `arn` falls due, so
    /// a case drives that outcome instead of waiting on the wall clock.
    fn hold_acm_validation(&self, arn: &str, due_at: SystemTime) {
        let state = self
            .engine
            .state::<service_com_amazonaws_acm_certificatemanager::state::ComAmazonawsAcmCertificatemanagerState>();
        let mut store = state.lock();
        let certificate = store
            .certificates
            .get_mut(arn)
            .unwrap_or_else(|| panic!("the holder must hold {arn:?}"));
        if let Some(pending) = certificate.pending_validation.as_mut() {
            pending.due_at = due_at;
        }
    }

    /// End the validity of the certificate `arn` names, as time passing after an import
    /// does: the holder still holds it, and it can no longer be used.
    fn expire_acm_certificate(&self, arn: &str) {
        let state = self
            .engine
            .state::<service_com_amazonaws_acm_certificatemanager::state::ComAmazonawsAcmCertificatemanagerState>();
        let mut store = state.lock();
        let certificate = store
            .certificates
            .get_mut(arn)
            .unwrap_or_else(|| panic!("the holder must hold {arn:?}"));
        certificate.not_after = Some(SystemTime::now() - Duration::from_secs(1));
    }

    /// Move the instant at which the association for `from_address` has its next
    /// transition fall due.
    ///
    /// The handler stores a due instant rather than sleeping, so a test drives the
    /// transition by moving that instant instead of waiting on the wall clock — which is
    /// what keeps every assertion about a transient status independent of how loaded the
    /// machine is.
    fn set_certificate_due(&self, identity: &str, from_address: &str, due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        let certificate = record
            .certificates
            .get_mut(&identity_key(from_address))
            .unwrap_or_else(|| {
                panic!("case {from_address:?}: the association must exist in the store")
            });
        certificate.status_due_at = due_at;
    }

    /// Hold an association's next clock-owed transition off, so an assertion about the
    /// status it carries now cannot race the span.
    fn hold_certificate(&self, identity: &str, from_address: &str) {
        self.set_certificate_due(
            identity,
            from_address,
            SystemTime::now() + CERTIFICATE_SETTLE + Duration::from_secs(3600),
        );
    }

    /// Bring an association's next clock-owed transition into the past, so the next read
    /// materialises it.
    fn due_certificate(&self, identity: &str, from_address: &str) {
        self.set_certificate_due(
            identity,
            from_address,
            SystemTime::now()
                .checked_sub(Duration::from_millis(1))
                .expect("an instant just before now"),
        );
    }

    /// The span the handler stored between accepting a call and the transition it owes.
    fn certificate_span(&self, identity: &str, from_address: &str, from: SystemTime) -> Duration {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        let record = store
            .identities
            .get(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        let certificate = record
            .certificates
            .get(&identity_key(from_address))
            .unwrap_or_else(|| {
                panic!("case {from_address:?}: the association must exist in the store")
            });
        certificate
            .status_due_at
            .duration_since(from)
            .expect("the due instant is at or after the call that started the transition")
    }

    /// Create `identity`, associate `arn` with `from_address` on it, and hold the
    /// association's `PROVISIONING` -> `ACTIVE` transition off.
    async fn with_held_association(&self, identity: &str, from_address: &str, arn: &str) {
        let created = self.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {identity:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );
        let associated = self
            .associate(json!({
                "EmailIdentity": identity,
                "FromAddress": from_address,
                "CertificateArn": arn,
            }))
            .await;
        assert_eq!(
            associated.status, 200,
            "case {identity:?}/{from_address:?}: expected the association to be accepted, got {} {:?}",
            associated.status, associated.body
        );
        self.hold_certificate(identity, from_address);
    }
}

/// One row of the association table: the identity the certificate is associated with, the
/// `FromAddress` the request carries (if any), and the address the listing must then
/// report the association under.
struct AssociationCase {
    case: &'static str,
    identity: &'static str,
    from_address: Option<&'static str>,
    reported_from_address: &'static str,
}

/// The rule spans the kind of identity and the form `FromAddress` arrives in, so the rows
/// cross an address identity with no `FromAddress`, with the identity repeated exactly and
/// with the same mailbox spelled with a differently cased domain, and a domain identity
/// with an address in the domain and one in a subdomain of it.
const ASSOCIATION_CASES: &[AssociationCase] = &[
    AssociationCase {
        case: "address identity, no FromAddress",
        identity: "signer@example.com",
        from_address: None,
        reported_from_address: "signer@example.com",
    },
    AssociationCase {
        case: "address identity, FromAddress repeating the identity",
        identity: "repeat@example.com",
        from_address: Some("repeat@example.com"),
        reported_from_address: "repeat@example.com",
    },
    AssociationCase {
        case: "address identity, FromAddress with a differently cased domain",
        identity: "cased@example.com",
        from_address: Some("cased@EXAMPLE.com"),
        reported_from_address: "cased@EXAMPLE.com",
    },
    AssociationCase {
        case: "domain identity, FromAddress in the domain",
        identity: "example.com",
        from_address: Some("sender@example.com"),
        reported_from_address: "sender@example.com",
    },
    AssociationCase {
        case: "domain identity, FromAddress in a subdomain",
        identity: "example.com",
        from_address: Some("sender@mail.corp.example.com"),
        reported_from_address: "sender@mail.corp.example.com",
    },
];

#[tokio::test]
async fn an_association_round_trips_through_the_identity_listing() {
    for case in ASSOCIATION_CASES {
        let AssociationCase {
            case,
            identity,
            from_address,
            reported_from_address,
        } = case;
        let ses = Ses::new();
        let created = ses.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );

        // Nothing is associated with a fresh identity.
        let empty = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            empty.status, 200,
            "case {case:?}: expected a listing of a fresh identity to be served, got {} {:?}",
            empty.status, empty.body
        );
        assert!(
            empty.certificates().is_empty(),
            "case {case:?}: expected no certificates on a fresh identity, got {:?}",
            empty.body
        );

        let arn = ses.signing_certificate(case).await;
        let mut request = json!({ "EmailIdentity": identity, "CertificateArn": arn });
        if let Some(address) = from_address {
            request["FromAddress"] = json!(address);
        }
        let before = SystemTime::now();
        let associated = ses.associate(request).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: expected the association to be accepted, got {} {:?}",
            associated.status, associated.body
        );

        // The transition is owed at an instant the handler stored, so hold it off before
        // asserting the status the caller sees straight after the call.
        let span = ses.certificate_span(identity, reported_from_address, before);
        assert!(
            span > Duration::ZERO,
            "case {case:?}: expected a non-zero span before the association becomes ACTIVE, got {span:?}"
        );
        ses.hold_certificate(identity, reported_from_address);

        let listed = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        let certificates = listed.certificates();
        assert_eq!(
            certificates.len(),
            1,
            "case {case:?}: expected exactly one association, got {:?}",
            listed.body
        );
        let certificate = &certificates[0];
        assert_eq!(
            member(certificate, "FromAddress"),
            *reported_from_address,
            "case {case:?}: expected the association to apply to {reported_from_address:?}, got {certificate:?}"
        );
        assert_eq!(
            member(certificate, "CertificateArn"),
            arn,
            "case {case:?}: expected the certificate ARN the request named, got {certificate:?}"
        );
        assert_eq!(
            member(certificate, "Status"),
            "PROVISIONING",
            "case {case:?}: expected a fresh association to be PROVISIONING, got {certificate:?}"
        );
        assert!(
            IDENTITY_CERTIFICATE_STATUS_VALUES.contains(&member(certificate, "Status").as_str()),
            "case {case:?}: expected a modelled IdentityCertificateStatus, got {certificate:?}"
        );
        assert!(
            listed.next_token().is_none(),
            "case {case:?}: expected no NextToken for a single-page listing, got {:?}",
            listed.body
        );

        // The transition the clock owes: once due, a read reports the association ready
        // for signing.
        ses.due_certificate(identity, reported_from_address);
        let active = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&active.certificates()[0], "Status"),
            "ACTIVE",
            "case {case:?}: expected the association to be ACTIVE once its transition fell due, got {:?}",
            active.body
        );

        // Removing it stops the signing at once, and the association is then cleaned up.
        let removed_request = match from_address {
            Some(address) => json!({ "EmailIdentity": identity, "FromAddress": address }),
            None => json!({ "EmailIdentity": identity }),
        };
        let removed = ses.disassociate(removed_request.clone()).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected the association to be removed, got {} {:?}",
            removed.status, removed.body
        );
        ses.hold_certificate(identity, reported_from_address);
        let inactive = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&inactive.certificates()[0], "Status"),
            "INACTIVE",
            "case {case:?}: expected a removed association to be INACTIVE, got {:?}",
            inactive.body
        );

        // The clean-up: INACTIVE falls to DEPROVISIONING, and then the association is
        // gone, which frees the address to take a new certificate.
        ses.due_certificate(identity, reported_from_address);
        let deprovisioning = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&deprovisioning.certificates()[0], "Status"),
            "DEPROVISIONING",
            "case {case:?}: expected the clean-up to report DEPROVISIONING, got {:?}",
            deprovisioning.body
        );
        ses.due_certificate(identity, reported_from_address);
        let gone = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert!(
            gone.certificates().is_empty(),
            "case {case:?}: expected the cleaned-up association to be gone, got {:?}",
            gone.body
        );

        // Idempotent: removing what is no longer there changes nothing and is served.
        let again = ses.disassociate(removed_request).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a removal with no matching association to succeed, got {} {:?}",
            again.status, again.body
        );
    }
}

/// One row of the refusal table: the request, and the status, error code and message the
/// service must answer it with.
struct RefusalCase {
    case: &'static str,
    identity: &'static str,
    from_address: Option<&'static str>,
    status: u16,
    code: &'static str,
    message: String,
}

/// The refusals an association is subject to: an identity this account does not hold, a
/// domain identity with no address to apply the certificate to, an address outside the
/// identity (including a domain whose name merely ends with the identity's, which DNS
/// does not make a subdomain), and an address identity the `FromAddress` does not name.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "identity this account does not hold",
            identity: "absent.example.com",
            from_address: Some("sender@absent.example.com"),
            status: 404,
            code: "NotFoundException",
            message: "Email identity <absent.example.com> does not exist.".to_string(),
        },
        RefusalCase {
            case: "domain identity with no FromAddress",
            identity: "example.com",
            from_address: None,
            status: 400,
            code: "BadRequestException",
            message: "FromAddress is required when EmailIdentity is a domain.".to_string(),
        },
        RefusalCase {
            case: "FromAddress in another domain",
            identity: "example.com",
            from_address: Some("sender@other.example.org"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <sender@other.example.org> does not belong to the domain \
                      <example.com> or one of its subdomains."
                .to_string(),
        },
        RefusalCase {
            case: "FromAddress in a domain that merely ends with the identity",
            identity: "example.com",
            from_address: Some("sender@notexample.com"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <sender@notexample.com> does not belong to the domain \
                      <example.com> or one of its subdomains."
                .to_string(),
        },
        RefusalCase {
            case: "FromAddress that is not an address at all",
            identity: "example.com",
            from_address: Some("example.com"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <example.com> does not belong to the domain <example.com> \
                      or one of its subdomains."
                .to_string(),
        },
        RefusalCase {
            case: "address identity with a FromAddress naming another mailbox",
            identity: "signer@example.com",
            from_address: Some("other@example.com"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <other@example.com> must match the email identity \
                      <signer@example.com>."
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn an_association_the_identity_cannot_take_is_refused() {
    let arn = certificate_arn("22222222-2222-2222-2222-222222222222");
    for case in refusal_cases() {
        let ses = Ses::new();
        // Every case but the unknown-identity one needs the identity to exist, so the
        // refusal under test is the one the case is about.
        for identity in ["example.com", "signer@example.com"] {
            let created = ses.create_identity(identity).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected {identity:?} to be created, got {} {:?}",
                case.case, created.status, created.body
            );
        }

        let mut request = json!({ "EmailIdentity": case.identity, "CertificateArn": arn });
        if let Some(address) = case.from_address {
            request["FromAddress"] = json!(address);
        }
        let refused = ses.associate(request).await;
        assert_eq!(
            refused.status, case.status,
            "case {:?}: expected HTTP {}, got {} {:?}",
            case.case, case.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            case.code,
            "case {:?}: expected {}, got {:?} ({:?})",
            case.case,
            case.code,
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            case.message,
            "case {:?}: expected the refusal message {:?}, got {:?}",
            case.case,
            case.message,
            refused.message()
        );
    }
}

#[tokio::test]
async fn one_address_holds_one_certificate_at_a_time() {
    // "Each email address can have only one certificate association": the second call is
    // refused whichever status the first association is in, until it has been removed and
    // cleaned up.
    let holding_statuses = ["PROVISIONING", "ACTIVE", "INACTIVE", "DEPROVISIONING"];
    for status in holding_statuses {
        let ses = Ses::new();
        let identity = "example.com";
        let from_address = "sender@example.com";
        let held = ses.signing_certificate(status).await;
        let replacement = ses.signing_certificate(status).await;
        ses.with_held_association(identity, from_address, &held)
            .await;
        // Walk the association into the status this row is about.
        match status {
            "PROVISIONING" => {}
            "ACTIVE" => {
                ses.due_certificate(identity, from_address);
                ses.list_certificates(json!({ "EmailIdentity": identity }))
                    .await;
                ses.hold_certificate(identity, from_address);
            }
            _ => {
                ses.due_certificate(identity, from_address);
                ses.list_certificates(json!({ "EmailIdentity": identity }))
                    .await;
                ses.disassociate(json!({
                    "EmailIdentity": identity,
                    "FromAddress": from_address,
                }))
                .await;
                if status == "DEPROVISIONING" {
                    ses.due_certificate(identity, from_address);
                    ses.list_certificates(json!({ "EmailIdentity": identity }))
                        .await;
                }
                ses.hold_certificate(identity, from_address);
            }
        }

        let listed = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&listed.certificates()[0], "Status"),
            status,
            "case {status:?}: expected the association to be in this status before the second \
             call, got {:?}",
            listed.body
        );

        let second = ses
            .associate(json!({
                "EmailIdentity": identity,
                "FromAddress": from_address,
                "CertificateArn": replacement,
            }))
            .await;
        if status == "DEPROVISIONING" {
            // "...unless the existing association is in the DEPROVISIONING state": one
            // being cleaned up is on its way out, so the address may take a new one.
            assert_eq!(
                second.status, 200,
                "case {status:?}: expected a new association over one being cleaned up, got {} {:?}",
                second.status, second.body
            );
            ses.hold_certificate(identity, from_address);
            let replaced = ses
                .list_certificates(json!({ "EmailIdentity": identity }))
                .await;
            assert_eq!(
                member(&replaced.certificates()[0], "CertificateArn"),
                replacement,
                "case {status:?}: expected the new certificate to be the one reported, got {:?}",
                replaced.body
            );
            assert_eq!(
                member(&replaced.certificates()[0], "Status"),
                "PROVISIONING",
                "case {status:?}: expected the replacement to start out PROVISIONING, got {:?}",
                replaced.body
            );
            continue;
        }
        assert_eq!(
            second.status, 400,
            "case {status:?}: expected the duplicate association to be refused, got {} {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.error_code(),
            "AlreadyExistsException",
            "case {status:?}: expected AlreadyExistsException, got {:?} ({:?})",
            second.error_code(),
            second.message()
        );
        assert_eq!(
            second.message(),
            format!(
                "A certificate is already associated with sender <{from_address}> on identity \
                 <{identity}>."
            ),
            "case {status:?}: expected the refusal to name the sender and the identity, got {:?}",
            second.message()
        );
    }
}

#[tokio::test]
async fn an_identity_that_still_signs_with_a_certificate_cannot_be_deleted() {
    let ses = Ses::new();
    let identity = "example.com";
    let from_address = "sender@example.com";
    let arn = ses.signing_certificate("still signing").await;
    ses.with_held_association(identity, from_address, &arn)
        .await;

    // Both statuses an association signs with hold the identity: one being prepared for
    // signing and one ready for it.
    for status in ["PROVISIONING", "ACTIVE"] {
        if status == "ACTIVE" {
            ses.due_certificate(identity, from_address);
            ses.list_certificates(json!({ "EmailIdentity": identity }))
                .await;
            ses.hold_certificate(identity, from_address);
        }
        let refused = ses.delete_identity(identity).await;
        assert_eq!(
            refused.status, 400,
            "case {status:?}: expected the delete to be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {status:?}: expected BadRequestException, got {:?} ({:?})",
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            format!(
                "Cannot delete <{}> because it has certificates associated with it. \
                 Disassociate all certificates and try again.",
                identity_arn("us-east-1", identity)
            ),
            "case {status:?}: expected the refusal to name the identity by ARN, got {:?}",
            refused.message()
        );
        let still_there = ses.get_identity(identity).await;
        assert_eq!(
            still_there.status, 200,
            "case {status:?}: expected the refused delete to leave the identity, got {} {:?}",
            still_there.status, still_there.body
        );
    }

    // Once nothing signs for it, the identity goes.
    let removed = ses
        .disassociate(json!({
            "EmailIdentity": identity,
            "FromAddress": from_address,
        }))
        .await;
    assert_eq!(
        removed.status, 200,
        "expected the association to be removed, got {} {:?}",
        removed.status, removed.body
    );
    let deleted = ses.delete_identity(identity).await;
    assert_eq!(
        deleted.status, 200,
        "expected the delete to be served once nothing signs for the identity, got {} {:?}",
        deleted.status, deleted.body
    );
    let read_back = ses.get_identity(identity).await;
    assert_eq!(
        read_back.status, 404,
        "expected the deleted identity to be gone, got {} {:?}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.message(),
        "Email identity <example.com> does not exist.",
        "expected the deleted identity's own not-found sentence, got {:?}",
        read_back.message()
    );
}

#[tokio::test]
async fn an_identity_holds_at_most_the_bounded_number_of_associations() {
    let ses = Ses::new();
    let identity = "example.com";
    let created = ses.create_identity(identity).await;
    assert_eq!(
        created.status, 200,
        "expected the identity to be created, got {} {:?}",
        created.status, created.body
    );

    // Fill the identity to its bound through the store rather than through the wire, so
    // the case runs in one request instead of thousands; the refusal under test is still
    // the handler's.
    {
        let state = ses
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .expect("the identity must exist in the store");
        let now = SystemTime::now();
        for index in 0..MAX_CERTIFICATES_PER_IDENTITY {
            let address = format!("bulk{index}@example.com");
            record.certificates.insert(
                identity_key(&address),
                IdentityCertificateRecord::new(
                    &address,
                    &certificate_arn(&format!("7777777{index}")),
                    true,
                    now,
                ),
            );
        }
    }

    let past_the_bound = ses
        .associate(json!({
            "EmailIdentity": identity,
            "FromAddress": "one-too-many@example.com",
            "CertificateArn": certificate_arn("88888888-8888-8888-8888-888888888888"),
        }))
        .await;
    assert_eq!(
        past_the_bound.status, 400,
        "expected the association past the bound to be refused, got {} {:?}",
        past_the_bound.status, past_the_bound.body
    );
    assert_eq!(
        past_the_bound.error_code(),
        "BadRequestException",
        "expected BadRequestException, got {:?} ({:?})",
        past_the_bound.error_code(),
        past_the_bound.message()
    );
    assert_eq!(
        past_the_bound.message(),
        format!(
            "Email identity <{identity}> already holds the maximum of \
             {MAX_CERTIFICATES_PER_IDENTITY} certificate associations."
        ),
        "expected the refusal to name the bound it is past, got {:?}",
        past_the_bound.message()
    );

    // An association an address already holds is still refused as a duplicate at the
    // bound, rather than as a growth the store cannot take.
    let duplicate = ses
        .associate(json!({
            "EmailIdentity": identity,
            "FromAddress": "bulk0@example.com",
            "CertificateArn": certificate_arn("99999999-9999-9999-9999-999999999999"),
        }))
        .await;
    assert_eq!(
        duplicate.error_code(),
        "AlreadyExistsException",
        "expected the duplicate at the bound to be refused as one, got {:?} ({:?})",
        duplicate.error_code(),
        duplicate.message()
    );
}

/// One row of the pagination table: how many associations the identity holds and the
/// `PageSize` the walk asks for.
struct PageCase {
    case: &'static str,
    stored: usize,
    page_size: Option<usize>,
}

/// The pagination rule spans the association count and the page size independently, so
/// the rows cross an empty, a single and a multi-association identity with an absent page
/// size, a page size of one, one that divides the count, one that does not, and one larger
/// than the count.
const PAGE_CASES: &[PageCase] = &[
    PageCase {
        case: "nothing associated, no page size",
        stored: 0,
        page_size: None,
    },
    PageCase {
        case: "nothing associated, page size 1",
        stored: 0,
        page_size: Some(1),
    },
    PageCase {
        case: "one association, page size 1",
        stored: 1,
        page_size: Some(1),
    },
    PageCase {
        case: "one association, no page size",
        stored: 1,
        page_size: None,
    },
    PageCase {
        case: "two associations, page size 1",
        stored: 2,
        page_size: Some(1),
    },
    PageCase {
        case: "ten associations, page size 1",
        stored: 10,
        page_size: Some(1),
    },
    PageCase {
        case: "ten associations, page size 5",
        stored: 10,
        page_size: Some(5),
    },
    PageCase {
        case: "ten associations, page size 4",
        stored: 10,
        page_size: Some(4),
    },
    PageCase {
        case: "ten associations, page size 100",
        stored: 10,
        page_size: Some(100),
    },
    PageCase {
        case: "ten associations, no page size",
        stored: 10,
        page_size: None,
    },
];

#[tokio::test]
async fn a_certificate_walk_visits_every_association_exactly_once() {
    for case in PAGE_CASES {
        let PageCase {
            case,
            stored,
            page_size,
        } = case;
        let ses = Ses::new();
        let identity = "example.com";
        let created = ses.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );

        let expected: BTreeSet<String> = (0..*stored)
            .map(|index| format!("sender{index}@example.com"))
            .collect();
        for (index, address) in expected.iter().enumerate() {
            let associated = ses
                .associate(json!({
                    "EmailIdentity": identity,
                    "FromAddress": address,
                    "CertificateArn": certificate_arn(&format!("6666666{index}")),
                }))
                .await;
            assert_eq!(
                associated.status, 200,
                "case {case:?}: expected {address:?} to be associated, got {} {:?}",
                associated.status, associated.body
            );
        }

        // One request per association plus one for the final page is the most any page
        // size can need; a request past that means the token loop does not terminate.
        let budget = stored + 2;
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut requests = 0;
        loop {
            requests += 1;
            assert!(
                requests <= budget,
                "case {case:?}: expected the walk to terminate within {budget} requests, \
                 visited {seen:?}"
            );
            let mut request = json!({ "EmailIdentity": identity });
            if let Some(size) = page_size {
                request["PageSize"] = json!(size);
            }
            if let Some(value) = &token {
                request["NextToken"] = json!(value);
            }
            let page = ses.list_certificates(request).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected the page to be served, got {} {:?}",
                page.status, page.body
            );
            let certificates = page.certificates();
            if let Some(size) = page_size {
                assert!(
                    certificates.len() <= *size,
                    "case {case:?}: expected at most {size} associations in a page, got {:?}",
                    page.body
                );
            }
            seen.extend(certificates.iter().map(|row| member(row, "FromAddress")));
            match page.next_token() {
                None => break,
                Some(next) => {
                    assert!(
                        !certificates.is_empty(),
                        "case {case:?}: expected a page handing back a token to carry \
                         associations, got {:?}",
                        page.body
                    );
                    token = Some(next);
                }
            }
        }

        assert_eq!(
            seen.len(),
            *stored,
            "case {case:?}: expected the walk to visit {stored} associations, visited {seen:?}"
        );
        let visited: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            visited, expected,
            "case {case:?}: expected the walk to visit every stored association exactly once, \
             visited {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: expected the walk to enumerate associations in a stable order, \
             visited {seen:?}"
        );
    }
}

/// One row of the `FromAddress`-against-an-address-identity table: the identity to
/// create, the `FromAddress` to associate against it, and whether that value names the
/// identity's mailbox.
struct FromAddressCase {
    case: &'static str,
    identity: &'static str,
    from_address: &'static str,
    names_the_identity: bool,
}

/// The forms a `FromAddress` on an ADDRESS identity can take, spanning what "names the
/// same mailbox" generalises over: the local part's spelling (which belongs to the
/// receiving mailbox, so its case counts), the domain's spelling (which DNS compares
/// without regard to case), the display-name wrapper RFC 5322 allows around an
/// addr-spec, the domain the address sits in (including a subdomain of the identity's,
/// which is a different mailbox), and a value that is no address at all.
fn from_address_cases() -> Vec<FromAddressCase> {
    vec![
        FromAddressCase {
            case: "the identity itself",
            identity: "signer@example.com",
            from_address: "signer@example.com",
            names_the_identity: true,
        },
        FromAddressCase {
            case: "the identity with its domain cased differently",
            identity: "signer@example.com",
            from_address: "signer@EXAMPLE.COM",
            names_the_identity: true,
        },
        FromAddressCase {
            case: "the identity in display-name form",
            identity: "signer@example.com",
            from_address: "Signer Name <signer@example.com>",
            names_the_identity: true,
        },
        FromAddressCase {
            case: "another mailbox in the identity's domain",
            identity: "signer@example.com",
            from_address: "other@example.com",
            names_the_identity: false,
        },
        FromAddressCase {
            case: "the identity's local part with its case changed",
            identity: "signer@example.com",
            from_address: "Signer@example.com",
            names_the_identity: false,
        },
        FromAddressCase {
            case: "the identity's local part in another domain",
            identity: "signer@example.com",
            from_address: "signer@other.example.org",
            names_the_identity: false,
        },
        FromAddressCase {
            case: "a subdomain of the identity's domain",
            identity: "signer@example.com",
            from_address: "signer@mail.example.com",
            names_the_identity: false,
        },
        FromAddressCase {
            case: "another mailbox in display-name form",
            identity: "signer@example.com",
            from_address: "Someone Else <other@example.com>",
            names_the_identity: false,
        },
        FromAddressCase {
            case: "a value that is not an address",
            identity: "signer@example.com",
            from_address: "example.com",
            names_the_identity: false,
        },
        FromAddressCase {
            case: "another identity of the account",
            identity: "Dotted.Signer@mail.example.com",
            from_address: "signer@example.com",
            names_the_identity: false,
        },
        FromAddressCase {
            case: "an identity in a subdomain, named as itself",
            identity: "Dotted.Signer@mail.example.com",
            from_address: "Dotted.Signer@mail.example.com",
            names_the_identity: true,
        },
    ]
}

#[tokio::test]
async fn a_from_address_outside_an_address_identity_is_refused_in_the_services_wording() {
    let arn = certificate_arn("44444444-4444-4444-4444-444444444444");
    for case in from_address_cases() {
        let FromAddressCase {
            case,
            identity,
            from_address,
            names_the_identity,
        } = case;
        let ses = Ses::new();
        for created in ["signer@example.com", "Dotted.Signer@mail.example.com"] {
            let reply = ses.create_identity(created).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected {created:?} to be created, got {} {:?}",
                reply.status, reply.body
            );
        }

        let reply = ses
            .associate(json!({
                "EmailIdentity": identity,
                "FromAddress": from_address,
                "CertificateArn": arn,
            }))
            .await;

        if names_the_identity {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected a FromAddress naming the identity's mailbox to be \
                 accepted, got {} {:?}",
                reply.status, reply.body
            );
            continue;
        }

        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?} ({:?})",
            reply.error_code(),
            reply.message()
        );
        // The sentence the service answers: its fixed text with the address the request
        // spelled and the identity it targeted, both as supplied, in its angle brackets.
        let expected =
            format!("FromAddress <{from_address}> must match the email identity <{identity}>.");
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected the refusal message {expected:?}, got {:?}",
            reply.message()
        );

        // A refused call creates nothing: neither the address it named nor the identity
        // it targeted holds an association afterwards.
        for listed in [identity, from_address] {
            let listing = ses
                .list_certificates(json!({ "EmailIdentity": listed }))
                .await;
            if listing.status != 200 {
                continue;
            }
            assert!(
                listing.certificates().is_empty(),
                "case {case:?}: expected the refused call to associate nothing with \
                 {listed:?}, got {:?}",
                listing.body
            );
        }
    }
}

#[tokio::test]
async fn a_listing_refuses_a_token_it_did_not_issue() {
    let ses = Ses::new();
    let identity = "example.com";
    ses.create_identity(identity).await;

    // A token from somewhere else is not a position in this listing: a base64 value of
    // another listing's making, and a value that is not base64 at all.
    let foreign = ::aws_smithy_types::base64::encode("sesv2:contacts:someone@example.com");
    for token in [foreign.as_str(), "not-a-token"] {
        let refused = ses
            .list_certificates(json!({ "EmailIdentity": identity, "NextToken": token }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {token:?}: expected the foreign token to be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {token:?}: expected BadRequestException, got {:?} ({:?})",
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            "Invalid next token.",
            "case {token:?}: expected this service's one bad-cursor sentence, got {:?}",
            refused.message()
        );
    }

    // A listing and a removal of an identity nobody created name the identity they could
    // not find.
    for reply in [
        ses.list_certificates(json!({ "EmailIdentity": "absent.example.com" }))
            .await,
        ses.disassociate(json!({
            "EmailIdentity": "absent.example.com",
            "FromAddress": "sender@absent.example.com",
        }))
        .await,
    ] {
        assert_eq!(
            reply.status, 404,
            "expected an unknown identity to be reported as not found, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.message(),
            "Email identity <absent.example.com> does not exist.",
            "expected the identity's own not-found sentence, got {:?}",
            reply.message()
        );
    }

    // A removal needs the address when the identity is a domain, as the association is
    // the domain's one for that address.
    let no_address = ses.disassociate(json!({ "EmailIdentity": identity })).await;
    assert_eq!(
        no_address.status, 400,
        "expected a removal with no FromAddress on a domain to be refused, got {} {:?}",
        no_address.status, no_address.body
    );
    assert_eq!(
        no_address.message(),
        "FromAddress is required when EmailIdentity is a domain.",
        "expected the missing-FromAddress sentence, got {:?}",
        no_address.message()
    );
}

/// What the HOLDER of the certificate an association names holds for the ARN the request
/// carries.
enum Holding {
    /// No certificate at all under that ARN.
    Nothing,
    /// A certificate whose request the holder could not issue.
    CouldNotBeIssued,
    /// A certificate the holder is still validating, so it is not issued.
    NotIssuedYet,
    /// A certificate brought in by its owner, issued by a certificate authority and
    /// asserting these key purposes; an empty list asserts no key purpose at all.
    Imported(&'static [rcgen::ExtendedKeyUsagePurpose]),
    /// A certificate brought in by its owner that no authority other than its own subject
    /// issued — it signed itself — asserting these key purposes.
    SelfSigned(&'static [rcgen::ExtendedKeyUsagePurpose]),
    /// An imported certificate asserting `emailProtection` whose validity has since run
    /// out.
    ImportedThenExpired,
}

/// One row of the preparation table: what the holder holds, and the status an association
/// naming it must reach.
struct Preparation {
    case: &'static str,
    holding: Holding,
    /// The modelled `IdentityCertificateStatus` the association ends in.
    settles_into: &'static str,
}

/// The rows span what the rule generalises over: an ARN the holder has nothing under, a
/// certificate it could not issue, one it has not issued yet, imported certificates whose
/// Extended Key Usage extension does and does not permit email protection (including the
/// `anyExtendedKeyUsage` purpose and the absence of the extension altogether), ones an
/// authority issued and ones that issued themselves, and one whose validity has run out.
///
/// The key purposes are the ones the holder publishes as `ExtendedKeyUsageName`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ExtendedKeyUsageName")` lists
/// `EMAIL_PROTECTION`, `TLS_WEB_SERVER_AUTHENTICATION`, `TLS_WEB_CLIENT_AUTHENTICATION`,
/// `CODE_SIGNING`, `ANY` and `NONE` among them); `emailProtection` is the purpose RFC
/// 5280 section 4.2.1.12 assigns to protecting email.
fn preparation_cases() -> Vec<Preparation> {
    vec![
        Preparation {
            case: "an ARN the holder has no certificate under",
            holding: Holding::Nothing,
            settles_into: "FAILED",
        },
        Preparation {
            case: "a certificate the holder could not issue",
            holding: Holding::CouldNotBeIssued,
            settles_into: "FAILED",
        },
        Preparation {
            case: "a certificate the holder has not issued yet",
            holding: Holding::NotIssuedYet,
            settles_into: "FAILED",
        },
        Preparation {
            case: "an imported certificate asserting emailProtection",
            holding: Holding::Imported(&[rcgen::ExtendedKeyUsagePurpose::EmailProtection]),
            settles_into: "ACTIVE",
        },
        Preparation {
            case: "an imported certificate asserting emailProtection among other purposes",
            holding: Holding::Imported(&[
                rcgen::ExtendedKeyUsagePurpose::ClientAuth,
                rcgen::ExtendedKeyUsagePurpose::EmailProtection,
                rcgen::ExtendedKeyUsagePurpose::CodeSigning,
            ]),
            settles_into: "ACTIVE",
        },
        Preparation {
            case: "an imported certificate asserting anyExtendedKeyUsage",
            holding: Holding::Imported(&[rcgen::ExtendedKeyUsagePurpose::Any]),
            settles_into: "ACTIVE",
        },
        Preparation {
            case: "an imported certificate asserting no key purpose at all",
            holding: Holding::Imported(&[]),
            settles_into: "ACTIVE",
        },
        Preparation {
            case: "a self-signed certificate asserting emailProtection",
            holding: Holding::SelfSigned(&[rcgen::ExtendedKeyUsagePurpose::EmailProtection]),
            settles_into: "FAILED",
        },
        Preparation {
            case: "a self-signed certificate asserting anyExtendedKeyUsage",
            holding: Holding::SelfSigned(&[rcgen::ExtendedKeyUsagePurpose::Any]),
            settles_into: "FAILED",
        },
        Preparation {
            case: "an imported certificate asserting only the TLS purposes",
            holding: Holding::Imported(&[
                rcgen::ExtendedKeyUsagePurpose::ServerAuth,
                rcgen::ExtendedKeyUsagePurpose::ClientAuth,
            ]),
            settles_into: "FAILED",
        },
        Preparation {
            case: "an imported certificate whose validity has run out",
            holding: Holding::ImportedThenExpired,
            settles_into: "FAILED",
        },
    ]
}

/// RULE: an association's status is what the service holding the certificate says about
/// that certificate — `ACTIVE`, by way of the `PROVISIONING` the preparation runs in, for
/// a certificate it holds that a certificate authority issued, that is still valid and
/// that may be used to protect email, and `FAILED` from the start for one it does not
/// hold, has not issued, or holds in a state nothing can be signed with, which leaves no
/// preparation to run.
#[tokio::test]
async fn the_preparation_of_an_association_ends_where_the_certificate_allows() {
    for Preparation {
        case,
        holding,
        settles_into,
    } in preparation_cases()
    {
        let ses = Ses::new();
        let identity = "example.com";
        let from_address = "sender@example.com";
        let created = ses.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );

        let arn = match holding {
            Holding::Nothing => certificate_arn("deadbeef-0000-4000-8000-000000000000"),
            Holding::CouldNotBeIssued => ses.unissued_certificate(case).await,
            Holding::NotIssuedYet => ses.pending_certificate(case).await,
            Holding::Imported(purposes) => ses.imported_certificate(case, purposes, 2100).await,
            Holding::SelfSigned(purposes) => {
                ses.self_signed_certificate(case, purposes, 2100).await
            }
            Holding::ImportedThenExpired => {
                let arn = ses
                    .imported_certificate(
                        case,
                        &[rcgen::ExtendedKeyUsagePurpose::EmailProtection],
                        2100,
                    )
                    .await;
                ses.expire_acm_certificate(&arn);
                arn
            }
        };

        let associated = ses
            .associate(json!({
                "EmailIdentity": identity,
                "FromAddress": from_address,
                "CertificateArn": arn,
            }))
            .await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: expected the association to be accepted, got {} {:?}",
            associated.status, associated.body
        );

        // Where the association STARTS is what the certificate allows: "when the
        // association is created, the certificate begins provisioning and its status is
        // PROVISIONING" for one there is something to prepare out of — a transition owed
        // at an instant the handler stored, so it is held off rather than waited on — while
        // one that cannot be prepared at all has nothing to provision and reads its
        // terminal status straight away.
        ses.hold_certificate(identity, from_address);
        let first_read = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        let starts_in = if settles_into == "ACTIVE" {
            "PROVISIONING"
        } else {
            settles_into
        };
        assert_eq!(
            member(&first_read.certificates()[0], "Status"),
            starts_in,
            "case {case:?}: expected a fresh association to be {starts_in}, got {:?}",
            first_read.body
        );

        ses.due_certificate(identity, from_address);
        let settled = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        let certificate = &settled.certificates()[0];
        assert_eq!(
            member(certificate, "Status"),
            settles_into,
            "case {case:?}: expected the preparation to end in {settles_into}, got {:?}",
            settled.body
        );
        assert!(
            IDENTITY_CERTIFICATE_STATUS_VALUES.contains(&member(certificate, "Status").as_str()),
            "case {case:?}: expected a modelled IdentityCertificateStatus, got {certificate:?}"
        );
        assert_eq!(
            member(certificate, "CertificateArn"),
            arn,
            "case {case:?}: expected the ARN the request named, got {certificate:?}"
        );
        // "The timestamp after which the certificate is no longer valid" belongs to the
        // certificate, and a preparation that failed reports none.
        if settles_into == "FAILED" {
            assert!(
                certificate.get("CertificateExpiryTime").is_none(),
                "case {case:?}: expected no expiry on a failed preparation, got {certificate:?}"
            );
        }

        // Whichever status it ended in, it is one the clock cannot move again: a further
        // read reports the same status.
        ses.due_certificate(identity, from_address);
        let again = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&again.certificates()[0], "Status"),
            settles_into,
            "case {case:?}: expected {settles_into} to be the status it stays in, got {:?}",
            again.body
        );
    }
}

/// RULE: an association whose preparation failed signs nothing, so the identity holding
/// it can be deleted — and it is still an association the account holds, so removing it
/// takes it out of use and the address can take another certificate.
#[tokio::test]
async fn a_failed_association_signs_nothing_and_can_still_be_removed() {
    let ses = Ses::new();
    let identity = "example.com";
    let from_address = "sender@example.com";
    ses.create_identity(identity).await;
    let unheld = certificate_arn("deadbeef-1111-4111-8111-111111111111");
    let associated = ses
        .associate(json!({
            "EmailIdentity": identity,
            "FromAddress": from_address,
            "CertificateArn": unheld,
        }))
        .await;
    assert_eq!(
        associated.status, 200,
        "expected the association to be accepted, got {} {:?}",
        associated.status, associated.body
    );
    ses.due_certificate(identity, from_address);
    let failed = ses
        .list_certificates(json!({ "EmailIdentity": identity }))
        .await;
    assert_eq!(
        member(&failed.certificates()[0], "Status"),
        "FAILED",
        "expected the preparation of a certificate nothing holds to fail, got {:?}",
        failed.body
    );

    // A second association for the same address is still refused: the account holds one
    // for it, failed or not.
    let second = ses
        .associate(json!({
            "EmailIdentity": identity,
            "FromAddress": from_address,
            "CertificateArn": certificate_arn("deadbeef-2222-4222-8222-222222222222"),
        }))
        .await;
    assert_eq!(
        (second.status, second.error_code().as_str()),
        (400, "AlreadyExistsException"),
        "expected the duplicate to be refused, got {} {:?}",
        second.status,
        second.body
    );

    // Removing it takes it out of use, and the clean-up then frees the address.
    let removed = ses
        .disassociate(json!({ "EmailIdentity": identity, "FromAddress": from_address }))
        .await;
    assert_eq!(
        removed.status, 200,
        "expected the removal to be served, got {} {:?}",
        removed.status, removed.body
    );
    ses.hold_certificate(identity, from_address);
    let inactive = ses
        .list_certificates(json!({ "EmailIdentity": identity }))
        .await;
    assert_eq!(
        member(&inactive.certificates()[0], "Status"),
        "INACTIVE",
        "expected a removed association to be INACTIVE, got {:?}",
        inactive.body
    );
    for _ in 0..2 {
        ses.due_certificate(identity, from_address);
        ses.list_certificates(json!({ "EmailIdentity": identity }))
            .await;
    }
    let gone = ses
        .list_certificates(json!({ "EmailIdentity": identity }))
        .await;
    assert!(
        gone.certificates().is_empty(),
        "expected the cleaned-up association to be gone, got {:?}",
        gone.body
    );

    // Nothing signs for the identity, so it can be deleted — which a certificate being
    // prepared or ready for signing would hold.
    let deleted = ses.delete_identity(identity).await;
    assert_eq!(
        deleted.status, 200,
        "expected the identity to be deletable once nothing signs for it, got {} {:?}",
        deleted.status, deleted.body
    );

    // And a FAILED association holds nothing back either, because it signs nothing: a
    // second account whose only association failed can delete its identity straight
    // away.
    let other = Ses::new();
    other.create_identity(identity).await;
    other
        .associate(json!({
            "EmailIdentity": identity,
            "FromAddress": from_address,
            "CertificateArn": certificate_arn("deadbeef-3333-4333-8333-333333333333"),
        }))
        .await;
    other.due_certificate(identity, from_address);
    let listed = other
        .list_certificates(json!({ "EmailIdentity": identity }))
        .await;
    assert_eq!(
        member(&listed.certificates()[0], "Status"),
        "FAILED",
        "expected the second account's association to have failed, got {:?}",
        listed.body
    );
    let deleted_with_failed = other.delete_identity(identity).await;
    assert_eq!(
        deleted_with_failed.status, 200,
        "expected an identity whose only association FAILED to be deletable, got {} {:?}",
        deleted_with_failed.status, deleted_with_failed.body
    );
}

/// One row of the issuance table: who signed the certificate the association names, the
/// key purposes that certificate asserts, and the status the association must carry while
/// nothing the clock owes it has fallen due.
struct Issuance {
    case: &'static str,
    /// Whether a certificate authority other than the certificate's own subject signed
    /// it.
    issued_by_an_authority: bool,
    purposes: &'static [rcgen::ExtendedKeyUsagePurpose],
    /// The modelled `IdentityCertificateStatus` the association is created in.
    created_in: &'static str,
    /// The one it carries once every transition the clock owes it has fallen due.
    settles_into: &'static str,
}

/// The rows differ along the one thing this rule turns on — whether an authority other
/// than the certificate's own subject signed it — crossed with the key purposes the
/// certificate asserts, so every row is compared with a certificate identical in every
/// other respect.
const ISSUANCE_CASES: &[Issuance] = &[
    Issuance {
        case: "a certificate an authority issued, asserting emailProtection",
        issued_by_an_authority: true,
        purposes: &[rcgen::ExtendedKeyUsagePurpose::EmailProtection],
        created_in: "PROVISIONING",
        settles_into: "ACTIVE",
    },
    Issuance {
        case: "a certificate that signed itself, asserting emailProtection",
        issued_by_an_authority: false,
        purposes: &[rcgen::ExtendedKeyUsagePurpose::EmailProtection],
        created_in: "FAILED",
        settles_into: "FAILED",
    },
    Issuance {
        case: "a certificate that signed itself, asserting anyExtendedKeyUsage",
        issued_by_an_authority: false,
        purposes: &[rcgen::ExtendedKeyUsagePurpose::Any],
        created_in: "FAILED",
        settles_into: "FAILED",
    },
    Issuance {
        case: "a certificate an authority issued, asserting no key purpose at all",
        issued_by_an_authority: true,
        purposes: &[],
        created_in: "PROVISIONING",
        settles_into: "ACTIVE",
    },
    Issuance {
        case: "a certificate that signed itself, asserting no key purpose at all",
        issued_by_an_authority: false,
        purposes: &[],
        created_in: "FAILED",
        settles_into: "FAILED",
    },
];

/// RULE: a certificate SES can prepare for signing is one a certificate AUTHORITY issued,
/// so an association naming a certificate that signed itself is created in the terminal
/// `FAILED` state — there is no preparation to run out of a certificate nothing but its
/// own key vouches for — while one naming a certificate an authority issued is created
/// `PROVISIONING` and becomes `ACTIVE` when that preparation falls due.
#[tokio::test]
async fn an_association_is_prepared_only_from_a_certificate_an_authority_issued() {
    for Issuance {
        case,
        issued_by_an_authority,
        purposes,
        created_in,
        settles_into,
    } in ISSUANCE_CASES
    {
        let ses = Ses::new();
        // An address identity, so the association applies to the identity's own address.
        let identity = "signer@example.com";
        let created = ses.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );
        let arn = if *issued_by_an_authority {
            ses.imported_certificate(case, purposes, 2100).await
        } else {
            ses.self_signed_certificate(case, purposes, 2100).await
        };

        let associated = ses
            .associate(json!({ "EmailIdentity": identity, "CertificateArn": arn }))
            .await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: expected the association to be accepted, got {} {:?}",
            associated.status, associated.body
        );

        // With every transition the clock owes the association held off, the status a
        // read reports is the one the association was CREATED in — nothing has fallen
        // due to move it.
        ses.hold_certificate(identity, identity);
        let listed = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        let certificate = &listed.certificates()[0];
        assert_eq!(
            member(certificate, "Status"),
            *created_in,
            "case {case:?}: expected the association to be created {created_in}, got {:?}",
            listed.body
        );
        assert!(
            IDENTITY_CERTIFICATE_STATUS_VALUES.contains(&member(certificate, "Status").as_str()),
            "case {case:?}: expected a modelled IdentityCertificateStatus, got {certificate:?}"
        );
        assert_eq!(
            member(certificate, "CertificateArn"),
            arn,
            "case {case:?}: expected the ARN the request named, got {certificate:?}"
        );

        // And once what the clock owes has fallen due, the status it ends in.
        ses.due_certificate(identity, identity);
        let settled = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&settled.certificates()[0], "Status"),
            *settles_into,
            "case {case:?}: expected the association to end in {settles_into}, got {:?}",
            settled.body
        );
    }
}
