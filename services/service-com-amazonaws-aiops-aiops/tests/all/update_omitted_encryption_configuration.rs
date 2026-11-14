// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `UpdateInvestigationGroup` stores in a group's
//! `encryptionConfiguration` when the request OMITS the parameter.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a member whose own documentation states what the service does
//! when the request omits the parameter is stored as that documented value by every
//! write, so an omission is an instruction and not a request to keep what was stored.
//! `UpdateInvestigationGroupRequest$encryptionConfiguration` documents it
//! (`smithy_query(query="doc",shape="UpdateInvestigationGroupRequest",member="encryptionConfiguration")`):
//! "If you omit this parameter, CloudWatch investigations will use an Amazon Web
//! Services key to encrypt the data." So an update that names no key leaves the group
//! reporting the Amazon Web Services owned key — the `AWS_OWNED_KEY` member of the
//! modelled `EncryptionConfigurationType` value set, with no `kmsKeyId` — whatever key
//! the group carried before, while an update that DOES name a key keeps that key.
//!
//! Rows are the cross product of two axes, so the outcome depends on neither the shape
//! of the stored configuration nor the shape of the patch:
//!
//! * what the group was created with: the member omitted, the Amazon Web Services key
//!   named, a customer managed key with its key id, and a bare `kmsKeyId` (a customer
//!   managed key named with NO key id is not among them: it names no key, so no group can
//!   be created with it — `tests/all/encryption_configuration_names_a_usable_key.rs`);
//! * what the omitting update carries instead: no member at all, a boolean, a string, a
//!   list, a map and every other member the operation accepts at once
//!   (`smithy_query(query="operation",operation="UpdateInvestigationGroup")`).
//!
//! An account holds one investigation group per Region, so every group below is created
//! in a Region of its own.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES;
use service_com_amazonaws_aiops_aiops::state::AWS_OWNED_KEY_TYPE;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// A second role ARN, so a replacement the patch asks for is distinguishable.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRoleTwo";

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

/// The Region slot `n` signs for; each group needs one of its own.
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
/// handlers read the Region and the caller principal from.
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

/// The whole group `GetInvestigationGroup` reports.
async fn get_ok(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    case: &str,
) -> Map<String, Value> {
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
    body.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a group document, got {body:?}"))
        .clone()
}

/// An SNS topic ARN long enough for the modelled `chatbotNotificationChannel` key
/// (len 20..2048) and one chat-application configuration ARN for it (the list is
/// modelled 1..5).
fn chat_channel() -> Value {
    json!({
        "arn:aws:sns:us-west-2:000000000000:investigation-notifications": [
            "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops"
        ]
    })
}

/// The configurations a group can be created with: the member omitted (`None`) and each
/// form the structure admits and the service can honour, since neither of its members is
/// `@required`.
///
/// The customer managed key type with NO key id is absent for a reason: it names no key,
/// so no group can be created with it at all — the suite for that refusal is
/// `tests/all/encryption_configuration_names_a_usable_key.rs`.
fn created_configurations(region: &str) -> Vec<(&'static str, Option<Value>)> {
    let key = key_arn(region);
    vec![
        ("created with no encryptionConfiguration", None),
        (
            "created with the Amazon Web Services key named",
            Some(json!({ "type": AWS_OWNED_KEY_TYPE })),
        ),
        (
            "created with a customer managed key and its key id",
            Some(json!({ "type": CUSTOMER_MANAGED_KEY_TYPE, "kmsKeyId": key })),
        ),
        (
            "created with a bare kmsKeyId",
            Some(json!({ "kmsKeyId": key })),
        ),
    ]
}

