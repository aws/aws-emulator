// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Secrets Manager secret lifecycle: `CreateSecret`,
//! `GetSecretValue`, `PutSecretValue`, `DescribeSecret`, `ListSecrets` and
//! `DeleteSecret`, driven over the wire through the multi-service router.
//!
//! Every test is a TABLE over the class the rule generalises: the value's type
//! and size, the form of the `SecretId`, how many versions have been put, the
//! recovery window, the page size, the filter key. One recorded request is one
//! row.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::FORCE_DELETE_PURGE_DELAY;
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
    error_code(&parsed, case)
}

/// The `__type` discriminator of an `awsJson1_1` error body, without namespace.
fn error_code(parsed: &Value, case: &str) -> String {
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

/// The staging labels a response reports, in the order it reports them.
fn stages(parsed: &Value, member: &str, case: &str) -> Vec<String> {
    parsed
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .iter()
        .map(|stage| stage.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Seconds since the epoch, for comparing a reported date with the clock.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// Assert the ARN is the one Secrets Manager allocates: the secret's name
/// followed by a hyphen and six random alphanumeric characters, in the region
/// and account the emulator presents.
fn assert_arn_shape(arn: &str, name: &str, case: &str) {
    let expected_prefix = format!(
        "arn:aws:secretsmanager:{REGION}:{}:secret:{name}",
        engine::EMULATOR_ACCOUNT_ID
    );
    let (prefix, suffix) = arn
        .rsplit_once('-')
        .unwrap_or_else(|| panic!("case {case:?}: ARN {arn} has no six-character suffix"));
    assert_eq!(
        prefix, expected_prefix,
        "case {case:?}: expected the ARN to start {expected_prefix}, got {arn}"
    );
    assert_eq!(
        suffix.len(),
        6,
        "case {case:?}: expected a six-character ARN suffix, got {suffix:?} in {arn}"
    );
    assert!(
        suffix.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected an alphanumeric ARN suffix, got {suffix:?} in {arn}"
    );
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

/// One row of the value table: what a caller stores in a secret.
///
/// `pub(crate)` so a sibling suite under `tests/all/` varies the value the same
/// way instead of writing a second copy of the table row.
pub(crate) struct ValueCase {
    pub(crate) label: &'static str,
    secret_string: Option<String>,
    secret_binary: Option<Vec<u8>>,
}

impl ValueCase {
    pub(crate) fn string(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label,
            secret_string: Some(value.into()),
            secret_binary: None,
        }
    }

    pub(crate) fn binary(label: &'static str, value: Vec<u8>) -> Self {
        Self {
            label,
            secret_string: None,
            secret_binary: Some(value),
        }
    }

    /// The value members as they go on the wire: a blob is base64 on
    /// `awsJson1_1`.
    pub(crate) fn wire_members(&self) -> Value {
        let mut members = serde_json::Map::new();
        if let Some(secret_string) = &self.secret_string {
            members.insert("SecretString".to_string(), json!(secret_string));
        }
        if let Some(secret_binary) = &self.secret_binary {
            members.insert(
                "SecretBinary".to_string(),
                json!(base64::engine::general_purpose::STANDARD.encode(secret_binary)),
            );
        }
        Value::Object(members)
    }

    /// Assert a response carries exactly this value back.
    pub(crate) fn assert_round_trip(&self, parsed: &Value, case: &str) {
        match &self.secret_string {
            Some(expected) => assert_eq!(
                parsed.get("SecretString").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?}: expected SecretString {expected:?}, got {parsed}"
            ),
            None => assert!(
                parsed.get("SecretString").is_none(),
                "case {case:?}: expected no SecretString, got {parsed}"
            ),
        }
        match &self.secret_binary {
            Some(expected) => {
                let encoded = parsed
                    .get("SecretBinary")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no SecretBinary in {parsed}"));
                let actual = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap_or_else(|_| panic!("case {case:?}: SecretBinary {encoded} is base64"));
                assert_eq!(
                    actual, *expected,
                    "case {case:?}: expected {} SecretBinary bytes back unchanged, got {} bytes",
                    expected.len(),
                    actual.len()
                );
            }
            None => assert!(
                parsed.get("SecretBinary").is_none(),
                "case {case:?}: expected no SecretBinary, got {parsed}"
            ),
        }
    }
}

/// The value table: the rule is "whatever a caller stored comes back
/// byte-for-byte", so the rows differ along the value's type, size and content.
fn value_cases() -> Vec<ValueCase> {
    vec![
        ValueCase::string("json credentials", r#"{"user":"admin","pass":"p1"}"#),
        ValueCase::string("one character", "x"),
        ValueCase::string("non-ascii", "pässwörd-✓-🔐"),
        ValueCase::string("whitespace and newlines", "line one\nline two\t end "),
        // SecretStringType is modelled with a maximum length of 65536.
        ValueCase::string("at the modelled maximum length", "a".repeat(65_536)),
        ValueCase::binary("every byte value", (0..=255u8).collect()),
        ValueCase::binary("one zero byte", vec![0]),
        // SecretBinaryType is modelled with a maximum length of 65536.
        ValueCase::binary("at the modelled maximum length", vec![0xAB; 65_536]),
    ]
}

#[tokio::test]
async fn a_created_secret_reads_back_byte_for_byte_through_get_and_describe() {
    let router = emulator();
    for (index, value) in value_cases().iter().enumerate() {
        let case = value.label;
        let name = format!("roundtrip/{index}");
        let mut request = json!({
            "Name": name,
            "Description": format!("value case {case}"),
            "KmsKeyId": "alias/aws/secretsmanager",
            "Tags": [{"Key": "case", "Value": case}],
        });
        let Value::Object(members) = value.wire_members() else {
            unreachable!("wire_members builds an object")
        };
        for (member, wire) in members {
            request[member] = wire;
        }

        let created = ok(&router, "CreateSecret", request, case).await;
        let arn = text(&created, "ARN", case);
        let version_id = text(&created, "VersionId", case);
        assert_eq!(
            text(&created, "Name", case),
            name,
            "case {case:?}: expected the stored Name back, got {created}"
        );
        assert_arn_shape(&arn, &name, case);
        assert_version_id_shape(&version_id, case);

        // Read the value back by name, in a separate request.
        let fetched = ok(&router, "GetSecretValue", json!({"SecretId": name}), case).await;
        value.assert_round_trip(&fetched, case);
        assert_eq!(
            text(&fetched, "ARN", case),
            arn,
            "case {case:?}: GetSecretValue reported a different ARN: {fetched}"
        );
        assert_eq!(
            text(&fetched, "VersionId", case),
            version_id,
            "case {case:?}: GetSecretValue reported a different VersionId: {fetched}"
        );
        assert_eq!(
            stages(&fetched, "VersionStages", case),
            vec!["AWSCURRENT".to_string()],
            "case {case:?}: expected the initial version staged AWSCURRENT, got {fetched}"
        );
        assert!(
            fetched.get("CreatedDate").and_then(Value::as_f64).is_some(),
            "case {case:?}: expected a CreatedDate on the version, got {fetched}"
        );

        // And the metadata back by ARN.
        let described = ok(&router, "DescribeSecret", json!({"SecretId": arn}), case).await;
        assert_eq!(
            described.get("VersionIdsToStages"),
            Some(&json!({version_id.clone(): ["AWSCURRENT"]})),
            "case {case:?}: expected the one stored version staged AWSCURRENT, got {described}"
        );
        assert_eq!(
            text(&described, "Description", case),
            format!("value case {case}"),
            "case {case:?}: expected the stored Description back, got {described}"
        );
        assert_eq!(
            text(&described, "KmsKeyId", case),
            "alias/aws/secretsmanager",
            "case {case:?}: expected the stored KmsKeyId back, got {described}"
        );
        assert_eq!(
            described.get("Tags"),
            Some(&json!([{"Key": "case", "Value": case}])),
            "case {case:?}: expected the stored Tags back, got {described}"
        );
        assert_eq!(
            described.get("RotationEnabled").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected rotation to be off, got {described}"
        );
        assert!(
            described.get("DeletedDate").is_none(),
            "case {case:?}: a live secret must report no DeletedDate, got {described}"
        );
        assert_eq!(
            described.get("PrimaryRegion"),
            None,
            "case {case:?}: PrimaryRegion is part of the replication view, so an unreplicated \
             secret must not report one, got {described}"
        );
        assert_eq!(
            described.get("ReplicationStatus"),
            None,
            "case {case:?}: an unreplicated secret must report no ReplicationStatus, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn a_secret_value_is_either_a_string_or_a_blob_and_never_both() {
    let router = emulator();
    let both = err(
        &router,
        "CreateSecret",
        json!({"Name": "both", "SecretString": "s", "SecretBinary": "AA=="}),
        "CreateSecret with both value members",
    )
    .await;
    assert_eq!(
        both, "InvalidParameterException",
        "expected InvalidParameterException for a request carrying both value members, got {both}"
    );

    // A secret created with no value has no version at all, so there is no
    // AWSCURRENT to read, and a later PutSecretValue creates the first version.
    let created = ok(
        &router,
        "CreateSecret",
        json!({"Name": "valueless"}),
        "CreateSecret with no value",
    )
    .await;
    assert!(
        created.get("VersionId").is_none(),
        "expected no VersionId for a secret created with no value, got {created}"
    );
    let missing = err(
        &router,
        "GetSecretValue",
        json!({"SecretId": "valueless"}),
        "GetSecretValue on a valueless secret",
    )
    .await;
    assert_eq!(
        missing, "ResourceNotFoundException",
        "expected ResourceNotFoundException with no staged version, got {missing}"
    );

    let neither = err(
        &router,
        "PutSecretValue",
        json!({"SecretId": "valueless"}),
        "PutSecretValue with no value member",
    )
    .await;
    assert_eq!(
        neither, "InvalidRequestException",
        "expected InvalidRequestException for a put carrying no value, got {neither}"
    );

    let put = ok(
        &router,
        "PutSecretValue",
        json!({"SecretId": "valueless", "SecretString": "first"}),
        "PutSecretValue on a valueless secret",
    )
    .await;
    let fetched = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": "valueless"}),
        "GetSecretValue after the first put",
    )
    .await;
    assert_eq!(
        text(&fetched, "VersionId", "first put"),
        text(&put, "VersionId", "first put"),
        "expected the first put version to be AWSCURRENT, got {fetched}"
    );
}

#[tokio::test]
async fn a_secret_id_is_accepted_as_a_name_a_full_arn_or_a_partial_arn() {
    let router = emulator();
    let name = "lookup/forms";
    let created = ok(
        &router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "v"}),
        "create for lookup",
    )
    .await;
    let arn = text(&created, "ARN", "create for lookup");
    let version_id = text(&created, "VersionId", "create for lookup");
    // The partial ARN a caller that never saw the six random characters builds.
    let partial_arn = arn
        .rsplit_once('-')
        .map(|(prefix, _)| prefix.to_string())
        .expect("the ARN carries a six-character suffix");

    for (case, secret_id) in [
        ("bare name", name.to_string()),
        ("full ARN", arn.clone()),
        ("partial ARN", partial_arn),
    ] {
        let fetched = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            text(&fetched, "VersionId", case),
            version_id,
            "case {case:?}: expected the same version for SecretId {secret_id:?}, got {fetched}"
        );
        let described = ok(
            &router,
            "DescribeSecret",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            text(&described, "ARN", case),
            arn,
            "case {case:?}: expected the same secret for SecretId {secret_id:?}, got {described}"
        );
    }

    for (case, operation) in [
        ("unknown name", "GetSecretValue"),
        ("unknown name", "DescribeSecret"),
    ] {
        let code = err(
            &router,
            operation,
            json!({"SecretId": "nothing/here"}),
            case,
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from {operation}, got {code}"
        );
    }
}

#[tokio::test]
async fn each_put_takes_awscurrent_and_demotes_the_version_it_came_from() {
    // The rule holds for any number of puts, so the table runs over that count.
    for puts in 1..=4usize {
        let case = format!("{puts} put(s)");
        let router = emulator();
        let name = "staging/labels";
        let created = ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "value 0"}),
            &case,
        )
        .await;
        let mut version_ids = vec![text(&created, "VersionId", &case)];
        for round in 1..=puts {
            let put = ok(
                &router,
                "PutSecretValue",
                json!({"SecretId": name, "SecretString": format!("value {round}")}),
                &case,
            )
            .await;
            assert_eq!(
                stages(&put, "VersionStages", &case),
                vec!["AWSCURRENT".to_string()],
                "case {case:?}: expected the new version staged AWSCURRENT, got {put}"
            );
            version_ids.push(text(&put, "VersionId", &case));
        }

        let current = ok(&router, "GetSecretValue", json!({"SecretId": name}), &case).await;
        assert_eq!(
            text(&current, "SecretString", &case),
            format!("value {puts}"),
            "case {case:?}: expected the newest value as AWSCURRENT, got {current}"
        );
        assert_eq!(
            text(&current, "VersionId", &case),
            version_ids[puts],
            "case {case:?}: expected the newest version as AWSCURRENT, got {current}"
        );

        let previous = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": name, "VersionStage": "AWSPREVIOUS"}),
            &case,
        )
        .await;
        assert_eq!(
            text(&previous, "SecretString", &case),
            format!("value {}", puts - 1),
            "case {case:?}: expected the version AWSCURRENT came from as AWSPREVIOUS, got \
             {previous}"
        );

        // Exactly the newest two versions carry a staging label; the older ones
        // are deprecated and left out, though they are still addressable.
        let described = ok(&router, "DescribeSecret", json!({"SecretId": name}), &case).await;
        let live = described
            .get("VersionIdsToStages")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no VersionIdsToStages in {described}"));
        assert_eq!(
            live.len(),
            2,
            "case {case:?}: expected exactly AWSCURRENT and AWSPREVIOUS to be live, got \
             {described}"
        );
        assert_eq!(
            live.get(&version_ids[puts]),
            Some(&json!(["AWSCURRENT"])),
            "case {case:?}: expected version {} staged AWSCURRENT, got {described}",
            version_ids[puts]
        );
        assert_eq!(
            live.get(&version_ids[puts - 1]),
            Some(&json!(["AWSPREVIOUS"])),
            "case {case:?}: expected version {} staged AWSPREVIOUS, got {described}",
            version_ids[puts - 1]
        );

        for (round, version_id) in version_ids.iter().enumerate() {
            let by_id = ok(
                &router,
                "GetSecretValue",
                json!({"SecretId": name, "VersionId": version_id}),
                &case,
            )
            .await;
            assert_eq!(
                text(&by_id, "SecretString", &case),
                format!("value {round}"),
                "case {case:?}: expected version {version_id} to still hold its own value, got \
                 {by_id}"
            );
        }
    }
}

