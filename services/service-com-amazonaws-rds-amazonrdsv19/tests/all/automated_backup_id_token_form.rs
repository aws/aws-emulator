// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RESOURCE ID an automated backup's ARN carries: the `ab-` token
//! inside `DBInstanceAutomatedBackupsArn`.
//!
//! Every wire call goes over the wire through `db_instances::emulator`, as a real awsQuery
//! form POST carrying the SigV4 credential scope the router keys on, so the assertions read
//! the XML the emulator itself answered with.
//!
//! The rule under test: the ARN an automated backup publishes names it by the `auto-backup`
//! resource type and an `ab-` prefixed token of 64 characters drawn only from the lowercase
//! base32 alphabet (`a`-`z` and the digits `2`-`7`, so never `0`, `1`, `8` or `9`) — the
//! width and the alphabet the service answers one in — minted as one stable function of the
//! source DB instance's `DbiResourceId` and its Region, out of which that resource id reads
//! back. So one backup reports one ARN on every read whichever member named it, two Regions
//! name the backups they each hold differently, and a value carrying a token of another width
//! or alphabet names no automated backup of any account and is refused on the input.
//!
//! The tables below span what the rule generalises over: the Regions a backup may live in,
//! the engine families its source instance may run, both statuses a reported backup can be in
//! (the `active` backup of a live instance and the `retained` backup of a deleted one), the
//! source resource-id forms a caller may name — the minted width in either case, and ids
//! shorter and longer than it — and each way a token can fall outside the published form:
//! every digit outside the alphabet, an uppercase character, a hyphen, and one character
//! either side of the published width.
//!
//! Nothing here waits out a span: the state the clock owes an instance is reached by polling a
//! REPORTING call through `lifecycle_poll_loops::poll_until`.

use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, REGION,
};
use crate::lifecycle_poll_loops::poll_until;

/// The account id every generated emulator presents, which the ARN form below is read
/// against.
const ACCOUNT: &str = "000000000000";

/// The master user every instance below is created with, and its password: test credentials,
/// and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The prefix an automated backup's own resource id carries inside the ARN
/// `DBInstanceAutomatedBackupsArn` publishes.
const BACKUP_ID_PREFIX: &str = "ab-";

/// How many characters that id carries behind its prefix, and the alphabet those characters
/// are drawn from: the width and the character set the recorded auto-backup ARN writes the id
/// in — lowercase base32, which holds no `0`, `1`, `8` or `9`.
const BACKUP_ID_TOKEN_LEN: usize = 64;
const BACKUP_ID_ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz234567";

/// The `awsQueryError` code the Query API answers a member value it will not serve with, and
/// the code of `DBInstanceAutomatedBackupNotFoundFault` as the model publishes it.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";
const NOT_FOUND: &str = "DBInstanceAutomatedBackupNotFound";

/// The statuses a reported automated backup carries: `active` "for current instances" and
/// `retained` "for deleted instances".
const ACTIVE: &str = "active";
const RETAINED: &str = "retained";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// One row of the published-form table: an instance to create, where and what it runs, and
/// whether the backup is read while the instance is still there or after it is deleted.
struct PublishedCase {
    case: &'static str,
    identifier: &'static str,
    region: &'static str,
    engine: &'static str,
    /// The `BackupRetentionPeriod` the create asks for.
    retention: i32,
    /// Whether the source instance is DELETED with its backups kept before the read, so what
    /// is read is the `retained` backup it left behind.
    retained: bool,
}

