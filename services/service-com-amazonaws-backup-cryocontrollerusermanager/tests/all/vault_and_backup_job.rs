// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the AWS Backup vault + backup-job slice.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `backup` SDK client sees — not a handler called directly.
//! The rules under test:
//!
//! * a vault is a stored resource: a create is visible to every later read of
//!   the same name, and a delete makes every vault-scoped read of it 403
//!   `AccessDeniedException`;
//! * a value outside what the model admits is refused with this service's
//!   `InvalidParameterValueException`, an absent required member with
//!   `MissingParameterValueException`;
//! * a protected resource's `ResourceType` is derived from the service
//!   namespace of its ARN;
//! * `StartBackupJob` stores a job AND a recovery point in the named vault, and
//!   `NumberOfRecoveryPoints` is the count of what is stored;
//! * a replayed `IdempotencyToken` returns the first job rather than a second;
//! * a page is `MaxResults` items plus a cursor to the next one.
//!
//! The harness at the top of this module (the router, the one-request `call`
//! and the small body readers) is `pub(crate)`, so a sibling suite in this test
//! binary drives the same wire path instead of growing a second copy of it.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` naming this service's signing name (`backup`) — how
/// the router picks the service — and the region in its credential scope, which
/// is the region every ARN the service mints carries.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/backup/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The region in [`AUTHORIZATION`]'s credential scope.
pub(crate) const REGION: &str = "us-west-2";

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// Longer than the job's settle span, so a read after this sleep observes the
/// completion the clock has already made due.
///
/// A blocking sleep rather than `tokio::time::sleep`: the workspace's `tokio`
/// dependency enables only `macros`, `rt` and `rt-multi-thread`, and the root
/// `Cargo.toml` that declares it is not editable from here, so the `time`
/// feature is not available. The handlers read the wall clock, so the wait has
/// to be real either way.
pub(crate) const PAST_SETTLE: Duration = Duration::from_millis(900);

/// The alphabet the `BackupVaultName` pattern admits: `^[a-zA-Z0-9\-\_]{2,50}$`.
pub(crate) const NAME_ALPHABET: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";