#[tokio::test]
async fn a_named_staging_label_moves_to_the_version_that_claims_it() {
    let router = emulator();
    let name = "staging/named";
    ok(
        &router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "first"}),
        "create for named labels",
    )
    .await;

    // A put that claims its own labels leaves AWSCURRENT where it was.
    let pending = ok(
        &router,
        "PutSecretValue",
        json!({"SecretId": name, "SecretString": "candidate", "VersionStages": ["AWSPENDING"]}),
        "put claiming AWSPENDING",
    )
    .await;
    let pending_id = text(&pending, "VersionId", "put claiming AWSPENDING");
    let current = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": name}),
        "AWSCURRENT after an AWSPENDING put",
    )
    .await;
    assert_eq!(
        text(&current, "SecretString", "AWSCURRENT after an AWSPENDING put"),
        "first",
        "expected AWSCURRENT to stay on the first version, got {current}"
    );

    // And the same label claimed again moves off the version that held it.
    let moved = ok(
        &router,
        "PutSecretValue",
        json!({"SecretId": name, "SecretString": "candidate two", "VersionStages": ["AWSPENDING"]}),
        "put re-claiming AWSPENDING",
    )
    .await;
    let moved_id = text(&moved, "VersionId", "put re-claiming AWSPENDING");
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": name}),
        "describe after moving AWSPENDING",
    )
    .await;
    let live = described
        .get("VersionIdsToStages")
        .and_then(Value::as_object)
        .expect("VersionIdsToStages is reported");
    assert_eq!(
        live.get(&moved_id),
        Some(&json!(["AWSPENDING"])),
        "expected AWSPENDING on the version that claimed it last, got {described}"
    );
    assert!(
        !live.contains_key(&pending_id),
        "expected the version AWSPENDING left to be deprecated, got {described}"
    );
    let still_readable = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": name, "VersionId": pending_id}),
        "read a deprecated version by id",
    )
    .await;
    assert_eq!(
        text(&still_readable, "SecretString", "read a deprecated version by id"),
        "candidate",
        "expected a deprecated version to keep its value, got {still_readable}"
    );
    let by_stage = err(
        &router,
        "GetSecretValue",
        json!({"SecretId": name, "VersionStage": "AWSNOSUCHLABEL"}),
        "read an unused staging label",
    )
    .await;
    assert_eq!(
        by_stage, "ResourceNotFoundException",
        "expected ResourceNotFoundException for a label on no version, got {by_stage}"
    );
}