/// Update bodies that name no `encryptionConfiguration`, one per member TYPE the
/// operation accepts plus one carrying every other member at once and one carrying
/// nothing.
fn omitting_patches() -> Vec<(&'static str, Value)> {
    vec![
        ("patched with no member at all", json!({})),
        (
            "patched with a boolean member",
            json!({ "isCloudTrailEventHistoryEnabled": false }),
        ),
        ("patched with a string member", json!({ "roleArn": OTHER_ROLE_ARN })),
        (
            "patched with a list member",
            json!({ "tagKeyBoundaries": ["Service", "Tier"] }),
        ),
        (
            "patched with a map member",
            json!({ "chatbotNotificationChannel": chat_channel() }),
        ),
        (
            "patched with every other member at once",
            json!({
                "isCloudTrailEventHistoryEnabled": false,
                "roleArn": OTHER_ROLE_ARN,
                "tagKeyBoundaries": ["Service"],
                "chatbotNotificationChannel": chat_channel(),
                "crossAccountConfigurations": [
                    { "sourceRoleArn": "arn:aws:iam::111122223333:role/CrossAccountInvestigation" }
                ],
            }),
        ),
    ]
}

/// An update that names no `encryptionConfiguration` leaves the group reporting the
/// Amazon Web Services owned key, whatever configuration it carried and whatever else
/// the request asked for — and the members the request DID name read back as asked.
#[tokio::test]
async fn an_update_omitting_the_member_stores_the_aws_owned_key() {
    assert!(
        ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&AWS_OWNED_KEY_TYPE),
        "the documented omission default must be a key type the model admits, got \
         {AWS_OWNED_KEY_TYPE:?} against {ENCRYPTION_CONFIGURATION_TYPE_VALUES:?}"
    );

    let router = emulator();
    let mut slot = 0usize;
    for (created_case, created) in created_configurations("us-west-2") {
        for (patch_case, patch) in omitting_patches() {
            let case = format!("{created_case}, {patch_case}");
            let region = region(slot);
            slot += 1;

            let mut body = json!({ "name": "omitted-encryption", "roleArn": ROLE_ARN });
            if let Some(created) = created.clone() {
                body["encryptionConfiguration"] = created;
            }
            let arn = create_ok(&router, &region, &body, &case).await;

            update_ok(&router, &region, &arn, &patch, &case).await;

            let after = get_ok(&router, &region, &arn, &case).await;
            let got = after.get("encryptionConfiguration");
            assert_eq!(
                got,
                Some(&json!({ "type": AWS_OWNED_KEY_TYPE })),
                "case {case:?}: an update that names no key must leave the group \
                 reporting the Amazon Web Services owned key, got {got:?} in {after:?}"
            );

            // Every member the patch DID name reached the record, so the group was
            // really updated rather than left alone.
            for (member, want) in patch.as_object().expect("an object patch") {
                assert_eq!(
                    after.get(member),
                    Some(want),
                    "case {case:?}: expected {member} to read back as {want}, got {:?}",
                    after.get(member)
                );
            }

            // Reporting the configuration does not change it.
            let again = get_ok(&router, &region, &arn, &case).await;
            assert_eq!(
                again.get("encryptionConfiguration"),
                got,
                "case {case:?}: a second read must report the same configuration, \
                 got {:?}",
                again.get("encryptionConfiguration")
            );
        }
    }
}

/// The other half of the same rule: an update that DOES name a key stores that key, so
/// the omission default is what an omission means and not what every update means.
#[tokio::test]
async fn an_update_naming_a_key_keeps_it() {
    let router = emulator();
    for (slot, (created_case, created)) in created_configurations("us-west-2")
        .into_iter()
        .enumerate()
    {
        let case = format!("{created_case}, patched with a customer managed key");
        let region = region(slot);

        let mut body = json!({ "name": "named-encryption", "roleArn": ROLE_ARN });
        if let Some(created) = created {
            body["encryptionConfiguration"] = created;
        }
        let arn = create_ok(&router, &region, &body, &case).await;

        let named = json!({ "type": CUSTOMER_MANAGED_KEY_TYPE, "kmsKeyId": key_arn(&region) });
        update_ok(
            &router,
            &region,
            &arn,
            &json!({ "encryptionConfiguration": named, "isCloudTrailEventHistoryEnabled": false }),
            &case,
        )
        .await;

        let after = get_ok(&router, &region, &arn, &case).await;
        let got = after.get("encryptionConfiguration");
        assert_eq!(
            got,
            Some(&named),
            "case {case:?}: an update naming a key must store it, got {got:?} in {after:?}"
        );
    }
}
