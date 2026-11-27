// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ENVELOPE `DescribeDBInstanceAutomatedBackups` publishes for one
//! automated backup, and for the FORM the two automated-backup calls hold their input to.
//!
//! Every call goes over the wire through `db_instances::emulator`, as a real awsQuery form
//! POST carrying the SigV4 credential scope the router keys on, so the assertions read the
//! XML the emulator itself answered with.
//!
//! The rules under test:
//!
//! * an automated backup reports "the database instance properties that existed at the time
//!   you deleted the source instance", so every property of it is READ from the source
//!   instance: `StorageType` is the source's storage type and `StorageThroughput` the
//!   throughput that type is provisioned with, whatever the source was created with;
//! * `RestoreWindow` — "the earliest and latest time a DB instance can be restored to" — is
//!   the backup's own structure, published whatever the backup covers, and carries NEITHER
//!   time while the backup is still `creating` ("waiting for the first automated snapshot to
//!   be available") rather than a span whose two ends are the same instant;
//! * `DBInstanceAutomatedBackupsArn` names the backup by an id of its OWN form — the `ab-`
//!   prefix and a fixed-width token of the lowercase base32 alphabet — unique to the
//!   backup and stable across reads;
//! * a member that must carry a DB resource id is checked for FORM before any lookup by every
//!   operation that takes it, so a value that is no `DbiResourceId` is `InvalidParameterValue`
//!   (HTTP 400) stating the rule, while a well-formed id is answered by the lookup it names
//!   — the same split the ARN member is held to.
//!
//! The tables below span what the rules generalise over: the storage types and throughputs an
//! instance may be created with (named and defaulted), the engine families it may run, each
//! way a value can fail to be a DB resource id, well-formed ids of the minted width and of
//! other widths, every operation that takes each member, and each way an auto-backup ARN's own
//! id can fail its form.
//!
//! Nothing here waits out a span: the state the clock owes an instance is reached by polling a
//! REPORTING call through `lifecycle_poll_loops::poll_until`.

use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_automated_backups::available_instance;
use crate::db_instances::{
    call, element_is_empty, emulator, expect_error, expect_ok, optional_text, subtrees, text,
    REGION,
};
use crate::lifecycle_poll_loops::poll_until;

/// The account id every generated emulator presents, which the ARN form below is read
/// against.
const ACCOUNT: &str = "000000000000";

/// The master user every instance below is created with, and its password: test credentials,
/// and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The `awsQueryError` code of `DBInstanceAutomatedBackupNotFoundFault`, as the model
/// publishes it.
const NOT_FOUND: &str = "DBInstanceAutomatedBackupNotFound";

/// The `awsQueryError` code the Query API answers a member value it will not serve with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The status an automated backup reports while its source instance is still coming up, and
/// the one it reports once the instance is there.
const CREATING: &str = "creating";
const ACTIVE: &str = "active";

/// The prefix an automated backup's own resource id carries, inside the ARN
/// `DBInstanceAutomatedBackupsArn` publishes.
const BACKUP_ID_PREFIX: &str = "ab-";

/// How many characters that id carries behind its prefix, and the alphabet they are drawn
/// from: the width and the character set the recorded auto-backup ARN writes it in — lowercase
/// base32, which holds no `0`, `1`, `8` or `9`. The id's own form is asserted in full, over
/// the Regions and source resource-id forms it is minted for, in
/// `automated_backup_id_token_form`.
const BACKUP_ID_TOKEN_LEN: usize = 64;
const BACKUP_ID_ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz234567";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// One row of the storage table: how the create spells the storage settings, and what the
/// source instance — and therefore its automated backup — then reports for them.
struct StorageCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The storage members the create names, if any.
    storage: &'static [(&'static str, &'static str)],
    /// `StorageType` the instance is provisioned with.
    expected_type: &'static str,
    /// `StorageThroughput` that storage type is provisioned with. Source:
    /// `CreateDBInstanceMessage$StorageThroughput`, which "applies only to the gp3 storage
    /// type" — every other type is provisioned none, reported as zero.
    expected_throughput: &'static str,
}

