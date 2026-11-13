// SPDX-License-Identifier: CC0-1.0

//! The vault read projection: what `DescribeBackupVault` and `ListBackupVaults`
//! report about a stored vault, driven as real HTTP through the router.
//!
//! The rules under test:
//!
//! * every backup vault is encrypted by a KMS key — the one the create request
//!   named, or the one the service creates when it named none ("if you do not
//!   specify a key, Backup creates a KMS key for you by default", model doc for
//!   `EncryptionKeyArn`) — and a vault read reports that key's ARN;
//! * a vault read reports ONLY the members the stored vault carries: a standard
//!   `BACKUP_VAULT` carries neither `VaultState` nor `EncryptionKeyType`, so
//!   neither appears in either view;
//! * a vault reports `Locked` with exactly the vault-lock configuration applied
//!   to it, and a `LockDate` exactly when the request named
//!   `ChangeableForDays`, until the configuration is deleted;
//! * `ChangeableForDays` and the retention bounds are refused outside the spans
//!   the model documents, and a refused configuration stores nothing;
//! * the described and the listed view of one vault agree member for member, so
//!   one projection serves both.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{
    call, call_in_region, create_vault, create_vault_in, epoch_seconds, error_code, router,
    string_at, ACCOUNT, REGION,
};

/// Seconds in a day — `ChangeableForDays` is a day count, the `LockDate` an
/// instant.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// How far the asserted `LockDate` may sit from `now + ChangeableForDays`: the
/// handler reads the wall clock, so the test brackets the call rather than
/// pinning one instant.
const CLOCK_SLACK: Duration = Duration::from_secs(5);

/// The published bounds of the vault-lock day counts, from the model docs:
/// `ChangeableForDays` "must be 3 or greater" with a maximum of 36,500 days;
/// the shortest `MinRetentionDays` is 1 day and the longest `MaxRetentionDays`
/// is 36500 days.
const MIN_CHANGEABLE_FOR_DAYS: i64 = 3;
const MAX_CHANGEABLE_FOR_DAYS: i64 = 36_500;
const MIN_RETENTION_DAYS: i64 = 1;
const MAX_RETENTION_DAYS: i64 = 36_500;

/// The members a vault carries in BOTH views, so one projection can be compared
/// against the other.
const SHARED_MEMBERS: &[&str] = &[
    "BackupVaultName",
    "BackupVaultArn",
    "VaultType",
    "VaultState",
    "CreationDate",
    "EncryptionKeyArn",
    "EncryptionKeyType",
    "CreatorRequestId",
    "NumberOfRecoveryPoints",
    "Locked",
    "MinRetentionDays",
    "MaxRetentionDays",
    "LockDate",
];

/// Whether `value` has the hyphenated 8-4-4-4-12 hex form a KMS key id carries.
fn is_key_id(value: &str) -> bool {
    let segments: Vec<&str> = value.split('-').collect();
    segments.len() == 5
        && segments
            .iter()
            .map(|segment| segment.len())
            .eq([8, 4, 4, 4, 12])
        && segments
            .iter()
            .all(|segment| segment.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Apply a vault-lock configuration.
async fn put_lock(router: &MultiServiceRouter, name: &str, body: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup-vaults/{name}/vault-lock"),
        Some(body),
    )
    .await
}

/// Delete the vault-lock configuration.
async fn delete_lock(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/backup-vaults/{name}/vault-lock"),
        None,
    )
    .await
}

/// The entry `ListBackupVaults` reports for `name`, if any.
async fn listed_vault(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, listed) = call(router, "GET", "/backup-vaults", None).await;
    assert_eq!(status, 200, "expected ListBackupVaults 200, got {status}");
    listed["BackupVaultList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| string_at(entry, "BackupVaultName") == name)
        .unwrap_or(Value::Null)
}

/// Describe `name` and assert the list entry for the same vault carries the
/// same values for every member both views have, then return the described
/// body.
async fn described_and_listed_agree(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/backup-vaults/{name}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBackupVault 200, got {status} {described:?}"
    );
    let listed = listed_vault(router, name).await;
    for member in SHARED_MEMBERS {
        assert_eq!(
            described.get(member),
            listed.get(member),
            "case {case:?}: the described and listed views must agree on {member}, \
             got {:?} and {:?}",
            described.get(member),
            listed.get(member)
        );
    }
    described
}