#[tokio::test]
async fn create_secret_is_idempotent_only_for_a_repeat_of_the_same_version() {
    let router = emulator();
    let token = "11111111-2222-3333-4444-555555555555";
    let first = ok(
        &router,
        "CreateSecret",
        json!({"Name": "idempotent", "SecretString": "v1", "ClientRequestToken": token}),
        "first create",
    )
    .await;
    let version_id = text(&first, "VersionId", "first create");
    assert_eq!(
        version_id, token,
        "expected the ClientRequestToken to become the version id, got {first}"
    );

    for (case, body, expected) in [
        (
            "same token and same value",
            json!({"Name": "idempotent", "SecretString": "v1", "ClientRequestToken": token}),
            None,
        ),
        (
            "same token and a different value",
            json!({"Name": "idempotent", "SecretString": "v2", "ClientRequestToken": token}),
            Some("ResourceExistsException"),
        ),
        (
            // A create that names NO token of its own creates no version at all:
            // it is refused for the missing token, which
            // `secret_client_request_token` is the suite for, so the rows here all
            // name one.
            "a different token",
            json!({
                "Name": "idempotent",
                "SecretString": "v1",
                "ClientRequestToken": "99999999-2222-3333-4444-555555555555",
            }),
            Some("ResourceExistsException"),
        ),
    ] {
        match expected {
            None => {
                let repeated = ok(&router, "CreateSecret", body, case).await;
                assert_eq!(
                    text(&repeated, "VersionId", case),
                    version_id,
                    "case {case:?}: expected the stored version back, got {repeated}"
                );
                assert_eq!(
                    text(&repeated, "ARN", case),
                    text(&first, "ARN", case),
                    "case {case:?}: expected the stored ARN back, got {repeated}"
                );
            }
            Some(code) => {
                let actual = err(&router, "CreateSecret", body, case).await;
                assert_eq!(
                    actual, code,
                    "case {case:?}: expected {code}, got {actual}"
                );
            }
        }
    }

    // The secret still holds exactly the one version the first create made.
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": "idempotent"}),
        "after the repeats",
    )
    .await;
    assert_eq!(
        described.get("VersionIdsToStages"),
        Some(&json!({token: ["AWSCURRENT"]})),
        "expected the repeats to have added no version, got {described}"
    );
}