/// RULE: the id an automated backup's ARN carries is the `ab-` prefix and 64 characters of the
/// lowercase base32 alphabet, published the same whichever member named the backup and
/// whatever status it is in.
#[tokio::test]
async fn the_published_backup_id_is_sixty_four_base32_characters() {
    let table: &[PublishedCase] = &[
        PublishedCase {
            case: "the active backup of a live instance",
            identifier: "emu-abid-active",
            region: REGION,
            engine: "postgres",
            retention: 1,
            retained: false,
        },
        PublishedCase {
            case: "another engine family, and a longer retention period",
            identifier: "emu-abid-mysql",
            region: REGION,
            engine: "mysql",
            retention: 7,
            retained: false,
        },
        PublishedCase {
            case: "an instance of another Region",
            identifier: "emu-abid-other-region",
            region: "eu-central-1",
            engine: "postgres",
            retention: 1,
            retained: false,
        },
        PublishedCase {
            case: "the retained backup of a deleted instance",
            identifier: "emu-abid-retained",
            region: REGION,
            engine: "mariadb",
            retention: 2,
            retained: true,
        },
    ];

    let router = emulator();
    for row in table {
        let case = row.case;
        let resource_id = available_instance(&router, row).await;
        let reported = expect_ok(
            case,
            describe_backups(
                &router,
                row.region,
                &params!["DbiResourceId" => &resource_id],
            )
            .await,
        );
        let backup = the_backup(case, &reported);
        let arn = text(&backup, "DBInstanceAutomatedBackupsArn");
        assert_backup_id_form(case, row.region, &arn);
        assert_eq!(
            text(&backup, "Status"),
            ACTIVE,
            "case {case:?}: the backup of a live instance must be {ACTIVE:?}, got {backup}"
        );

        // The same ARN on a second read, and the same record when the ARN is what names it:
        // the id is a function of the backup, so the value a client kept resolves back to the
        // one backup it was published for.
        let reread = expect_ok(
            case,
            describe_backups(
                &router,
                row.region,
                &params!["DBInstanceAutomatedBackupsArn" => &arn],
            )
            .await,
        );
        let reread = the_backup(case, &reread);
        assert_eq!(
            text(&reread, "DBInstanceAutomatedBackupsArn"),
            arn,
            "case {case:?}: a backup named by its ARN must report that ARN, got {reread}"
        );
        assert_eq!(
            text(&reread, "DbiResourceId"),
            resource_id,
            "case {case:?}: the ARN must resolve to its source instance, got {reread}"
        );

        if !row.retained {
            continue;
        }
        // The backup OUTLIVES its source instance, and the ARN it publishes is the same one:
        // the id is the source instance's, not the record's.
        delete_instance(&router, row).await;
        let by_arn: Vec<(&str, String)> =
            params!["DBInstanceAutomatedBackupsArn" => &arn].to_vec();
        let (settled, _) = poll_until(
            case,
            &format!("{} leaving a retained backup", row.identifier),
            || describe_backups(&router, row.region, &by_arn),
            |status, body| {
                status == 200 && optional_text(body, "Status").as_deref() == Some(RETAINED)
            },
        )
        .await;
        let kept = the_backup(case, &settled);
        assert_backup_id_form(case, row.region, &text(&kept, "DBInstanceAutomatedBackupsArn"));
        assert_eq!(
            text(&kept, "DBInstanceAutomatedBackupsArn"),
            arn,
            "case {case:?}: a retained backup must keep the ARN it published, got {kept}"
        );
    }
}

/// One row of the minting table: how a caller spells the source instance's resource id, and
/// what the id behind the backup's ARN then says about it.
struct MintingCase {
    case: &'static str,
    dbi_resource_id: &'static str,
    /// The `DbiResourceId` that id's backup ARN reads back as, where the id is of the width
    /// this service mints one in; `None` for an id of another width, which names no instance
    /// of this store and so carries no source field to read.
    reads_back_as: Option<&'static str>,
}

