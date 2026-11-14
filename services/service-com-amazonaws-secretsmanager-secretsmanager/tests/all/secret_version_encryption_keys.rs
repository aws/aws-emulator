// SPDX-License-Identifier: CC0-1.0

//! Which KMS key a secret version reports, end to end over the wire.
//!
//! Every case drives the real HTTP path — an `awsJson1_1` POST through the router's
//! `dispatch` — so these are the same bytes an SDK client sends. Nothing reaches into the
//! service's state struct: the key a version is held under is only ever read back through
//! `ListSecretVersionIds`, beside the key `DescribeSecret` reports for the secret itself.
//!
//! THE RULE: `SecretVersionsListEntry$KmsKeyIds` names the key the secret's configuration
//! names, exactly as the secret holds it — the value `DescribeSecret` answers as its
//! `KmsKeyId`, in whichever of the forms that member accepts the caller gave it, and
//! whatever an `UpdateSecret` has since re-keyed it to. Only a secret whose configuration
//! names NO key at all reports the default key's own token, which is the one case the
//! service has no identifier of the caller's to report.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    DEFAULT_ENCRYPTION_KEY_ID, DEFAULT_KMS_KEY_ALIAS,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// One call that must be served, answering its payload.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/\
                 secretsmanager/aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        )
        .body(::bytes::Bytes::from(
            ::serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        ::serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// The keys every version of `secret_id` reports, as one set of lists in listing order.
async fn reported_keys(router: &MultiServiceRouter, case: &str, secret_id: &str) -> Vec<Vec<String>> {
    ok(
        router,
        case,
        "ListSecretVersionIds",
        json!({ "SecretId": secret_id, "IncludeDeprecated": true }),
    )
    .await["Versions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["KmsKeyIds"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|key| key.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .collect()
}

/// The key `DescribeSecret` reports for `secret_id`, or `None` for a secret that names none.
async fn configured_key(router: &MultiServiceRouter, case: &str, secret_id: &str) -> Option<String> {
    ok(
        router,
        case,
        "DescribeSecret",
        json!({ "SecretId": secret_id }),
    )
    .await["KmsKeyId"]
        .as_str()
        .map(str::to_string)
}

/// One row of the rekey table: the case, the `KmsKeyId` its `UpdateSecret` names (or none at
/// all), and the keys the secret's versions must then report.
type RekeyRow = (&'static str, Option<String>, Vec<String>);

/// A key ARN of the form `KmsKeyId` documents, for a key of this account.
fn key_arn(key_id: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/{key_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The id the `number`-th version this suite stakes is created under.
///
/// A write that stores a value has to NAME the version it creates, and
/// `ClientRequestToken` is `len 32..64` — so the token takes the 36-character form a UUID
/// has, which is the form the service's own ids take.
fn version_token(number: usize) -> String {
    format!("00000000-0000-0000-0000-{number:012}")
}

/// THE RULE over the re-keying an `UpdateSecret` performs.
///
/// The rows span what the rule generalises over: a secret re-keyed onto the Amazon Web
/// Services managed key by its alias, onto that key by the empty string the model documents
/// for it, onto another customer managed key by bare id and by ARN, and one whose update
/// names no key at all (so it stays on the key it was created with). Every row starts from a
/// secret created under a customer managed key, which is the state a key change is made from.
#[tokio::test]
async fn a_version_reports_the_key_the_secret_is_configured_with_after_a_rekey() {
    let created_under = "1234abcd-12ab-34cd-56ef-1234567890ab";
    let another_key = "9876fedc-98fe-76dc-54ba-9876543210fe";
    let another_key_arn = key_arn(another_key);
    let rows: Vec<RekeyRow> = vec![
        (
            "re-keyed onto the managed key by its alias",
            Some(DEFAULT_KMS_KEY_ALIAS.to_string()),
            vec![DEFAULT_KMS_KEY_ALIAS.to_string()],
        ),
        (
            "re-keyed onto the managed key by the empty string",
            Some(String::new()),
            vec![DEFAULT_KMS_KEY_ALIAS.to_string()],
        ),
        (
            "re-keyed onto another key by its id",
            Some(another_key.to_string()),
            vec![another_key.to_string()],
        ),
        (
            "re-keyed onto another key by its ARN",
            Some(another_key_arn.clone()),
            vec![another_key_arn.clone()],
        ),
        (
            "an update that names no key",
            None,
            vec![created_under.to_string()],
        ),
    ];

    let router = emulator();
    for (number, (case, rekey_to, expected)) in rows.iter().enumerate() {
        let name = format!("rekey/{}", case.replace(' ', "-"));
        ok(
            &router,
            case,
            "CreateSecret",
            json!({
                "Name": name.clone(),
                "SecretString": "value-0",
                "KmsKeyId": created_under,
                "ClientRequestToken": version_token(number),
            }),
        )
        .await;
        assert_eq!(
            reported_keys(&router, case, &name).await,
            vec![vec![created_under.to_string()]],
            "case {case:?}: a fresh secret's version reports the key it was created under"
        );

        let mut update = json!({
            "SecretId": name.clone(),
            "Description": "re-keyed in place",
        });
        if let Some(key) = rekey_to {
            update["KmsKeyId"] = json!(key);
        }
        ok(&router, case, "UpdateSecret", update).await;

        let reported = reported_keys(&router, case, &name).await;
        assert_eq!(
            reported,
            vec![expected.clone()],
            "case {case:?}: expected the version to report {expected:?}"
        );
        // The version's key and the secret's own configuration are ONE fact, so the two
        // reads cannot disagree about it.
        assert_eq!(
            configured_key(&router, case, &name).await.as_deref(),
            expected.first().map(String::as_str),
            "case {case:?}: DescribeSecret must report the same key the version does"
        );
    }
}

/// THE RULE's one exception: a secret whose configuration names NO key reports the default
/// key's own token, and goes on reporting it over every version it holds — until an update
/// gives it a key of its own, after which it reports that key.
#[tokio::test]
async fn only_a_secret_that_names_no_key_reports_the_default_key_token() {
    let router = emulator();
    let case = "a secret created with no key";
    let name = "rekey/no-key-at-first";
    ok(
        &router,
        case,
        "CreateSecret",
        json!({
            "Name": name,
            "SecretString": "value-0",
            "ClientRequestToken": version_token(0),
        }),
    )
    .await;
    ok(
        &router,
        case,
        "PutSecretValue",
        json!({
            "SecretId": name,
            "SecretString": "value-1",
            "ClientRequestToken": version_token(1),
        }),
    )
    .await;
    assert_eq!(
        reported_keys(&router, case, name).await,
        vec![
            vec![DEFAULT_ENCRYPTION_KEY_ID.to_string()],
            vec![DEFAULT_ENCRYPTION_KEY_ID.to_string()],
        ],
        "case {case:?}: every version of a secret that names no key reports the default token"
    );
    assert_eq!(
        configured_key(&router, case, name).await,
        None,
        "case {case:?}: such a secret reports no KmsKeyId of its own"
    );

    let case = "the same secret given a key of its own";
    let given = "alias/my-secrets";
    ok(
        &router,
        case,
        "UpdateSecret",
        json!({ "SecretId": name, "KmsKeyId": given }),
    )
    .await;
    assert_eq!(
        reported_keys(&router, case, name).await,
        vec![vec![given.to_string()], vec![given.to_string()]],
        "case {case:?}: the versions report the key the secret now names"
    );
}
