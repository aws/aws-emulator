// SPDX-License-Identifier: CC0-1.0

//! The custom email sender trigger read by the handler it is written for: a `nodejs22.x` ES
//! module that OPENS the encrypted code with the runtime's own bundled SDK and `crypto`, and
//! forwards the plaintext to a queue.
//!
//! The rule pinned here: an operation that issues a code on a pool configuring a
//! `CustomEmailSender` answers its own 200 and hands the code to that function encrypted under
//! `LambdaConfig.KMSKeyID` — "Amazon Cognito uses the key to encrypt codes and temporary
//! passwords sent to `CustomEmailSender` and `CustomSMSSender`" (`LambdaConfigType$KMSKeyID`,
//! this service's model) — in the published message format a decrypting client parses. So a
//! function written the way AWS's own documentation writes one, against the SDK and the
//! `crypto` module the managed runtime supplies and NOTHING in its own package, recovers the
//! code: it asks the key service to unwrap the data key, derives the content key, opens the
//! message, and the plaintext it reports is the code the operation that CONSUMES the code
//! accepts. A code that is not the delivered one is refused `CodeMismatchException`, with the
//! wording of what it did not match, and a completed sign-in's identity token describes the
//! user the store holds.
//!
//! The rows differ along the stage that issued the code — a sign-up confirmation, a password
//! reset and an `EMAIL_OTP` sign-in challenge, each consumed by its own operation — because
//! the rule is the trigger's and not one operation's. Nothing here asserts a value baked into
//! a service: the key ARN, the queue URL, the function ARN, the pool id and the code itself
//! all come back from the state the calls created, and the code is only ever read out of what
//! the function reported.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The local name of Cognito's Smithy shape, which is the `X-Amz-Target` prefix.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The `X-Amz-Target` prefix of the key service.
const KMS_TARGET_PREFIX: &str = "TrentService";

/// The `X-Amz-Target` prefix of Amazon SQS.
const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// Where Lambda's `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying Lambda's `Role` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The password the reset row sets.
const NEW_PASSWORD: &str = "Passw0rd!2025";

/// How long this suite waits for a message the trigger function forwarded. Generous: the
/// wait is for the message, and a loaded host takes far longer than an idle one.
const DELIVERY_WAIT: Duration = Duration::from_secs(60);

/// How long a poll of the queue waits for a message to appear.
const RECEIVE_WAIT_SECONDS: u64 = 1;

/// How long this suite waits for the trigger function to become `Active`, and how often it
/// looks again. Generous for the same reason.
const ACTIVE_BUDGET: Duration = Duration::from_secs(60);

/// How often that wait looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Cognito, the key service, Lambda and Amazon SQS over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
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