/// RULE: every property of an automated backup is the SOURCE INSTANCE's, so the storage type
/// and throughput the backup reports are the ones the instance itself reports — never a
/// default written into the projection.
#[tokio::test]
async fn the_published_envelope_reports_the_source_instances_storage() {
    // The rows span the storage types a create may name, the baseline and a named throughput
    // of the one type that has one, the type a create that names none is provisioned with,
    // and two engine families.
    let table: &[StorageCase] = &[
        StorageCase {
            case: "general purpose SSD gp2, which has no provisioned throughput",
            identifier: "emu-abf-gp2",
            engine: "postgres",
            storage: &[("StorageType", "gp2")],
            expected_type: "gp2",
            expected_throughput: "0",
        },
        StorageCase {
            case: "general purpose SSD gp3, provisioned its baseline throughput",
            identifier: "emu-abf-gp3",
            engine: "mysql",
            storage: &[("StorageType", "gp3")],
            expected_type: "gp3",
            expected_throughput: "125",
        },
        StorageCase {
            case: "general purpose SSD gp3 with the throughput the create named",
            identifier: "emu-abf-gp3-named",
            engine: "mysql",
            storage: &[("StorageType", "gp3"), ("StorageThroughput", "500")],
            expected_type: "gp3",
            expected_throughput: "500",
        },
        StorageCase {
            case: "magnetic storage",
            identifier: "emu-abf-standard",
            engine: "postgres",
            storage: &[("StorageType", "standard")],
            expected_type: "standard",
            expected_throughput: "0",
        },
        StorageCase {
            case: "a create that names no storage type at all",
            identifier: "emu-abf-default",
            engine: "mariadb",
            storage: &[],
            expected_type: "gp2",
            expected_throughput: "0",
        },
    ];

    let router = emulator();
    let mut published_arns: Vec<String> = Vec::new();
    for row in table {
        let mut extra: Vec<(&str, String)> = params!["BackupRetentionPeriod" => 1].to_vec();
        extra.extend(
            row.storage
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        let resource_id =
            available_instance(&router, row.case, row.identifier, row.engine, &extra).await;

        // What the INSTANCE reports for its storage: the values the backup has to mirror,
        // read from the instance rather than written here.
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => row.identifier],
            )
            .await,
        );
        let instance_type = text(&described, "StorageType");
        let instance_throughput = optional_text(&described, "StorageThroughput");
        assert_eq!(
            instance_type, row.expected_type,
            "case {:?}: expected the instance on {:?}, got {described}",
            row.case, row.expected_type
        );

        let backup = the_backup(
            row.case,
            &expect_ok(
                row.case,
                describe_backups(&router, &params!["DbiResourceId" => &resource_id]).await,
            ),
        );
        let reported_type = text(&backup, "StorageType");
        assert_eq!(
            reported_type, instance_type,
            "case {:?}: the backup must report the source instance's storage type {:?}, got \
             {reported_type:?}",
            row.case, instance_type
        );
        assert_eq!(
            reported_type, row.expected_type,
            "case {:?}: expected {:?}, got {reported_type:?}",
            row.case, row.expected_type
        );
        let reported_throughput = text(&backup, "StorageThroughput");
        assert_eq!(
            Some(reported_throughput.clone()),
            instance_throughput,
            "case {:?}: the backup must report the source instance's storage throughput \
             {instance_throughput:?}, got {reported_throughput:?}",
            row.case
        );
        assert_eq!(
            reported_throughput, row.expected_throughput,
            "case {:?}: expected {:?}, got {reported_throughput:?}",
            row.case, row.expected_throughput
        );

        // The ARN this backup publishes is of the form an auto-backup ARN is written in, and
        // it is the ARN the one derivation of that form answers for this instance.
        let backups_arn = text(&backup, "DBInstanceAutomatedBackupsArn");
        assert_backup_arn_form(row.case, &backups_arn);
        assert_eq!(
            backups_arn,
            st::instance_automated_backups_arn(REGION, &resource_id),
            "case {:?}: the report must publish the ARN this service derives for the \
             backup of {resource_id}, got {backups_arn}",
            row.case
        );
        // Read again: the id is stable, which is what a client that stored it relies on.
        let reread = the_backup(
            row.case,
            &expect_ok(
                row.case,
                describe_backups(&router, &params!["DbiResourceId" => &resource_id]).await,
            ),
        );
        assert_eq!(
            text(&reread, "DBInstanceAutomatedBackupsArn"),
            backups_arn,
            "case {:?}: the published ARN must not change between two reads, got {reread}",
            row.case
        );
        // And it is the ARN of THIS backup: no two backups of this Region share one.
        assert!(
            !published_arns.contains(&backups_arn),
            "case {:?}: {backups_arn} was already published for another backup",
            row.case
        );
        published_arns.push(backups_arn);
    }
}

