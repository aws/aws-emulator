// SPDX-License-Identifier: CC0-1.0

//! What a `SecureString`'s KMS key being unusable does to the reads and the writes of
//! every parameter sealed under it.
//!
//! A `SecureString`'s value is sealed under the KMS key its put named, and Parameter Store
//! holds no key of its own: it asks KMS to seal the value on the way in and to open it on
//! the way out. A KMS key runs a cryptographic operation only while its KEY STATE permits
//! one, so a key an operator has suspended — disabled as a kill switch, scheduled for
//! deletion, or waiting for key material it has none of — seals nothing and opens nothing,
//! and both halves of Parameter Store's `SecureString` path are refused with the
//! `InvalidKeyId` each models for a key that cannot serve the request. The sentence is
//! KMS's own, relayed with the metadata block an AWS error carries.
//!
//! The tables span what the rule generalises over rather than the one recorded read:
//!
//! * the key state (disabled, pending deletion, and a key born pending import), and with
//!   it the KMS error code each is refused under;
//! * the read that reports a decrypted value (`GetParameter`, `GetParameters`,
//!   `GetParameterHistory`, `GetParametersByPath`);
//! * the form the parameter names its key in (key id, key ARN, alias) — the refusal names
//!   the key by the ARN state holds for it either way;
//! * the reads that decrypt NOTHING and are therefore never refused (`WithDecryption`
//!   absent and false, and the two parameter types the flag is ignored for);
//! * the write (a new parameter and an overwrite of one that exists), which leaves the
//!   parameter exactly the versions it had;
//! * the key becoming usable again, after which the value is served once more.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::parameter_store::{call, error_code, error_message, router_with_kms, REGION};

/// The hierarchy every parameter of a case lives in, so one `GetParametersByPath` reads
/// them all.
const SUITE_PATH: &str = "/key-state";

/// The service name KMS's own error metadata carries, which a relayed refusal names.
const KMS_SERVICE: &str = "AWSKMS";

/// The status a relayed KMS client error states.
const KMS_STATUS: u16 = 400;

/// The error Parameter Store models for a key that cannot serve the request: "The query
/// key ID isn't valid"
/// (`smithy_query(query="operation",operation="GetParameter,PutParameter")`).
const INVALID_KEY_ID: &str = "InvalidKeyId";

/// KMS's own code for a cryptographic operation on a key that is merely DISABLED
/// (`smithy_query(query="shape",shape="DisabledException",
/// model_path="services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json")`).
const DISABLED_EXCEPTION: &str = "DisabledException";

/// KMS's own code for a cryptographic operation on a key in any other state that runs
/// none (`KMSInvalidStateException`, same model).
const INVALID_STATE_EXCEPTION: &str = "KMSInvalidStateException";

/// One KMS call, returning `(status, body)`. KMS is an awsJson1_1 service reached by
/// `X-Amz-Target: TrentService.<Operation>`.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A KMS call that must succeed.
async fn kms_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, response) = kms(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case}: KMS {operation} {body} must succeed, got {status} with {response}"
    );
    response
}

/// A symmetric encryption key created through KMS's own `CreateKey`, as `(key id, ARN)`.
///
/// `extra` carries whatever else the create states — `Origin`, for the key that is born
/// waiting for key material it has none of.
async fn create_key(router: &MultiServiceRouter, case: &str, extra: &Value) -> (String, String) {
    let mut body = json!({ "Description": case });
    if let Value::Object(members) = extra {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    let created = kms_ok(router, case, "CreateKey", &body).await;
    let metadata = &created["KeyMetadata"];
    let key_id = metadata["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateKey reports a KeyId, got {created}"))
        .to_string();
    let arn = metadata["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateKey reports an Arn, got {created}"))
        .to_string();
    (key_id, arn)
}

