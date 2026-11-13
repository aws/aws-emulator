// SPDX-License-Identifier: CC0-1.0

//! A node handler that OPENS AN ENVELOPE: the key service's own client package, imported by
//! name from the runtime's bundled SDK, and the symmetric cipher the recovered key feeds.
//!
//! The rule pinned here: a managed Node.js runtime supplies the AWS SDK for JavaScript v3 for
//! every service the caller can reach, and a package of it exports that service's own modelled
//! surface — a `<Operation>Command` class per operation the service binds and an error class
//! per error shape it declares — so an ES module's `import { KMSClient, DecryptCommand } from
//! "@aws-sdk/client-kms"` LINKS and the command it binds makes the call. A member the service's
//! model types as a blob travels as the bytes the handler holds, in both directions: the bytes
//! a handler passes are what the service receives, and the bytes it answers arrive as a byte
//! array. Beside that, `crypto.createCipheriv`/`createDecipheriv` compute the real transform of
//! the AES cipher the call names, so a payload sealed under a key the key service recovered
//! opens again — and a cipher the runtime does not compute, a tag that does not verify and a
//! ciphertext no key sealed are each REFUSED rather than answered with plaintext.
//!
//! The rows differ along what the rule generalises over: the cipher (each mode this runtime
//! computes, at each AES key size), and then the ways opening fails. Nothing here asserts a
//! value baked into a service: the key ARN comes back from `CreateKey`, the data key is drawn
//! inside the run, and every refusal is read off the answer the service itself gave.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the handler's client is built
/// with.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` this function carries: a managed Node release that bundles the SDK.
const RUNTIME: &str = "nodejs22.x";

/// The `X-Amz-Target` prefix of the key service, which is its service shape's own name.
const KMS_TARGET_PREFIX: &str = "TrentService";

/// How long this suite waits for the function to become `Active`, and how often it looks
/// again: the transition is owed by the CLOCK, so the wait is for that CONDITION and the
/// deadline is generous — a loaded host may take far longer than an idle one.
const ACTIVE_BUDGET: Duration = Duration::from_secs(60);

/// How often the wait above looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The payload the handler seals and opens again.
const PAYLOAD: &str = "137492";

/// The additional data the authenticated rows bind their message to.
const CONTEXT: &str = "the message this code travels in";