/// One `awsJson` call to the service whose signing name is `service`, addressed by
/// `X-Amz-Target`.
async fn json_call(
    router: &MultiServiceRouter,
    service: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header("authorization", authorization(service))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Cognito call, as its status and parsed body.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_call(
        router,
        "cognito-idp",
        "application/x-amz-json-1.1",
        &format!("{COGNITO_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await
}

/// A Cognito call that must answer 200, with its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = cognito(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// One key-service call that must answer 200.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = json_call(
        router,
        "kms",
        "application/x-amz-json-1.1",
        &format!("{KMS_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "KMS {operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// One Amazon SQS call that must answer 200.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = json_call(
        router,
        "sqs",
        "application/x-amz-json-1.0",
        &format!("{SQS_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "SQS {operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// A .zip deployment package holding `source` as `index.mjs`, and nothing else: no
/// `node_modules`, so every `@aws-sdk/*` and `node:` specifier in it is the runtime's to
/// resolve.
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

/// Create the `nodejs22.x` trigger function and return the ARN Lambda minted for it.
///
/// The function is created and then waited FOR: an invocation only reaches a function Lambda
/// reports as `Active`, and that transition is the clock's, so the wait is for that condition
/// with a generous deadline.
async fn create_function(router: &MultiServiceRouter, name: &str, source: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri(FUNCTIONS_URI)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(
            serde_json::to_vec(&json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "nodejs22.x",
                "Handler": "index.handler",
                "Timeout": 60,
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
                },
            }))
            .expect("the create body serializes"),
        ))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let created: Value = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 201,
        "CreateFunction must answer 201, got {status} with {created}"
    );
    let deadline = Instant::now() + ACTIVE_BUDGET;
    loop {
        let read = lambda_get(router, &format!("{FUNCTIONS_URI}/{name}/configuration")).await;
        if read["State"].as_str() == Some("Active") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the function {name:?} never became Active within {ACTIVE_BUDGET:?}, got {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction must mint an ARN, got {created}"))
        .to_string()
}

/// One Lambda read, as its parsed body.
async fn lambda_get(router: &MultiServiceRouter, uri: &str) -> Value {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null)
}

/// The handler the trigger function runs: the function AWS's own documentation describes for
/// this trigger, written against the runtime alone.
///
/// It reads the published message the code travels in — the version, the algorithm suite, the
/// message id, the one encrypted data key with the key that wrapped it, and the single framed
/// body — asks the key service to unwrap that data key, derives the content key the suite
/// states, checks the header's own tag and opens the frame. Every name it imports comes from
/// the runtime: the key service's client package and `node:crypto`.
fn handler_source(queue_url: &str) -> String {
    format!(
        r#"
import {{ KMSClient, DecryptCommand }} from "@aws-sdk/client-kms";
import {{ SQSClient, SendMessageCommand }} from "@aws-sdk/client-sqs";
import {{ createDecipheriv, hkdfSync }} from "node:crypto";

const QUEUE_URL = "{queue_url}";
const REGION = "{REGION}";

const kms = new KMSClient({{ region: REGION }});
const sqs = new SQSClient({{ region: REGION }});

const TAG_BYTES = 16;
const ID_BYTES = 32;
const NONCE_BYTES = 12;
const FINAL_FRAME = "AWSKMSEncryptionClient Final Frame";
const DERIVE_KEY = "DERIVEKEY";

async function decryptCode(encoded) {{
    const message = Buffer.from(encoded, "base64");
    let at = 0;
    const take = (count) => {{
        const part = Buffer.from(message.subarray(at, at + count));
        at += count;
        return part;
    }};
    const u16 = () => {{
        const value = message.readUInt16BE(at);
        at += 2;
        return value;
    }};
    const u32 = () => {{
        const value = message.readUInt32BE(at);
        at += 4;
        return value;
    }};
    take(1);                            // the message format version
    const suite = u16();                // the algorithm suite the message is under
    const messageId = take(ID_BYTES);
    take(u16());                        // the encryption context, if the message carries one
    u16();                              // how many encrypted data keys follow
    take(u16());                        // the key provider
    const providerInfo = take(u16()).toString("utf8");
    const encryptedDataKey = take(u16());
    take(1);                            // the content type
    u32();                              // the frame length
    take(ID_BYTES);                     // the key commitment
    const header = Buffer.from(message.subarray(0, at));
    const headerTag = take(TAG_BYTES);
    u32();                              // the final-frame marker
    const sequence = u32();
    const iv = take(NONCE_BYTES);
    const contentLength = u32();
    const frame = Buffer.from(message.subarray(at));

    const unwrapped = await kms.send(new DecryptCommand({{
        CiphertextBlob: encryptedDataKey,
        KeyId: providerInfo,
    }}));
    const dataKey = Buffer.from(unwrapped.Plaintext);
    const suiteBytes = Buffer.alloc(2);
    suiteBytes.writeUInt16BE(suite, 0);
    const contentKey = Buffer.from(hkdfSync(
        "sha512",
        dataKey,
        messageId,
        Buffer.concat([suiteBytes, Buffer.from(DERIVE_KEY, "utf8")]),
        32
    ));

    const headerCheck = createDecipheriv("aes-256-gcm", contentKey, Buffer.alloc(NONCE_BYTES));
    headerCheck.setAAD(header);
    headerCheck.setAuthTag(headerTag);
    headerCheck.update(Buffer.alloc(0));
    headerCheck.final();

    const sequenceBytes = Buffer.alloc(4);
    sequenceBytes.writeUInt32BE(sequence, 0);
    const lengthBytes = Buffer.alloc(8);
    lengthBytes.writeBigUInt64BE(BigInt(contentLength), 0);
    const body = createDecipheriv("aes-256-gcm", contentKey, iv);
    body.setAAD(Buffer.concat([
        messageId,
        Buffer.from(FINAL_FRAME, "utf8"),
        sequenceBytes,
        lengthBytes,
    ]));
    body.setAuthTag(Buffer.from(frame.subarray(frame.length - TAG_BYTES)));
    return Buffer.concat([
        body.update(Buffer.from(frame.subarray(0, frame.length - TAG_BYTES))),
        body.final(),
    ]).toString("utf8");
}}

export const handler = async (event) => {{
    let report;
    try {{
        report = {{
            ok: true,
            code: await decryptCode(event.request.code),
            triggerSource: event.triggerSource,
            userName: event.userName,
            type: event.request.type,
            hasResponse: Object.keys(event).includes("response"),
        }};
    }} catch (error) {{
        report = {{
            ok: false,
            name: error.name,
            message: error.message,
            stack: error.stack,
        }};
    }}
    await sqs.send(new SendMessageCommand({{
        QueueUrl: QUEUE_URL,
        MessageBody: JSON.stringify(report),
    }}));
    return {{}};
}};
"#
    )
}

/// One message the trigger function forwarded, waited for until [`DELIVERY_WAIT`] is spent.
async fn next_report(router: &MultiServiceRouter, case: &str, queue_url: &str) -> Value {
    let deadline = Instant::now() + DELIVERY_WAIT;
    while Instant::now() < deadline {
        let received = sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": queue_url,
                "MaxNumberOfMessages": 1,
                "WaitTimeSeconds": RECEIVE_WAIT_SECONDS,
            }),
        )
        .await;
        if let Some(body) = received["Messages"]
            .as_array()
            .and_then(|messages| messages.first())
            .and_then(|first| first["Body"].as_str())
        {
            return serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("case {case:?}: the report is not JSON ({error}): {body}")
            });
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: no report from the trigger function reached the queue in {DELIVERY_WAIT:?}");
}