/// RULE: the backup id is ONE function of the source instance's `DbiResourceId` and its
/// Region — stable across reads, insensitive to the case the id was spelled in, different per
/// Region and per source instance — and the source id reads back out of it.
#[tokio::test]
async fn the_backup_id_is_a_stable_function_of_the_source_id_and_the_region() {
    // The rows span the resource-id forms a caller may name: the minted width in upper, lower
    // and mixed case, a second minted-width id, and ids shorter and longer than the minted
    // width. The minted width is `DbiResourceId`'s own, `db-` and 26 characters.
    let table: &[MintingCase] = &[
        MintingCase {
            case: "a resource id of the minted width",
            dbi_resource_id: "db-L2IJCEXJP7XQ7HOJ4SIEXAMPLE",
            reads_back_as: Some("db-L2IJCEXJP7XQ7HOJ4SIEXAMPLE"),
        },
        MintingCase {
            case: "the same id spelled in lowercase, which is not case-sensitive",
            dbi_resource_id: "db-l2ijcexjp7xq7hoj4siexample",
            reads_back_as: Some("db-L2IJCEXJP7XQ7HOJ4SIEXAMPLE"),
        },
        MintingCase {
            case: "the same id spelled in mixed case",
            dbi_resource_id: "DB-L2ijCEXJp7xq7HOJ4siEXAMPLE",
            reads_back_as: Some("db-L2IJCEXJP7XQ7HOJ4SIEXAMPLE"),
        },
        MintingCase {
            case: "another resource id of the minted width",
            dbi_resource_id: "db-4PCZZZQG5R6HKL7MN2UWTYABCD",
            reads_back_as: Some("db-4PCZZZQG5R6HKL7MN2UWTYABCD"),
        },
        MintingCase {
            case: "a well-formed resource id shorter than the minted width",
            dbi_resource_id: "db-ABC123",
            reads_back_as: None,
        },
        MintingCase {
            case: "a well-formed resource id longer than the minted width",
            dbi_resource_id: "db-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            reads_back_as: None,
        },
    ];
    // The Regions the same source id is minted a backup id in.
    let regions = [REGION, "us-east-1", "eu-central-1"];

    let mut minted: Vec<(String, String)> = Vec::new();
    for row in table {
        let case = row.case;
        for region in regions {
            let arn = st::instance_automated_backups_arn(region, row.dbi_resource_id);
            assert_backup_id_form(case, region, &arn);
            assert_eq!(
                arn,
                st::instance_automated_backups_arn(region, row.dbi_resource_id),
                "case {case:?}: the id must be the same on every read, got {arn:?}"
            );
            assert_eq!(
                st::dbi_resource_id_in_automated_backups_arn(&arn),
                row.reads_back_as.map(str::to_string),
                "case {case:?}: expected {:?} to read back out of {arn:?}",
                row.reads_back_as
            );
            minted.push((format!("{case} in {region}"), arn));
        }
    }

    // Two Regions name the backups they each hold differently, and so do two source
    // instances — but the three spellings of ONE id name ONE backup, which is why the ids
    // minted for them collide and are counted once here.
    let spellings_of_one_id = 3;
    let distinct: std::collections::BTreeSet<&String> =
        minted.iter().map(|(_, arn)| arn).collect();
    assert_eq!(
        distinct.len(),
        minted.len() - regions.len() * (spellings_of_one_id - 1),
        "expected one id per source instance and Region, got {minted:?}"
    );
}

/// One row of the refusal table: a token behind the `ab-` prefix, and why it is outside the
/// published form.
struct TokenCase {
    case: &'static str,
    token: String,
}

/// RULE: a token outside the published width or alphabet names no automated backup of any
/// account, so the value is refused on the INPUT by every operation that takes the member,
/// while a token of the published form naming nothing is the not-found fault — worded with the
/// source instance's resource id, which that token is read back out of.
#[tokio::test]
async fn a_token_outside_the_published_width_or_alphabet_names_no_backup() {
    let mut table: Vec<TokenCase> = vec![
        TokenCase {
            case: "a token one character short of the published width",
            token: "a".repeat(BACKUP_ID_TOKEN_LEN - 1),
        },
        TokenCase {
            case: "a token one character over the published width",
            token: "a".repeat(BACKUP_ID_TOKEN_LEN + 1),
        },
        TokenCase {
            case: "a token of the regional resource-id form",
            token: "L2IJCEXJP7XQ7HOJ4SIEXAMPLE".to_string(),
        },
        TokenCase {
            case: "a token carrying an uppercase character",
            token: format!("A{}", "b".repeat(BACKUP_ID_TOKEN_LEN - 1)),
        },
        TokenCase {
            case: "a token carrying a hyphen",
            token: format!("-{}", "b".repeat(BACKUP_ID_TOKEN_LEN - 1)),
        },
    ];
    // Every digit the published alphabet does NOT hold: a token of the published width whose
    // only fault is one such character, which is what a hexadecimal tail would carry.
    for digit in ['0', '1', '8', '9'] {
        table.push(TokenCase {
            case: "a token carrying a digit outside the alphabet",
            token: format!("{digit}{}", "b".repeat(BACKUP_ID_TOKEN_LEN - 1)),
        });
    }

    let router = emulator();
    // A token of the published form carrying a source instance's id nothing was created
    // under: well formed, so the answer is the lookup's and it names that source id.
    let unknown_resource_id = "db-NOTHINGWASEVERCREATEDUNDER";
    let unknown_arn = st::instance_automated_backups_arn(REGION, unknown_resource_id);
    assert_backup_id_form("a well-formed ARN naming no backup", REGION, &unknown_arn);

    for operation in [
        "DescribeDBInstanceAutomatedBackups",
        "DeleteDBInstanceAutomatedBackup",
    ] {
        for row in &table {
            let value = auto_backup_arn(&row.token);
            let case = format!("{operation}: {}", row.case);
            let refused = expect_error(
                &case,
                call(
                    &router,
                    operation,
                    &params!["DBInstanceAutomatedBackupsArn" => &value],
                )
                .await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message,
                format!(
                    "The provided ARN ({value}) is invalid for this parameter \
                     (DbInstanceAutomatedBackupsArn)."
                ),
                "case {case:?}: expected the ARN refusal for {value:?}, got {message:?}"
            );
        }

        let case = format!("{operation}: a well-formed ARN naming no backup");
        let refused = expect_error(
            &case,
            call(
                &router,
                operation,
                &params!["DBInstanceAutomatedBackupsArn" => &unknown_arn],
            )
            .await,
            404,
            NOT_FOUND,
        );
        let message = text(&refused, "Message");
        assert!(
            message.contains(unknown_resource_id),
            "case {case:?}: expected the refusal to name {unknown_resource_id:?}, got \
             {message:?}"
        );
    }
}

