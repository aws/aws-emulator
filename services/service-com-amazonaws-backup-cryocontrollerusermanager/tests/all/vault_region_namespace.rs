// SPDX-License-Identifier: CC0-1.0

//! The Region a backup vault belongs to is part of what identifies it.
//!
//! The rule under test: "Backup vaults are identified by names that are unique
//! to the account used to create them and the Amazon Web Services Region where
//! they are created" (model doc for `CreateBackupVault`'s `BackupVaultName`,
//! repeated on the output member). A vault name is therefore unique within ONE
//! Region of the account, which decides every vault-keyed answer:
//!
//! * a create that reuses a name in the Region that already holds it is refused
//!   400 `AlreadyExistsException`, while the SAME name in another Region creates
//!   a second, independent vault whose ARN carries that other Region;
//! * each of those vaults has its own creation instant, its own default
//!   encryption key and its own, initially empty, recovery-point set;
//! * every vault-scoped call resolves the name in the Region it was made in, so
//!   one Region's vault answers no read, delete, listing or backup of another's.
//!
//! The rows span pairs of Regions the rule generalises over — the suite's own
//! Region and another, the reverse order, two Regions no other suite uses, and a
//! Region of another partition — rather than the one pair any single deployment
//! happens to use.
//!
//! Every case drives real HTTP through the router, each request signed for the
//! Region it is made in, so what is asserted is the wire behaviour a `backup`
//! SDK client bound to that Region sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{
    call_in_region, create_vault_in, error_code, router, string_at, ACCOUNT, REGION,
};

/// The name every case deploys into more than one Region — the multi-Region
/// pattern a backup/copy reference architecture uses.
const SHARED_NAME: &str = "shared-vault-name";

/// The role a backup job is run with.
fn role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// The ARN of a backup vault of this account in `region`, as the model documents
/// the form ("for example,
/// `arn:aws:backup:us-east-1:123456789012:backup-vault:aBackupVault`").
fn vault_arn(region: &str, name: &str) -> String {
    format!("arn:aws:backup:{region}:{ACCOUNT}:backup-vault:{name}")
}

/// `DescribeBackupVault` through a client bound to `region`.
async fn describe_in(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", &format!("/backup-vaults/{name}"), None).await
}

/// `DeleteBackupVault` through a client bound to `region`.
async fn delete_in(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DELETE",
        &format!("/backup-vaults/{name}"),
        None,
    )
    .await
}

/// The `BackupVaultName`s `ListBackupVaults` reports to a client bound to
/// `region`.
async fn listed_names(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, listed) = call_in_region(router, region, "GET", "/backup-vaults", None).await;
    assert_eq!(
        status, 200,
        "expected ListBackupVaults 200 in {region}, got {status} {listed:?}"
    );
    listed["BackupVaultList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| string_at(entry, "BackupVaultName"))
        .collect()
}

/// `StartBackupJob` into `name` through a client bound to `region`.
async fn start_job_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, Value) {
    let body = json!({
        "BackupVaultName": name,
        "ResourceArn": format!("arn:aws:dynamodb:{region}:{ACCOUNT}:table/demo"),
        "IamRoleArn": role(),
    });
    call_in_region(
        router,
        region,
        "PUT",
        "/backup-jobs",
        Some(&body.to_string()),
    )
    .await
}

/// `NumberOfRecoveryPoints` as the vault read reports it to a client bound to
/// `region`; `-1` when the read does not answer with a vault at all.
async fn counted_points(router: &MultiServiceRouter, region: &str, name: &str) -> i64 {
    let (_, described) = describe_in(router, region, name).await;
    described
        .get("NumberOfRecoveryPoints")
        .and_then(Value::as_i64)
        .unwrap_or(-1)
}

/// The pairs of Regions the rows span.
fn region_pairs() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("the suite's Region, then another", REGION, "us-east-1"),
        ("the same pair in the other order", "us-east-1", REGION),
        (
            "two Regions no other case works in",
            "eu-central-1",
            "ap-southeast-2",
        ),
        (
            "a Region of another partition",
            "us-gov-west-1",
            "eu-west-1",
        ),
    ]
}