#[tokio::test]
async fn delete_secret_honours_the_published_recovery_window_range() {
    // The documented range is 7..30 days inclusive, with 30 the default; the
    // rows are each published bound, one inside, and one past each bound.
    // (DeleteSecretRequest$RecoveryWindowInDays, this service's Smithy model.)
    for (case, window, expected_days) in [
        ("the published minimum", Some(7_i64), Some(7_i64)),
        ("inside the range", Some(15), Some(15)),
        ("the published maximum", Some(30), Some(30)),
        ("absent, so the documented default", None, Some(30)),
        ("one day past the minimum", Some(6), None),
        ("one day past the maximum", Some(31), None),
        ("zero", Some(0), None),
    ] {
        let router = emulator();
        let name = "window/case";
        ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "v"}),
            case,
        )
        .await;
        let mut body = json!({"SecretId": name});
        if let Some(window) = window {
            body["RecoveryWindowInDays"] = json!(window);
        }

        match expected_days {
            None => {
                let code = err(&router, "DeleteSecret", body, case).await;
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case:?}: expected InvalidParameterException for window {window:?}, \
                     got {code}"
                );
                // A refused delete leaves the secret readable.
                let fetched = ok(
                    &router,
                    "GetSecretValue",
                    json!({"SecretId": name}),
                    case,
                )
                .await;
                assert_eq!(
                    text(&fetched, "SecretString", case),
                    "v",
                    "case {case:?}: a refused delete must change nothing, got {fetched}"
                );
            }
            Some(days) => {
                let requested_at = epoch_seconds(SystemTime::now());
                let deleted = ok(&router, "DeleteSecret", body, case).await;
                let deletion_date = deleted
                    .get("DeletionDate")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case:?}: no DeletionDate in {deleted}"));
                let expected = requested_at + (days * 86_400) as f64;
                assert!(
                    (deletion_date - expected).abs() < 300.0,
                    "case {case:?}: expected a DeletionDate near {expected} (request time plus \
                     {days} days), got {deletion_date}"
                );
                // DescribeSecret reports the instant the deletion was REQUESTED
                // as DeletedDate — "the date and time the deletion of the secret
                // occurred" — so the recovery window is the span between it and
                // the DeletionDate the delete reported.
                let described = ok(
                    &router,
                    "DescribeSecret",
                    json!({"SecretId": name}),
                    case,
                )
                .await;
                let marked = described
                    .get("DeletedDate")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case:?}: no DeletedDate in {described}"));
                assert!(
                    (marked - requested_at).abs() < 300.0,
                    "case {case:?}: expected DeletedDate near the request time {requested_at}, \
                     got {marked}"
                );
                let window_days = ((deletion_date - marked) / 86_400.0).round() as i64;
                assert_eq!(
                    window_days, days,
                    "case {case:?}: expected DeletionDate - DeletedDate to measure {days} days, \
                     got {window_days} ({described})"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_recovery_window_and_a_forced_delete_cannot_both_be_asked_for() {
    let router = emulator();
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "conflict", "SecretString": "v"}),
        "create for the conflicting delete",
    )
    .await;
    for (case, force) in [("with force true", true), ("with force false", false)] {
        let code = err(
            &router,
            "DeleteSecret",
            json!({
                "SecretId": "conflict",
                "RecoveryWindowInDays": 7,
                "ForceDeleteWithoutRecovery": force,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException when both are named, got {code}"
        );
    }
}

#[tokio::test]
async fn a_secret_scheduled_for_deletion_is_hidden_but_still_described() {
    let router = emulator();
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "scheduled", "SecretString": "v"}),
        "create the scheduled secret",
    )
    .await;
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "untouched", "SecretString": "v"}),
        "create the untouched secret",
    )
    .await;
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "scheduled", "RecoveryWindowInDays": 7}),
        "schedule the deletion",
    )
    .await;

    let listed = ok(&router, "ListSecrets", json!({}), "list after scheduling").await;
    assert_eq!(
        listed_names(&listed),
        vec!["untouched".to_string()],
        "expected a scheduled secret to be hidden from ListSecrets, got {listed}"
    );
    let with_deleted = ok(
        &router,
        "ListSecrets",
        json!({"IncludePlannedDeletion": true}),
        "list including planned deletion",
    )
    .await;
    assert_eq!(
        listed_names(&with_deleted),
        vec!["scheduled".to_string(), "untouched".to_string()],
        "expected IncludePlannedDeletion to show it, got {with_deleted}"
    );

    for (case, operation) in [
        ("read the value", "GetSecretValue"),
        ("put a new value", "PutSecretValue"),
    ] {
        let mut body = json!({"SecretId": "scheduled"});
        if operation == "PutSecretValue" {
            body["SecretString"] = json!("another");
        }
        let code = err(&router, operation, body, case).await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException from {operation} on a secret \
             marked for deletion, got {code}"
        );
    }

    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": "scheduled"}),
        "describe the scheduled secret",
    )
    .await;
    assert!(
        described.get("DeletedDate").and_then(Value::as_f64).is_some(),
        "expected DescribeSecret to still report the scheduled secret with a DeletedDate, got \
         {described}"
    );

    // A name scheduled for deletion cannot be reused until it is gone.
    let reused = err(
        &router,
        "CreateSecret",
        json!({"Name": "scheduled", "SecretString": "v"}),
        "recreate a scheduled name",
    )
    .await;
    assert_eq!(
        reused, "InvalidRequestException",
        "expected InvalidRequestException when recreating a scheduled name, got {reused}"
    );
}