/// RULE: a `RestoreWindow` carries an earliest and a latest time only while the backup HAS an
/// instant it can be restored to; one still waiting for its first automated snapshot publishes
/// the member carrying neither, rather than a window whose two ends are the same instant.
#[tokio::test]
async fn a_backup_with_nothing_to_restore_to_yet_reports_an_empty_restore_window() {
    let router = emulator();
    let case = "a backup read through the span its source instance is coming up in";
    let identifier = "emu-abf-window";
    let resource_id = create_instance(
        &router,
        case,
        identifier,
        "postgres",
        &params!["BackupRetentionPeriod" => 2],
    )
    .await;

    // The create was accepted a moment ago, so the backup is still `creating`: the status the
    // member documents for one "waiting for the first automated snapshot to be available".
    let first = the_backup(
        case,
        &expect_ok(
            case,
            describe_backups(&router, &params!["DbiResourceId" => &resource_id]).await,
        ),
    );
    assert_eq!(
        text(&first, "Status"),
        CREATING,
        "case {case:?}: the backup of an instance that is still coming up must report \
         {CREATING:?}, got {first}"
    );
    let coming_up = subtrees(&first, "RestoreWindow");
    assert_eq!(
        coming_up.len(),
        1,
        "case {case:?}: the backup publishes its window member whatever it covers, got {first}"
    );
    assert!(
        element_is_empty(&coming_up[0], "RestoreWindow"),
        "case {case:?}: a backup with no instant to restore to must carry neither time, got \
         {first}"
    );

    // Once the instance is up, the backup covers a window — polled for, never waited out.
    let query = params!["DbiResourceId" => &resource_id];
    let (reported, _) = poll_until(
        case,
        "the backup reaching active",
        || describe_backups(&router, &query),
        |status, body| status == 200 && optional_text(body, "Status").as_deref() == Some(ACTIVE),
    )
    .await;
    let backup = the_backup(case, &reported);
    let window = subtrees(&backup, "RestoreWindow");
    assert_eq!(
        window.len(),
        1,
        "case {case:?}: an active backup must report the window it can be restored in, got \
         {backup}"
    );
    let earliest = text(&window[0], "EarliestTime");
    let latest = text(&window[0], "LatestTime");
    assert!(
        earliest < latest,
        "case {case:?}: expected EarliestTime strictly before LatestTime, got {earliest} and \
         {latest}"
    );
}

/// One row of the resource-id table: the value the member carries, and whether it IS a DB
/// resource id — a well-formed one is a lookup, anything else is refused on the input.
struct ResourceIdCase {
    case: &'static str,
    value: &'static str,
    well_formed: bool,
}

/// The sentence a value that is no `DbiResourceId` is refused with: the rule itself, entirely
/// fixed text, naming the wire parameter Amazon RDS prints for the member.
fn expected_invalid_resource_id() -> String {
    "The parameter dbi-resource-id is not a valid DbiResourceId. The DbiResourceId must begin \
     with db- prefix; remaining part must contain only ASCII letters and digits; and must not \
     be empty."
        .to_string()
}