/// One name, two Regions, two independent vaults: the duplicate is refused only
/// in the Region that already holds the name, and the second Region's create
/// succeeds with its own ARN, creation instant, default key and empty
/// recovery-point set.
#[tokio::test]
async fn one_name_is_an_independent_vault_in_each_region() {
    for (case, first, second) in region_pairs() {
        let router = router();

        let (status, created) = create_vault_in(&router, first, SHARED_NAME, "{}").await;
        assert_eq!(
            (status, error_code(&created).as_str()),
            (200, "<none>"),
            "case {case:?}: expected CreateBackupVault 200 in {first}, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "BackupVaultArn"),
            vault_arn(first, SHARED_NAME),
            "case {case:?}: the vault belongs to {first}, got {created:?}"
        );

        // The SAME name in the SAME Region, with no CreatorRequestId, is the
        // duplicate the rule refuses — with the condition's own fixed sentence.
        let (status, refused) = create_vault_in(&router, first, SHARED_NAME, "{}").await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (
                400,
                "AlreadyExistsException",
                "Backup vault with the same name already exists"
            ),
            "case {case:?}: expected the duplicate in {first} to be refused, got {status} \
             {refused:?}"
        );

        // The same name in the OTHER Region is a different vault.
        let (status, far) = create_vault_in(&router, second, SHARED_NAME, "{}").await;
        assert_eq!(
            (status, error_code(&far).as_str()),
            (200, "<none>"),
            "case {case:?}: expected CreateBackupVault 200 in {second}, got {status} {far:?}"
        );
        assert_eq!(
            string_at(&far, "BackupVaultArn"),
            vault_arn(second, SHARED_NAME),
            "case {case:?}: the second vault belongs to {second}, got {far:?}"
        );
        assert_ne!(
            string_at(&far, "BackupVaultArn"),
            string_at(&created, "BackupVaultArn"),
            "case {case:?}: the two vaults must not share one ARN, got {far:?}"
        );
        assert!(
            far.get("CreationDate").is_some(),
            "case {case:?}: the second vault records its own creation instant, got {far:?}"
        );

        // Each Region's read answers from its OWN vault.
        for region in [first, second] {
            let (status, described) = describe_in(&router, region, SHARED_NAME).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeBackupVault 200 in {region}, got {status} \
                 {described:?}"
            );
            assert_eq!(
                string_at(&described, "BackupVaultArn"),
                vault_arn(region, SHARED_NAME),
                "case {case:?}: the described vault is {region}'s own, got {described:?}"
            );
            assert_eq!(
                described.get("NumberOfRecoveryPoints").and_then(Value::as_i64),
                Some(0),
                "case {case:?}: a fresh vault in {region} holds no recovery points, got \
                 {described:?}"
            );
            let key = string_at(&described, "EncryptionKeyArn");
            assert!(
                key.starts_with(&format!("arn:aws:kms:{region}:{ACCOUNT}:key/")),
                "case {case:?}: the vault in {region} is encrypted by a key of that Region, \
                 got {key:?}"
            );
            assert_eq!(
                listed_names(&router, region).await,
                vec![SHARED_NAME.to_string()],
                "case {case:?}: {region} lists exactly its own vault"
            );
        }
    }
}

/// One Region's vault answers no call made in another: the recovery points it
/// holds, the reads of it, its listing and its delete are all that Region's
/// alone.
#[tokio::test]
async fn a_vault_of_one_region_answers_no_call_made_in_another() {
    // The Region that holds the vault, and the Regions that hold none of that
    // name — including one that holds a vault of its own later on.
    let home = REGION;
    let strangers = ["us-east-1", "eu-central-1", "ap-southeast-2"];

    let router = router();
    let (status, created) = create_vault_in(&router, home, SHARED_NAME, "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200 in {home}, got {status} {created:?}"
    );

    // A backup into the vault lands in the Region that holds it: the vault's
    // recovery-point index carries it from the moment the job is started, so
    // there is nothing here to wait for.
    let (status, started) = start_job_in(&router, home, SHARED_NAME).await;
    assert_eq!(
        status, 200,
        "setup: expected StartBackupJob 200 in {home}, got {status} {started:?}"
    );
    assert_eq!(
        counted_points(&router, home, SHARED_NAME).await,
        1,
        "case \"the Region that holds the vault\": its own backup is the point it counts"
    );

    for region in strangers {
        let case = format!("a call in {region}, which holds no vault of that name");

        let (status, described) = describe_in(&router, region, SHARED_NAME).await;
        assert_eq!(
            (status, error_code(&described).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: a vault-scoped read of a name this Region does not hold is \
             refused for lack of privileges, got {status} {described:?}"
        );

        let (status, refused) = delete_in(&router, region, SHARED_NAME).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (
                400,
                "ResourceNotFoundException",
                format!("Backup vault {SHARED_NAME} not found").as_str()
            ),
            "case {case:?}: the delete finds no vault of that name, got {status} {refused:?}"
        );

        let (status, refused) = start_job_in(&router, region, SHARED_NAME).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (
                400,
                "ResourceNotFoundException",
                format!("Backup vault {SHARED_NAME} not found").as_str()
            ),
            "case {case:?}: a backup cannot be taken into another Region's vault, got \
             {status} {refused:?}"
        );

        assert!(
            listed_names(&router, region).await.is_empty(),
            "case {case:?}: the listing reports no vault of another Region"
        );
    }

    // The home Region's vault is untouched by every refusal above, and a vault
    // created in one of those Regions now holds none of its recovery points.
    assert_eq!(
        counted_points(&router, home, SHARED_NAME).await,
        1,
        "case \"the home Region after the refusals\": its vault still holds its own backup"
    );
    let far = strangers[0];
    let (status, created) = create_vault_in(&router, far, SHARED_NAME, "{}").await;
    assert_eq!(
        status, 200,
        "case \"the name created in {far} after all\": expected 200, got {status} {created:?}"
    );
    assert_eq!(
        counted_points(&router, far, SHARED_NAME).await,
        0,
        "case \"the second Region's vault\": it holds none of the first Region's points"
    );
    assert_eq!(
        counted_points(&router, home, SHARED_NAME).await,
        1,
        "case \"the home Region after the second create\": its own count is unchanged"
    );

    // Deleting the second Region's vault frees that name in THAT Region only.
    let (status, deleted) = delete_in(&router, far, SHARED_NAME).await;
    assert_eq!(
        status, 200,
        "case \"deleting the second Region's vault\": expected 200, got {status} {deleted:?}"
    );
    let (status, described) = describe_in(&router, far, SHARED_NAME).await;
    assert_eq!(
        (status, error_code(&described).as_str()),
        (403, "AccessDeniedException"),
        "case \"reading the deleted vault\": expected the invisible-vault refusal, got \
         {status} {described:?}"
    );
    let (status, described) = describe_in(&router, home, SHARED_NAME).await;
    assert_eq!(
        status, 200,
        "case \"the home Region after the other Region's delete\": its vault is still there, \
         got {status} {described:?}"
    );
}

