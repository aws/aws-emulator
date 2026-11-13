// SPDX-License-Identifier: CC0-1.0

//! What every suite in this crate needs to drive the emulator: one emulator behind the
//! real router dispatch, and the HTTP call that reaches it.
//!
//! Factored out of the per-suite helpers so the suites that assert WHAT A MESSAGE SAYS
//! (`validation_message_rendering`, `not_found_message`) and the one that drives the
//! Conversions API (`conversion_events`) share one request path: a divergence between two
//! copies of the harness would read as a divergence in the service.
//!
//! It also builds the KEY MATERIAL the business-public-key suites submit
//! ([`rsa_public_key_pem`] and the structures beside it), so one builder serves every suite
//! that needs a real X.509 RSA public key rather than each inventing a PEM-shaped value.

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use service_com_amazonaws_socialmessaging_socialmessaging::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use spki::der::asn1::{AnyRef, BitStringRef, UintRef};
use spki::der::Encode;
use spki::{AlgorithmIdentifier, ObjectIdentifier, SubjectPublicKeyInfo};
use std::sync::Arc;

/// A SigV4 `Authorization` header scoped to this service, which is what the router
/// routes on (`emulator_server::resolve_route_key`) and what the handlers read the
/// ARN region from.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
                      Credential=AKIDEXAMPLE/20260101/us-east-1/social-messaging/aws4_request, \
                      SignedHeaders=host, Signature=0";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT_ID: &str = "000000000000";

/// The header the console's sign-up flow presents (`CONSOLE_CSRF_HEADER` in
/// `src/state.rs`), and a token value for it. `AssociateWhatsAppBusinessAccount` is served
/// through that flow alone, so every call that drives a sign-up goes through
/// [`console_call`]; a suite asserting the refusal calls [`call`], which presents nothing.
pub const CONSOLE_CSRF_HEADER: &str = "x-csrf-token";
/// A console CSRF token: any non-empty value is a request that came through the flow.
pub const CONSOLE_CSRF_TOKEN: &str = "console-csrf-token";

/// One emulator: a single shared engine behind the service, mounted in the router.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the shared engine beside it — for a suite that has to read or
/// set a fact of the account that no operation of this service writes.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so a suite does not depend on the process-wide auth switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The response of one call: HTTP status, the `x-amzn-errortype` the protocol carries on
/// a modelled failure, and the parsed JSON body.
pub struct Answer {
    pub status: u16,
    pub error_type: String,
    pub body: Value,
}

impl Answer {
    pub fn at(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    pub fn string_at(&self, pointer: &str) -> String {
        self.at(pointer).as_str().unwrap_or_default().to_string()
    }

    /// The message an error body carries, whichever casing the protocol rendered it in.
    pub fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    call_with_headers(router, method, uri, body, &[]).await
}

/// One call made the way the console's sign-up flow makes it: carrying the console CSRF
/// header, which is what a console-only operation admits.
pub async fn console_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    call_with_headers(
        router,
        method,
        uri,
        body,
        &[(CONSOLE_CSRF_HEADER, CONSOLE_CSRF_TOKEN)],
    )
    .await
}