#[tokio::test]
async fn a_forced_delete_marks_the_secret_and_the_clock_then_removes_it() {
    // "Secrets Manager performs the actual deletion with an asynchronous
    // background process, so there might be a short delay before the secret is
    // permanently deleted." (`DeleteSecretRequest$ForceDeleteWithoutRecovery`,
    // this service's Smithy model.) So a forced delete skips the recovery window
    // but is not instantaneous: the secret is marked for deletion as of the
    // call, and the clock owes its removal a short span later.
    let router = emulator();
    let created = ok(
        &router,
        "CreateSecret",
        json!({"Name": "forced", "SecretString": "v"}),
        "create for the forced delete",
    )
    .await;
    let arn = text(&created, "ARN", "create for the forced delete");
    let started = Instant::now();
    let deleted = ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "forced", "ForceDeleteWithoutRecovery": true}),
        "force delete",
    )
    .await;
    assert_eq!(
        text(&deleted, "ARN", "force delete"),
        arn,
        "expected the deleted secret's ARN back, got {deleted}"
    );

    // Straight after the call the secret is still THERE, marked for deletion:
    // the value cannot be read, and the refusal is about its state, not about a
    // secret that does not exist. Only asserted while the span cannot yet have
    // elapsed, so a loaded machine never turns this into a false failure.
    if started.elapsed() < FORCE_DELETE_PURGE_DELAY {
        let case = "read the value straight after the forced delete";
        let (status, body) = call(&router, "GetSecretValue", &json!({"SecretId": "forced"})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 client error, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body, case),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException for a secret marked for deletion, \
             got {body}"
        );
        let described = ok(
            &router,
            "DescribeSecret",
            json!({"SecretId": "forced"}),
            "describe straight after the forced delete",
        )
        .await;
        assert!(
            described.get("DeletedDate").is_some(),
            "case {case:?}: expected the secret to still be reported with a DeletedDate, got \
             {described}"
        );
    }

    // Once the span has elapsed the removal the clock owed has happened.
    tokio::time::sleep(FORCE_DELETE_PURGE_DELAY + Duration::from_millis(200)).await;
    for (case, operation) in [
        ("read the value", "GetSecretValue"),
        ("describe it", "DescribeSecret"),
    ] {
        let code = err(&router, operation, json!({"SecretId": "forced"}), case).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from {operation} once the forced \
             delete has been materialised, got {code}"
        );
    }
    let listed = ok(
        &router,
        "ListSecrets",
        json!({"IncludePlannedDeletion": true}),
        "list after the forced delete",
    )
    .await;
    assert!(
        listed_names(&listed).is_empty(),
        "expected no secrets after the forced delete, got {listed}"
    );

    // "If you forcibly delete an already deleted or nonexistent secret, the
    // operation does not return ResourceNotFoundException." The delete succeeds
    // having deleted nothing, and answers for the secret the request NAMED:
    // `DeleteSecretResponse` carries ARN, Name and DeletionDate, so a teardown
    // path reading them back works whether or not the secret was ever created.
    let repeated = ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "forced", "ForceDeleteWithoutRecovery": true}),
        "force delete a secret that is gone",
    )
    .await;
    assert_eq!(
        repeated.get("Name").and_then(Value::as_str),
        Some("forced"),
        "expected the forced delete of a secret that is gone to answer its Name, got {repeated}"
    );
    let repeated_arn = repeated
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an ARN in the forced delete answer, got {repeated}"));
    assert!(
        repeated_arn.starts_with(&format!(
            "arn:aws:secretsmanager:{REGION}:{}:secret:forced-",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "expected an ARN for the named secret in the signed Region, got {repeated_arn:?}"
    );
    assert!(
        repeated.get("DeletionDate").is_some(),
        "expected a DeletionDate in the forced delete answer, got {repeated}"
    );
    // And it created nothing: the name is still free.
    let after = ok(
        &router,
        "ListSecrets",
        json!({"IncludePlannedDeletion": true}),
        "list after forcing the delete of a secret that is gone",
    )
    .await;
    assert!(
        listed_names(&after).is_empty(),
        "expected the forced delete of an absent secret to create nothing, got {after}"
    );

    // The name is free again, and the new secret gets a different ARN.
    let recreated = ok(
        &router,
        "CreateSecret",
        json!({"Name": "forced", "SecretString": "v"}),
        "recreate after the forced delete",
    )
    .await;
    assert_ne!(
        text(&recreated, "ARN", "recreate after the forced delete"),
        arn,
        "expected a fresh ARN after recreating the name, got {recreated}"
    );
}

/// The names `ListSecrets` reported, in the order it reported them.
fn listed_names(parsed: &Value) -> Vec<String> {
    parsed
        .get("SecretList")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry
                        .get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn list_secrets_pages_through_every_secret_exactly_once() {
    let names: Vec<String> = (0..5).map(|index| format!("page/{index}")).collect();
    // MaxResultsType is modelled with a range of 1..100, so the rows run over
    // page sizes from the published minimum up past the number of secrets.
    for page_size in [1_usize, 2, 3, 5, 7] {
        let case = format!("page size {page_size}");
        let router = emulator();
        for name in &names {
            ok(
                &router,
                "CreateSecret",
                json!({"Name": name, "SecretString": name}),
                &case,
            )
            .await;
        }

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"MaxResults": page_size});
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let listed = ok(&router, "ListSecrets", body, &case).await;
            let page = listed_names(&listed);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries in a page, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: pagination did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, names,
            "case {case:?}: expected every secret exactly once, in name order, got {seen:?}"
        );
    }

    // A token this service did not mint is refused rather than silently reset.
    let router = emulator();
    let code = err(
        &router,
        "ListSecrets",
        json!({"NextToken": "not-a-cursor"}),
        "a token the service never minted",
    )
    .await;
    assert_eq!(
        code, "InvalidNextTokenException",
        "expected InvalidNextTokenException for a token the service never minted, got {code}"
    );
}