/// A legal hold is a Region's own: the vault names its `RecoveryPointSelection`
/// carries name vaults of the Region the hold was created in, so a hold placed
/// in one Region covers none of another Region's recovery points — not even
/// through the entry that names every vault.
///
/// The rows differ along the selection form the rule generalises over: the
/// shared vault name, that name beside one no Region holds, and the entry that
/// names every vault.
#[tokio::test]
async fn a_legal_hold_covers_the_recovery_points_of_its_own_region() {
    /// The `VaultNames` entry that names every vault rather than a vault
    /// (`crate::state`'s `VAULT_NAMES_WILDCARD`).
    const EVERY_VAULT: &str = "*";

    let home = REGION;
    let far = "us-east-1";

    let router = router();
    let mut points: Vec<(&str, String)> = Vec::new();
    for region in [home, far] {
        let (status, created) = create_vault_in(&router, region, SHARED_NAME, "{}").await;
        assert_eq!(
            status, 200,
            "setup: expected CreateBackupVault 200 in {region}, got {status} {created:?}"
        );
        let (status, started) = start_job_in(&router, region, SHARED_NAME).await;
        assert_eq!(
            status, 200,
            "setup: expected StartBackupJob 200 in {region}, got {status} {started:?}"
        );
        points.push((region, string_at(&started, "RecoveryPointArn")));
    }
    // Nothing to wait for: a hold covers the recovery points that already
    // existed when it was placed, whatever state their backups are in, and both
    // backups were started above.
    let home_point = points
        .iter()
        .find(|(region, _)| *region == home)
        .map(|(_, arn)| arn.clone())
        .expect("the backup taken in the home Region");

    let selections: Vec<(&str, Value)> = vec![
        ("the shared vault name", json!([SHARED_NAME])),
        (
            "the shared name beside one no Region holds",
            json!([SHARED_NAME, "no-such-vault"]),
        ),
        ("the entry that names every vault", json!([EVERY_VAULT])),
    ];

    for (case, vault_names) in selections {
        let request = json!({
            "Title": "Region scope",
            "Description": format!("a hold over {case}"),
            "RecoveryPointSelection": { "VaultNames": vault_names },
        });
        let (status, created) = call_in_region(
            &router,
            home,
            "POST",
            "/legal-holds",
            Some(&request.to_string()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLegalHold 200 in {home}, got {status} {created:?}"
        );
        let id = string_at(&created, "LegalHoldId");

        let (status, page) = call_in_region(
            &router,
            home,
            "GET",
            &format!("/legal-holds/{id}/recovery-points"),
            None,
        )
        .await;
        let covered: Vec<String> = page
            .get("RecoveryPoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|point| string_at(point, "RecoveryPointArn"))
            .collect();
        assert_eq!(
            (status, covered.as_slice()),
            (200, [home_point.clone()].as_slice()),
            "case {case:?}: the hold covers {home}'s own recovery point and no other \
             Region's, got {status} {page:?}"
        );
    }
}