/// Every vault reports a KMS key ARN: the one the create named, or the one this
/// service creates for the account when it named none. Neither `VaultState` nor
/// `EncryptionKeyType` is reported for a standard `BACKUP_VAULT`.
///
/// The table's rows differ along the shape of the key the create names — none
/// at all, a key in the signed region, in another region, in another partition,
/// and an alias ARN — which is what the rule generalises over.
#[tokio::test]
async fn every_vault_reports_the_key_it_is_encrypted_with() {
    let cases: Vec<(&str, Option<String>)> = vec![
        ("no key named", None),
        (
            "a key in the signed region",
            Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab"
            )),
        ),
        (
            "a key in another region",
            Some(format!(
                "arn:aws:kms:eu-west-1:{ACCOUNT}:key/0f0e0d0c-0b0a-0908-0706-050403020100"
            )),
        ),
        (
            "a key in another partition",
            Some(format!(
                "arn:aws-us-gov:kms:us-gov-west-1:{ACCOUNT}:key/\
                 abcdef01-2345-6789-abcd-ef0123456789"
            )),
        ),
        (
            "a key named by its alias ARN",
            Some(format!("arn:aws:kms:{REGION}:{ACCOUNT}:alias/aws-backup")),
        ),
    ];

    let router = router();
    let mut default_keys: Vec<String> = Vec::new();
    for (index, (case, key)) in cases.iter().enumerate() {
        let name = format!("key-vault-{index}");
        let body = match key {
            Some(key) => format!(r#"{{"EncryptionKeyArn":"{key}"}}"#),
            None => "{}".to_string(),
        };
        let (status, created) = create_vault(&router, &name, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupVault 200, got {status} {created:?}"
        );

        let described = described_and_listed_agree(&router, case, &name).await;
        let reported = string_at(&described, "EncryptionKeyArn");
        match key {
            Some(key) => assert_eq!(
                &reported, key,
                "case {case:?}: expected the key the create named, got {described:?}"
            ),
            None => {
                let prefix = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/");
                let key_id = reported.strip_prefix(&prefix);
                assert!(
                    key_id.is_some_and(is_key_id),
                    "case {case:?}: expected a KMS key ARN of the form {prefix}<key id>, \
                     got {reported:?}"
                );
                default_keys.push(reported.clone());
            }
        }
        assert!(
            described.get("VaultState").is_none(),
            "case {case:?}: a standard BACKUP_VAULT reports no VaultState, got {described:?}"
        );
        assert!(
            described.get("EncryptionKeyType").is_none(),
            "case {case:?}: a standard BACKUP_VAULT reports no EncryptionKeyType, \
             got {described:?}"
        );

        // A read reports the stored key, so a second read reports the same one.
        let again = described_and_listed_agree(&router, case, &name).await;
        assert_eq!(
            again, described,
            "case {case:?}: a read must not change what the next read sees, got {again:?}"
        );
    }

    // A second vault that names no key is encrypted by the SAME default key:
    // the key belongs to the account and region, not to one vault.
    let (status, _) = create_vault(&router, "another-default-vault", "{}").await;
    assert_eq!(status, 200, "expected CreateBackupVault 200, got {status}");
    let described = described_and_listed_agree(
        &router,
        "a second vault that names no key",
        "another-default-vault",
    )
    .await;
    default_keys.push(string_at(&described, "EncryptionKeyArn"));
    let distinct: std::collections::BTreeSet<&String> = default_keys.iter().collect();
    assert_eq!(
        distinct.len(),
        1,
        "case \"two vaults with no key named\": expected one default key for the account, \
         got {default_keys:?}"
    );
}

