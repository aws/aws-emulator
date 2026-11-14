// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `encryptionConfiguration` a CREATE stores, over every
//! FORM the model admits for the structure.
//!
//! The rule: `encryptionConfiguration` is how a request names the key its investigation
//! data is encrypted with, so a write keeps the key the request names — a configuration
//! carrying a `kmsKeyId` is stored with it, and one naming the id but no `type` is
//! stored as the customer managed key type it asks for — while a request that names no
//! key at all gets the documented Amazon Web Services owned key. The stored member
//! therefore always names how the group's data is encrypted.
//!
//! A form whose two members DISAGREE names no such key, and is refused with the modelled
//! `ValidationException` instead of being stored: the customer managed key type naming no
//! `kmsKeyId`, and the Amazon Web Services owned key type naming one. The rule behind
//! those two rows, over both writers of the member and over what KMS reports about the
//! key, is `tests/all/encryption_configuration_names_a_usable_key.rs`; here they are the
//! two forms of this table that are not stored.
//!
//! Neither member of the structure is `@required`
//! (`smithy_query(query="shape",shape="EncryptionConfiguration")`), so the member
//! absent, a structure carrying nothing and a structure carrying only a `kmsKeyId` are
//! all requests the model admits.
//!
//! The rows are DERIVED from the modelled value set
//! (`ENCRYPTION_CONFIGURATION_TYPE_VALUES`, which carries exactly what
//! `smithy_query(query="shape",shape="EncryptionConfigurationType")` reports) crossed
//! with the presence of a `kmsKeyId`, so the table spans every key type the model
//! declares rather than a hand-picked one, and grows with the value set.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes. An account holds one
//! investigation group per Region, so each case signs for a Region of its own.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES;
use service_com_amazonaws_aiops_aiops::state::{
    AWS_OWNED_KEY_TYPE, KMS_KEY_ID_NOT_ACCEPTED, KMS_KEY_ID_REQUIRED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The customer-managed member of the modelled `EncryptionConfigurationType` value set
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`), surfaced as
/// `ENCRYPTION_CONFIGURATION_TYPE_VALUES`.
const CUSTOMER_MANAGED_KEY_TYPE: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// Real AWS Regions the cases sign for, in order; past the list the names are synthetic
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

/// The Region case `n` signs for.
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}

/// A `kmsKeyId` in `region` that satisfies the modelled member (len 1..256, `^arn:.*$`).
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

/// The form of the `encryptionConfiguration` a create carries.
#[derive(Clone, Copy, Debug)]
enum Form {
    /// The member is absent from the request body.
    Omitted,
    /// The member is present: `key_type` is the value set member it names, if any, and
    /// `kms_key_id` says whether it also carries a key id.
    Present {
        key_type: Option<&'static str>,
        kms_key_id: bool,
    },
}

/// Every form, differing along whether the member is there at all, whether it names a
/// key type and whether it carries a key id. The typed half is derived from the
/// modelled value set, so each of its members is covered with and without a key id.
fn forms() -> Vec<Form> {
    let mut forms = vec![
        Form::Omitted,
        Form::Present {
            key_type: None,
            kms_key_id: false,
        },
        Form::Present {
            key_type: None,
            kms_key_id: true,
        },
    ];
    for key_type in ENCRYPTION_CONFIGURATION_TYPE_VALUES {
        for kms_key_id in [false, true] {
            forms.push(Form::Present {
                key_type: Some(key_type),
                kms_key_id,
            });
        }
    }
    forms
}

/// The `encryptionConfiguration` the request body carries for `form`, or `None` when the
/// form omits the member.
fn supplied(form: Form, key: &str) -> Option<Value> {
    match form {
        Form::Omitted => None,
        Form::Present {
            key_type,
            kms_key_id,
        } => {
            let mut structure = serde_json::Map::new();
            if let Some(key_type) = key_type {
                structure.insert("type".to_string(), json!(key_type));
            }
            if kms_key_id {
                structure.insert("kmsKeyId".to_string(), json!(key));
            }
            Some(Value::Object(structure))
        }
    }
}

/// Why a form is refused rather than stored, or `None` when the form is one the service
/// can honour: the two members have to AGREE, so the customer managed key type requires a
/// key id and the Amazon Web Services owned key type admits none.
fn refusal(form: Form) -> Option<String> {
    match form {
        Form::Present {
            key_type: Some(CUSTOMER_MANAGED_KEY_TYPE),
            kms_key_id: false,
        } => Some(KMS_KEY_ID_REQUIRED.to_string()),
        Form::Present {
            key_type: Some(AWS_OWNED_KEY_TYPE),
            kms_key_id: true,
        } => Some(KMS_KEY_ID_NOT_ACCEPTED.to_string()),
        _ => None,
    }
}

/// What the rule says the group stores for `form`, and so what every later read of it
/// reports: the configuration as given when it names a key type, the customer managed
/// key type with the supplied id when it names an id and no type, and the documented
/// Amazon Web Services owned key when it names no key at all.
fn expected(form: Form, key: &str) -> Value {
    match supplied(form, key) {
        Some(structure) if structure.get("type").is_some() => structure,
        Some(structure) if structure.get("kmsKeyId").is_some() => {
            json!({ "type": CUSTOMER_MANAGED_KEY_TYPE, "kmsKeyId": key })
        }
        _ => json!({ "type": AWS_OWNED_KEY_TYPE }),
    }
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

/// Whatever form a create carries, the group it mints reports a configuration that names
/// how its data is encrypted — and a read of it does not change what a later read sees.
/// A form whose members disagree mints no group at all.
#[tokio::test]
async fn a_create_stores_a_configuration_that_names_the_key_type_for_every_form() {
    let router = emulator();

    for (index, form) in forms().into_iter().enumerate() {
        let case = format!("{form:?}");
        let region = region(index);
        let key = key_arn(&region);

        let mut body = json!({ "name": "encryption-forms", "roleArn": ROLE_ARN });
        if let Some(structure) = supplied(form, &key) {
            body["encryptionConfiguration"] = structure;
        }
        let (status, created) =
            call(&router, &region, "POST", "/investigationGroups", Some(&body)).await;

        if let Some(sentence) = refusal(form) {
            assert_eq!(
                (
                    status,
                    created.get("message").and_then(Value::as_str).unwrap_or("")
                ),
                (400, sentence.as_str()),
                "case {case:?}: a create carrying {:?} must be refused with {sentence:?}, got \
                 {status} / {created:?}",
                supplied(form, &key)
            );
            // Nothing was minted, so this Region still holds no group.
            let (status, listed) = call(&router, &region, "GET", "/investigationGroups", None).await;
            let groups = listed
                .get("investigationGroups")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            assert_eq!(
                (status, groups),
                (200, 0),
                "case {case:?}: a refused create must store no group, got {status} / {listed:?}"
            );
            continue;
        }

        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateInvestigationGroup, got {status} / {created:?}"
        );
        let arn = created
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the create carried no arn, got {created:?}"))
            .to_string();

        let got = stored_configuration(&router, &region, &arn, &case).await;
        let want = expected(form, &key);
        assert_eq!(
            got, want,
            "case {case:?}: a create carrying {:?} must store {want}, got {got:?}",
            supplied(form, &key)
        );

        // The invariant the rule exists for: the stored member names a key type out of
        // the modelled value set, whatever the request carried.
        let key_type = got.get("type").and_then(Value::as_str);
        assert!(
            key_type.is_some_and(|value| ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&value)),
            "case {case:?}: the stored configuration must name a modelled key type out of \
             {ENCRYPTION_CONFIGURATION_TYPE_VALUES:?}, got {got:?}"
        );

        // Reporting it does not change it: the next read of the same group reports the
        // same configuration.
        let again = stored_configuration(&router, &region, &arn, &case).await;
        assert_eq!(
            again, got,
            "case {case:?}: a second read must report the same configuration, got {again:?}"
        );
    }
}