/// One call carrying `headers` beside the ones every call in this crate sends.
pub async fn call_with_headers(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "social-messaging.us-east-1.amazonaws.com")
        .header("authorization", SIGNED);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        }
        None => Bytes::new(),
    };
    let request = builder.body(payload).expect("the request builds");
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
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            json!({ "__unparsed": String::from_utf8_lossy(&bytes).to_string() })
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Percent-encode a value for a query string, so an ARN-form `id` (which carries `:` and
/// `/`) survives the round trip. `NON_ALPHANUMERIC` is the conservative set the other
/// harnesses in this tree encode with (see
/// `services/service-com-amazonaws-accessanalyzer-accessanalyzer/tests/all/harness.rs`).
pub fn query_escape(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// The wire name this service's `InvalidParametersException` carries — the shape followed by
/// the locator of the namespace that declares it, which is the form this service's
/// parameter refusal was recorded naming (`src/service/error_type_locator.rs`).
pub const INVALID_PARAMETERS_ERROR_TYPE: &str =
    "InvalidParametersException:http://internal.amazon.com/coral/com.amazonaws.socialmessaging/";

/// The PEM label RFC 7468 section 13 gives an X.509 `SubjectPublicKeyInfo`, which is the
/// envelope the `businessPublicKey` member is documented to arrive in.
pub const PUBLIC_KEY_PEM_LABEL: &str = "PUBLIC KEY";

/// A deterministic RSA modulus of `bits` bits: the leading bit is set, so the integer
/// really is that wide and carries no leading zero byte.
///
/// The bytes are arbitrary — nothing here signs or encrypts with the key, and the service
/// judges the STRUCTURE of the key material, which is what these builders produce.
fn modulus(bits: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..bits / 8)
        .map(|index| u8::try_from(index % 251).expect("a byte") | 1)
        .collect();
    bytes[0] |= 0x80;
    bytes
}

/// The DER of a PKCS#1 `RSAPublicKey` with a `bits`-bit modulus and the usual 65537
/// exponent — the structure that lives inside an X.509 RSA `SubjectPublicKeyInfo`, and on
/// its own the PKCS#1 form rather than the X.509 one.
pub fn pkcs1_rsa_public_key_der(bits: usize) -> Vec<u8> {
    let modulus = modulus(bits);
    let key = pkcs1::RsaPublicKey {
        modulus: UintRef::new(&modulus).expect("a positive modulus"),
        public_exponent: UintRef::new(&[0x01, 0x00, 0x01]).expect("a positive exponent"),
    };
    key.to_der().expect("the RSA public key encodes")
}

/// `contents` enveloped as a PEM block under `label`.
pub fn pem_block(label: &str, contents: &[u8]) -> String {
    pem::encode(&pem::Pem::new(label.to_string(), contents.to_vec()))
}

/// The DER of an X.509 `SubjectPublicKeyInfo` naming `algorithm` and carrying `key`.
fn subject_public_key_info(
    algorithm: ObjectIdentifier,
    parameters: Option<AnyRef<'_>>,
    key: &[u8],
) -> Vec<u8> {
    let info = SubjectPublicKeyInfo {
        algorithm: AlgorithmIdentifier {
            oid: algorithm,
            parameters,
        },
        subject_public_key: BitStringRef::new(0, key).expect("a whole number of bytes"),
    };
    info.to_der().expect("the public key info encodes")
}

/// A PEM-encoded X.509 RSA public key with a `bits`-bit modulus: what the
/// `businessPublicKey` member is documented to carry ("The PEM-encoded 2048-bit RSA public
/// key to set").
pub fn rsa_public_key_pem(bits: usize) -> String {
    let key = pkcs1_rsa_public_key_der(bits);
    // `rsaEncryption` takes NULL parameters (RFC 4055); the OID itself comes from `pkcs1`.
    let info = subject_public_key_info(pkcs1::ALGORITHM_OID, Some(AnyRef::NULL), &key);
    pem_block(PUBLIC_KEY_PEM_LABEL, &info)
}

/// A PEM-encoded X.509 public key of ANOTHER algorithm: Ed25519, whose
/// `id-Ed25519` OID is `1.3.101.112` and whose key is 32 bytes with absent parameters
/// (RFC 8410) — a well-formed public key that is not an RSA one.
pub fn ed25519_public_key_pem() -> String {
    let info = subject_public_key_info(
        ObjectIdentifier::new_unwrap("1.3.101.112"),
        None,
        &[0x2a; 32],
    );
    pem_block(PUBLIC_KEY_PEM_LABEL, &info)
}

/// Start a signup and return the `associateInProgressToken` the callback issued — the
/// only token a `setupFinalization` can finalize.
pub async fn start_signup(router: &MultiServiceRouter) -> String {
    let callback = console_call(
        router,
        "POST",
        "/v1/whatsapp/signup",
        Some(json!({ "signupCallback": { "accessToken": "meta-access-token" } })),
    )
    .await;
    assert_eq!(
        callback.status, 200,
        "signupCallback: expected 200, got {} {:?}",
        callback.status, callback.body
    );
    callback.string_at("/signupCallbackResult/associateInProgressToken")
}

/// A setup-finalization body finalizing `token` and linking a new account with
/// `meta_numbers` registered under it.
pub fn setup_body(token: &str, waba_id: Option<&str>, meta_numbers: &[&str]) -> Value {
    let phone_numbers: Vec<Value> = meta_numbers
        .iter()
        .map(|id| json!({ "id": id, "twoFactorPin": "123456" }))
        .collect();
    let mut setup = json!({
        "associateInProgressToken": token,
        "phoneNumbers": phone_numbers,
    });
    if let Some(waba_id) = waba_id {
        setup["waba"] = json!({ "id": waba_id });
    }
    json!({ "setupFinalization": setup })
}

/// Link one account and return the id the service minted for it: a signup callback hands
/// back the token its finalization has to carry.
pub async fn link(router: &MultiServiceRouter, waba_id: &str, meta_numbers: &[&str]) -> String {
    let token = start_signup(router).await;
    let answer = console_call(
        router,
        "POST",
        "/v1/whatsapp/signup",
        Some(setup_body(&token, Some(waba_id), meta_numbers)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {waba_id:?}: expected 200 from AssociateWhatsAppBusinessAccount, got {} {:?}",
        answer.status, answer.body
    );
    answer.string_at("/linkedWhatsAppBusinessAccountId")
}