/// RULE: `DbiResourceId` carries a value with a GRAMMAR — the `db-` prefix and a non-empty
/// remainder of ASCII letters and digits — checked by every operation that takes it BEFORE any
/// lookup: a value outside it is `InvalidParameterValue` stating the rule, and a well-formed
/// id is answered by the lookup it names, whatever that operation answers for a resource id
/// nothing was created under.
#[tokio::test]
async fn a_dbi_resource_id_that_is_not_one_is_refused_on_the_input_by_every_call_that_takes_it() {
    // The rows span each way a value can fail the form (no prefix, an empty remainder, a
    // remainder carrying a hyphen, an underscore, a dot or a character outside ASCII, an ARN,
    // another resource kind's prefix) and the well-formed ids that reach the lookup: the width
    // this service mints one in, a shorter one, and one written in the other case.
    let table: &[ResourceIdCase] = &[
        ResourceIdCase {
            case: "a value with no prefix at all",
            value: "nonexistent-resource-id",
            well_formed: false,
        },
        ResourceIdCase {
            case: "a DB instance identifier",
            value: "emu-abf-identifier",
            well_formed: false,
        },
        ResourceIdCase {
            case: "the prefix and nothing else",
            value: "db-",
            well_formed: false,
        },
        ResourceIdCase {
            case: "a remainder carrying a hyphen",
            value: "db-not-a-resource-id",
            well_formed: false,
        },
        ResourceIdCase {
            case: "a remainder carrying an underscore",
            value: "db-NOT_A_RESOURCE_ID",
            well_formed: false,
        },
        ResourceIdCase {
            case: "a remainder carrying a dot",
            value: "db-NOT.A.RESOURCEID",
            well_formed: false,
        },
        ResourceIdCase {
            case: "a remainder outside ASCII",
            value: "db-RESOURCEIDÉ",
            well_formed: false,
        },
        ResourceIdCase {
            case: "an ARN of the source DB instance",
            value: "arn:aws:rds:us-west-2:000000000000:db:emu-abf-identifier",
            well_formed: false,
        },
        ResourceIdCase {
            case: "the resource id of another kind of resource",
            value: "cluster-ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            well_formed: false,
        },
        ResourceIdCase {
            case: "an id of the width this service mints one in",
            value: "db-NOTHINGWASEVERCREATEDUNDER",
            well_formed: true,
        },
        ResourceIdCase {
            case: "a shorter id of letters and digits",
            value: "db-abc123",
            well_formed: true,
        },
        ResourceIdCase {
            case: "an id whose prefix is written in the other case",
            value: "DB-ABCDEF123",
            well_formed: true,
        },
    ];

    let router = emulator();
    // Every operation whose input carries the member: the two automated-backup calls, which
    // resolve it to ONE backup, and the two snapshot reports, which NARROW by it.
    for operation in [
        "DescribeDBInstanceAutomatedBackups",
        "DeleteDBInstanceAutomatedBackup",
        "DescribeDBSnapshots",
        "DescribeDBSnapshotTenantDatabases",
    ] {
        for row in table {
            let case = format!("{operation}: {}", row.case);
            let answer = call(&router, operation, &params!["DbiResourceId" => row.value]).await;
            if row.well_formed {
                // A well-formed id reaches the lookup, and what the lookup answers is that
                // operation's own business: the automated-backup calls resolve the member to
                // one backup and refuse the id that names none, and a report that merely
                // narrows by it serves the records that match — none.
                match narrows_by_the_member(operation) {
                    true => {
                        expect_ok(&case, answer);
                    }
                    false => {
                        let refused = expect_error(&case, answer, 404, NOT_FOUND);
                        let message = text(&refused, "Message");
                        assert_eq!(
                            message,
                            expected_not_found(operation, row.value),
                            "case {case:?}: expected the not-found sentence this call carries, \
                             got {message:?}"
                        );
                    }
                }
                continue;
            }
            let refused = expect_error(&case, answer, 400, INVALID_PARAMETER_VALUE);
            let message = text(&refused, "Message");
            assert_eq!(
                message,
                expected_invalid_resource_id(),
                "case {case:?}: expected the form rule for {:?}, got {message:?}",
                row.value
            );
        }
    }
}

/// Whether `operation` NARROWS its report by the member rather than resolving it to one
/// resource — the reports whose answer for an id naming nothing is an empty report.
fn narrows_by_the_member(operation: &str) -> bool {
    matches!(
        operation,
        "DescribeDBSnapshots" | "DescribeDBSnapshotTenantDatabases"
    )
}