/// The `KeyState` KMS reports for `key_id` right now, asked of `DescribeKey` so a case
/// reads the state the service publishes rather than assuming the transition took.
async fn described_key_state(router: &MultiServiceRouter, case: &str, key_id: &str) -> String {
    let described = kms_ok(router, case, "DescribeKey", &json!({ "KeyId": key_id })).await;
    described["KeyMetadata"]["KeyState"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeKey reports a KeyState, got {described}"))
        .to_string()
}

/// One row of the key-state table: how an EXISTING key is driven out of a usable state,
/// what `DescribeKey` then reports, and the KMS error code and sentence a cryptographic
/// operation under it is refused with.
///
/// The states and their prose come from the `KeyState` enum the KMS model declares
/// (`smithy_query(query="shape",shape="KeyState")`), whose camel-case values KMS words as
/// lowercase prose; the two codes are the errors its cryptographic operations model for a
/// key state (see [`DISABLED_EXCEPTION`] and [`INVALID_STATE_EXCEPTION`]).
struct StateRow {
    case: &'static str,
    /// The KMS operation that takes the key out of a usable state, and what else it needs.
    operation: &'static str,
    extra: Value,
    /// The `KeyState` `DescribeKey` reports once it has.
    key_state: &'static str,
    code: &'static str,
    prose: &'static str,
}

/// The states a key already holding a sealed value can be driven into, so both the read
/// and the write path can be exercised for each.
fn state_rows() -> Vec<StateRow> {
    vec![
        StateRow {
            case: "the key is disabled",
            operation: "DisableKey",
            extra: json!({}),
            key_state: "Disabled",
            code: DISABLED_EXCEPTION,
            prose: "is disabled.",
        },
        StateRow {
            case: "the key's deletion is scheduled",
            operation: "ScheduleKeyDeletion",
            extra: json!({ "PendingWindowInDays": 7 }),
            key_state: "PendingDeletion",
            code: INVALID_STATE_EXCEPTION,
            prose: "is pending deletion.",
        },
    ]
}

/// Drive `key_id` out of a usable state the way `row` names, and confirm with `DescribeKey`
/// that KMS now reports the state the row expects — so every refusal below is that state
/// and nothing else.
async fn make_unusable(router: &MultiServiceRouter, case: &str, row: &StateRow, key_id: &str) {
    let mut body = json!({ "KeyId": key_id });
    if let Value::Object(members) = &row.extra {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    kms_ok(router, case, row.operation, &body).await;
    let reported = described_key_state(router, case, key_id).await;
    assert_eq!(
        reported, row.key_state,
        "case {case}: KMS must report the key as {}, got {reported:?}",
        row.key_state
    );
}

/// Store `name` as a `SecureString` of `value` sealed under `key`, which must succeed.
async fn put_secure(router: &MultiServiceRouter, case: &str, name: &str, key: &str, value: &str) {
    let (status, response) = call(
        router,
        "PutParameter",
        &json!({
            "Name": name,
            "Type": "SecureString",
            "Value": value,
            "KeyId": key,
            "Overwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: PutParameter {name} under {key} must succeed, got {status} with {response}"
    );
}

/// Assert `operation` with `body` is refused as the relayed KMS refusal `row` words for
/// `key_arn`, and report the request id that refusal carried.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    key_arn: &str,
    row: &StateRow,
) -> String {
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: {operation} {body} must be refused with HTTP 400, got {status} with \
         {response}"
    );
    assert_eq!(
        error_code(&response),
        INVALID_KEY_ID,
        "case {case}: {operation} {body} must be refused with {INVALID_KEY_ID}, got {response}"
    );
    assert_relayed(case, &error_message(&response), key_arn, row)
}

/// Assert `message` is KMS's own sentence for `row`'s key state about `key_arn`, followed
/// by the metadata block an AWS error carries, and report the request id in it.
///
/// Everything but that id is fixed by the key and the refusal, so the whole prefix and
/// suffix are asserted; the id is the one per-call value, so it is returned rather than
/// compared against anything.
fn assert_relayed(case: &str, message: &str, key_arn: &str, row: &StateRow) -> String {
    let prefix = format!(
        "{key_arn} {} (Service: {KMS_SERVICE}; Status Code: {KMS_STATUS}; Error Code: {}; \
         Request ID: ",
        row.prose, row.code
    );
    let suffix = "; Proxy: null)";
    let request_id = message
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected the refusal to read {prefix:?}<request id>{suffix:?}, got \
                 {message:?}"
            )
        });
    assert!(
        !request_id.is_empty() && request_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case}: expected a minted request id, got {request_id:?} in {message:?}"
    );
    request_id.to_string()
}