/// One row of the vault-lock table: the case name, the request body, and the
/// `MinRetentionDays`, `MaxRetentionDays` and `ChangeableForDays` it names.
type LockCase = (&'static str, String, Option<i64>, Option<i64>, Option<i64>);

/// Create a vault with a request whose SigV4 credential scope names `region`.
///
/// The shared harness signs for one region; this suite needs several, and one
/// that is not a region name at all.
async fn create_vault_in_region(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, Value) {
    create_vault_in(router, region, name, "{}").await
}

/// The region of the vault ARN and of the default key comes from the request's
/// credential scope when that scope names a region, and falls back to the
/// service's default region when the scope's region field is not shaped like a
/// region name — so a caller cannot stitch an arbitrary string into a minted
/// ARN or into the keys of this service's state.
#[tokio::test]
async fn an_arn_carries_a_region_shaped_scope_or_the_default_region() {
    // Rows: the credential-scope region, and the region the ARNs must carry.
    let cases: Vec<(&str, String, &str)> = vec![
        ("the region the suite signs for", REGION.to_string(), REGION),
        ("another region", "eu-central-1".to_string(), "eu-central-1"),
        (
            "a region of another partition",
            "us-gov-west-1".to_string(),
            "us-gov-west-1",
        ),
        ("an upper-case scope field", "US-EAST-2".to_string(), "us-east-1"),
        (
            "a scope field longer than any region name",
            "a".repeat(64),
            "us-east-1",
        ),
        (
            "a scope field with characters no region has",
            "us-east-1;rm".to_string(),
            "us-east-1",
        ),
    ];

    for (index, (case, scope_region, expected_region)) in cases.iter().enumerate() {
        let router = router();
        let name = format!("region-vault-{index}");
        let (status, created) = create_vault_in_region(&router, scope_region, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupVault 200, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "BackupVaultArn"),
            format!("arn:aws:backup:{expected_region}:{ACCOUNT}:backup-vault:{name}"),
            "case {case:?}: expected an ARN in {expected_region}, got {created:?}"
        );
        // A vault belongs to the Region it was created in, so it is described
        // through a client in the Region its ARN carries.
        let (status, described) = call_in_region(
            &router,
            expected_region,
            "GET",
            &format!("/backup-vaults/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBackupVault 200, got {status} {described:?}"
        );
        let key = string_at(&described, "EncryptionKeyArn");
        assert!(
            key.starts_with(&format!("arn:aws:kms:{expected_region}:{ACCOUNT}:key/")),
            "case {case:?}: expected a default key in {expected_region}, got {key:?}"
        );
    }
}

/// A vault reports `Locked` with exactly the configuration applied to it: the
/// retention bounds the request named, and a `LockDate` exactly when it named
/// `ChangeableForDays`.
///
/// The rows differ along which of the three members the request carries and
/// along the day counts, including each published bound.
#[tokio::test]
async fn a_vault_reports_the_lock_configuration_applied_to_it() {
    let cases: Vec<LockCase> = vec![
        (
            "every member named",
            r#"{"MinRetentionDays":7,"MaxRetentionDays":30,"ChangeableForDays":3}"#.to_string(),
            Some(7),
            Some(30),
            Some(3),
        ),
        (
            "no changeable-for span, so no lock date",
            r#"{"MinRetentionDays":7,"MaxRetentionDays":30}"#.to_string(),
            Some(7),
            Some(30),
            None,
        ),
        (
            "no retention bounds",
            format!(r#"{{"ChangeableForDays":{MAX_CHANGEABLE_FOR_DAYS}}}"#),
            None,
            None,
            Some(MAX_CHANGEABLE_FOR_DAYS),
        ),
        (
            "a minimum only, at the shortest it admits",
            format!(r#"{{"MinRetentionDays":{MIN_RETENTION_DAYS},"ChangeableForDays":30}}"#),
            Some(MIN_RETENTION_DAYS),
            None,
            Some(30),
        ),
        (
            "a maximum only, at the longest it admits",
            format!(r#"{{"MaxRetentionDays":{MAX_RETENTION_DAYS}}}"#),
            None,
            Some(MAX_RETENTION_DAYS),
            None,
        ),
        (
            "equal bounds",
            r#"{"MinRetentionDays":2555,"MaxRetentionDays":2555,"ChangeableForDays":3}"#
                .to_string(),
            Some(2555),
            Some(2555),
            Some(3),
        ),
        (
            "an empty configuration",
            "{}".to_string(),
            None,
            None,
            None,
        ),
    ];

    for (index, (case, body, min_days, max_days, changeable_for_days)) in cases.iter().enumerate() {
        let router = router();
        let name = format!("lock-vault-{index}");
        let (status, created) = create_vault(&router, &name, "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupVault 200, got {status} {created:?}"
        );
        let unlocked = described_and_listed_agree(&router, case, &name).await;
        assert_eq!(
            unlocked.get("Locked").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a vault with no lock configuration is unlocked, got {unlocked:?}"
        );

        let before = SystemTime::now();
        let (status, applied) = put_lock(&router, &name, body).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutBackupVaultLockConfiguration 200, got {status} {applied:?}"
        );

        let locked = described_and_listed_agree(&router, case, &name).await;
        assert_eq!(
            locked.get("Locked").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: a vault with a lock configuration is locked, got {locked:?}"
        );
        assert_eq!(
            locked.get("MinRetentionDays").and_then(Value::as_i64),
            *min_days,
            "case {case:?}: expected MinRetentionDays {min_days:?}, got {locked:?}"
        );
        assert_eq!(
            locked.get("MaxRetentionDays").and_then(Value::as_i64),
            *max_days,
            "case {case:?}: expected MaxRetentionDays {max_days:?}, got {locked:?}"
        );
        match changeable_for_days {
            None => assert!(
                locked.get("LockDate").is_none(),
                "case {case:?}: no ChangeableForDays means no LockDate, got {locked:?}"
            ),
            Some(days) => {
                let lock_date = locked
                    .get("LockDate")
                    .and_then(Value::as_f64)
                    .unwrap_or_default();
                let offset = (days * SECONDS_PER_DAY) as f64;
                let earliest =
                    epoch_seconds(before.checked_sub(CLOCK_SLACK).unwrap_or(before)) + offset;
                let latest =
                    epoch_seconds(after.checked_add(CLOCK_SLACK).unwrap_or(after)) + offset;
                assert!(
                    (earliest..=latest).contains(&lock_date),
                    "case {case:?}: expected a LockDate in {earliest}..={latest} \
                     ({days} days after the call), got {lock_date}"
                );
            }
        }
        // The creation instant is untouched by the lock.
        assert_eq!(
            locked.get("CreationDate"),
            created.get("CreationDate"),
            "case {case:?}: the lock must not change the creation instant, got {locked:?}"
        );

        // Every row's lock is still changeable (its lock date, when it has one,
        // is days away), so deleting the configuration returns the vault to
        // unlocked with no bounds and no lock date in either view.
        let (status, dropped) = delete_lock(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBackupVaultLockConfiguration 200, got {status} \
             {dropped:?}"
        );
        let unlocked_again = described_and_listed_agree(&router, case, &name).await;
        assert_eq!(
            unlocked_again, unlocked,
            "case {case:?}: deleting the configuration must restore the unlocked projection, \
             got {unlocked_again:?}"
        );
    }
}

/// A day count outside the span the model documents is refused with
/// `InvalidParameterValueException`, and the vault keeps the configuration it
/// had — nothing partial is stored. Each row sits on a published bound or one
/// step past it.
#[tokio::test]
async fn a_lock_day_count_outside_its_documented_span_is_refused() {
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "ChangeableForDays at its minimum",
            format!(r#"{{"ChangeableForDays":{MIN_CHANGEABLE_FOR_DAYS}}}"#),
            true,
        ),
        (
            "ChangeableForDays one below its minimum",
            format!(r#"{{"ChangeableForDays":{}}}"#, MIN_CHANGEABLE_FOR_DAYS - 1),
            false,
        ),
        (
            "ChangeableForDays at its maximum",
            format!(r#"{{"ChangeableForDays":{MAX_CHANGEABLE_FOR_DAYS}}}"#),
            true,
        ),
        (
            "ChangeableForDays one past its maximum",
            format!(r#"{{"ChangeableForDays":{}}}"#, MAX_CHANGEABLE_FOR_DAYS + 1),
            false,
        ),
        (
            "MinRetentionDays at its minimum",
            format!(r#"{{"MinRetentionDays":{MIN_RETENTION_DAYS}}}"#),
            true,
        ),
        (
            "MinRetentionDays one below its minimum",
            format!(r#"{{"MinRetentionDays":{}}}"#, MIN_RETENTION_DAYS - 1),
            false,
        ),
        (
            "MinRetentionDays one past the longest retention",
            format!(r#"{{"MinRetentionDays":{}}}"#, MAX_RETENTION_DAYS + 1),
            false,
        ),
        (
            "MaxRetentionDays at its maximum",
            format!(r#"{{"MaxRetentionDays":{MAX_RETENTION_DAYS}}}"#),
            true,
        ),
        (
            "MaxRetentionDays one past its maximum",
            format!(r#"{{"MaxRetentionDays":{}}}"#, MAX_RETENTION_DAYS + 1),
            false,
        ),
        (
            "MaxRetentionDays one below the shortest retention",
            format!(r#"{{"MaxRetentionDays":{}}}"#, MIN_RETENTION_DAYS - 1),
            false,
        ),
        (
            "a negative day count",
            r#"{"ChangeableForDays":-1}"#.to_string(),
            false,
        ),
        (
            "a minimum longer than the maximum",
            r#"{"MinRetentionDays":31,"MaxRetentionDays":30}"#.to_string(),
            false,
        ),
    ];

    for (index, (case, body, accepted)) in cases.iter().enumerate() {
        let router = router();
        let name = format!("bounds-vault-{index}");
        let (status, _) = create_vault(&router, &name, "{}").await;
        assert_eq!(status, 200, "case {case:?}: setup create should succeed");

        let (status, response) = put_lock(&router, &name, body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the configuration to be accepted, got {status} \
                 {response:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected 400 InvalidParameterValueException, got {status} \
                 {response:?}"
            );
            let described = described_and_listed_agree(&router, case, &name).await;
            assert_eq!(
                described.get("Locked").and_then(Value::as_bool),
                Some(false),
                "case {case:?}: a refused configuration must store nothing, got {described:?}"
            );
            for member in ["MinRetentionDays", "MaxRetentionDays", "LockDate"] {
                assert!(
                    described.get(member).is_none(),
                    "case {case:?}: a refused configuration must leave {member} unset, \
                     got {described:?}"
                );
            }
        }
    }
}

/// A later configuration replaces the stored one while the vault lock is still
/// changeable, and a vault-lock request naming a vault that does not exist is
/// refused for lack of privileges: a vault-lock action is authorized against the
/// named vault before it is looked up, so the refusal says nothing about whether
/// that vault exists. `vault_lock_refusal_order.rs` covers that rule across both
/// vault-lock actions and every reason a vault can be invisible.
#[tokio::test]
async fn a_changeable_lock_is_replaced_and_an_invisible_vault_is_denied() {
    let router = router();
    let (status, _) = create_vault(&router, "relock-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");

    let replacements: Vec<(&str, &str, Option<i64>, Option<i64>)> = vec![
        (
            "the first configuration",
            r#"{"MinRetentionDays":7,"MaxRetentionDays":30}"#,
            Some(7),
            Some(30),
        ),
        (
            "wider bounds",
            r#"{"MinRetentionDays":1,"MaxRetentionDays":36500}"#,
            Some(1),
            Some(36500),
        ),
        ("no bounds at all", "{}", None, None),
    ];
    for (case, body, min_days, max_days) in replacements {
        let (status, response) = put_lock(&router, "relock-vault", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let described = described_and_listed_agree(&router, case, "relock-vault").await;
        assert_eq!(
            (
                described.get("MinRetentionDays").and_then(Value::as_i64),
                described.get("MaxRetentionDays").and_then(Value::as_i64),
            ),
            (min_days, max_days),
            "case {case:?}: expected the stored bounds to be the ones last applied, \
             got {described:?}"
        );
    }

    for (case, name) in [
        ("a vault that was never created", "no-such-vault"),
        ("a name close to the created one", "relock-vaults"),
    ] {
        let (status, response) = put_lock(&router, name, r#"{"MinRetentionDays":7}"#).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected 403 AccessDeniedException from the put, got {status} \
             {response:?}"
        );
        let (status, response) = delete_lock(&router, name).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected 403 AccessDeniedException from the delete, got {status} \
             {response:?}"
        );
    }

    // Deleting a configuration the vault does not have leaves it unlocked.
    let (status, response) = delete_lock(&router, "relock-vault").await;
    assert_eq!(
        status, 200,
        "case \"deleting a configuration twice\": expected 200, got {status} {response:?}"
    );
    let described = described_and_listed_agree(
        &router,
        "deleting a configuration twice",
        "relock-vault",
    )
    .await;
    assert_eq!(
        described.get("Locked").and_then(Value::as_bool),
        Some(false),
        "case \"deleting a configuration twice\": expected the vault unlocked, got {described:?}"
    );
}