/// Shared with the sibling suites in this target (`tests/all.rs` declares them
/// as modules of one test crate), so there is ONE request harness for this
/// service rather than one per file.
pub(crate) fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null`
/// for an empty body).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let (status, _errortype, parsed) = call_reported(router, method, path, body).await;
    (status, parsed)
}

/// The SAME request, with the error shape the response NAMES to a client
/// (`x-amzn-errortype`, how restJson1 resolves an error shape) beside the status
/// and the body; `"<unset>"` when the header is absent, which is what a
/// successful answer carries.
pub(crate) async fn call_reported(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String, Value) {
    dispatch_signed(router, AUTHORIZATION, method, path, body).await
}

/// The ONE request this harness makes: one HTTP request carrying
/// `authorization`, answered with the status, the error shape the response names
/// and the parsed body. Every other caller in this test crate reaches the router
/// through it, so there is one dispatch path rather than one per signing
/// variant.
async fn dispatch_signed(
    router: &MultiServiceRouter,
    authorization: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", authorization);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |b| {
        bytes::Bytes::copy_from_slice(b.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map_or_else(|| "<unset>".to_string(), str::to_string);
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, errortype, parsed)
}

/// An instant as the seconds since the epoch this protocol serialises a
/// timestamp as. FRACTIONAL: the model's timestamps are "accurate to
/// milliseconds", so an instant keeps its sub-second part here instead of being
/// truncated to a whole second, which would make it a DIFFERENT instant.
///
/// Shared with the sibling suites for the reason given on [`router`].
pub(crate) fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The wire error code of an error response (`__type`), or `"<none>"`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

pub(crate) async fn create_vault(
    router: &MultiServiceRouter,
    name: &str,
    body: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup-vaults/{name}"),
        Some(body),
    )
    .await
}

/// The SAME one-request harness as [`call`], with the SigV4 credential scope
/// naming `region` instead of [`REGION`].
///
/// A backup vault is identified by its name within one account AND one Region,
/// so a suite that works in more than one Region signs each request for the
/// Region it is made in; there is ONE such harness for this service rather than
/// one per file.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/backup/aws4_request, \
         SignedHeaders=host, Signature=00"
    );
    let (status, _errortype, parsed) =
        dispatch_signed(router, &authorization, method, path, body).await;
    (status, parsed)
}

/// `CreateBackupVault` through a client bound to `region`, so the vault belongs
/// to that Region.
pub(crate) async fn create_vault_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    body: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "PUT",
        &format!("/backup-vaults/{name}"),
        Some(body),
    )
    .await
}

pub(crate) async fn start_job(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    call(router, "PUT", "/backup-jobs", Some(body)).await
}

pub(crate) fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A deterministic name of `length` characters drawn from [`NAME_ALPHABET`], so
/// the length bound is exercised across the whole admitted alphabet rather than
/// on one hand-picked spelling.
pub(crate) fn name_of_length(length: usize) -> String {
    (0..length)
        .map(|i| char::from(NAME_ALPHABET[(i * 7 + length) % NAME_ALPHABET.len()]))
        .collect()
}

/// A vault round-trips: create, describe, list, delete, then every read of the
/// deleted name is refused with 403 `AccessDeniedException` (the access decision
/// a vault-scoped read makes before it looks the vault up — see
/// `vault_read_visibility.rs`).
///
/// The table spans the whole `BackupVaultName` length bound (2 and 50, the
/// published ends, plus interior lengths) and both punctuation characters the
/// pattern admits.
#[tokio::test]
async fn a_created_vault_round_trips_and_a_deleted_one_is_gone() {
    let mut cases: Vec<String> = vec![
        "ab".to_string(),
        "demo-vault".to_string(),
        "under_score_1".to_string(),
        "MiXeD-Case_09".to_string(),
    ];
    cases.extend([2, 3, 17, 49, 50].map(name_of_length));

    for case in &cases {
        let router = router();
        let (status, created) = create_vault(&router, case, "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupVault 200, got {status} {created:?}"
        );
        let expected_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{case}");
        assert_eq!(
            string_at(&created, "BackupVaultArn"),
            expected_arn,
            "case {case:?}: expected the synthesized vault ARN, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "BackupVaultName"),
            *case,
            "case {case:?}: expected the created name echoed from state, got {created:?}"
        );
        assert!(
            created.get("CreationDate").is_some(),
            "case {case:?}: expected a CreationDate, got {created:?}"
        );

        let (status, described) =
            call(&router, "GET", &format!("/backup-vaults/{case}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBackupVault 200, got {status} {described:?}"
        );
        assert_eq!(
            string_at(&described, "BackupVaultArn"),
            expected_arn,
            "case {case:?}: describe must report the stored ARN, got {described:?}"
        );
        assert_eq!(
            described.get("VaultType").and_then(Value::as_str),
            Some("BACKUP_VAULT"),
            "case {case:?}: expected VaultType BACKUP_VAULT, got {described:?}"
        );
        assert!(
            described.get("VaultState").is_none(),
            "case {case:?}: a standard BACKUP_VAULT reports no VaultState, got {described:?}"
        );
        assert_eq!(
            described.get("NumberOfRecoveryPoints").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a fresh vault holds no recovery points, got {described:?}"
        );
        assert!(
            described.get("EncryptionKeyType").is_none(),
            "case {case:?}: a standard BACKUP_VAULT reports no EncryptionKeyType, got {described:?}"
        );
        assert_eq!(
            described.get("CreationDate"),
            created.get("CreationDate"),
            "case {case:?}: describe must report the creation instant create returned"
        );

        let (status, listed) = call(&router, "GET", "/backup-vaults", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListBackupVaults 200, got {status} {listed:?}"
        );
        let names: Vec<String> = listed["BackupVaultList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| string_at(entry, "BackupVaultName"))
            .collect();
        assert_eq!(
            names,
            vec![case.clone()],
            "case {case:?}: expected the vault listed exactly once, got {names:?}"
        );

        let (status, deleted) =
            call(&router, "DELETE", &format!("/backup-vaults/{case}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBackupVault 200, got {status} {deleted:?}"
        );

        let (status, gone) = call(&router, "GET", &format!("/backup-vaults/{case}"), None).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: describe after delete must 403 AccessDeniedException, got {status} {gone:?}"
        );
        let (status, gone) = call(
            &router,
            "GET",
            &format!("/backup-vaults/{case}/recovery-points"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: listing a deleted vault's recovery points must 403, got {status} {gone:?}"
        );
        let (_, listed) = call(&router, "GET", "/backup-vaults", None).await;
        let remaining = listed["BackupVaultList"].as_array().cloned().unwrap_or_default();
        assert!(
            remaining.is_empty(),
            "case {case:?}: a deleted vault must not be listed, got {remaining:?}"
        );
    }
}

/// A `BackupVaultName` the model's pattern refuses never reaches state: the
/// request is answered with this service's `InvalidParameterValueException`,
/// which is the code every Backup operation models for a bad parameter value.
///
/// The rows walk off each published end of the `{2,50}` bound and off the
/// admitted alphabet.
#[tokio::test]
async fn a_vault_name_outside_the_modelled_pattern_is_refused() {
    let one_char = name_of_length(1);
    let fifty_one = name_of_length(51);
    let cases: Vec<(&str, &str)> = vec![
        ("one character — under the published minimum of 2", one_char.as_str()),
        ("51 characters — over the published maximum of 50", fifty_one.as_str()),
        ("a dot, which the alphabet excludes", "has.dot"),
        ("a percent-encoded space, which the alphabet excludes", "has%20space"),
        ("a slash-free plus sign, which the alphabet excludes", "has+plus"),
    ];

    for (case, name) in cases {
        let router = router();
        let (status, body) = create_vault(&router, name, "{}").await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({name:?}): expected 400 InvalidParameterValueException, got {status} {body:?}"
        );

        // And nothing was stored for the refused name.
        let (_, listed) = call(&router, "GET", "/backup-vaults", None).await;
        let stored = listed["BackupVaultList"].as_array().cloned().unwrap_or_default();
        assert!(
            stored.is_empty(),
            "case {case:?} ({name:?}): a refused create must store nothing, got {stored:?}"
        );
    }
}

/// `MaxResults` outside the model's `@range(min: 1, max: 1000)` is refused the
/// same way, and each published bound is accepted.
#[tokio::test]
async fn max_results_honours_the_modelled_range() {
    let router = router();
    let (status, _) = create_vault(&router, "range-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");

    for (case, max_results, expect_ok) in [
        ("one below the published minimum", "0", false),
        ("the published minimum", "1", true),
        ("the published maximum", "1000", true),
        ("one above the published maximum", "1001", false),
    ] {
        let path = format!("/backup-vaults/range-vault/recovery-points?maxResults={max_results}");
        let (status, body) = call(&router, "GET", &path, None).await;
        if expect_ok {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): expected 200, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?} (maxResults={max_results}): expected 400 InvalidParameterValueException, got {status} {body:?}"
            );
        }
    }
}

/// A repeat create collides unless it is a retry of the original request — the
/// same `CreatorRequestId` and the same creation parameters.
#[tokio::test]
async fn a_repeat_create_collides_unless_it_replays_the_original_request() {
    let key = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab");
    let cases: Vec<(&str, String, String, bool)> = vec![
        (
            "no idempotency token at all, so the repeat is not a retry",
            "{}".to_string(),
            "{}".to_string(),
            false,
        ),
        (
            "the same token and the same parameters, so it is a retry",
            r#"{"CreatorRequestId":"tok-1"}"#.to_string(),
            r#"{"CreatorRequestId":"tok-1"}"#.to_string(),
            true,
        ),
        (
            "the same token but a different encryption key",
            format!(r#"{{"CreatorRequestId":"tok-1","EncryptionKeyArn":"{key}"}}"#),
            r#"{"CreatorRequestId":"tok-1"}"#.to_string(),
            false,
        ),
        (
            "the same token but different tags",
            r#"{"CreatorRequestId":"tok-1","BackupVaultTags":{"team":"a"}}"#.to_string(),
            r#"{"CreatorRequestId":"tok-1","BackupVaultTags":{"team":"b"}}"#.to_string(),
            false,
        ),
        (
            "a different token",
            r#"{"CreatorRequestId":"tok-1"}"#.to_string(),
            r#"{"CreatorRequestId":"tok-2"}"#.to_string(),
            false,
        ),
    ];

    for (case, first, second, expect_retry) in cases {
        let router = router();
        let (status, created) = create_vault(&router, "dup-vault", &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create should succeed, got {status} {created:?}"
        );
        let (status, repeat) = create_vault(&router, "dup-vault", &second).await;
        if expect_retry {
            assert_eq!(
                status, 200,
                "case {case:?}: a retry should replay the original, got {status} {repeat:?}"
            );
            assert_eq!(
                repeat.get("CreationDate"),
                created.get("CreationDate"),
                "case {case:?}: a retry must replay the ORIGINAL creation instant, got {repeat:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&repeat).as_str()),
                (400, "AlreadyExistsException"),
                "case {case:?}: expected 400 AlreadyExistsException, got {status} {repeat:?}"
            );
        }
    }
}

/// A vault created with an `EncryptionKeyArn` reports it. `EncryptionKeyType`
/// stays unset: AWS does not populate it for a standard `BACKUP_VAULT`,
/// whatever key encrypts it.
#[tokio::test]
async fn a_customer_managed_key_is_reported_from_state() {
    let router = router();
    let key = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab");
    let body = format!(r#"{{"EncryptionKeyArn":"{key}","CreatorRequestId":"cmk-1"}}"#);
    let (status, _) = create_vault(&router, "cmk-vault", &body).await;
    assert_eq!(status, 200, "CreateBackupVault with a key should succeed");

    let (status, described) = call(&router, "GET", "/backup-vaults/cmk-vault", None).await;
    assert_eq!(status, 200, "expected DescribeBackupVault 200, got {status}");
    assert_eq!(
        string_at(&described, "EncryptionKeyArn"),
        key,
        "case \"customer managed key\": expected the stored key ARN, got {described:?}"
    );
    assert!(
        described.get("EncryptionKeyType").is_none(),
        "case \"customer managed key\": a BACKUP_VAULT reports no EncryptionKeyType, got {described:?}"
    );
    assert_eq!(
        string_at(&described, "CreatorRequestId"),
        "cmk-1",
        "case \"customer managed key\": expected the stored CreatorRequestId, got {described:?}"
    );
}

/// A protected resource's `ResourceType` is derived from the service namespace
/// of its ARN (and, where one namespace carries two Backup types, from the
/// resource it names). A namespace the catalog does not name is reported as the
/// namespace itself.
#[tokio::test]
async fn resource_type_is_derived_from_the_protected_resource_arn() {
    let cases: Vec<(&str, &str)> = vec![
        ("arn:aws:dynamodb:us-west-2:000000000000:table/demo", "DynamoDB"),
        ("arn:aws:ec2:us-west-2:000000000000:volume/vol-01234567", "EBS"),
        ("arn:aws:ec2:us-west-2:000000000000:instance/i-01234567", "EC2"),
        ("arn:aws:rds:us-west-2:000000000000:cluster:aurora-demo", "Aurora"),
        ("arn:aws:rds:us-west-2:000000000000:db:postgres-demo", "RDS"),
        ("arn:aws:s3:::demo-bucket", "S3"),
        (
            "arn:aws:elasticfilesystem:us-west-2:000000000000:file-system/fs-012345",
            "EFS",
        ),
        ("arn:aws:fsx:us-west-2:000000000000:file-system/fs-012345", "FSx"),
        ("arn:aws:redshift:us-west-2:000000000000:cluster:rs-demo", "Redshift"),
        ("arn:aws:neptune:us-west-2:000000000000:cluster:neptune-demo", "Neptune"),
        (
            "arn:aws:timestream:us-west-2:000000000000:database/demo/table/t",
            "Timestream",
        ),
        (
            // Not in the catalog: the namespace is what the ARN says about it.
            "arn:aws:storagegateway:us-west-2:000000000000:gateway/sgw-012345",
            "storagegateway",
        ),
    ];

    let router = router();
    let (status, _) = create_vault(&router, "types-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");

    for (resource_arn, expected) in cases {
        let body = format!(
            r#"{{"BackupVaultName":"types-vault","ResourceArn":"{resource_arn}","IamRoleArn":"{role}"}}"#
        );
        let (status, started) = start_job(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {resource_arn:?}: expected StartBackupJob 200, got {status} {started:?}"
        );
        let job_id = string_at(&started, "BackupJobId");
        let (status, described) =
            call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {resource_arn:?}: expected DescribeBackupJob 200, got {status} {described:?}"
        );
        assert_eq!(
            string_at(&described, "ResourceType"),
            expected,
            "case {resource_arn:?}: expected ResourceType {expected:?}, got {described:?}"
        );
    }
}

/// A started job and its recovery point are both stored, read back through
/// `DescribeBackupJob`, and the `CREATED` -> `COMPLETED` transition the clock
/// owes is materialised from the stored due instant.
#[tokio::test]
async fn a_started_job_is_stored_and_settles_from_created_to_completed() {
    let router = router();
    let (status, _) = create_vault(&router, "job-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");

    let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let body = format!(
        r#"{{"BackupVaultName":"job-vault","ResourceArn":"{resource_arn}","IamRoleArn":"{role}",
             "IdempotencyToken":"tok-1","Lifecycle":{{"DeleteAfterDays":35}},
             "BackupOptions":{{"WindowsVSS":"enabled"}}}}"#
    );
    let (status, started) = start_job(&router, &body).await;
    assert_eq!(
        status, 200,
        "expected StartBackupJob 200, got {status} {started:?}"
    );
    let job_id = string_at(&started, "BackupJobId");
    let point_arn = string_at(&started, "RecoveryPointArn");
    assert!(
        !job_id.is_empty(),
        "expected a generated BackupJobId, got {started:?}"
    );
    assert!(
        point_arn.starts_with(&format!(
            "arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:"
        )),
        "expected a synthesized recovery-point ARN, got {point_arn:?}"
    );
    assert_eq!(
        started.get("IsParent").and_then(Value::as_bool),
        Some(false),
        "a job started on one resource is not a parent, got {started:?}"
    );

    // Straight after the call the job is still in its transient state.
    let (status, described) = call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBackupJob 200, got {status} {described:?}"
    );
    assert_eq!(
        described.get("State").and_then(Value::as_str),
        Some("CREATED"),
        "case \"before the settle span\": expected State CREATED, got {described:?}"
    );
    assert!(
        described.get("CompletionDate").is_none(),
        "case \"before the settle span\": an unsettled job has no CompletionDate, got {described:?}"
    );
    // Everything reported comes from the stored job, not echoed from this request.
    assert_eq!(
        string_at(&described, "BackupVaultName"),
        "job-vault",
        "expected the stored vault name, got {described:?}"
    );
    assert_eq!(
        string_at(&described, "BackupVaultArn"),
        format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:job-vault"),
        "expected the stored vault ARN, got {described:?}"
    );
    assert_eq!(
        string_at(&described, "RecoveryPointArn"),
        point_arn,
        "expected the stored recovery-point ARN, got {described:?}"
    );
    assert_eq!(
        string_at(&described, "ResourceArn"),
        resource_arn,
        "expected the stored resource ARN, got {described:?}"
    );
    assert_eq!(
        string_at(&described, "IamRoleArn"),
        role,
        "expected the stored role ARN, got {described:?}"
    );
    assert_eq!(
        described.get("IsEncrypted").and_then(Value::as_bool),
        Some(false),
        "the vault carries no key, so the job is not encrypted, got {described:?}"
    );
    assert_eq!(
        described["BackupOptions"]["WindowsVSS"].as_str(),
        Some("enabled"),
        "expected the stored BackupOptions, got {described:?}"
    );

    std::thread::sleep(PAST_SETTLE);

    let (status, settled) = call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBackupJob 200, got {status} {settled:?}"
    );
    assert_eq!(
        settled.get("State").and_then(Value::as_str),
        Some("COMPLETED"),
        "case \"after the settle span\": expected State COMPLETED, got {settled:?}"
    );
    assert_eq!(
        settled.get("PercentDone").and_then(Value::as_str),
        Some("100.0"),
        "case \"after the settle span\": expected PercentDone 100.0, got {settled:?}"
    );
    assert!(
        settled.get("CompletionDate").is_some(),
        "case \"after the settle span\": expected a CompletionDate, got {settled:?}"
    );
    // The completion instant is the one that came due, not the instant of this read.
    assert_eq!(
        settled.get("CompletionDate"),
        settled.get("ExpectedCompletionDate"),
        "the completion recorded must be the instant it came due, got {settled:?}"
    );

    // The recovery point settled with it.
    let (status, listed) = call(
        &router,
        "GET",
        "/backup-vaults/job-vault/recovery-points",
        None,
    )
    .await;
    assert_eq!(status, 200, "expected the list to succeed, got {status}");
    let points = listed["RecoveryPoints"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        points.len(),
        1,
        "one job stored one recovery point, got {points:?}"
    );
    assert_eq!(
        string_at(&points[0], "RecoveryPointArn"),
        point_arn,
        "the listed recovery point must be the one the job returned, got {points:?}"
    );
    assert_eq!(
        points[0].get("Status").and_then(Value::as_str),
        Some("COMPLETED"),
        "after the settle span the recovery point is COMPLETED, got {points:?}"
    );
    assert_eq!(
        string_at(&points[0], "ResourceArn"),
        resource_arn,
        "expected the stored resource ARN on the recovery point, got {points:?}"
    );
    assert_eq!(
        points[0].get("ResourceType").and_then(Value::as_str),
        Some("DynamoDB"),
        "expected the derived resource type on the recovery point, got {points:?}"
    );
    assert!(
        points[0].get("CalculatedLifecycle").is_some(),
        "a stored Lifecycle makes a CalculatedLifecycle due, got {points:?}"
    );

    // A replayed idempotency token returns the SAME job and stores no second one.
    let (status, replayed) = start_job(&router, &body).await;
    assert_eq!(
        status, 200,
        "expected the replay to succeed, got {status} {replayed:?}"
    );
    assert_eq!(
        (
            string_at(&replayed, "BackupJobId"),
            string_at(&replayed, "RecoveryPointArn")
        ),
        (job_id.clone(), point_arn.clone()),
        "case \"replayed IdempotencyToken\": expected the first job's ids, got {replayed:?}"
    );
    let (_, listed) = call(
        &router,
        "GET",
        "/backup-vaults/job-vault/recovery-points",
        None,
    )
    .await;
    let points = listed["RecoveryPoints"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        points.len(),
        1,
        "case \"replayed IdempotencyToken\": no second recovery point, got {points:?}"
    );
}

/// `NumberOfRecoveryPoints` is the count of what is stored, the list pages by
/// `MaxResults` with a round-tripping cursor, and `ByResourceArn` filters.
#[tokio::test]
async fn recovery_points_are_counted_paged_and_filtered() {
    let router = router();
    let (status, _) = create_vault(&router, "page-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");

    let resources = [
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/first"),
        format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-second"),
        "arn:aws:s3:::third-bucket".to_string(),
    ];
    let mut arns = Vec::new();
    for resource in &resources {
        let body = format!(
            r#"{{"BackupVaultName":"page-vault","ResourceArn":"{resource}","IamRoleArn":"{role}"}}"#
        );
        let (status, started) = start_job(&router, &body).await;
        assert_eq!(
            status, 200,
            "setup {resource:?}: expected StartBackupJob 200, got {status} {started:?}"
        );
        arns.push(string_at(&started, "RecoveryPointArn"));
    }

    let (_, described) = call(&router, "GET", "/backup-vaults/page-vault", None).await;
    assert_eq!(
        described.get("NumberOfRecoveryPoints").and_then(Value::as_i64),
        Some(3),
        "expected the count of what is stored, got {described:?}"
    );

    // Walk the whole list one page size at a time; every size must yield the
    // stored order exactly once.
    for page_size in [1_usize, 2, 3, 4] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path = format!(
                "/backup-vaults/page-vault/recovery-points?maxResults={page_size}"
            );
            if let Some(cursor) = &token {
                path.push_str(&format!("&nextToken={cursor}"));
            }
            let (status, body) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: expected 200, got {status} {body:?}"
            );
            let page = body["RecoveryPoints"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page_size={page_size}: a page must not exceed MaxResults, got {page:?}"
            );
            seen.extend(page.iter().map(|point| string_at(point, "RecoveryPointArn")));
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= 5,
                "case page_size={page_size}: pagination did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, arns,
            "case page_size={page_size}: expected every stored recovery point once, in order, got {seen:?}"
        );
        let expected_pages = arns.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page_size={page_size}: expected {expected_pages} page(s), got {pages}"
        );
    }

    // ByResourceArn selects the one recovery point of that resource; a resource
    // with no recovery point selects none.
    for (index, resource) in resources.iter().enumerate() {
        let path = format!(
            "/backup-vaults/page-vault/recovery-points?resourceArn={resource}"
        );
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {resource:?}: expected 200, got {status} {body:?}"
        );
        let page: Vec<String> = body["RecoveryPoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|point| string_at(point, "RecoveryPointArn"))
            .collect();
        assert_eq!(
            page,
            vec![arns[index].clone()],
            "case {resource:?}: ByResourceArn must select just that resource, got {page:?}"
        );
    }
    let unbacked = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/never-backed-up");
    let path = format!("/backup-vaults/page-vault/recovery-points?resourceArn={unbacked}");
    let (status, body) = call(&router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case \"a resource with no recovery point\": expected 200, got {status} {body:?}"
    );
    let page = body["RecoveryPoints"].as_array().cloned().unwrap_or_default();
    assert!(
        page.is_empty(),
        "case \"a resource with no recovery point\": expected no items, got {page:?}"
    );

    // ByResourceType selects by the DERIVED type.
    for (resource_type, expected) in [("EBS", 1_usize), ("S3", 1), ("RDS", 0)] {
        let path =
            format!("/backup-vaults/page-vault/recovery-points?resourceType={resource_type}");
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: expected 200, got {status} {body:?}"
        );
        let page = body["RecoveryPoints"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            page.len(),
            expected,
            "case {resource_type:?}: expected {expected} item(s), got {page:?}"
        );
    }

    // A vault holding recovery points cannot be deleted.
    let (status, refused) =
        call(&router, "DELETE", "/backup-vaults/page-vault", None).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidRequestException"),
        "a non-empty vault must be refused, got {status} {refused:?}"
    );
}

/// A cursor this service did not vend is refused rather than silently restarting
/// the result set.
#[tokio::test]
async fn an_unrecognised_cursor_is_refused() {
    let router = router();
    let (status, _) = create_vault(&router, "token-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");

    for (case, token) in [
        ("not hex at all", "zzzz"),
        ("hex that decodes to no stored key", "6e6f7065"),
    ] {
        let path = format!("/backup-vaults/token-vault/recovery-points?nextToken={token}");
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({token:?}): expected 400 InvalidParameterValueException, got {status} {body:?}"
        );
    }
}

/// Everything that names a resource this service does not hold is
/// `ResourceNotFoundException`; an absent required member is
/// `MissingParameterValueException`.
///
/// The vault-scoped READS are deliberately absent from this table: they decide
/// access before existence and so answer 403 `AccessDeniedException` instead —
/// `vault_read_visibility.rs` covers them. A mutation names the resource it could
/// not act on, which is what the rows below assert.
#[tokio::test]
async fn unknown_resources_and_absent_members_are_refused_by_their_own_code() {
    let router = router();
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");

    let start_unknown_vault = format!(
        r#"{{"BackupVaultName":"never-created","ResourceArn":"{resource}","IamRoleArn":"{role}"}}"#
    );
    let not_found: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "delete a vault that was never created",
            "DELETE",
            "/backup-vaults/never-created",
            None,
        ),
        (
            "describe a job id that was never issued",
            "GET",
            "/backup-jobs/00000000-0000-0000-0000-000000000000",
            None,
        ),
        (
            "start a job into a vault that was never created",
            "PUT",
            "/backup-jobs",
            Some(start_unknown_vault.as_str()),
        ),
    ];
    for (case, method, path, body) in not_found {
        let (status, response) = call(&router, method, path, body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected 400 ResourceNotFoundException, got {status} {response:?}"
        );
    }

    let (status, _) = create_vault(&router, "missing-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let absent: Vec<(&str, String)> = vec![
        (
            "no IamRoleArn",
            format!(r#"{{"BackupVaultName":"missing-vault","ResourceArn":"{resource}"}}"#),
        ),
        (
            "no ResourceArn",
            format!(r#"{{"BackupVaultName":"missing-vault","IamRoleArn":"{role}"}}"#),
        ),
        (
            "no BackupVaultName",
            format!(r#"{{"ResourceArn":"{resource}","IamRoleArn":"{role}"}}"#),
        ),
    ];
    for (case, body) in absent {
        let (status, response) = start_job(&router, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "MissingParameterValueException"),
            "case {case:?}: expected 400 MissingParameterValueException, got {status} {response:?}"
        );
    }
}

/// A vault belongs to one account, and this emulator holds exactly one: a read
/// that names another account's vault is refused for lack of privileges rather
/// than told the vault is absent.
#[tokio::test]
async fn a_read_scoped_to_another_account_is_denied() {
    let router = router();
    let (status, _) = create_vault(&router, "scoped-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");

    for (case, account, expect_ok) in [
        ("this emulator's own account", ACCOUNT, true),
        ("another account entirely", "111122223333", false),
    ] {
        let path = format!("/backup-vaults/scoped-vault?backupVaultAccountId={account}");
        let (status, body) = call(&router, "GET", &path, None).await;
        if expect_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected 403 AccessDeniedException, got {status} {body:?}"
            );
        }
    }
}

/// A member the model types as `ARN` with no pattern is still an ARN: a value
/// that is not one is refused with `InvalidParameterValueException` rather than
/// stored or used as a filter that can never match.
///
/// The rows walk each structural part of `arn:<partition>:<service>:<region>:<account>:<resource>`.
#[tokio::test]
async fn a_member_typed_as_an_arn_must_be_one() {
    let not_arns: Vec<(&str, &str)> = vec![
        ("empty", ""),
        ("no arn scheme", "demo-table"),
        ("the scheme only", "arn:"),
        ("too few fields", "arn:aws:dynamodb:us-west-2:000000000000"),
        ("an empty partition", "arn::dynamodb:us-west-2:000000000000:table/demo"),
        ("an empty service", "arn:aws::us-west-2:000000000000:table/demo"),
        ("an empty resource", "arn:aws:dynamodb:us-west-2:000000000000:"),
        ("a URL, not an ARN", "https://example.invalid/table/demo"),
    ];
    // An ARN with empty region and account segments IS well formed (`arn:aws:s3:::b`).
    let arns = ["arn:aws:s3:::demo-bucket", "arn:aws-cn:dynamodb:cn-north-1:000000000000:table/demo"];

    let router = router();
    let (status, _) = create_vault(&router, "arn-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");

    for (case, value) in &not_arns {
        // CreateBackupVault's EncryptionKeyArn.
        let body = serde_json::json!({ "EncryptionKeyArn": value }).to_string();
        let (status, response) = create_vault(&router, "arn-key-vault", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({value:?}) as EncryptionKeyArn: expected 400 InvalidParameterValueException, got {status} {response:?}"
        );

        // StartBackupJob's ResourceArn and IamRoleArn.
        for member in ["ResourceArn", "IamRoleArn"] {
            let mut payload = serde_json::json!({
                "BackupVaultName": "arn-vault",
                "ResourceArn": resource,
                "IamRoleArn": role,
            });
            payload[member] = serde_json::Value::String((*value).to_string());
            let (status, response) = start_job(&router, &payload.to_string()).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?} ({value:?}) as {member}: expected 400 InvalidParameterValueException, got {status} {response:?}"
            );
        }

        // ListRecoveryPointsByBackupVault's ByResourceArn filter.
        let path = format!(
            "/backup-vaults/arn-vault/recovery-points?resourceArn={}",
            value.replace(' ', "%20")
        );
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({value:?}) as ByResourceArn: expected 400 InvalidParameterValueException, got {status} {response:?}"
        );
    }

    for value in arns {
        let payload = serde_json::json!({
            "BackupVaultName": "arn-vault",
            "ResourceArn": value,
            "IamRoleArn": role,
        });
        let (status, response) = start_job(&router, &payload.to_string()).await;
        assert_eq!(
            status, 200,
            "case {value:?}: a well-formed ARN must be accepted, got {status} {response:?}"
        );
    }
}

/// Each backup window honours the range the model documents, and a value inside
/// it is accepted.
#[tokio::test]
async fn the_backup_windows_honour_their_documented_range() {
    // Source: the StartBackupJob model docs — StartWindowMinutes "must be at
    // least 60 minutes" with "a maximum value of 100 years (52,560,000
    // minutes)"; CompleteWindowMinutes has the same maximum.
    let cases: Vec<(&str, &str, i64, bool)> = vec![
        ("one below the documented minimum", "StartWindowMinutes", 59, false),
        ("the documented minimum", "StartWindowMinutes", 60, true),
        ("the documented maximum", "StartWindowMinutes", 52_560_000, true),
        ("one above the documented maximum", "StartWindowMinutes", 52_560_001, false),
        ("negative", "StartWindowMinutes", -1, false),
        ("the widest the type can hold", "StartWindowMinutes", i64::MAX, false),
        ("zero", "CompleteWindowMinutes", 0, true),
        ("negative", "CompleteWindowMinutes", -1, false),
        ("the documented maximum", "CompleteWindowMinutes", 52_560_000, true),
        ("one above the documented maximum", "CompleteWindowMinutes", 52_560_001, false),
        ("the widest the type can hold", "CompleteWindowMinutes", i64::MAX, false),
    ];

    let router = router();
    let (status, _) = create_vault(&router, "window-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");

    for (case, member, minutes, expect_ok) in cases {
        let mut payload = serde_json::json!({
            "BackupVaultName": "window-vault",
            "ResourceArn": resource,
            "IamRoleArn": role,
        });
        payload[member] = serde_json::Value::from(minutes);
        let (status, response) = start_job(&router, &payload.to_string()).await;
        if expect_ok {
            assert_eq!(
                status, 200,
                "case {case:?} ({member}={minutes}): expected 200, got {status} {response:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?} ({member}={minutes}): expected 400 InvalidParameterValueException, got {status} {response:?}"
            );
        }
    }
}

/// `CalculatedLifecycle` is the stored `Lifecycle` counted forward from the
/// recovery point's creation; `-1` is the documented "keep indefinitely"
/// sentinel, and a day count no instant can express makes nothing due — neither
/// may overflow.
#[tokio::test]
async fn calculated_lifecycle_follows_the_stored_day_counts() {
    // (case, DeleteAfterDays, is an instant due?)
    let cases: Vec<(&str, i64, bool)> = vec![
        ("one day", 1, true),
        ("the common 35-day retention", 35, true),
        ("a century", 36_500, true),
        ("the documented keep-indefinitely sentinel", -1, false),
        ("more days than an instant can express", i64::MAX, false),
        ("enough days to overflow seconds", i64::MAX / 86_400, false),
    ];

    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");

    for (case, delete_after_days, expect_due) in cases {
        let router = router();
        let (status, _) = create_vault(&router, "lifecycle-vault", "{}").await;
        assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
        let payload = serde_json::json!({
            "BackupVaultName": "lifecycle-vault",
            "ResourceArn": resource,
            "IamRoleArn": role,
            "Lifecycle": { "DeleteAfterDays": delete_after_days },
        });
        let (status, response) = start_job(&router, &payload.to_string()).await;
        assert_eq!(
            status, 200,
            "case {case:?} (DeleteAfterDays={delete_after_days}): expected 200, got {status} {response:?}"
        );

        let (status, listed) = call(
            &router,
            "GET",
            "/backup-vaults/lifecycle-vault/recovery-points",
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the list to succeed, got {status} {listed:?}"
        );
        let points = listed["RecoveryPoints"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected the one stored recovery point, got {points:?}"
        );
        assert_eq!(
            points[0]["Lifecycle"]["DeleteAfterDays"].as_i64(),
            Some(delete_after_days),
            "case {case:?}: the stored Lifecycle must be reported verbatim, got {points:?}"
        );
        let due = points[0]
            .get("CalculatedLifecycle")
            .and_then(|calculated| calculated.get("DeleteAt"))
            .is_some();
        assert_eq!(
            due, expect_due,
            "case {case:?} (DeleteAfterDays={delete_after_days}): expected DeleteAt present={expect_due}, got {points:?}"
        );
    }
}

/// A vault name the service could never have stored is a bad parameter value,
/// not an absent resource — including on the two operations whose model types
/// the member as a plain string.
#[tokio::test]
async fn describe_and_delete_apply_the_vault_name_rule_too() {
    let one_char = name_of_length(1);
    let fifty_one = name_of_length(51);
    let cases: Vec<(&str, &str)> = vec![
        ("one character — under the published minimum of 2", one_char.as_str()),
        ("51 characters — over the published maximum of 50", fifty_one.as_str()),
        ("a dot, which the alphabet excludes", "has.dot"),
        ("a percent-encoded space, which the alphabet excludes", "has%20space"),
    ];

    let router = router();
    for (case, name) in cases {
        for method in ["GET", "DELETE"] {
            let (status, body) = call(&router, method, &format!("/backup-vaults/{name}"), None).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?} ({name:?}, {method}): expected 400 InvalidParameterValueException, got {status} {body:?}"
            );
        }
    }
}

/// An idempotency token identifies ONE call: reused on a call that names a
/// different vault, resource or role it is not a retry, and a replay is resolved
/// only after the vault it names is.
#[tokio::test]
async fn an_idempotency_token_only_replays_the_call_it_identifies() {
    let router = router();
    for vault in ["token-a", "token-b"] {
        let (status, _) = create_vault(&router, vault, "{}").await;
        assert_eq!(status, 200, "setup {vault:?}: CreateBackupVault should succeed");
    }
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let other_role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/OtherBackupRole");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/first");
    let other_resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/second");
    let first = serde_json::json!({
        "BackupVaultName": "token-a",
        "ResourceArn": resource,
        "IamRoleArn": role,
        "IdempotencyToken": "shared",
    });
    let (status, started) = start_job(&router, &first.to_string()).await;
    assert_eq!(status, 200, "setup: StartBackupJob should succeed, got {started:?}");
    let first_job = string_at(&started, "BackupJobId");

    let differing: Vec<(&str, &str, &str, &str)> = vec![
        ("the same call", "token-a", resource.as_str(), role.as_str()),
        ("a different vault", "token-b", resource.as_str(), role.as_str()),
        ("a different resource", "token-a", other_resource.as_str(), role.as_str()),
        ("a different role", "token-a", resource.as_str(), other_role.as_str()),
    ];
    for (case, vault, resource_arn, iam_role_arn) in differing {
        let payload = serde_json::json!({
            "BackupVaultName": vault,
            "ResourceArn": resource_arn,
            "IamRoleArn": iam_role_arn,
            "IdempotencyToken": "shared",
        });
        let (status, response) = start_job(&router, &payload.to_string()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let same = string_at(&response, "BackupJobId") == first_job;
        let expect_same = case == "the same call";
        assert_eq!(
            same, expect_same,
            "case {case:?}: expected replay of the first job={expect_same}, got {response:?}"
        );
    }

    // The vault is resolved BEFORE the token, so a replay naming a vault that is
    // gone is refused rather than answered from the stored job.
    let (status, _) = call(&router, "DELETE", "/backup-vaults/token-b", None).await;
    assert_eq!(status, 400, "token-b holds a recovery point, so it cannot be deleted");
    let payload = serde_json::json!({
        "BackupVaultName": "never-created",
        "ResourceArn": resource,
        "IamRoleArn": role,
        "IdempotencyToken": "shared",
    });
    let (status, response) = start_job(&router, &payload.to_string()).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case \"a replay naming an unknown vault\": expected 400 ResourceNotFoundException, got {status} {response:?}"
    );
}

/// The vault store is bounded: once it holds the ceiling this emulator keeps,
/// another create is refused with the modelled `LimitExceededException` instead
/// of growing the store without limit.
#[tokio::test]
async fn the_vault_store_is_bounded() {
    // The ceiling is `crate::state::MAX_VAULTS`; the test walks up to it rather
    // than naming the number twice.
    let router = router();
    let mut created = 0_usize;
    let mut refused: Option<(u16, String)> = None;
    for index in 0..2_000 {
        let (status, body) = create_vault(&router, &format!("bound-{index}"), "{}").await;
        if status == 200 {
            created += 1;
            continue;
        }
        refused = Some((status, error_code(&body)));
        break;
    }
    let Some((status, code)) = refused else {
        panic!("expected a ceiling after {created} vaults, but 2000 creates all succeeded");
    };
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "case \"one vault past the ceiling\": expected 400 LimitExceededException after {created} vaults, got {status} {code}"
    );
    assert!(
        created > 0,
        "case \"the ceiling\": the store must accept at least one vault, got {created}"
    );

    // The store still serves everything it did accept.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/backup-vaults?maxResults={created}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "expected ListBackupVaults 200, got {status}");
    assert_eq!(
        listed["BackupVaultList"].as_array().map(Vec::len),
        Some(created),
        "case \"the ceiling\": every accepted vault must still be listed"
    );
}
