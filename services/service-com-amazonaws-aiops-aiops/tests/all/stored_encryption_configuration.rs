// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a WRITE stores in an investigation group's
//! `encryptionConfiguration`, and therefore for what every later read of the group
//! reports about how its data is encrypted.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: `encryptionConfiguration` is how a request names the key its
//! investigation data is encrypted with, so a write keeps the key the request names — a
//! configuration carrying a `kmsKeyId` is stored with it, and one naming the id but no
//! `type` (`type` is not `@required`, see
//! `smithy_query(query="shape",shape="EncryptionConfiguration")`) is stored as the
//! customer managed key type it asks for — while a request that names no key at all
//! (the member omitted, or a structure carrying neither member) gets the documented
//! Amazon Web Services owned key.
//! Both writers of the member apply it, so each row below is driven through
//! `CreateInvestigationGroup` AND through `UpdateInvestigationGroup`.
//!
//! A configuration whose two members disagree names no such key and is refused instead of
//! stored, which `tests/all/encryption_configuration_names_a_usable_key.rs` is the suite
//! for; the row below naming the customer managed key type with no key id is here because
//! this table spans the FORMS of the structure, and it states what that form answers.
//!
//! An account holds one investigation group per Region, so each row signs for two
//! Regions of its own: one for the create leg and one for the update leg.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES;
use service_com_amazonaws_aiops_aiops::state::{AWS_OWNED_KEY_TYPE, KMS_KEY_ID_REQUIRED};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The customer-managed member of the modelled `EncryptionConfigurationType` value set
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`), surfaced as
/// `ENCRYPTION_CONFIGURATION_TYPE_VALUES`.
const CUSTOMER_MANAGED_KEY_TYPE: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// Real AWS Regions the rows sign for, in order; past the list the names are synthetic
/// ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "eu-central-1",
    "ap-southeast-4",
    "sa-east-1",
    "ca-central-1",
    "eu-north-1",
    "af-south-1",
    "ap-east-1",
    "us-gov-west-1",
];

/// The Region slot `n` signs for; each row needs two of its own.
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}

/// A `kmsKeyId` that satisfies the modelled member (len 1..256, `^arn:.*$`).
fn key_arn(region: &str) -> String {
    format!("arn:aws:kms:{region}:{ACCOUNT}:key/8f1c2d3e-4a5b-6c7d-8e9f-0a1b2c3d4e5f")
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the region and the caller principal from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is what
/// an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// Create a group that must succeed, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value, case: &str) -> String {
    let (status, created) = call(router, region, "POST", "/investigationGroups", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateInvestigationGroup, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the create carried no arn, got {created:?}"))
        .to_string()
}

/// `PATCH` a group with `patch`, asserting the update succeeded.
async fn update_ok(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    patch: &Value,
    case: &str,
) {
    let (status, answer) = call(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", label(identifier)),
        Some(patch),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateInvestigationGroup, got {status} / {answer:?}"
    );
}

/// The `encryptionConfiguration` a read of the group reports.
async fn stored_configuration(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", label(identifier)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetInvestigationGroup, got {status} / {body:?}"
    );
    body.get("encryptionConfiguration")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the read reported no encryptionConfiguration, got {body:?}")
        })
}

/// One row: the configuration a write carries (or omits) and what the group stores for
/// it — or, for a configuration the service cannot honour, the sentence the write is
/// refused with.
struct Row {
    case: &'static str,
    /// The `encryptionConfiguration` the request carries; `None` omits the member.
    supplied: Option<Value>,
    /// What a read of the group must report afterwards, when the write is served; `Null`
    /// for a row that is refused, which stores nothing for a read to report.
    expected: Value,
    /// The sentence the write is refused with, when the configuration names no key the
    /// group's data could be encrypted under.
    refused: Option<String>,
}

/// The rows, differing along the FORM of the configuration the request carries: the
/// member absent, a structure carrying no member at all, a structure carrying only a
/// `kmsKeyId`, and each member of the modelled value set named explicitly, with and
/// without a key id.
fn rows(region: &str) -> Vec<Row> {
    let key = key_arn(region);
    let default = json!({ "type": AWS_OWNED_KEY_TYPE });
    vec![
        Row {
            case: "the member omitted entirely",
            supplied: None,
            expected: default.clone(),
            refused: None,
        },
        Row {
            case: "an empty structure",
            supplied: Some(json!({})),
            expected: default.clone(),
            refused: None,
        },
        Row {
            case: "a kmsKeyId with no type",
            supplied: Some(json!({ "kmsKeyId": key })),
            expected: json!({ "type": CUSTOMER_MANAGED_KEY_TYPE, "kmsKeyId": key }),
            refused: None,
        },
        Row {
            case: "the AWS owned key, named",
            supplied: Some(default.clone()),
            expected: default,
            refused: None,
        },
        Row {
            case: "a customer managed key with its key id",
            supplied: Some(json!({ "type": CUSTOMER_MANAGED_KEY_TYPE, "kmsKeyId": key })),
            expected: json!({ "type": CUSTOMER_MANAGED_KEY_TYPE, "kmsKeyId": key }),
            refused: None,
        },
        Row {
            case: "a customer managed key named with no key id",
            supplied: Some(json!({ "type": CUSTOMER_MANAGED_KEY_TYPE })),
            // Nothing is stored for a refused write, so this row states no stored value:
            // `refused` is what it asserts.
            expected: Value::Null,
            refused: Some(KMS_KEY_ID_REQUIRED.to_string()),
        },
    ]
}

/// The `message` a refused call carried.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A create stores the configuration the rule says it stores, and a read reports it — and
/// a configuration the service cannot honour is refused and stores nothing.
#[tokio::test]
async fn a_create_stores_a_configuration_that_names_the_key_type() {
    let router = emulator();
    for (index, row) in rows("us-west-2").into_iter().enumerate() {
        let region = region(index);
        let mut body = json!({ "name": "stored-encryption", "roleArn": ROLE_ARN });
        if let Some(supplied) = row.supplied.clone() {
            body["encryptionConfiguration"] = supplied;
        }
        if let Some(sentence) = &row.refused {
            let (status, refusal) =
                call(&router, &region, "POST", "/investigationGroups", Some(&body)).await;
            assert_eq!(
                (status, message(&refusal)),
                (400, sentence.clone()),
                "case {:?}: create carrying {:?} must be refused with {sentence:?}, got {status} \
                 / {refusal:?}",
                row.case,
                row.supplied
            );
            continue;
        }
        let arn = create_ok(&router, &region, &body, row.case).await;
        let got = stored_configuration(&router, &region, &arn, row.case).await;
        assert_eq!(
            got, row.expected,
            "case {:?}: create carrying {:?} must store {}, got {got:?}",
            row.case, row.supplied, row.expected
        );
    }
}

/// An update stores the same configuration the create would have stored for the same
/// request — so the key the request names survives the write, and a request naming no
/// key at all replaces the stored one with the documented default rather than erasing
/// what the group reports or keeping a key the request did not name. A configuration the
/// create would have refused the update refuses too, leaving the stored one alone.
#[tokio::test]
async fn an_update_stores_the_configuration_the_create_would_store() {
    let router = emulator();
    for (index, row) in rows("us-west-2").into_iter().enumerate() {
        // Every row is the update's case too, the omitted-member one included: the
        // member documents what the service does when the request omits the parameter,
        // so both writers store what the rule returns for the configuration they were
        // given.
        let supplied = row.supplied.clone();
        let region = region(index);
        // Create with the customer managed pair, so a configuration the update stores
        // is distinguishable from the one it replaced in every row.
        let before = json!({ "type": CUSTOMER_MANAGED_KEY_TYPE, "kmsKeyId": key_arn(&region) });
        let arn = create_ok(
            &router,
            &region,
            &json!({ "name": "stored-encryption", "roleArn": ROLE_ARN,
                     "encryptionConfiguration": before.clone() }),
            row.case,
        )
        .await;

        // The omitted-member row sends a body that carries no `encryptionConfiguration`
        // key at all, which is what omitting the parameter means on the wire.
        let mut patch = json!({});
        if let Some(supplied) = supplied.clone() {
            patch["encryptionConfiguration"] = supplied;
        }
        if let Some(sentence) = &row.refused {
            let (status, refusal) = call(
                &router,
                &region,
                "PATCH",
                &format!("/investigationGroups/{}", label(&arn)),
                Some(&patch),
            )
            .await;
            assert_eq!(
                (status, message(&refusal)),
                (400, sentence.clone()),
                "case {:?}: update carrying {patch:?} must be refused with {sentence:?}, got \
                 {status} / {refusal:?}",
                row.case
            );
            let got = stored_configuration(&router, &region, &arn, row.case).await;
            assert_eq!(
                got, before,
                "case {:?}: a refused update must leave the stored configuration as it was \
                 ({before}), got {got:?}",
                row.case
            );
            continue;
        }
        update_ok(&router, &region, &arn, &patch, row.case).await;
        let got = stored_configuration(&router, &region, &arn, row.case).await;
        assert_eq!(
            got, row.expected,
            "case {:?}: update carrying {patch:?} must store {}, got {got:?}",
            row.case, row.expected
        );

        // Whatever the row, the stored member keeps naming how the group's data is
        // encrypted: a type from the modelled value set.
        let key_type = got.get("type").and_then(Value::as_str);
        assert!(
            key_type.is_some_and(|value| ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&value)),
            "case {:?}: the stored configuration must name a modelled key type out of \
             {ENCRYPTION_CONFIGURATION_TYPE_VALUES:?}, got {got:?}",
            row.case
        );

        // And the read is stable: reporting it does not change it.
        let again = stored_configuration(&router, &region, &arn, row.case).await;
        assert_eq!(
            again, got,
            "case {:?}: a second read must report the same configuration, got {again:?}",
            row.case
        );
    }
}