/// RULE: the id inside an automated-backups ARN is of the backup's own form, so a value
/// carrying an id of another form names no backup of any account and is refused on the input,
/// while one carrying a well-formed id that names nothing is the not-found fault — worded with
/// the source instance's resource id, which that id is read back out of.
#[tokio::test]
async fn an_automated_backups_arn_is_held_to_the_form_the_backup_id_is_written_in() {
    let router = emulator();
    // The id of the published form carrying a source instance's token nothing was created
    // under: well formed, so the answer is the lookup's.
    let unknown_resource_id = "db-NOTHINGWASEVERCREATEDUNDER";
    let unknown_arn = st::instance_automated_backups_arn(REGION, unknown_resource_id);
    // And ids of other forms: the shorter, digits-and-uppercase id of a regional resource,
    // one over the published width, and one carrying a character outside the alphabet.
    let malformed: &[(&str, String)] = &[
        (
            "an id of the regional resource-id form",
            auto_backup_arn("L2IJCEXJP7XQ7HOJ4SIEXAMPLE"),
        ),
        (
            "an id one character short of the published width",
            auto_backup_arn(&"a".repeat(BACKUP_ID_TOKEN_LEN - 1)),
        ),
        (
            "an id one character over the published width",
            auto_backup_arn(&"a".repeat(BACKUP_ID_TOKEN_LEN + 1)),
        ),
        (
            "an id carrying an uppercase character",
            auto_backup_arn(&format!("A{}", "b".repeat(BACKUP_ID_TOKEN_LEN - 1))),
        ),
        (
            "an id carrying a hyphen",
            auto_backup_arn(&format!("-{}", "b".repeat(BACKUP_ID_TOKEN_LEN - 1))),
        ),
    ];

    for operation in [
        "DescribeDBInstanceAutomatedBackups",
        "DeleteDBInstanceAutomatedBackup",
    ] {
        for (what, value) in malformed {
            let case = format!("{operation}: {what}");
            let refused = expect_error(
                &case,
                call(
                    &router,
                    operation,
                    &params!["DBInstanceAutomatedBackupsArn" => value],
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
        assert_eq!(
            message,
            expected_not_found(operation, unknown_resource_id),
            "case {case:?}: expected the refusal to name the source instance's resource id, \
             got {message:?}"
        );
    }
}

/// The sentence `operation` words `DBInstanceAutomatedBackupNotFoundFault` in: the report
/// speaks of the backup of one DB instance, the delete of the backups themselves, each with
/// the value the request named the source instance by as its only variable part.
fn expected_not_found(operation: &str, named: &str) -> String {
    match operation {
        "DeleteDBInstanceAutomatedBackup" => {
            format!("Automated backups could not be found for: {named}")
        }
        _ => format!("DBInstanceAutomatedBackup for DBInstance {named} not found."),
    }
}

/// An auto-backup ARN of this Region and account carrying `id` behind the `ab-` prefix — what
/// a client that composed one itself would send.
fn auto_backup_arn(id: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:auto-backup:{BACKUP_ID_PREFIX}{id}")
}

/// Assert `arn` is written in the form an automated backup's ARN is published in: this
/// Region's and account's auto-backup resource, and an id of the backup's own form.
fn assert_backup_arn_form(case: &str, arn: &str) {
    let prefix = auto_backup_arn("");
    let Some(token) = arn.strip_prefix(&prefix) else {
        panic!("case {case:?}: expected an auto-backup ARN under {prefix:?}, got {arn:?}");
    };
    assert_eq!(
        token.len(),
        BACKUP_ID_TOKEN_LEN,
        "case {case:?}: expected {BACKUP_ID_TOKEN_LEN} characters of backup id, got \
         {token:?} in {arn:?}"
    );
    assert!(
        token
            .chars()
            .all(|character| BACKUP_ID_ALPHABET.contains(character)),
        "case {case:?}: expected a backup id drawn only from {BACKUP_ID_ALPHABET:?}, got \
         {token:?}"
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

/// `DescribeDBInstanceAutomatedBackups` with the members one case names.
async fn describe_backups(
    router: &MultiServiceRouter,
    query: &[(&str, String)],
) -> (u16, String) {
    call(router, "DescribeDBInstanceAutomatedBackups", query).await
}

/// `CreateDBInstance` for one row, and the `DbiResourceId` the created instance reports —
/// answered as soon as the create is accepted, while the instance is still `creating`, which
/// is the state the window rule above is read in. Every other case reaches an AVAILABLE
/// instance through the sibling suite's [`available_instance`] loop.
async fn create_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    extra: &[(&str, String)],
) -> String {
    let mut request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => engine,
        "DBInstanceClass" => "db.t3.micro",
        "AllocatedStorage" => 20,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
    ]
    .to_vec();
    request.extend(extra.iter().cloned());
    let created = expect_ok(case, call(router, "CreateDBInstance", &request).await);
    text(&created, "DbiResourceId")
}

