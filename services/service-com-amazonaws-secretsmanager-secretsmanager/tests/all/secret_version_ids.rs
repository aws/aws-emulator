// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListSecretVersionIds`, driven over the wire through the
//! multi-service router.
//!
//! The rules under test: a listing reports the versions the secret actually
//! holds, in the order they were created, with the staging labels each version
//! still carries; a version carrying no label is deprecated and is reported only
//! when `IncludeDeprecated` asks for it; the listing pages over `MaxResults` /
//! `NextToken` so every version appears exactly once; the `SecretId` resolves in
//! every documented form; and the listing is a report — it changes nothing a
//! later caller sees.
//!
//! Every test is a TABLE over the class its rule generalises: the number of
//! versions put, the page size, the form of the `SecretId`, the form of the KMS
//! key, whether deprecated versions are asked for.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    DEFAULT_ENCRYPTION_KEY_ID, DEFAULT_KMS_KEY_ALIAS, FORCE_DELETE_PURGE_DELAY,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for, so an ARN is checked against the region the
/// caller actually addressed rather than a default.
const REGION: &str = "us-west-2";

/// A SigV4 header naming `secretsmanager` in `REGION`; the router routes on the
/// credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must fail, and return the wire error code.
async fn err(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> String {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// The `Versions` entries a listing reports, in the order it reports them.
fn versions(parsed: &Value, case: &str) -> Vec<Value> {
    parsed
        .get("Versions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Versions in {parsed}"))
        .clone()
}

/// The `VersionId`s a listing reports, in the order it reports them.
fn version_ids(parsed: &Value, case: &str) -> Vec<String> {
    versions(parsed, case)
        .iter()
        .map(|entry| text(entry, "VersionId", case))
        .collect()
}

/// The staging labels one entry reports, empty when it reports none (a
/// deprecated version: `SecretVersionStagesType` has a minimum length of 1, so
/// an empty list is never sent).
fn entry_stages(entry: &Value) -> Vec<String> {
    entry
        .get("VersionStages")
        .and_then(Value::as_array)
        .map(|stages| {
            stages
                .iter()
                .map(|stage| stage.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create a secret holding an initial value, and return `(ARN, first VersionId)`.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> (String, String) {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "value-0"}),
        case,
    )
    .await;
    (
        text(&created, "ARN", case),
        text(&created, "VersionId", case),
    )
}

/// Put one more value on a secret, taking `AWSCURRENT`, and return its
/// `VersionId`.
async fn put(router: &MultiServiceRouter, name: &str, index: usize, case: &str) -> String {
    let put = ok(
        router,
        "PutSecretValue",
        json!({"SecretId": name, "SecretString": format!("value-{index}")}),
        case,
    )
    .await;
    text(&put, "VersionId", case)
}

/// `SecretVersionIdType` is modelled with a length bound of 32..64.
fn assert_version_id_shape(version_id: &str, case: &str) {
    assert!(
        (32..=64).contains(&version_id.len()),
        "case {case:?}: expected a VersionId of 32..64 characters, got {version_id:?} \
         ({} characters)",
        version_id.len()
    );
}

#[tokio::test]
async fn a_listing_reports_the_labelled_versions_and_hides_the_deprecated_ones() {
    // Each `PutSecretValue` takes AWSCURRENT and leaves AWSPREVIOUS on the
    // version it came from, so exactly the two newest versions carry a label and
    // every older one is deprecated. The rows run over the number of versions
    // the secret holds, from none at all up past that pair.
    for stored in 0..=5usize {
        let case = format!("{stored} version(s)");
        let router = emulator();
        let name = format!("versions/listed-{stored}");
        let mut created_ids: Vec<String> = Vec::new();
        let arn = if stored == 0 {
            // "If you include SecretString or SecretBinary then Secrets Manager
            // creates an initial secret version" — a secret created with
            // neither holds no version at all.
            let created = ok(
                &router,
                "CreateSecret",
                json!({"Name": name.clone()}),
                &case,
            )
            .await;
            text(&created, "ARN", &case)
        } else {
            let (arn, first) = create(&router, &name, &case).await;
            created_ids.push(first);
            for index in 1..stored {
                created_ids.push(put(&router, &name, index, &case).await);
            }
            arn
        };

        // Every version, deprecated ones included, in the order they were made.
        let all = ok(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": name.clone(), "IncludeDeprecated": true}),
            &case,
        )
        .await;
        assert_eq!(
            version_ids(&all, &case),
            created_ids,
            "case {case:?}: expected every created version in creation order, got {all}"
        );
        assert_eq!(
            text(&all, "ARN", &case),
            arn,
            "case {case:?}: expected the secret's ARN on the listing, got {all}"
        );
        assert_eq!(
            text(&all, "Name", &case),
            name,
            "case {case:?}: expected the secret's name on the listing, got {all}"
        );
        for entry in versions(&all, &case) {
            assert_version_id_shape(&text(&entry, "VersionId", &case), &case);
            assert!(
                entry.get("CreatedDate").and_then(Value::as_f64).is_some(),
                "case {case:?}: expected a CreatedDate on every version, got {entry}"
            );
        }
        // The labels, per version: the newest carries AWSCURRENT, the one before
        // it AWSPREVIOUS, anything older carries none.
        let labelled: Vec<Vec<String>> = versions(&all, &case)
            .iter()
            .map(entry_stages)
            .collect();
        let expected_labels: Vec<Vec<String>> = (0..stored)
            .map(|index| match stored - index {
                1 => vec!["AWSCURRENT".to_string()],
                2 => vec!["AWSPREVIOUS".to_string()],
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(
            labelled, expected_labels,
            "case {case:?}: expected the newest version to carry AWSCURRENT and the one \
             before it AWSPREVIOUS, got {all}"
        );

        // The default listing leaves the deprecated versions out, and an
        // explicit `IncludeDeprecated: false` is the same listing.
        let expected_default: Vec<String> = created_ids
            .iter()
            .skip(stored.saturating_sub(2))
            .cloned()
            .collect();
        for body in [
            json!({"SecretId": name.clone()}),
            json!({"SecretId": name.clone(), "IncludeDeprecated": false}),
        ] {
            let listed = ok(&router, "ListSecretVersionIds", body.clone(), &case).await;
            assert_eq!(
                version_ids(&listed, &case),
                expected_default,
                "case {case:?}: expected only the labelled versions for request {body}, got \
                 {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_pages_through_every_version_exactly_once() {
    let router = emulator();
    let name = "versions/paged";
    let (_, first) = create(&router, name, "setup").await;
    let mut created_ids = vec![first];
    for index in 1..5 {
        created_ids.push(put(&router, name, index, "setup").await);
    }

    // MaxResultsType is modelled with a range of 1..100, so the rows run over
    // page sizes from the published minimum up past the number of versions.
    for page_size in [1_usize, 2, 3, 5, 7] {
        let case = format!("page size {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "SecretId": name,
                "IncludeDeprecated": true,
                "MaxResults": page_size,
            });
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let listed = ok(&router, "ListSecretVersionIds", body, &case).await;
            let page = version_ids(&listed, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} versions in a page, got {page:?}"
            );
            assert!(
                !page.is_empty(),
                "case {case:?}: expected a non-empty page, got {listed}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= created_ids.len() + 1,
                "case {case:?}: pagination did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created_ids,
            "case {case:?}: expected every version exactly once in creation order"
        );
    }
}

#[tokio::test]
async fn a_next_token_the_service_never_minted_is_refused() {
    let router = emulator();
    let name = "versions/token";
    create(&router, name, "setup").await;
    // A token is opaque, so the rows are the forms a client can get one wrong
    // in: not a cursor at all, a well-formed cursor naming no version, and a
    // cursor this service really did mint — for another secret's listing, whose
    // versions this one does not hold.
    let from_another_listing = {
        let other = "versions/token-other";
        create(&router, other, "setup").await;
        for index in 1..3 {
            put(&router, other, index, "setup").await;
        }
        let all = ok(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": other, "IncludeDeprecated": true, "MaxResults": 1}),
            "setup",
        )
        .await;
        text(&all, "NextToken", "setup")
    };
    for (case, token) in [
        ("not a cursor at all", "not-a-cursor".to_string()),
        ("hex that names no version", hex_of("no-such-version")),
        ("a cursor from another listing", from_another_listing),
    ] {
        let code = err(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": name, "NextToken": token}),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException for token {token:?}, got {code}"
        );
    }
}

/// The hex encoding this service's `NextToken` cursors use, for building a
/// well-formed token that still names nothing.
fn hex_of(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn a_secret_id_is_accepted_as_a_name_a_full_arn_or_a_partial_arn() {
    let router = emulator();
    let name = "versions/lookup";
    let (arn, first) = create(&router, name, "setup").await;
    let second = put(&router, name, 1, "setup").await;
    let partial = arn
        .rsplit_once('-')
        .expect("the ARN ends in a hyphen and six characters")
        .0
        .to_string();
    for (case, secret_id) in [
        ("bare name", name.to_string()),
        ("full ARN", arn.clone()),
        ("partial ARN", partial),
    ] {
        let listed = ok(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": secret_id, "IncludeDeprecated": true}),
            case,
        )
        .await;
        assert_eq!(
            version_ids(&listed, case),
            vec![first.clone(), second.clone()],
            "case {case:?}: expected the same versions whatever form the SecretId takes, got \
             {listed}"
        );
        assert_eq!(
            text(&listed, "ARN", case),
            arn,
            "case {case:?}: expected the full ARN back, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_secret_id_that_names_nothing_is_a_resource_not_found() {
    let router = emulator();
    let name = "versions/missing";
    let (arn, _) = create(&router, name, "setup").await;
    // The rows are the ways a `SecretId` can name no stored secret: a name
    // nothing was created under, an ARN of the right shape for another secret,
    // and the ARN of a secret that has been force-deleted — which Secrets
    // Manager removes with "an asynchronous background process, so there might
    // be a short delay before the secret is permanently deleted"
    // (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's Smithy
    // model), so this row waits for the removal the clock owes.
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": name, "ForceDeleteWithoutRecovery": true}),
        "setup",
    )
    .await;
    tokio::time::sleep(FORCE_DELETE_PURGE_DELAY + Duration::from_millis(200)).await;
    for (case, secret_id) in [
        ("a name nothing was created under", "versions/never".to_string()),
        (
            "an ARN of no secret",
            format!(
                "arn:aws:secretsmanager:{REGION}:{}:secret:versions/never-AbCdEf",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("the ARN of a deleted secret", arn),
    ] {
        let code = err(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for SecretId {secret_id:?}, got \
             {code}"
        );
    }
}

#[tokio::test]
async fn every_version_reports_the_kms_key_the_secret_was_created_with() {
    // "The KMS keys used to encrypt the secret version." The key reported is the one the
    // secret's configuration NAMES, in the form the caller gave it (`KmsKeyIdType`, this
    // service's model: "the ARN, key ID, or alias") — including the Amazon Web Services
    // managed key's own alias, which is a key the caller named like any other. Only a secret
    // whose configuration names NO key at all has no identifier of the caller's to report,
    // and that is the one case the service answers with the default key's own token.
    let router = emulator();
    for (case, kms_key_id, expected) in [
        ("no key given", None, vec![DEFAULT_ENCRYPTION_KEY_ID.to_string()]),
        (
            "the default key named by its alias",
            Some(DEFAULT_KMS_KEY_ALIAS.to_string()),
            vec![DEFAULT_KMS_KEY_ALIAS.to_string()],
        ),
        (
            "a bare key id",
            Some("1234abcd-12ab-34cd-56ef-1234567890ab".to_string()),
            vec!["1234abcd-12ab-34cd-56ef-1234567890ab".to_string()],
        ),
        (
            "an alias",
            Some("alias/my-secrets".to_string()),
            vec!["alias/my-secrets".to_string()],
        ),
        (
            "a full key ARN",
            Some(format!(
                "arn:aws:kms:{REGION}:{}:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                engine::EMULATOR_ACCOUNT_ID
            )),
            vec![format!(
                "arn:aws:kms:{REGION}:{}:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                engine::EMULATOR_ACCOUNT_ID
            )],
        ),
    ] {
        let name = format!("versions/kms-{}", case.replace(' ', "-"));
        let mut body = json!({"Name": name.clone(), "SecretString": "value-0"});
        if let Some(kms_key_id) = &kms_key_id {
            body["KmsKeyId"] = json!(kms_key_id);
        }
        ok(&router, "CreateSecret", body, case).await;
        put(&router, &name, 1, case).await;

        let listed = ok(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": name.clone(), "IncludeDeprecated": true}),
            case,
        )
        .await;
        let entries = versions(&listed, case);
        assert_eq!(
            entries.len(),
            2,
            "case {case:?}: expected both versions listed, got {listed}"
        );
        for entry in entries {
            let reported: Option<Vec<String>> =
                entry.get("KmsKeyIds").and_then(Value::as_array).map(|keys| {
                    keys.iter()
                        .map(|key| key.as_str().unwrap_or_default().to_string())
                        .collect()
                });
            assert_eq!(
                reported,
                Some(expected.clone()),
                "case {case:?}: expected KmsKeyIds {expected:?}, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn listing_the_versions_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let name = "versions/reporting";
    create(&router, name, "setup").await;
    for index in 1..4 {
        put(&router, name, index, "setup").await;
    }
    // The rows are the listings a caller can ask for; none of them may move a
    // label, drop a version or change what Describe/Get reports afterwards.
    // Retrieving a value records the date of that retrieval (the one thing
    // `LastAccessedDate` reports), so the baselines are taken after it: what no
    // listing may change is everything else.
    let before_value = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": name}),
        "before",
    )
    .await;
    let before_describe = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": name}),
        "before",
    )
    .await;
    let baseline = ok(
        &router,
        "ListSecretVersionIds",
        json!({"SecretId": name, "IncludeDeprecated": true}),
        "before",
    )
    .await;
    for (case, body) in [
        ("default listing", json!({"SecretId": name})),
        (
            "with deprecated versions",
            json!({"SecretId": name, "IncludeDeprecated": true}),
        ),
        (
            "one version per page",
            json!({"SecretId": name, "IncludeDeprecated": true, "MaxResults": 1}),
        ),
    ] {
        ok(&router, "ListSecretVersionIds", body, case).await;
        let repeated = ok(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": name, "IncludeDeprecated": true}),
            case,
        )
        .await;
        assert_eq!(
            repeated, baseline,
            "case {case:?}: expected the listing to be unchanged by reading it"
        );
        let describe = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        assert_eq!(
            describe, before_describe,
            "case {case:?}: expected DescribeSecret to be unchanged by the listing"
        );
        let value = ok(&router, "GetSecretValue", json!({"SecretId": name}), case).await;
        assert_eq!(
            value, before_value,
            "case {case:?}: expected GetSecretValue to be unchanged by the listing"
        );
    }
}