#[tokio::test]
async fn list_secrets_sorts_by_the_requested_attribute() {
    let router = emulator();
    for name in ["sort/c", "sort/a", "sort/b"] {
        ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "v"}),
            "create for sorting",
        )
        .await;
    }
    // SortBy and SortOrder accept the values in SORT_BY_TYPE_VALUES and
    // SORT_ORDER_TYPE_VALUES, from this service's model.
    for (case, body, expected) in [
        (
            "no sort",
            json!({}),
            vec!["sort/a", "sort/b", "sort/c"],
        ),
        (
            "by name ascending",
            json!({"SortBy": "name", "SortOrder": "asc"}),
            vec!["sort/a", "sort/b", "sort/c"],
        ),
        (
            "by name descending",
            json!({"SortBy": "name", "SortOrder": "desc"}),
            vec!["sort/c", "sort/b", "sort/a"],
        ),
        (
            "by created date descending",
            json!({"SortBy": "created-date", "SortOrder": "desc"}),
            vec!["sort/b", "sort/a", "sort/c"],
        ),
    ] {
        let listed = ok(&router, "ListSecrets", body, case).await;
        assert_eq!(
            listed_names(&listed),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected {expected:?}, got {listed}"
        );
    }
}

#[tokio::test]
async fn list_secrets_filters_by_every_documented_filter_key() {
    let router = emulator();
    ok(
        &router,
        "CreateSecret",
        json!({
            "Name": "alpha/one",
            "SecretString": "v",
            "Description": "Production database",
            "Tags": [{"Key": "env", "Value": "prod"}],
        }),
        "create alpha",
    )
    .await;
    ok(
        &router,
        "CreateSecret",
        json!({
            "Name": "beta/two",
            "SecretString": "v",
            "Description": "staging cache",
            "Tags": [{"Key": "team", "Value": "core"}],
        }),
        "create beta",
    )
    .await;
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "gamma/three", "SecretString": "v"}),
        "create gamma",
    )
    .await;

    // The per-key rules come from Filter$Key in this service's model: name,
    // tag-key, tag-value, primary-region and owning-service are case-sensitive
    // prefix matches, description is a prefix match that is not case-sensitive,
    // and `all` breaks the value into words and searches every attribute, not
    // case-sensitive. The two keys that search text — description and `all` —
    // match a prefix of any WORD of what they search, which is the unit a search
    // of a sentence selects by. A value prefixed `!` negates the match.
    for (case, key, value, expected) in [
        ("name, prefix", "name", "alpha", vec!["alpha/one"]),
        ("name, case-sensitive", "name", "ALPHA", vec![]),
        (
            "description, not case-sensitive",
            "description",
            "PRODUCTION",
            vec!["alpha/one"],
        ),
        (
            "description, a prefix of a later word",
            "description",
            "databa",
            vec!["alpha/one"],
        ),
        (
            "description, a word it does not carry",
            "description",
            "atabase",
            vec![],
        ),
        ("tag-key", "tag-key", "env", vec!["alpha/one"]),
        ("tag-value", "tag-value", "pro", vec!["alpha/one"]),
        (
            "primary-region",
            "primary-region",
            "us-west",
            vec!["alpha/one", "beta/two", "gamma/three"],
        ),
        ("owning-service", "owning-service", "secretsmanager", vec![]),
        ("all, over the name", "all", "beta", vec!["beta/two"]),
        (
            "all, over the description and not case-sensitive",
            "all",
            "STAGING",
            vec!["beta/two"],
        ),
        (
            "negated name",
            "name",
            "!alpha",
            vec!["beta/two", "gamma/three"],
        ),
    ] {
        let listed = ok(
            &router,
            "ListSecrets",
            json!({"Filters": [{"Key": key, "Values": [value]}]}),
            case,
        )
        .await;
        assert_eq!(
            listed_names(&listed),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: filter {key}={value:?} expected {expected:?}, got {listed}"
        );
    }

    // Values within one filter are alternatives; separate filters all hold.
    let either = ok(
        &router,
        "ListSecrets",
        json!({"Filters": [{"Key": "name", "Values": ["alpha", "gamma"]}]}),
        "two values for one key",
    )
    .await;
    assert_eq!(
        listed_names(&either),
        vec!["alpha/one".to_string(), "gamma/three".to_string()],
        "expected either value to match, got {either}"
    );
    let both = ok(
        &router,
        "ListSecrets",
        json!({"Filters": [
            {"Key": "name", "Values": ["alpha"]},
            {"Key": "tag-value", "Values": ["core"]},
        ]}),
        "two filters",
    )
    .await;
    assert!(
        listed_names(&both).is_empty(),
        "expected both filters to have to hold, got {both}"
    );

    // A key outside `FilterNameStringType` never reaches the handler: the
    // boundary refuses it against the modelled enum.
    let (status, parsed) = call(
        &router,
        "ListSecrets",
        &json!({"Filters": [{"Key": "nonsense", "Values": ["alpha"]}]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a filter key outside the modelled enum to be refused, got {status} with body \
         {parsed}"
    );
}

#[tokio::test]
async fn filtering_a_non_ascii_attribute_never_slices_a_character_in_half() {
    // A filter value's byte length can land inside a multi-byte character of
    // the attribute it is matched against, so the rows differ along the value's
    // byte length (one, two, several) against attributes whose first character
    // is two, three or four bytes wide.
    let router = emulator();
    ok(
        &router,
        "CreateSecret",
        json!({
            "Name": "unicorn/secret",
            "SecretString": "v",
            "Description": "Ünicorn café 🔐 vault",
            "Tags": [{"Key": "éq", "Value": "✓ok"}],
        }),
        "create the non-ascii secret",
    )
    .await;
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "ascii/secret", "SecretString": "v", "Description": "plain"}),
        "create the ascii secret",
    )
    .await;

    for (case, key, value, expected) in [
        ("description, one byte", "description", "U", vec![]),
        ("description, two bytes", "description", "Un", vec![]),
        ("description, several bytes", "description", "Unicorn", vec![]),
        (
            "description, matching the ascii secret",
            "description",
            "pla",
            vec!["ascii/secret"],
        ),
        ("tag-key, one byte", "tag-key", "e", vec![]),
        ("tag-value, one byte", "tag-value", "o", vec![]),
        ("all, one byte matching nothing", "all", "Z", vec![]),
        (
            "all, one byte inside a character",
            "all",
            "U",
            vec!["ascii/secret", "unicorn/secret"],
        ),
        (
            "all, over an attribute both secrets share",
            "all",
            "us",
            vec!["ascii/secret", "unicorn/secret"],
        ),
        ("all, several words", "all", "plain us", vec!["ascii/secret"]),
    ] {
        let listed = ok(
            &router,
            "ListSecrets",
            json!({"Filters": [{"Key": key, "Values": [value]}]}),
            case,
        )
        .await;
        assert_eq!(
            listed_names(&listed),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: filter {key}={value:?} expected {expected:?}, got {listed}"
        );
    }

    // A non-ASCII filter VALUE never reaches the handler at all:
    // `FilterValueStringType` is modelled with an ASCII-only pattern.
    let (status, parsed) = call(
        &router,
        "ListSecrets",
        &json!({"Filters": [{"Key": "description", "Values": ["Ün"]}]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a non-ascii filter value to be refused against the modelled pattern, got \
         {status} with body {parsed}"
    );
}

#[tokio::test]
async fn a_secret_name_is_accepted_only_over_the_documented_charset() {
    // "The secret name can contain ASCII letters, numbers, and the following
    // characters: /_+=.@-" (CreateSecretRequest$Name, this service's Smithy
    // model). The rows cover each documented character, and one character of
    // each class the documentation leaves out.
    let router = emulator();
    for (case, name, accepted) in [
        ("letters and digits", "Secret123", true),
        ("every documented extra character", "a/b_c+d=e.f@g-h", true),
        ("a single character", "s", true),
        ("a space", "my secret", false),
        ("a colon", "my:secret", false),
        ("a percent sign", "my%secret", false),
        ("a non-ascii letter", "Ünicorn", false),
        ("an emoji", "secret🔐", false),
        ("an ARN-shaped name", "arn:aws:secretsmanager:::secret:x", false),
    ] {
        let body = json!({"Name": name, "SecretString": "v"});
        if accepted {
            let created = ok(&router, "CreateSecret", body, case).await;
            assert_eq!(
                text(&created, "Name", case),
                name,
                "case {case:?}: expected the stored Name back, got {created}"
            );
            assert_arn_shape(&text(&created, "ARN", case), name, case);
        } else {
            let code = err(&router, "CreateSecret", body, case).await;
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected ValidationException for name {name:?}, got {code}"
            );
            let missing = err(
                &router,
                "DescribeSecret",
                json!({"SecretId": name}),
                case,
            )
            .await;
            assert_eq!(
                missing, "ResourceNotFoundException",
                "case {case:?}: a refused name must store nothing, got {missing}"
            );
        }
    }
}

