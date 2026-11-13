// SPDX-License-Identifier: CC0-1.0

//! The CRYPTOGRAPHIC surface a `nodejs*` handler reaches of its runtime, beside the digests:
//! the key a digest derives, the symmetric transform under a derived key, the same transform
//! through the WHATWG interface, and the 64-bit readings a framed binary format needs.
//!
//! The rule pinned here: a managed Node.js runtime's `crypto` module computes what the call
//! names. `hkdfSync` answers the extract-and-expand construction's own key, so a key a
//! handler derives is the key any other implementation of that construction derives from the
//! same inputs; `createCipheriv`/`createDecipheriv` and `crypto.webcrypto.subtle`'s
//! `encrypt`/`decrypt` compute the AES transform the call names, so what one sealed the other
//! opens and a message that does not authenticate is REFUSED rather than answered with
//! plaintext; and a `Buffer` reads and writes a 64-bit field exactly, which a value past the
//! integers a double holds requires. A handler that is handed an envelope-encrypted payload
//! stands on exactly these.
//!
//! The rows differ along what each rule generalises over: the published vectors of the
//! derivation, the interface the transform is reached through, and the magnitude of a 64-bit
//! field. Every row RETURNS a computed value, which is what proves the computation happened.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` this function carries.
const RUNTIME: &str = "nodejs22.x";

/// Lambda alone: nothing here reaches another service.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names Lambda, which is what routes the
/// request. The signature is never checked here.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
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
        .header("authorization", authorization())
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

/// How long this suite waits for the function to become `Active`, and how often it looks
/// again: the transition is owed by the CLOCK, so the wait is for that CONDITION and the
/// deadline is generous — a loaded host may take far longer than an idle one.
const ACTIVE_BUDGET: Duration = Duration::from_secs(60);