/// Lambda and the key service over ONE shared engine, as `core/router/src/lib.rs` mounts them:
/// the handler's SDK call is dispatched back into this same router, so the key it reaches is
/// the one this suite created.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One key-service call that must answer 200.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("authorization", authorization("kms"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{KMS_TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    assert_eq!(
        parts.status.as_u16(),
        200,
        "KMS {operation} must answer 200, got {} with {parsed}",
        parts.status.as_u16()
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// A .zip deployment package holding `source` as `index.mjs` — one ES module, and nothing else:
/// no `node_modules`, so the `@aws-sdk/*` specifier in it is the runtime's to resolve.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("index.mjs", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a `nodejs22.x` function whose code is `source` and wait for it to become `Active`.
async fn create_function(router: &MultiServiceRouter, name: &str, source: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    // An invocation only reaches a function Lambda reports as `Active`, and that transition is
    // the CLOCK's: the wait is for the condition, with a generous deadline.
    let deadline = std::time::Instant::now() + ACTIVE_BUDGET;
    loop {
        let (_status, _headers, read) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &[],
        )
        .await;
        if read["State"].as_str() == Some("Active") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the function {name:?} never became Active within {ACTIVE_BUDGET:?}, got {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `Invoke` the function with `event`, answering the `FunctionError` signal the response
/// carried and the payload the run produced.
async fn invoke(
    router: &MultiServiceRouter,
    name: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (signal, payload)
}

/// The handler: for every row of the event it draws a data key, has the key service wrap and
/// unwrap it, and then seals and opens the payload under the recovered key with the cipher the
/// row names.
///
/// The import is the whole point of the first half: an ES module's named imports are bound when
/// the module is LINKED, so a package that exported its client alone would fail here before a
/// line of this ran.
const HANDLER: &str = r#"
import {
    KMSClient,
    EncryptCommand,
    DecryptCommand,
    InvalidCiphertextException,
    KMSServiceException,
} from "@aws-sdk/client-kms";
import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

const KEY_BYTES = { "128": 16, "192": 24, "256": 32 };

function ivBytes(cipher) {
    return cipher.endsWith("-gcm") ? 12 : 16;
}

async function recoveredKey(kms, keyId, cipher) {
    const key = randomBytes(KEY_BYTES[cipher.split("-")[1]]);
    const wrapped = await kms.send(new EncryptCommand({ KeyId: keyId, Plaintext: key }));
    const unwrapped = await kms.send(
        new DecryptCommand({ CiphertextBlob: wrapped.CiphertextBlob })
    );
    return { key, recovered: Buffer.from(unwrapped.Plaintext), wrapped };
}

function sealed(cipher, key, iv, aad, payload) {
    const sealing = createCipheriv(cipher, key, iv);
    if (cipher.endsWith("-gcm")) {
        sealing.setAAD(aad);
    }
    const bytes = Buffer.concat([
        sealing.update(Buffer.from(payload, "utf8")),
        sealing.final(),
    ]);
    return { bytes, tag: cipher.endsWith("-gcm") ? sealing.getAuthTag() : null };
}

function opened(cipher, key, iv, aad, bytes, tag) {
    const opening = createDecipheriv(cipher, key, iv);
    if (cipher.endsWith("-gcm")) {
        opening.setAAD(aad);
        opening.setAuthTag(tag);
    }
    return Buffer.concat([opening.update(bytes), opening.final()]).toString("utf8");
}

function reported(error) {
    return {
        ok: false,
        name: error.name,
        code: error.code === undefined ? null : error.code,
        message: error.message,
        isInvalidCiphertext: error instanceof InvalidCiphertextException,
        isServiceException: error instanceof KMSServiceException,
    };
}

export const handler = async (event) => {
    const kms = new KMSClient({ region: event.region });
    const aad = Buffer.from(event.context, "utf8");
    const rows = {};
    for (const row of event.rows) {
        try {
            if (row.kind === "foreign ciphertext") {
                await kms.send(
                    new DecryptCommand({ CiphertextBlob: randomBytes(96) })
                );
                rows[row.case] = { ok: true, opened: null };
                continue;
            }
            if (row.kind === "unknown cipher") {
                createDecipheriv(row.cipher, randomBytes(32), randomBytes(12));
                rows[row.case] = { ok: true, opened: null };
                continue;
            }
            const held = await recoveredKey(kms, event.keyId, row.cipher);
            const message = sealed(row.cipher, held.recovered, Buffer.from(row.iv, "hex"), aad, event.payload);
            if (row.kind === "altered tag") {
                const tag = Buffer.from(message.tag);
                tag[0] = tag[0] ^ 0xff;
                rows[row.case] = {
                    ok: true,
                    opened: opened(row.cipher, held.recovered, Buffer.from(row.iv, "hex"), aad, message.bytes, tag),
                };
                continue;
            }
            rows[row.case] = {
                ok: true,
                keyRecovered: held.recovered.equals(held.key),
                keyLength: held.recovered.length,
                wrappedIsBytes: held.wrapped.CiphertextBlob instanceof Uint8Array,
                wrappedKeyId: held.wrapped.KeyId,
                sealedBase64: message.bytes.toString("base64"),
                tagLength: message.tag === null ? 0 : message.tag.length,
                opened: opened(
                    row.cipher,
                    held.recovered,
                    Buffer.from(row.iv, "hex"),
                    aad,
                    message.bytes,
                    message.tag
                ),
            };
        } catch (error) {
            rows[row.case] = reported(error);
        }
    }
    return rows;
};
"#;

/// One row of the run: the case, the cipher it names, and which of the rule's halves it drives.
struct Row {
    case: &'static str,
    cipher: &'static str,
    kind: &'static str,
}

/// The rows, spanning what the rule generalises over.
///
/// The ciphers are the ones this runtime computes — every mode at every AES key size — read
/// from the runtime's own `crypto.getCiphers()` list rather than from any one call site; the
/// three refusal rows are the ways an open fails.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for cipher in [
        "aes-128-gcm",
        "aes-192-gcm",
        "aes-256-gcm",
        "aes-128-cbc",
        "aes-192-cbc",
        "aes-256-cbc",
        "aes-128-ctr",
        "aes-192-ctr",
        "aes-256-ctr",
    ] {
        rows.push(Row {
            case: cipher,
            cipher,
            kind: "round trip",
        });
    }
    rows.push(Row {
        case: "a tag of another message",
        cipher: "aes-256-gcm",
        kind: "altered tag",
    });
    rows.push(Row {
        case: "a cipher this runtime does not compute",
        cipher: "aes-256-ocb",
        kind: "unknown cipher",
    });
    rows.push(Row {
        case: "a ciphertext no key sealed",
        cipher: "aes-256-gcm",
        kind: "foreign ciphertext",
    });
    rows
}