#[tokio::test]
async fn the_store_is_bounded_and_refuses_growth_with_the_modelled_quota_error() {
    let router = emulator();
    // Fill the store to the emulator's own bound.
    for index in 0..1_000 {
        ok(
            &router,
            "CreateSecret",
            json!({"Name": format!("quota/{index:04}"), "SecretString": "v"}),
            "fill the store",
        )
        .await;
    }
    let refused = err(
        &router,
        "CreateSecret",
        json!({"Name": "quota/one-too-many", "SecretString": "v"}),
        "one secret past the bound",
    )
    .await;
    assert_eq!(
        refused, "LimitExceededException",
        "expected LimitExceededException once the store is full, got {refused}"
    );
    // Releasing one record makes room again — once the forced deletion the
    // clock owes has been materialised, a short span after the call.
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "quota/0000", "ForceDeleteWithoutRecovery": true}),
        "release one record",
    )
    .await;
    tokio::time::sleep(FORCE_DELETE_PURGE_DELAY + Duration::from_millis(200)).await;
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "quota/one-too-many", "SecretString": "v"}),
        "create after releasing a record",
    )
    .await;
}

#[tokio::test]
async fn one_secret_holds_a_bounded_number_of_versions() {
    let router = emulator();
    let name = "versions/bounded";
    ok(
        &router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "value 0"}),
        "create for the version bound",
    )
    .await;
    // Each put naming a label of its own leaves every earlier version labelled,
    // so nothing is ever deprecated and the versions accumulate — up to the
    // emulator's own per-secret bound of 100.
    for index in 1..100 {
        ok(
            &router,
            "PutSecretValue",
            json!({
                "SecretId": name,
                "SecretString": format!("value {index}"),
                "VersionStages": [format!("LABEL{index}")],
            }),
            "fill the versions",
        )
        .await;
    }
    let refused = err(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": name,
            "SecretString": "one too many",
            "VersionStages": ["LABEL100"],
        }),
        "one version past the bound",
    )
    .await;
    assert_eq!(
        refused, "LimitExceededException",
        "expected LimitExceededException once the secret is full of labelled versions, got \
         {refused}"
    );
    // A plain put is refused too at the bound: the version AWSCURRENT is taken
    // from becomes AWSPREVIOUS, so it frees nothing.
    let plain = err(
        &router,
        "PutSecretValue",
        json!({"SecretId": name, "SecretString": "one too many"}),
        "a plain put at the bound",
    )
    .await;
    assert_eq!(
        plain, "LimitExceededException",
        "expected LimitExceededException for a plain put at the bound, got {plain}"
    );

    // A put that takes an existing label away frees that version, so it goes
    // through and the secret stays at the bound.
    let reused = ok(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": name,
            "SecretString": "relabelled",
            "VersionStages": ["LABEL1"],
        }),
        "put re-claiming a label at the bound",
    )
    .await;
    let by_stage = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": name, "VersionStage": "LABEL1"}),
        "read the re-claimed label",
    )
    .await;
    assert_eq!(
        text(&by_stage, "VersionId", "read the re-claimed label"),
        text(&reused, "VersionId", "read the re-claimed label"),
        "expected LABEL1 on the version that claimed it last, got {by_stage}"
    );
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": name}),
        "describe at the bound",
    )
    .await;
    let live = described
        .get("VersionIdsToStages")
        .and_then(Value::as_object)
        .expect("VersionIdsToStages is reported");
    assert_eq!(
        live.len(),
        100,
        "expected the secret to stay at its bound of 100 versions, got {}",
        live.len()
    );
}