/// The code the function reported for `case`, with the trigger source it was handed.
///
/// `digits` is how many decimal digits the issuing stage's code carries: a verification code
/// and an email one-time password are each a fixed-length decimal code, of their own length.
fn reported_code(case: &str, report: &Value, source: &str, digits: usize) -> String {
    assert_eq!(
        report["ok"], json!(true),
        "case {case:?}: expected the function to open the code it was handed, got {report}"
    );
    assert_eq!(
        report["triggerSource"].as_str(),
        Some(source),
        "case {case:?}: expected the issuing stage's own trigger source, got {report}"
    );
    assert_eq!(
        report["type"].as_str(),
        Some("customEmailSenderRequestV1"),
        "case {case:?}: expected the trigger's own request type, got {report}"
    );
    let code = report["code"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a code in the report, got {report}"));
    assert!(
        code.len() == digits && code.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected the {digits} digits this stage's code carries, got {code:?}"
    );
    code.to_string()
}

/// The claims of a compact-serialized JWT, read the way any consumer of one reads them: the
/// middle segment, base64url-decoded.
fn jwt_claims(token: &str) -> Value {
    let segment = token
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("a JWT carries three segments, got {token:?}"));
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment.trim_end_matches('='))
        .unwrap_or_else(|error| panic!("the claims are base64url ({error}): {segment}"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("the claims are a JSON document ({error})"))
}

