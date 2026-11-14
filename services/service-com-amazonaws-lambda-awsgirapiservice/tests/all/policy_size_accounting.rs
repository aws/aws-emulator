// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the size a resource-policy grant is REFUSED over: what the
//! `PolicyLengthExceededException` states, and what the published 20 KB quota is applied
//! to.
//!
//! The rule under test: the figure is the final policy's MEASURED size — the bytes of the
//! document the store would hold (the `Version`/`Id` envelope, every statement already in
//! it and the statement being added) plus the fixed per-statement cost the policy-size
//! accounting counts on top of each statement's own compact JSON — and the grant that
//! pushes that measure past the quota is the one refused, with that same measure named in
//! the message.
//!
//! It is asserted over a table whose rows differ in how large each statement is, so the
//! refusal lands after a different number of grants in each: a row of four big statements
//! and a row of dozens of small ones measure different distances past the document a read
//! gives back, which is what tells a per-statement cost apart from a flat one. The last
//! test drives the same accounting through the layer-version grant, the other operation
//! that refuses over this quota.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction`, `AddPermission` and `GetPolicy` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here. Nothing in it is verified: the harness turns
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The published quota on a resource-based policy, in bytes (Lambda quotas, "Function
/// resource-based policy": 20 KB).
const MAX_POLICY_SIZE_BYTES: usize = 20_480;

/// What the policy-size accounting counts for every statement the final policy holds,
/// beyond that statement's own compact JSON. Named here rather than imported so the test
/// pins the figure the service must measure with.
///
/// Where the figure comes from: the recorded refusal of the corpus unit
/// `lam-err-policy-length-exceeded` states a final size of 23993 for a five-statement
/// policy whose compact document — the one the same fixture's recorded read reports, byte
/// for byte — is 23968 bytes. Change this only against a recording, and prefer one over a
/// policy of a different statement count, which is what would settle the per-statement
/// reading of those 25 bytes.
const STATEMENT_OVERHEAD_BYTES: usize = 5;

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// A fresh router on its own engine, reached the way the production router reaches it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Create a function to grant permissions on.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Publish one version of a layer to grant permissions on.
async fn publish_layer_version(router: &MultiServiceRouter, name: &str) {
    let bytes: Vec<u8> = (0..96u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        &format!("/2018-10-31/layers/{name}/versions"),
        &json!({
            "Content": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&bytes) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "PublishLayerVersion {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The size a refusal states, when the answer is the policy-quota refusal.
///
/// It also checks the fixed text around the two variables, so a row that reports the
/// figure in another sentence fails here rather than silently parsing to nothing.
fn stated_size(case: &str, answer: &Answer) -> usize {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected HTTP 400 past the quota, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains("PolicyLengthExceededException"),
        "case {case:?}: expected PolicyLengthExceededException, got {}",
        answer.error_code()
    );
    let stated = answer.string("message").unwrap_or_default();
    let size = stated
        .strip_prefix("The final policy size (")
        .and_then(|rest| rest.split_once(')'))
        .and_then(|(size, tail)| {
            (tail == format!(" is bigger than the limit ({MAX_POLICY_SIZE_BYTES})."))
                .then(|| size.parse::<usize>().ok())
                .flatten()
        });
    size.unwrap_or_else(|| {
        panic!("case {case:?}: expected the final-policy-size refusal, got {stated:?}")
    })
}

/// The size the refusal states is the final document's bytes plus the per-statement cost
/// for every statement in it — over statements of four different sizes, so the refusal
/// lands at a different statement count in each row.
#[tokio::test]
async fn the_stated_size_counts_the_final_document_and_every_statement_in_it() {
    // Each row's padding sets how big one statement is, and so how many fit: the first row
    // crosses the quota after a handful of grants, the last after dozens of them. The
    // padding stays inside AddPermission's own 5 KB request quota.
    let cases: &[(&str, usize)] = &[
        ("statements of about 4.4 KB", 4_400),
        ("statements of about 2.4 KB", 2_400),
        ("statements of about 0.9 KB", 900),
        ("statements of about 0.15 KB", 150),
    ];
    let mut counts = Vec::new();
    for (case, padding) in cases {
        let router = router();
        create_function(&router, "bulky").await;
        // Fixed-width statement ids, so every statement renders to the same number of
        // bytes and the arithmetic below needs one statement's size only.
        let body = |index: usize| {
            json!({
                "StatementId": format!("stmt-{index:04}"),
                "Action": "lambda:InvokeFunction",
                "Principal": "s3.amazonaws.com",
                "SourceArn": format!("arn:aws:s3:::{}", "b".repeat(*padding)),
            })
        };
        let mut accepted = 0usize;
        let mut statement_sizes = Vec::new();
        let mut refusal = None;
        for index in 0..300usize {
            let answer = call(
                &router,
                "POST",
                &format!("{FUNCTIONS_URI}/bulky/policy"),
                &body(index),
            )
            .await;
            if answer.status == 201 {
                accepted += 1;
                statement_sizes.push(
                    answer
                        .string("Statement")
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: no Statement member in {}", answer.text())
                        })
                        .len(),
                );
                continue;
            }
            refusal = Some(answer);
            break;
        }
        let refused = refusal.unwrap_or_else(|| {
            panic!("case {case:?}: the quota must be crossed, {accepted} grants accepted")
        });
        assert!(
            accepted > 1,
            "case {case:?}: expected several statements to fit inside the quota, got {accepted}"
        );
        let statement_bytes = statement_sizes.first().copied().unwrap_or_default();
        assert!(
            statement_sizes
                .iter()
                .all(|bytes| *bytes == statement_bytes),
            "case {case:?}: every statement of a row must render to the same size, got \
             {statement_sizes:?}"
        );

        // What the read gives back is the document the accepted grants left: the refused
        // grant changed nothing.
        let document = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/bulky/policy"),
            &Value::Null,
        )
        .await
        .string("Policy")
        .unwrap_or_else(|| panic!("case {case:?}: the read must report the stored document"));
        let held = serde_json::from_str::<Value>(&document)
            .ok()
            .and_then(|parsed| {
                parsed
                    .get("Statement")
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or_default();
        assert_eq!(
            held, accepted,
            "case {case:?}: expected the {accepted} accepted statements to be held, got {held} \
             in {document:?}"
        );

        // The final document is the stored one with the refused statement joined onto its
        // statement list; whatever the measure counts on top of it has to divide evenly by
        // the statements it holds, and the quotient is the row's per-statement cost.
        let final_document = document.len() + 1 + statement_bytes;
        let statements = accepted + 1;
        let stated = stated_size(case, &refused);
        let beyond_document = stated.checked_sub(final_document).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the measure to count at least the {final_document} \
                 bytes of the final document, got {stated}"
            )
        });
        assert_eq!(
            beyond_document % statements,
            0,
            "case {case:?}: expected what the measure counts beyond the {final_document} \
             document bytes ({beyond_document}) to be a cost per each of the {statements} \
             statements, which it does not divide by"
        );
        assert!(
            stated > MAX_POLICY_SIZE_BYTES,
            "case {case:?}: expected the stated size past the {MAX_POLICY_SIZE_BYTES} byte \
             quota, got {stated}"
        );
        // The accepted side is bounded by the same measure, not by the document's bytes.
        let per_statement = beyond_document / statements;
        let held_measure = document.len() + per_statement * accepted;
        assert!(
            held_measure <= MAX_POLICY_SIZE_BYTES,
            "case {case:?}: the {accepted} accepted grants measure {held_measure}, past the \
             {MAX_POLICY_SIZE_BYTES} byte quota"
        );
        counts.push((*case, statements, per_statement));
    }
    // The rows crossed the quota at different statement counts, so one cost per statement
    // is what has to come out of all of them: a single flat offset added to the document,
    // or a cost that varied with the statements' size, divides into a different figure in
    // each row.
    let distinct: std::collections::BTreeSet<usize> =
        counts.iter().map(|(_, statements, _)| *statements).collect();
    assert_eq!(
        distinct.len(),
        counts.len(),
        "the rows must cross the quota at different statement counts, got {counts:?}"
    );
    assert!(
        counts
            .iter()
            .all(|(_, _, cost)| *cost == STATEMENT_OVERHEAD_BYTES),
        "every row must measure {STATEMENT_OVERHEAD_BYTES} bytes per statement beyond the \
         document, got (case, statements, cost per statement) {counts:?}"
    );
}