#[tokio::test]
async fn a_reporting_operation_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "stable", "SecretString": "v", "Description": "unchanging"}),
        "create for the read-stability check",
    )
    .await;

    // Retrieving a value records the date of that retrieval, the one thing
    // `LastAccessedDate` reports ("This field is omitted if the secret has never
    // been retrieved in the Region", `DescribeSecretResponse$LastAccessedDate`).
    // The baseline below is therefore taken once a retrieval has happened: what
    // no read may change is everything else, this read included.
    ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": "stable"}),
        "the first retrieval",
    )
    .await;
    let first = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": "stable"}),
        "describe after the first retrieval",
    )
    .await;
    assert!(
        first.get("LastAccessedDate").is_some(),
        "expected the retrieval to be recorded as a LastAccessedDate, got {first}"
    );
    for case in ["after a get", "after a list", "after a second describe"] {
        match case {
            "after a get" => {
                ok(
                    &router,
                    "GetSecretValue",
                    json!({"SecretId": "stable"}),
                    case,
                )
                .await;
            }
            "after a list" => {
                ok(&router, "ListSecrets", json!({}), case).await;
            }
            _ => {
                ok(
                    &router,
                    "DescribeSecret",
                    json!({"SecretId": "stable"}),
                    case,
                )
                .await;
            }
        }
        let again = ok(
            &router,
            "DescribeSecret",
            json!({"SecretId": "stable"}),
            case,
        )
        .await;
        assert_eq!(
            again, first,
            "case {case:?}: a read changed what DescribeSecret reports: expected {first}, got \
             {again}"
        );
    }
}

#[tokio::test]
async fn a_secret_created_with_replica_regions_is_stored_with_them() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateSecret",
        json!({
            "Name": "replicated",
            "SecretString": "v",
            "AddReplicaRegions": [{"Region": "eu-west-1"}],
        }),
        "create with one replica region",
    )
    .await;
    assert_eq!(
        created
            .get("ReplicationStatus")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "expected one ReplicationStatus entry for the requested Region, got {created}"
    );
    // And the secret is stored, with the replica a later read reports. The
    // per-Region status rules have their own suite (`secret_tags_and_replication`).
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": "replicated"}),
        "describe after creating with a replica region",
    )
    .await;
    assert_eq!(
        described
            .get("ReplicationStatus")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("Region"))
            .and_then(Value::as_str),
        Some("eu-west-1"),
        "expected the replica Region back from DescribeSecret, got {described}"
    );
    assert_eq!(
        described.get("PrimaryRegion").and_then(Value::as_str),
        Some(REGION),
        "a replicated secret reports the Region it is in, got {described}"
    );
}