/// The IV of one row: as many bytes as the mode takes, written out so the run is the same
/// whatever the draw inside it gives.
fn iv(cipher: &str) -> String {
    let length = if cipher.ends_with("-gcm") { 12 } else { 16 };
    (0..length)
        .map(|index| format!("{:02x}", 0x30_u8 + index as u8))
        .collect()
}

/// What the handler's report must be, read off the row.
#[tokio::test]
async fn a_node_handler_opens_an_envelope_the_key_service_unwrapped() {
    let router = router();
    let created = kms(
        &router,
        "CreateKey",
        &json!({ "Description": "the envelope a handler opens" }),
    )
    .await;
    let key_arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey must mint a key ARN, got {created}"))
        .to_string();
    create_function(&router, "envelope-opener", HANDLER).await;

    let rows = rows();
    let event = json!({
        "region": REGION,
        "keyId": key_arn,
        "payload": PAYLOAD,
        "context": CONTEXT,
        "rows": rows
            .iter()
            .map(|row| json!({
                "case": row.case,
                "cipher": row.cipher,
                "kind": row.kind,
                "iv": iv(row.cipher),
            }))
            .collect::<Vec<Value>>(),
    });
    let (signal, report) = invoke(&router, "envelope-opener", &event).await;
    assert_eq!(
        signal, None,
        "expected the run to answer, got the function error {signal:?} with {report}"
    );

    for row in rows {
        let held = &report[row.case];
        match row.kind {
            "round trip" => {
                assert_eq!(
                    held["ok"],
                    json!(true),
                    "case {:?}: expected the envelope to open, got {held}",
                    row.case
                );
                assert_eq!(
                    held["keyRecovered"],
                    json!(true),
                    "case {:?}: expected the key service to answer with the very bytes the \
                     handler wrapped, got {held}",
                    row.case
                );
                assert_eq!(
                    held["wrappedIsBytes"],
                    json!(true),
                    "case {:?}: expected the wrapped key to arrive as bytes, got {held}",
                    row.case
                );
                assert_eq!(
                    held["wrappedKeyId"].as_str(),
                    Some(key_arn.as_str()),
                    "case {:?}: expected the key the ciphertext was sealed under, got {held}",
                    row.case
                );
                assert_eq!(
                    held["opened"].as_str(),
                    Some(PAYLOAD),
                    "case {:?}: expected the payload back, got {held}",
                    row.case
                );
                assert_ne!(
                    held["sealedBase64"].as_str(),
                    Some(
                        base64::engine::general_purpose::STANDARD
                            .encode(PAYLOAD.as_bytes())
                            .as_str()
                    ),
                    "case {:?}: expected the payload to be enciphered, got the plaintext back: \
                     {held}",
                    row.case
                );
                let expected_tag = if row.cipher.ends_with("-gcm") { 16 } else { 0 };
                assert_eq!(
                    held["tagLength"],
                    json!(expected_tag),
                    "case {:?}: expected a {expected_tag}-byte tag, got {held}",
                    row.case
                );
            }
            // Each refusal row must report a refusal rather than a value: an open that did
            // not open answers with no plaintext at all.
            _ => {
                assert_eq!(
                    held["ok"],
                    json!(false),
                    "case {:?}: expected the call to be refused, got {held}",
                    row.case
                );
                assert!(
                    held["opened"].is_null(),
                    "case {:?}: expected no plaintext from a refusal, got {held}",
                    row.case
                );
            }
        }
    }

    // The refusal rows, each by WHAT it is: the names and codes are the ones the failure
    // carries, read where a JS consumer reads them.
    let altered = &report["a tag of another message"];
    assert_eq!(
        altered["message"].as_str(),
        Some("Unsupported state or unable to authenticate data"),
        "expected the message Node reports for a tag that does not verify, got {altered}"
    );
    let unknown = &report["a cipher this runtime does not compute"];
    assert_eq!(
        (unknown["name"].as_str(), unknown["code"].as_str()),
        (Some("TypeError"), Some("ERR_CRYPTO_UNKNOWN_CIPHER")),
        "expected a cipher this runtime does not compute to be refused by name, got {unknown}"
    );
    let foreign = &report["a ciphertext no key sealed"];
    assert_eq!(
        (
            foreign["name"].as_str(),
            foreign["isInvalidCiphertext"].as_bool(),
            foreign["isServiceException"].as_bool(),
        ),
        (
            Some("InvalidCiphertextException"),
            Some(true),
            Some(true)
        ),
        "expected the key service's own error shape for a ciphertext it did not seal, got \
         {foreign}"
    );
}