/// The same accounting measures a layer version's policy, the other grant this quota
/// refuses.
#[tokio::test]
async fn the_same_measure_refuses_a_layer_version_grant() {
    let case = "a layer version's policy";
    let router = router();
    publish_layer_version(&router, "padded").await;
    // Statement ids at the model's `StatementId` maximum of 100 characters, so a bounded
    // number of grants crosses the quota.
    let body = |index: usize| {
        json!({
            "StatementId": format!("padding-statement-{index:0>82}"),
            "Action": "lambda:GetLayerVersion",
            "Principal": "111122223333",
        })
    };
    let mut accepted = 0usize;
    let mut statement_sizes = Vec::new();
    let mut refusal = None;
    for index in 0..300usize {
        let answer = call(
            &router,
            "POST",
            "/2018-10-31/layers/padded/versions/1/policy",
            &body(index),
        )
        .await;
        if answer.status == 201 {
            accepted += 1;
            statement_sizes.push(
                answer
                    .string("Statement")
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no Statement member in {}", answer.text())
                    })
                    .len(),
            );
            continue;
        }
        refusal = Some(answer);
        break;
    }
    let refused = refusal.unwrap_or_else(|| {
        panic!("case {case:?}: the quota must be crossed, {accepted} grants accepted")
    });
    assert!(
        accepted > 1,
        "case {case:?}: expected several statements to fit inside the quota, got {accepted}"
    );
    let statement_bytes = statement_sizes.first().copied().unwrap_or_default();
    assert!(
        statement_sizes
            .iter()
            .all(|bytes| *bytes == statement_bytes),
        "case {case:?}: every statement must render to the same size, got {statement_sizes:?}"
    );
    let document = call(
        &router,
        "GET",
        "/2018-10-31/layers/padded/versions/1/policy",
        &Value::Null,
    )
    .await
    .string("Policy")
    .unwrap_or_else(|| panic!("case {case:?}: the read must report the stored document"));
    let final_document = document.len() + 1 + statement_bytes;
    let expected = final_document + STATEMENT_OVERHEAD_BYTES * (accepted + 1);
    let stated = stated_size(case, &refused);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected the measure of a {} statement document ({final_document} \
         document bytes plus {STATEMENT_OVERHEAD_BYTES} per statement), got {stated}",
        accepted + 1
    );
}