/// The value a served `GetParameter` body reports.
fn reported_value(case: &str, response: &Value) -> String {
    response["Parameter"]["Value"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the read reports a Value, got {response}"))
        .to_string()
}

/// The version a served `GetParameter` body reports.
fn reported_version(case: &str, response: &Value) -> i64 {
    response["Parameter"]["Version"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: the read reports a Version, got {response}"))
}

/// `GetParameter` of `name` asking for no decryption, which must be served: it opens
/// nothing, so it reports the stored ciphertext whatever the key's state is.
async fn get_sealed(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, response) = call(router, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case}: a read that decrypts nothing must be served, got {status} with {response}"
    );
    response
}

/// A decrypting read is refused while the parameter's key is in a state that runs no
/// cryptographic operation — whichever read reports the value, and whichever form the
/// parameter names the key in.
#[tokio::test]
async fn a_decrypting_read_is_refused_while_the_parameters_key_is_unusable() {
    for row in state_rows() {
        let router = router_with_kms();
        let (key_id, key_arn) = create_key(&router, row.case, &json!({})).await;
        let alias = "alias/key-state-suite";
        kms_ok(
            &router,
            row.case,
            "CreateAlias",
            &json!({ "AliasName": alias, "TargetKeyId": key_id }),
        )
        .await;

        // The same key, named three ways: the refusal names it by the ARN state holds for
        // it either way, because the key is taken from the parameter rather than from the
        // request.
        let forms: [(&str, &str); 3] = [
            ("by-id", key_id.as_str()),
            ("by-arn", key_arn.as_str()),
            ("by-alias", alias),
        ];
        for (leaf, key) in forms {
            let name = format!("{SUITE_PATH}/{leaf}");
            put_secure(&router, row.case, &name, key, &format!("secret {leaf}")).await;
            // The positive control: while the key is usable the value is served, so the
            // refusals below are the key's state and nothing else.
            let (status, served) = call(
                &router,
                "GetParameter",
                &json!({ "Name": name, "WithDecryption": true }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {}, {leaf}: the read must be served while the key is usable, got {status} \
                 with {served}",
                row.case
            );
            assert_eq!(
                reported_value(row.case, &served),
                format!("secret {leaf}"),
                "case {}, {leaf}: the usable key must serve the stored plaintext",
                row.case
            );
        }

        make_unusable(&router, row.case, &row, &key_id).await;

        for (leaf, _) in forms {
            let name = format!("{SUITE_PATH}/{leaf}");
            let reads: [(&str, Value); 3] = [
                (
                    "GetParameter",
                    json!({ "Name": name, "WithDecryption": true }),
                ),
                (
                    "GetParameters",
                    json!({ "Names": [name], "WithDecryption": true }),
                ),
                (
                    "GetParameterHistory",
                    json!({ "Name": name, "WithDecryption": true }),
                ),
            ];
            for (operation, body) in reads {
                let case = format!("{}, {leaf}, {operation}", row.case);
                assert_refused(&router, &case, operation, &body, &key_arn, &row).await;
            }
        }

        // One read of the whole hierarchy opens every value it reports, so the first
        // refusal is the whole read's.
        let case = format!("{}, GetParametersByPath", row.case);
        assert_refused(
            &router,
            &case,
            "GetParametersByPath",
            &json!({ "Path": SUITE_PATH, "Recursive": true, "WithDecryption": true }),
            &key_arn,
            &row,
        )
        .await;
    }
}

/// A read that opens nothing is never refused by the key's state: the flag absent or
/// false reports the stored ciphertext, and "this flag is ignored for String and
/// StringList parameter types".
#[tokio::test]
async fn a_read_that_decrypts_nothing_is_served_while_the_key_is_unusable() {
    for row in state_rows() {
        let router = router_with_kms();
        let (key_id, _) = create_key(&router, row.case, &json!({})).await;

        let secure = format!("{SUITE_PATH}/sealed");
        let plaintext = "a sealed secret";
        put_secure(&router, row.case, &secure, &key_id, plaintext).await;
        let sealed_before = reported_value(row.case, &get_sealed(&router, row.case, &secure).await);
        assert_ne!(
            sealed_before, plaintext,
            "case {}: a SecureString must be stored as a ciphertext, not as its value",
            row.case
        );

        // The two types the flag is ignored for are not encrypted at all, so they name no
        // key and nothing about the key's state reaches them.
        let plain: [(&str, &str, &str); 2] = [
            ("string", "String", "plain text"),
            ("stringlist", "StringList", "one,two,three"),
        ];
        for (leaf, type_, value) in plain {
            let (status, response) = call(
                &router,
                "PutParameter",
                &json!({
                    "Name": format!("{SUITE_PATH}/{leaf}"),
                    "Type": type_,
                    "Value": value,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {}, {type_}: the put must succeed, got {status} with {response}",
                row.case
            );
        }

        make_unusable(&router, row.case, &row, &key_id).await;

        for flag in [None, Some(false)] {
            let case = format!("{}, WithDecryption {flag:?}", row.case);
            let mut body = json!({ "Name": secure });
            if let Some(flag) = flag {
                body["WithDecryption"] = json!(flag);
            }
            let (status, response) = call(&router, "GetParameter", &body).await;
            assert_eq!(
                status, 200,
                "case {case}: a read that opens nothing must be served, got {status} with \
                 {response}"
            );
            assert_eq!(
                reported_value(&case, &response),
                sealed_before,
                "case {case}: the undecrypted read must report the stored ciphertext"
            );
        }

        for (leaf, type_, value) in plain {
            let case = format!("{}, {type_}", row.case);
            let (status, response) = call(
                &router,
                "GetParameter",
                &json!({ "Name": format!("{SUITE_PATH}/{leaf}"), "WithDecryption": true }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the flag is ignored for {type_}, so the read must be served, got \
                 {status} with {response}"
            );
            assert_eq!(
                reported_value(&case, &response),
                value,
                "case {case}: the read must report the stored value"
            );
        }
    }
}

/// A write of a `SecureString` naming a key that runs no cryptographic operation is
/// refused the same way, and creates nothing: the parameter keeps the versions it had, and
/// a name that did not exist still does not.
#[tokio::test]
async fn a_write_under_an_unusable_key_is_refused_and_creates_no_version() {
    for row in state_rows() {
        let router = router_with_kms();
        let (key_id, key_arn) = create_key(&router, row.case, &json!({})).await;

        let existing = format!("{SUITE_PATH}/existing");
        let first = "the first value";
        put_secure(&router, row.case, &existing, &key_id, first).await;
        let sealed_before = reported_value(row.case, &get_sealed(&router, row.case, &existing).await);

        make_unusable(&router, row.case, &row, &key_id).await;

        // An overwrite of a parameter that exists.
        let case = format!("{}, overwrite", row.case);
        assert_refused(
            &router,
            &case,
            "PutParameter",
            &json!({
                "Name": existing,
                "Type": "SecureString",
                "Value": "a replacement",
                "KeyId": key_id,
                "Overwrite": true,
            }),
            &key_arn,
            &row,
        )
        .await;
        let held = get_sealed(&router, &case, &existing).await;
        assert_eq!(
            reported_version(&case, &held),
            1,
            "case {case}: the refused write must leave the parameter at the version it had"
        );
        assert_eq!(
            reported_value(&case, &held),
            sealed_before,
            "case {case}: the refused write must leave the stored ciphertext untouched"
        );

        // And a create of a name nothing holds.
        let fresh = format!("{SUITE_PATH}/fresh");
        let case = format!("{}, create", row.case);
        assert_refused(
            &router,
            &case,
            "PutParameter",
            &json!({
                "Name": fresh,
                "Type": "SecureString",
                "Value": "a new secret",
                "KeyId": key_arn,
            }),
            &key_arn,
            &row,
        )
        .await;
        let (status, response) = call(&router, "GetParameter", &json!({ "Name": fresh })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused write must create no parameter, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ParameterNotFound",
            "case {case}: the refused write must create no parameter, got {response}"
        );
    }
}

/// A key born waiting for key material it has none of runs no cryptographic operation
/// either, so it seals nothing: the state a key is IN decides the call, not how it got
/// there. `Origin` EXTERNAL "creates a KMS key with no key material", which waits in
/// `PendingImport`.
#[tokio::test]
async fn a_key_awaiting_its_material_seals_no_value() {
    let case = "the key is awaiting imported key material";
    let router = router_with_kms();
    let (key_id, key_arn) = create_key(&router, case, &json!({ "Origin": "EXTERNAL" })).await;
    let reported = described_key_state(&router, case, &key_id).await;
    assert_eq!(
        reported, "PendingImport",
        "case {case}: KMS must report the key as PendingImport, got {reported:?}"
    );

    let row = StateRow {
        case,
        operation: "",
        extra: json!({}),
        key_state: "PendingImport",
        code: INVALID_STATE_EXCEPTION,
        prose: "is pending import.",
    };
    let name = format!("{SUITE_PATH}/awaiting");
    assert_refused(
        &router,
        case,
        "PutParameter",
        &json!({
            "Name": name,
            "Type": "SecureString",
            "Value": "a secret with nothing to seal it",
            "KeyId": key_id,
        }),
        &key_arn,
        &row,
    )
    .await;
}

/// The relayed refusal carries a request id of its OWN per call: the id is the one
/// per-call field of the metadata block, so two refused calls state two ids, and the rest
/// of the sentence is identical.
#[tokio::test]
async fn each_relayed_refusal_states_its_own_request_id() {
    for row in state_rows() {
        let router = router_with_kms();
        let (key_id, key_arn) = create_key(&router, row.case, &json!({})).await;
        let name = format!("{SUITE_PATH}/repeated");
        put_secure(&router, row.case, &name, &key_id, "a repeated read").await;
        make_unusable(&router, row.case, &row, &key_id).await;

        let body = json!({ "Name": name, "WithDecryption": true });
        let mut ids = Vec::new();
        for probe in 0..3 {
            let case = format!("{}, probe {probe}", row.case);
            ids.push(assert_refused(&router, &case, "GetParameter", &body, &key_arn, &row).await);
        }
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {}: every refusal must state its own request id, got {ids:?}",
            row.case
        );
    }
}

/// The key state is a state, not a verdict about the parameter: a key made usable again
/// opens the value it always could, at the version the refused writes never changed.
#[tokio::test]
async fn a_key_enabled_again_serves_the_value_once_more() {
    let case = "the key is disabled and then enabled again";
    let router = router_with_kms();
    let (key_id, key_arn) = create_key(&router, case, &json!({})).await;
    let name = format!("{SUITE_PATH}/revived");
    let plaintext = "a secret the operator sealed";
    put_secure(&router, case, &name, &key_id, plaintext).await;

    let row = &state_rows()[0];
    make_unusable(&router, case, row, &key_id).await;
    assert_refused(
        &router,
        case,
        "GetParameter",
        &json!({ "Name": name, "WithDecryption": true }),
        &key_arn,
        row,
    )
    .await;

    kms_ok(&router, case, "EnableKey", &json!({ "KeyId": key_id })).await;
    let reported = described_key_state(&router, case, &key_id).await;
    assert_eq!(
        reported, "Enabled",
        "case {case}: KMS must report the key as Enabled again, got {reported:?}"
    );

    let (status, response) = call(
        &router,
        "GetParameter",
        &json!({ "Name": name, "WithDecryption": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: the read must be served once the key is usable again, got {status} with \
         {response}"
    );
    assert_eq!(
        reported_value(case, &response),
        plaintext,
        "case {case}: the revived key must open the value it sealed"
    );
    assert_eq!(
        reported_version(case, &response),
        1,
        "case {case}: nothing in between created a version"
    );
}