/// How often the wait above looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Wait until `name` reports `State` `Active`, which is when an invocation reaches it.
async fn await_active(router: &MultiServiceRouter, name: &str) {
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

/// A .zip deployment package holding `source` as `index.mjs`, and nothing else.
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

/// The handler: it runs every probe of the event and reports what each computed, so one run
/// answers the whole table.
const HANDLER: &str = r#"
import {
    createCipheriv,
    createDecipheriv,
    getCiphers,
    hkdf,
    hkdfSync,
    webcrypto,
} from "node:crypto";

const PROBES = {
    // RFC 5869's first published test vector of the construction, which fixes the extract
    // and the expand steps and the output length together.
    "the published derivation vector": () => Buffer.from(hkdfSync(
        "sha256",
        Buffer.from("0b".repeat(22), "hex"),
        Buffer.from("000102030405060708090a0b0c", "hex"),
        Buffer.from("f0f1f2f3f4f5f6f7f8f9", "hex"),
        42
    )).toString("hex"),

    // The same derivation with another digest answers another key: the digest is a parameter
    // of the construction and not a decoration.
    "another digest derives another key": () => {
        const inputs = [
            Buffer.from("0b".repeat(22), "hex"),
            Buffer.from("000102030405060708090a0b0c", "hex"),
            Buffer.from("f0f1f2f3f4f5f6f7f8f9", "hex"),
            32,
        ];
        const first = Buffer.from(hkdfSync("sha256", ...inputs)).toString("hex");
        const other = Buffer.from(hkdfSync("sha512", ...inputs)).toString("hex");
        return first !== other;
    },

    // The callback form answers the same key the synchronous one does.
    "the callback form of the derivation": () => new Promise((resolve, reject) => {
        hkdf("sha256", "secret", "salt", "info", 16, (error, derived) => {
            if (error) {
                reject(error);
                return;
            }
            resolve(Buffer.from(derived).toString("hex")
                === Buffer.from(hkdfSync("sha256", "secret", "salt", "info", 16)).toString("hex"));
        });
    }),

    // The construction derives at most 255 times the digest's own length, so a longer
    // request is out of range rather than an allocation.
    "a derived length past the construction's ceiling": () => {
        try {
            hkdfSync("sha256", "secret", "salt", "info", 255 * 32 + 1);
            return null;
        } catch (error) {
            return error.code;
        }
    },

    // A derived key feeds the cipher: what the node interface sealed, it opens again.
    "a derived key seals and opens a payload": () => {
        const key = Buffer.from(hkdfSync("sha512", "data-key", "message-id", "DERIVEKEY", 32));
        const iv = Buffer.from("0a0b0c0d0e0f00010203040506", "hex").subarray(0, 12);
        const aad = Buffer.from("the context", "utf8");
        const sealing = createCipheriv("aes-256-gcm", key, iv);
        sealing.setAAD(aad);
        const sealed = Buffer.concat([sealing.update(Buffer.from("137492", "utf8")), sealing.final()]);
        const opening = createDecipheriv("aes-256-gcm", key, iv);
        opening.setAAD(aad);
        opening.setAuthTag(sealing.getAuthTag());
        return Buffer.concat([opening.update(sealed), opening.final()]).toString("utf8");
    },

    // A counter mode is a stream, so `update` hands over the bytes of what has arrived —
    // which is what a handler that calls it alone reads.
    "a counter mode hands its bytes over as they arrive": () => {
        const key = Buffer.from(hkdfSync("sha256", "stream-key", "salt", "info", 32));
        const iv = Buffer.from("00112233445566778899aabbccddeeff", "hex");
        const text = "the whole message, in two parts";
        const sealing = createCipheriv("aes-256-ctr", key, iv);
        const first = sealing.update(Buffer.from(text.slice(0, 11), "utf8"));
        const second = sealing.update(Buffer.from(text.slice(11), "utf8"));
        const sealed = Buffer.concat([first, second]);
        const opening = createDecipheriv("aes-256-ctr", key, iv);
        return {
            lengths: [first.length, second.length, sealing.final().length],
            opened: opening.update(sealed).toString("utf8"),
        };
    },

    // A mode that authenticates nothing has nowhere to carry additional data, so being
    // given some is a state the cipher cannot be in.
    "additional data on a mode that authenticates nothing": () => {
        try {
            createCipheriv("aes-256-cbc", Buffer.alloc(32), Buffer.alloc(16))
                .setAAD(Buffer.from("bound", "utf8"));
            return null;
        } catch (error) {
            return error.code;
        }
    },

    // The WHATWG interface over the same transform: a ciphertext it produces carries the tag
    // appended, and it opens its own.
    "the web interface seals and opens a payload": async () => {
        const raw = Buffer.from(hkdfSync("sha256", "web-key", "salt", "info", 32));
        const key = await webcrypto.subtle.importKey("raw", raw, "AES-GCM", true, [
            "encrypt",
            "decrypt",
        ]);
        const iv = Buffer.from("102030405060708090a0b0c0", "hex");
        const parameters = { name: "AES-GCM", iv, additionalData: Buffer.from("bound", "utf8") };
        const sealed = await webcrypto.subtle.encrypt(parameters, key, Buffer.from("851274", "utf8"));
        const opened = await webcrypto.subtle.decrypt(parameters, key, sealed);
        return {
            text: Buffer.from(opened).toString("utf8"),
            tagAppended: sealed.byteLength === "851274".length + 16,
            exported: Buffer.from(await webcrypto.subtle.exportKey("raw", key)).equals(raw),
        };
    },

    // A message the web interface cannot authenticate is refused by what that is.
    "the web interface refuses a message it cannot authenticate": async () => {
        const raw = Buffer.from(hkdfSync("sha256", "web-key", "salt", "info", 32));
        const key = await webcrypto.subtle.importKey("raw", raw, "AES-GCM", false, ["decrypt"]);
        const iv = Buffer.from("102030405060708090a0b0c0", "hex");
        try {
            await webcrypto.subtle.decrypt(
                { name: "AES-GCM", iv },
                key,
                Buffer.alloc(32)
            );
            return null;
        } catch (error) {
            return error.name;
        }
    },

    // A 64-bit field reads back exactly, including a value past the integers a double holds.
    "a 64-bit field round trips": () => {
        const held = Buffer.alloc(8);
        held.writeBigUInt64BE(BigInt("9007199254740993"), 0);
        return {
            read: held.readBigUInt64BE(0).toString(),
            bytes: held.toString("hex"),
            little: (() => {
                const other = Buffer.alloc(8);
                other.writeBigUInt64LE(BigInt(258), 0);
                return other.readBigUInt64LE(0).toString();
            })(),
        };
    },

    // What the runtime reports it computes is what it computes.
    "the ciphers the runtime reports": () => getCiphers().join(","),
};

export const handler = async (event) => {
    const answers = {};
    for (const probe of event.probes) {
        try {
            answers[probe] = await PROBES[probe]();
        } catch (error) {
            answers[probe] = { failed: String(error && error.message) };
        }
    }
    return answers;
};
"#;

/// RULE: the runtime's `crypto` computes what the call names — the derivation, the cipher
/// through either interface, and the 64-bit readings beside them.
#[tokio::test]
async fn a_node_handlers_crypto_computes_what_it_names() {
    let router = router();
    let (status, _headers, created) = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": "crypto-surface",
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HANDLER)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    await_active(&router, "crypto-surface").await;

    // Each row: the probe, and what it must compute.
    let rows: &[(&str, Value)] = &[
        (
            "the published derivation vector",
            json!(
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
            ),
        ),
        ("another digest derives another key", json!(true)),
        ("the callback form of the derivation", json!(true)),
        (
            "a derived length past the construction's ceiling",
            json!("ERR_OUT_OF_RANGE"),
        ),
        ("a derived key seals and opens a payload", json!("137492")),
        (
            "a counter mode hands its bytes over as they arrive",
            json!({
                "lengths": [11, 20, 0],
                "opened": "the whole message, in two parts",
            }),
        ),
        (
            "additional data on a mode that authenticates nothing",
            json!("ERR_CRYPTO_INVALID_STATE"),
        ),
        (
            "the web interface seals and opens a payload",
            json!({ "text": "851274", "tagAppended": true, "exported": true }),
        ),
        (
            "the web interface refuses a message it cannot authenticate",
            json!("OperationError"),
        ),
        (
            "a 64-bit field round trips",
            json!({
                "read": "9007199254740993",
                "bytes": "0020000000000001",
                "little": "258",
            }),
        ),
        (
            "the ciphers the runtime reports",
            json!(
                "aes-128-gcm,aes-192-gcm,aes-256-gcm,aes-128-cbc,aes-192-cbc,aes-256-cbc,\
                 aes-128-ctr,aes-192-ctr,aes-256-ctr"
            ),
        ),
    ];
    let (status, headers, answers) = lambda(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/crypto-surface/invocations"),
        json!({ "probes": rows.iter().map(|(probe, _)| *probe).collect::<Vec<&str>>() })
            .to_string()
            .as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from Invoke, got {status} ({answers})"
    );
    assert!(
        headers.get("x-amz-function-error").is_none(),
        "expected the run to answer, got the function error {:?} with {answers}",
        headers.get("x-amz-function-error")
    );
    for (probe, expected) in rows {
        let held = &answers[*probe];
        assert_eq!(
            held, expected,
            "case {probe:?}: expected {expected}, got {held}"
        );
    }
}