/// An auto-backup ARN of this Region and account carrying `token` behind the `ab-` prefix —
/// what a client that composed one itself would send.
fn auto_backup_arn(token: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:auto-backup:{BACKUP_ID_PREFIX}{token}")
}

/// Assert `arn` names an automated backup of `region` by an id of the published form: the
/// `auto-backup` resource type, the `ab-` prefix, and the published width in the published
/// alphabet.
fn assert_backup_id_form(case: &str, region: &str, arn: &str) {
    let prefix = format!("arn:aws:rds:{region}:{ACCOUNT}:auto-backup:{BACKUP_ID_PREFIX}");
    let Some(token) = arn.strip_prefix(&prefix) else {
        panic!("case {case:?}: expected an auto-backup ARN under {prefix:?}, got {arn:?}");
    };
    assert_eq!(
        token.chars().count(),
        BACKUP_ID_TOKEN_LEN,
        "case {case:?}: expected {BACKUP_ID_TOKEN_LEN} characters of backup id, got \
         {token:?} in {arn:?}"
    );
    let outside: String = token
        .chars()
        .filter(|character| !BACKUP_ID_ALPHABET.contains(*character))
        .collect();
    assert!(
        outside.is_empty(),
        "case {case:?}: expected a backup id drawn only from {BACKUP_ID_ALPHABET:?}, got \
         {outside:?} in {token:?}"
    );
}

/// The one `<DBInstanceAutomatedBackup>` subtree a report carries.
fn the_backup(case: &str, body: &str) -> String {
    let reported = subtrees(body, "DBInstanceAutomatedBackup");
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one automated backup, got {body}"
    );
    reported.into_iter().next().expect("one backup")
}

/// `DescribeDBInstanceAutomatedBackups` in `region`, with the members one case names.
async fn describe_backups(
    router: &MultiServiceRouter,
    region: &str,
    query: &[(&str, String)],
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeDBInstanceAutomatedBackups",
        query,
    )
    .await
}

/// `CreateDBInstance` for one row in its own Region, and the `DbiResourceId` the created
/// instance reports once it is `available` — the condition a client waits for before it reads
/// the instance's automated backup, polled for rather than waited out.
async fn available_instance(router: &MultiServiceRouter, row: &PublishedCase) -> String {
    let request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => row.identifier,
        "Engine" => row.engine,
        "DBInstanceClass" => "db.t3.micro",
        "AllocatedStorage" => 20,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
        "BackupRetentionPeriod" => row.retention,
    ]
    .to_vec();
    expect_ok(
        row.case,
        call_in(router, row.region, "CreateDBInstance", &request).await,
    );
    let (described, _) = poll_until(
        row.case,
        &format!("{} reaching available", row.identifier),
        || describe_instance(router, row),
        |status, body| {
            status == 200 && optional_text(body, "DBInstanceStatus").as_deref() == Some("available")
        },
    )
    .await;
    text(&described, "DbiResourceId")
}

/// `DescribeDBInstances` for one row's instance — the REPORTING call the polls here make.
async fn describe_instance(router: &MultiServiceRouter, row: &PublishedCase) -> (u16, String) {
    call_in(
        router,
        row.region,
        "DescribeDBInstances",
        &params!["DBInstanceIdentifier" => row.identifier],
    )
    .await
}

/// `DeleteDBInstance` for one row, KEEPING its automated backups, polled until the instance
/// itself is gone.
async fn delete_instance(router: &MultiServiceRouter, row: &PublishedCase) {
    let request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => row.identifier,
        "SkipFinalSnapshot" => true,
        "DeleteAutomatedBackups" => false,
    ]
    .to_vec();
    expect_ok(
        row.case,
        call_in(router, row.region, "DeleteDBInstance", &request).await,
    );
    poll_until(
        row.case,
        &format!("{} being gone", row.identifier),
        || describe_instance(router, row),
        |status, _| status == 404,
    )
    .await;
}