/// The error code of an error body (`__type`), without its namespace, and its message.
fn refusal(body: &Value) -> (String, String) {
    (
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string(),
        body.get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// RULE: the code a custom sender is handed is the code the consuming operation accepts, and
/// the function recovers it with the runtime's own SDK and `crypto` alone.
///
/// Three stages, each consumed by its own operation: a sign-up confirmation, a password reset
/// and an `EMAIL_OTP` sign-in challenge. Each is also driven with a code that is not the
/// delivered one, which must be refused.
#[tokio::test]
async fn a_node_handler_opens_the_code_the_custom_sender_was_handed() {
    let router = router();
    let queue = sqs(
        &router,
        "CreateQueue",
        &json!({ "QueueName": "custom-sender-reports" }),
    )
    .await;
    let queue_url = queue["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue must mint a URL, got {queue}"))
        .to_string();
    let created = kms(
        &router,
        "CreateKey",
        &json!({ "Description": "the custom sender's key" }),
    )
    .await;
    let key_arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey must mint a key ARN, got {created}"))
        .to_string();
    let function_arn =
        create_function(&router, "custom-sender", &handler_source(&queue_url)).await;

    let pool = ok(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": "node-custom-sender",
            "AutoVerifiedAttributes": ["email"],
            "Policies": {
                "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] }
            },
            "LambdaConfig": {
                "CustomEmailSender": { "LambdaVersion": "V1_0", "LambdaArn": function_arn },
                "KMSKeyID": key_arn,
            },
        }),
    )
    .await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {pool}"))
        .to_string();
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "node-custom-sender-app",
            "ExplicitAuthFlows": [
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_USER_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
            ],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {client}"))
        .to_string();

    // The sign-up stage: the operation answers its own 200 and the code reaches the function.
    let case = "SignUp";
    let username = "node-reader";
    let address = "node-reader@example.test";
    let signed_up = ok(
        &router,
        "SignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "UserAttributes": [{ "Name": "email", "Value": address }],
        }),
    )
    .await;
    assert_eq!(
        signed_up["UserConfirmed"],
        json!(false),
        "case {case:?}: expected the user to await its confirmation, got {signed_up}"
    );
    assert!(
        signed_up["UserSub"].as_str().is_some_and(|sub| !sub.is_empty()),
        "case {case:?}: expected the user's own subject, got {signed_up}"
    );
    assert_eq!(
        signed_up["CodeDeliveryDetails"]["AttributeName"].as_str(),
        Some("email"),
        "case {case:?}: expected the delivery of the address's own code, got {signed_up}"
    );
    let report = next_report(&router, case, &queue_url).await;
    assert_eq!(
        report["userName"].as_str(),
        Some(username),
        "case {case:?}: expected the event to name the user, got {report}"
    );
    assert_eq!(
        report["hasResponse"],
        json!(false),
        "case {case:?}: a sender delivers the message rather than amending the operation, so \
         the event carries no response half, got {report}"
    );
    let code = reported_code(case, &report, "CustomEmailSender_SignUp", 6);

    let (status, refused) = cognito(
        &router,
        "ConfirmSignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": "000000",
        }),
    )
    .await;
    assert_eq!(
        (status, refusal(&refused)),
        (
            400,
            (
                "CodeMismatchException".to_string(),
                "Invalid verification code provided, please try again.".to_string()
            )
        ),
        "case {case:?}: expected a code that is not the delivered one to be refused, got \
         {status} with {refused}"
    );
    ok(
        &router,
        "ConfirmSignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": code,
        }),
    )
    .await;

    // The password-reset stage: the same rule, a different issuing operation and a different
    // consumer.
    let case = "ForgotPassword";
    ok(
        &router,
        "ForgotPassword",
        &json!({ "ClientId": client_id, "Username": username }),
    )
    .await;
    let reset_report = next_report(&router, case, &queue_url).await;
    let reset_code = reported_code(case, &reset_report, "CustomEmailSender_ForgotPassword", 6);
    assert_ne!(
        reset_code, code,
        "case {case:?}: expected a fresh code, got the one the sign-up delivered"
    );
    let (status, refused) = cognito(
        &router,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": "000000",
            "Password": NEW_PASSWORD,
        }),
    )
    .await;
    assert_eq!(
        (status, refusal(&refused)),
        (
            400,
            (
                "CodeMismatchException".to_string(),
                "Invalid verification code provided, please try again.".to_string()
            )
        ),
        "case {case:?}: expected a code that is not the delivered one to be refused, got \
         {status} with {refused}"
    );
    ok(
        &router,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": reset_code,
            "Password": NEW_PASSWORD,
        }),
    )
    .await;

    // The reset code really changed the password, which is what makes the delivered code the
    // one the flow turns on.
    ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": NEW_PASSWORD },
        }),
    )
    .await;

    // The sign-in challenge stage: the one-time password of an `EMAIL_OTP` challenge travels
    // to the same function, and the response that carries it back is what mints the tokens.
    let case = "EMAIL_OTP";
    let challenge = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_AUTH",
            "AuthParameters": { "USERNAME": username, "PREFERRED_CHALLENGE": "EMAIL_OTP" },
        }),
    )
    .await;
    assert_eq!(
        challenge["ChallengeName"].as_str(),
        Some("EMAIL_OTP"),
        "case {case:?}: expected the preferred challenge to be the one started, got {challenge}"
    );
    let session = challenge["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a challenge carries a session, got {challenge}"))
        .to_string();
    let otp_report = next_report(&router, case, &queue_url).await;
    let otp = reported_code(case, &otp_report, "CustomEmailSender_Authentication", 8);

    // A code the challenge did not deliver does not match the authentication state the
    // response carries, which is what this refusal reports — the sentence of the state, not
    // the one the operations consuming a delivered verification code carry.
    let (status, refused) = cognito(
        &router,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": session,
            "ChallengeResponses": { "USERNAME": username, "EMAIL_OTP_CODE": "00000000" },
        }),
    )
    .await;
    assert_eq!(
        (status, refusal(&refused)),
        (
            400,
            (
                "CodeMismatchException".to_string(),
                "Invalid code or auth state for the user.".to_string()
            )
        ),
        "case {case:?}: expected a code that is not the delivered one to be refused, got \
         {status} with {refused}"
    );
    let signed_in = ok(
        &router,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": session,
            "ChallengeResponses": { "USERNAME": username, "EMAIL_OTP_CODE": otp },
        }),
    )
    .await;

    // The identity token the completed sign-in minted describes the user the store holds: its
    // own attributes are its claims, with the verification flag as the boolean it states.
    let claims = jwt_claims(
        signed_in["AuthenticationResult"]["IdToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an identity token, got {signed_in}")),
    );
    assert_eq!(
        (
            claims["token_use"].as_str(),
            claims["cognito:username"].as_str(),
            claims["email"].as_str(),
            claims["email_verified"].as_bool(),
        ),
        (Some("id"), Some(username), Some(address), Some(true)),
        "case {case:?}: expected the identity token to describe the confirmed, verified user, \
         got {claims}"
    );
}
