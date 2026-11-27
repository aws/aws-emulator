// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REPLICATION of a DB instance's automated backups to another
//! Amazon Web Services Region: what `StartDBInstanceAutomatedBackupsReplication` enables,
//! what `StopDBInstanceAutomatedBackupsReplication` stops, and what
//! `DescribeDBInstanceAutomatedBackups` reports in each of the two Regions afterwards, and
//! what `DeleteDBInstanceAutomatedBackup` then removes.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope the
//! router keys on — each call signed for the Region it belongs to, which is what makes the
//! source and the destination two different Regions of one emulator. The assertions read the
//! XML the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: the automated backups of the DB instance a `SourceDBInstanceArn`
//! names are replicated INTO the Region the enabling call was made to, which is a different
//! Region from the source's; the destination Region then reports them as an automated backup
//! of its own, under the three statuses `DescribeDBInstanceAutomatedBackups` publishes —
//! `creating` while it is "waiting for the first automated snapshot to be available", `active`
//! once it has one, and `retained` "after backup replication is stopped" — with its own ARN in
//! the destination Region, the SOURCE instance's Region, ARN, resource id and engine and
//! storage properties, and the retention period the request asked for or the source's own; and
//! the source instance's own automated backup reports the replication while it is in force.
//! A source that does not exist, is not up, keeps no automated backups, or is in the Region of
//! the call itself has no replication to enable, and a Region with no replication in force has
//! none to stop.
//!
//! The tables below span what the rule generalises over: the engine families an instance runs
//! (the two whose native wire this emulator speaks, one it does not, and MariaDB), the Region
//! pairs a replication runs between, the retention period the request asks for including both
//! published bounds and one past each, several destination Regions of ONE source, each member
//! the delete of the retained backups accepts, and each way a start and a stop can be refused.
//!
//! Nothing here waits out a span: every state a transition this service owes leads to is
//! reached by polling a REPORTING call through `lifecycle_poll_loops::poll_until`.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    AUTOMATED_BACKUPS_NOT_ENABLED_MESSAGE, BACKUP_RETENTION_RANGE,
    READ_REPLICA_SOURCE_NOT_AVAILABLE_MESSAGE,
};

use crate::db_instances::{
    call_in, element_is_empty, emulator, expect_error, expect_ok, optional_text, subtrees, text,
    texts,
};
use crate::lifecycle_poll_loops::poll_until;

/// The account id every generated emulator presents, which the ARN forms below are read
/// against.
const ACCOUNT: &str = "000000000000";

/// The `awsQueryError` code of `DBInstanceNotFoundFault`, as the model publishes it.
const NOT_FOUND: &str = "DBInstanceNotFound";

/// The `awsQueryError` code of `InvalidDBInstanceStateFault`.
const INVALID_STATE: &str = "InvalidDBInstanceState";

/// The `awsQueryError` code of `InvalidDBInstanceAutomatedBackupStateFault`, which
/// `DeleteDBInstanceAutomatedBackup` answers for backups it cannot delete.
const INVALID_STATE_BACKUP: &str = "InvalidDBInstanceAutomatedBackupState";

/// The `awsQueryError` code of `DBInstanceAutomatedBackupNotFoundFault`, which the report
/// answers for a value that resolves to no automated backup.
const NOT_FOUND_BACKUP: &str = "DBInstanceAutomatedBackupNotFound";

/// The Query API error code Amazon RDS answers a value a parameter does not accept with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The status a replicated automated backup reports while it is "waiting for the first
/// automated snapshot to be available".
const CREATING: &str = "creating";

/// The status it reports once it has one.
const ACTIVE: &str = "active";

/// The status it reports "after backup replication is stopped".
const RETAINED: &str = "retained";

/// The master user every instance below is created with, and its password: test credentials,
/// and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// One row of the replication table: a source instance in one Region whose automated backups
/// are replicated into another.
struct ReplicationCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The Region the source DB instance is created in.
    source_region: &'static str,
    /// The Region its automated backups are replicated into.
    destination_region: &'static str,
    /// The `BackupRetentionPeriod` the source instance keeps.
    source_retention: i32,
    /// The `BackupRetentionPeriod` the start asks for, or `None` to inherit the source's.
    requested_retention: Option<i32>,
}

impl ReplicationCase {
    /// The period the replicated backups are kept for: the one the request asked for, else
    /// the source instance's own.
    fn expected_retention(&self) -> i32 {
        self.requested_retention.unwrap_or(self.source_retention)
    }
}

/// `CreateDBInstance` in `region`, waiting until the instance is `available`, and the
/// `DbiResourceId` and `DBInstanceArn` it reports.
///
/// `pub(crate)` so the sibling suite that drives the STOP leg's own refusal reaches a source
/// instance through this one setup rather than growing a second copy of it.
pub(crate) async fn available_instance(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    identifier: &str,
    engine: &str,
    retention: i32,
) -> (String, String) {
    create_instance(router, case, region, identifier, engine, retention).await;
    // The instance comes up on its own, so what this waits for is the CONDITION a client
    // waits for before it replicates the instance's automated backups.
    let (described, _) = poll_until(
        case,
        &format!("{identifier} reaching available in {region}"),
        || describe_instance(router, region, identifier),
        |status, body| {
            status == 200 && optional_text(body, "DBInstanceStatus").as_deref() == Some("available")
        },
    )
    .await;
    (
        text(&described, "DbiResourceId"),
        text(&described, "DBInstanceArn"),
    )
}

/// `CreateDBInstance` in `region`, without waiting for it to come up.
async fn create_instance(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    identifier: &str,
    engine: &str,
    retention: i32,
) -> String {
    let request = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => engine,
        "DBInstanceClass" => "db.t3.micro",
        "AllocatedStorage" => 20,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
        "BackupRetentionPeriod" => retention,
    ];
    expect_ok(
        case,
        call_in(router, region, "CreateDBInstance", &request).await,
    )
}

/// `DescribeDBInstances` for one instance in `region`.
async fn describe_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeDBInstances",
        &params!["DBInstanceIdentifier" => identifier],
    )
    .await
}

/// `StartDBInstanceAutomatedBackupsReplication` in `destination`, for the source instance
/// `source_arn` names.
pub(crate) async fn start_replication(
    router: &MultiServiceRouter,
    destination: &str,
    source_arn: &str,
    extra: &[(&str, String)],
) -> (u16, String) {
    let mut request: Vec<(&str, String)> =
        params!["SourceDBInstanceArn" => source_arn].to_vec();
    request.extend(extra.iter().cloned());
    call_in(
        router,
        destination,
        "StartDBInstanceAutomatedBackupsReplication",
        &request,
    )
    .await
}

/// `StopDBInstanceAutomatedBackupsReplication` in `destination`.
pub(crate) async fn stop_replication(
    router: &MultiServiceRouter,
    destination: &str,
    source_arn: &str,
) -> (u16, String) {
    call_in(
        router,
        destination,
        "StopDBInstanceAutomatedBackupsReplication",
        &params!["SourceDBInstanceArn" => source_arn],
    )
    .await
}

/// `DescribeDBInstanceAutomatedBackups` in `region`, narrowed to the backups of the source
/// instance whose resource id is `resource_id`.
pub(crate) async fn describe_backups_of(
    router: &MultiServiceRouter,
    region: &str,
    resource_id: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeDBInstanceAutomatedBackups",
        &params!["DbiResourceId" => resource_id],
    )
    .await
}

/// `DeleteDBInstanceAutomatedBackup` in `region`, naming the backups by `member`.
async fn delete_backups(
    router: &MultiServiceRouter,
    region: &str,
    member: &str,
    value: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DeleteDBInstanceAutomatedBackup",
        &params![member => value],
    )
    .await
}

/// The one `<DBInstanceAutomatedBackup>` subtree a report carries.
pub(crate) fn the_backup(case: &str, body: &str) -> String {
    let reported = subtrees(body, "DBInstanceAutomatedBackup");
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: exactly one DBInstanceAutomatedBackup must be reported, got {body}"
    );
    reported.into_iter().next().expect("one backup")
}

/// The ARNs a backup's `DBInstanceAutomatedBackupsReplications` names: "the list of
/// replications to different Amazon Web Services Regions associated with the automated
/// backup", EMPTY when the list carries none.
fn replication_arns(backup: &str) -> Vec<String> {
    subtrees(backup, "DBInstanceAutomatedBackupsReplications")
        .iter()
        .flat_map(|list| texts(list, "DBInstanceAutomatedBackupsArn"))
        .collect()
}

/// The automated-backups ARN the emulator publishes for the backups of the instance whose
/// resource id is `resource_id`, held in `region`, read through the one derivation of that
/// form this service has — so a case that names backups by their ARN names the ARN the report
/// publishes for them, in the Region that holds them.
fn backups_arn(region: &str, resource_id: &str) -> String {
    service_com_amazonaws_rds_amazonrdsv19::state::instance_automated_backups_arn(
        region,
        resource_id,
    )
}

/// A start replicates the source instance's automated backups into the Region of the call:
/// the destination reports them as an automated backup of its own, with its own ARN there,
/// the source's identity and properties, and the retention period the request asked for —
/// `creating` first and `active` once the clock has taken it there — and the source
/// instance's own backup reports the replication.
#[tokio::test]
async fn start_replicates_the_source_instance_backups_into_the_destination_region() {
    let router = emulator();
    let table: &[ReplicationCase] = &[
        ReplicationCase {
            case: "a postgres instance replicated west to east, retention inherited",
            identifier: "emu-abr-postgres",
            engine: "postgres",
            source_region: "us-west-2",
            destination_region: "us-east-1",
            source_retention: 1,
            requested_retention: None,
        },
        ReplicationCase {
            case: "a mysql instance replicated between European Regions, retention raised",
            identifier: "emu-abr-mysql",
            engine: "mysql",
            source_region: "eu-central-1",
            destination_region: "eu-west-1",
            source_retention: 7,
            requested_retention: Some(14),
        },
        ReplicationCase {
            case: "an oracle instance, an engine this emulator serves no wire for, at the \
                   published retention ceiling",
            identifier: "emu-abr-oracle",
            engine: "oracle-ee",
            source_region: "ap-southeast-2",
            destination_region: "ap-northeast-1",
            source_retention: 35,
            requested_retention: Some(*BACKUP_RETENTION_RANGE.end()),
        },
        ReplicationCase {
            case: "a mariadb instance whose replicated backups are kept for fewer days than \
                   the source's",
            identifier: "emu-abr-mariadb",
            engine: "mariadb",
            source_region: "us-east-2",
            destination_region: "us-west-1",
            source_retention: 10,
            requested_retention: Some(1),
        },
    ];

    for row in table {
        let case = row.case;
        let (resource_id, source_arn) = available_instance(
            &router,
            case,
            row.source_region,
            row.identifier,
            row.engine,
            row.source_retention,
        )
        .await;
        let destination_arn = backups_arn(row.destination_region, &resource_id);

        let extra: Vec<(&str, String)> = row
            .requested_retention
            .map(|period| params!["BackupRetentionPeriod" => period].to_vec())
            .unwrap_or_default();
        let started = expect_ok(
            case,
            start_replication(&router, row.destination_region, &source_arn, &extra).await,
        );

        // The call answers the backups it just began replicating: their own ARN in the
        // destination Region, the SOURCE instance's identity and Region, and the retention
        // period asked for — `creating`, because no snapshot has been replicated yet.
        let backup = the_backup(case, &started);
        assert_eq!(
            text(&backup, "DBInstanceAutomatedBackupsArn"),
            destination_arn,
            "case {case:?}: the replicated backups' ARN must be in the destination Region, \
             got {backup}"
        );
        assert_eq!(
            text(&backup, "Status"),
            CREATING,
            "case {case:?}: a replication just enabled waits for its first snapshot, got \
             {backup}"
        );
        assert_eq!(
            text(&backup, "DBInstanceIdentifier"),
            row.identifier,
            "case {case:?}: the backups must name the source instance, got {backup}"
        );
        assert_eq!(
            text(&backup, "DbiResourceId"),
            resource_id,
            "case {case:?}: the backups must carry the source instance's resource id, got \
             {backup}"
        );
        assert_eq!(
            text(&backup, "Region"),
            row.source_region,
            "case {case:?}: Region is the source instance's, got {backup}"
        );
        assert_eq!(
            text(&backup, "DBInstanceArn"),
            source_arn,
            "case {case:?}: DBInstanceArn is the source instance's, got {backup}"
        );
        assert_eq!(
            text(&backup, "BackupRetentionPeriod"),
            row.expected_retention().to_string(),
            "case {case:?}: expected the replicated backups to be kept for {} days, got \
             {backup}",
            row.expected_retention()
        );
        assert_eq!(
            text(&backup, "Engine"),
            row.engine,
            "case {case:?}: the backups report the source instance's engine, got {backup}"
        );
        let window = subtrees(&backup, "RestoreWindow");
        assert_eq!(
            window.len(),
            1,
            "case {case:?}: the backups publish the window member whatever they cover, got \
             {backup}"
        );
        assert!(
            element_is_empty(&window[0], "RestoreWindow"),
            "case {case:?}: a replication with no snapshot yet can be restored to no \
             instant, so the window carries neither time, got {backup}"
        );

        // The destination Region reports them from now on, and the clock takes them to
        // `active`: polling a REPORTING call is what a client does, and only a transition
        // this service owes can end the loop.
        let (reported, _) = poll_until(
            case,
            "the replicated backups reaching active",
            || describe_backups_of(&router, row.destination_region, &resource_id),
            |status, body| {
                status == 200 && optional_text(body, "Status").as_deref() == Some(ACTIVE)
            },
        )
        .await;
        let backup = the_backup(case, &reported);
        assert_eq!(
            text(&backup, "DBInstanceAutomatedBackupsArn"),
            destination_arn,
            "case {case:?}: the destination Region reports the backups under their own ARN, \
             got {backup}"
        );
        // Active backups cover a window: it reaches back no further than the instant
        // replication began, so both of its bounds are reported.
        let window = subtrees(&backup, "RestoreWindow");
        assert_eq!(
            window.len(),
            1,
            "case {case:?}: active replicated backups report the window they cover, got \
             {backup}"
        );
        for member in ["EarliestTime", "LatestTime"] {
            assert!(
                optional_text(&window[0], member).is_some(),
                "case {case:?}: the restore window must carry {member}, got {backup}"
            );
        }
        assert!(
            replication_arns(&backup).is_empty(),
            "case {case:?}: the replicated backups are replicated nowhere further, got \
             {backup}"
        );

        // And the SOURCE Region's report of the instance's own automated backup names the
        // replication, which is the other half of the same relationship.
        let at_source = expect_ok(
            case,
            describe_backups_of(&router, row.source_region, &resource_id).await,
        );
        let source_backup = the_backup(case, &at_source);
        assert_eq!(
            text(&source_backup, "DBInstanceAutomatedBackupsArn"),
            backups_arn(row.source_region, &resource_id),
            "case {case:?}: the source Region's backup keeps its own ARN, got {source_backup}"
        );
        assert_eq!(
            replication_arns(&source_backup),
            vec![destination_arn.clone()],
            "case {case:?}: the source instance's backup must report the replication to \
             {destination_arn}, got {source_backup}"
        );
    }
}

/// One source instance's automated backups replicate into SEVERAL Regions at once, each
/// Region keeping its own record under its own ARN, and the source's backup reports every one
/// of them.
#[tokio::test]
async fn one_source_replicates_into_several_destination_regions() {
    let router = emulator();
    let case = "one postgres source replicated into three Regions";
    let source_region = "us-west-2";
    let destinations = ["us-east-1", "eu-west-2", "sa-east-1"];
    let (resource_id, source_arn) = available_instance(
        &router,
        case,
        source_region,
        "emu-abr-fanout",
        "postgres",
        3,
    )
    .await;

    for destination in destinations {
        let started = expect_ok(
            case,
            start_replication(&router, destination, &source_arn, &[]).await,
        );
        assert_eq!(
            text(&the_backup(case, &started), "DBInstanceAutomatedBackupsArn"),
            backups_arn(destination, &resource_id),
            "case {case:?}: each destination Region reports the backups under its own ARN"
        );
    }

    let at_source = expect_ok(
        case,
        describe_backups_of(&router, source_region, &resource_id).await,
    );
    let mut reported = replication_arns(&the_backup(case, &at_source));
    reported.sort();
    let mut expected: Vec<String> = destinations
        .iter()
        .map(|destination| backups_arn(destination, &resource_id))
        .collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case {case:?}: the source's backup must report one replication per destination \
         Region, got {reported:?}"
    );

    // Each destination keeps only its OWN record: a Region reports the backups replicated
    // into it and no other Region's.
    for destination in destinations {
        let reported = expect_ok(
            case,
            describe_backups_of(&router, destination, &resource_id).await,
        );
        let backup = the_backup(case, &reported);
        assert_eq!(
            text(&backup, "DBInstanceAutomatedBackupsArn"),
            backups_arn(destination, &resource_id),
            "case {case:?}: {destination} must report only its own replicated backups, got \
             {backup}"
        );
    }
}

/// A start the request names a `BackupRetentionPeriod` on is held to the range a DB
/// instance's retention period is published with: both bounds are accepted and a value past
/// either is refused, with the service's own sentence for a period outside the range.
#[tokio::test]
async fn the_requested_retention_period_is_held_to_its_published_range() {
    let router = emulator();
    let case = "the retention period a start asks for";
    let source_region = "us-west-2";
    let (resource_id, source_arn) =
        available_instance(&router, case, source_region, "emu-abr-retention", "mysql", 5).await;

    // Each accepted period is replicated into a destination Region of its own, since one
    // source has one record per destination Region: the published floor, the published
    // ceiling, and a value between them.
    let accepted = [
        (*BACKUP_RETENTION_RANGE.start(), "us-east-1"),
        (*BACKUP_RETENTION_RANGE.end(), "eu-west-1"),
        (12, "ap-south-1"),
    ];
    for (period, destination) in accepted {
        let started = expect_ok(
            &format!("{case}: {period} days"),
            start_replication(
                &router,
                destination,
                &source_arn,
                &params!["BackupRetentionPeriod" => period],
            )
            .await,
        );
        let backup = the_backup(case, &started);
        assert_eq!(
            text(&backup, "BackupRetentionPeriod"),
            period.to_string(),
            "case {case:?}: {period} days is within the published range, got {backup}"
        );
        assert_eq!(
            text(&backup, "DBInstanceAutomatedBackupsArn"),
            backups_arn(destination, &resource_id),
            "case {case:?}: {destination} holds the backups it was asked to, got {backup}"
        );
    }

    // And one past each bound is refused, naming the value and the bounds.
    for period in [*BACKUP_RETENTION_RANGE.start() - 1, *BACKUP_RETENTION_RANGE.end() + 1] {
        let case = format!("{case}: {period} days");
        let refused = expect_error(
            &case,
            start_replication(
                &router,
                "ca-central-1",
                &source_arn,
                &params!["BackupRetentionPeriod" => period],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        assert_eq!(
            text(&refused, "Message"),
            format!(
                "Invalid backup retention period: {period}. Retention period must be between \
                 {} and {}.",
                BACKUP_RETENTION_RANGE.start(),
                BACKUP_RETENTION_RANGE.end()
            ),
            "case {case:?}: the refusal must name the period and the bounds, got {refused}"
        );
        // Nothing was replicated into that Region, so it reports no backups of this source.
        expect_error(
            &case,
            describe_backups_of(&router, "ca-central-1", &resource_id).await,
            404,
            NOT_FOUND_BACKUP,
        );
    }
}

/// A stop ends the replication and leaves the backups it had taken RETAINED in the
/// destination Region: the record stays and reads `retained`, the source's backup stops
/// reporting the replication, a second stop is refused, and a start begins a new replication
/// of the same source into the same Region.
#[tokio::test]
async fn stop_retains_the_replicated_backups_and_can_be_started_again() {
    let router = emulator();
    let case = "a replication stopped and started again";
    let source_region = "us-west-2";
    let destination = "us-east-1";
    let identifier = "emu-abr-stopstart";
    let (resource_id, source_arn) = available_instance(
        &router,
        case,
        source_region,
        identifier,
        "postgres",
        4,
    )
    .await;
    let destination_arn = backups_arn(destination, &resource_id);

    expect_ok(
        case,
        start_replication(&router, destination, &source_arn, &[]).await,
    );
    poll_until(
        case,
        "the replicated backups reaching active",
        || describe_backups_of(&router, destination, &resource_id),
        |status, body| status == 200 && optional_text(body, "Status").as_deref() == Some(ACTIVE),
    )
    .await;

    // The stop answers the backups as they now stand: retained by the destination Region.
    let stopped = expect_ok(case, stop_replication(&router, destination, &source_arn).await);
    let backup = the_backup(case, &stopped);
    assert_eq!(
        text(&backup, "Status"),
        RETAINED,
        "case {case:?}: stopping a replication retains the backups it took, got {backup}"
    );
    assert_eq!(
        text(&backup, "DBInstanceAutomatedBackupsArn"),
        destination_arn,
        "case {case:?}: the retained backups keep the ARN they were replicated under, got \
         {backup}"
    );
    // The destination Region still reports them, and reports them retained.
    let reported = expect_ok(
        case,
        describe_backups_of(&router, destination, &resource_id).await,
    );
    assert_eq!(
        text(&the_backup(case, &reported), "Status"),
        RETAINED,
        "case {case:?}: the destination Region keeps reporting the retained backups, got \
         {reported}"
    );
    // And the source instance's own backup no longer reports a replication to it.
    let at_source = expect_ok(
        case,
        describe_backups_of(&router, source_region, &resource_id).await,
    );
    assert!(
        replication_arns(&the_backup(case, &at_source)).is_empty(),
        "case {case:?}: a stopped replication is no longer one the source reports, got \
         {at_source}"
    );

    // A second stop has no replication to stop: the Region of the call no longer has one from
    // this source, which is the one state this operation refuses.
    let refused = expect_error(
        case,
        stop_replication(&router, destination, &source_arn).await,
        400,
        INVALID_STATE,
    );
    assert_eq!(
        text(&refused, "Message"),
        format!("DB Instance {source_arn} is not replicating to the current region."),
        "case {case:?}: the refusal must name the ARN the request carried, got {refused}"
    );

    // Starting again begins a new replication into the same Region, of the source as it
    // stands now: the record reads `creating` once more and the source reports it again.
    let restarted = expect_ok(
        case,
        start_replication(&router, destination, &source_arn, &[]).await,
    );
    assert_eq!(
        text(&the_backup(case, &restarted), "Status"),
        CREATING,
        "case {case:?}: a replication enabled again waits for its first snapshot, got \
         {restarted}"
    );
    let at_source = expect_ok(
        case,
        describe_backups_of(&router, source_region, &resource_id).await,
    );
    assert_eq!(
        replication_arns(&the_backup(case, &at_source)),
        vec![destination_arn],
        "case {case:?}: the source's backup reports the replication again, got {at_source}"
    );
}

/// A start whose settings are re-asserted on a replication already in force keeps the
/// backups' status and ARN and takes the settings the call carried: the enable is in force
/// once, and nothing the request named is dropped.
#[tokio::test]
async fn starting_a_replication_already_in_force_re_asserts_its_settings() {
    let router = emulator();
    let case = "a start on a replication already in force";
    let destination = "us-east-1";
    let (resource_id, source_arn) =
        available_instance(&router, case, "us-west-2", "emu-abr-again", "mysql", 2).await;

    expect_ok(
        case,
        start_replication(
            &router,
            destination,
            &source_arn,
            &params!["BackupRetentionPeriod" => 3],
        )
        .await,
    );
    let (active, _) = poll_until(
        case,
        "the replicated backups reaching active",
        || describe_backups_of(&router, destination, &resource_id),
        |status, body| status == 200 && optional_text(body, "Status").as_deref() == Some(ACTIVE),
    )
    .await;
    assert_eq!(
        text(&the_backup(case, &active), "BackupRetentionPeriod"),
        "3",
        "case {case:?}: the first start's retention period is the one in force, got {active}"
    );

    let again = expect_ok(
        case,
        start_replication(
            &router,
            destination,
            &source_arn,
            &params!["BackupRetentionPeriod" => 9, "Tags.Tag.1.Key" => "owner", "Tags.Tag.1.Value" => "platform"],
        )
        .await,
    );
    let backup = the_backup(case, &again);
    assert_eq!(
        text(&backup, "Status"),
        ACTIVE,
        "case {case:?}: backups already replicating are not taken again, got {backup}"
    );
    assert_eq!(
        text(&backup, "BackupRetentionPeriod"),
        "9",
        "case {case:?}: the period this call named must be in force, got {backup}"
    );
    assert_eq!(
        texts(&backup, "Key"),
        vec!["owner".to_string()],
        "case {case:?}: the tags this call named must be on the replicated backups, got \
         {backup}"
    );

    // And the store, not the response, is what a later reader sees.
    let reported = expect_ok(
        case,
        describe_backups_of(&router, destination, &resource_id).await,
    );
    let backup = the_backup(case, &reported);
    assert_eq!(
        text(&backup, "BackupRetentionPeriod"),
        "9",
        "case {case:?}: the re-asserted period must be stored, got {backup}"
    );
    assert_eq!(
        texts(&backup, "Value"),
        vec!["platform".to_string()],
        "case {case:?}: the re-asserted tag must be stored, got {backup}"
    );
}

/// The backups a STOPPED replication left a Region are deletable there like any other
/// retained automated backup — by the source instance's resource id and by their own ARN —
/// while the backups of a replication still IN FORCE are not: the replication is stopped
/// first.
#[tokio::test]
async fn the_backups_a_stopped_replication_left_are_deletable() {
    let router = emulator();
    let region = "us-west-2";
    let table = [
        ("DbiResourceId", "emu-abr-delete-by-id", "us-east-1"),
        (
            "DBInstanceAutomatedBackupsArn",
            "emu-abr-delete-by-arn",
            "eu-west-1",
        ),
    ];

    for (member, identifier, destination) in table {
        let case = format!("the replicated backups deleted by {member}");
        let case = case.as_str();
        let (resource_id, source_arn) =
            available_instance(&router, case, region, identifier, "mysql", 2).await;
        let destination_arn = backups_arn(destination, &resource_id);
        let named = match member {
            "DBInstanceAutomatedBackupsArn" => destination_arn.as_str(),
            _ => resource_id.as_str(),
        };

        expect_ok(
            case,
            start_replication(&router, destination, &source_arn, &[]).await,
        );
        // A replication in force is not deleted: it is stopped first, which is the state
        // the modelled fault reports.
        let refused = expect_error(
            case,
            delete_backups(&router, destination, member, named).await,
            400,
            INVALID_STATE_BACKUP,
        );
        assert_eq!(
            text(&refused, "Message"),
            format!(
                "The automated backup is being replicated from the DB instance {identifier} \
                 and can't be deleted until the replication is stopped."
            ),
            "case {case:?}: the refusal must name the source instance, got {refused}"
        );

        expect_ok(case, stop_replication(&router, destination, &source_arn).await);
        let deleted = expect_ok(
            case,
            delete_backups(&router, destination, member, named).await,
        );
        let backup = the_backup(case, &deleted);
        assert_eq!(
            text(&backup, "DBInstanceAutomatedBackupsArn"),
            destination_arn,
            "case {case:?}: the delete reports the backups it removed, got {backup}"
        );
        assert_eq!(
            text(&backup, "Status"),
            RETAINED,
            "case {case:?}: the backups it removed were the retained ones, got {backup}"
        );
        // Nothing reports them afterwards, and the same delete finds nothing to remove.
        expect_error(
            case,
            describe_backups_of(&router, destination, &resource_id).await,
            404,
            NOT_FOUND_BACKUP,
        );
        expect_error(
            case,
            delete_backups(&router, destination, member, named).await,
            404,
            NOT_FOUND_BACKUP,
        );
        // The SOURCE instance's own automated backup is untouched by the destination
        // Region's delete.
        let at_source = expect_ok(
            case,
            describe_backups_of(&router, region, &resource_id).await,
        );
        assert_eq!(
            text(&the_backup(case, &at_source), "Status"),
            ACTIVE,
            "case {case:?}: the source instance keeps its own active backup, got {at_source}"
        );
    }
}

/// How a start and a stop can be refused: the source must be an existing DB instance named by
/// a DB instance ARN, in another Region than the call, up, and keeping automated backups of
/// its own.
#[tokio::test]
async fn a_source_that_cannot_be_replicated_is_refused() {
    let router = emulator();
    let region = "us-west-2";
    let destination = "us-east-1";

    // A well-formed DB instance ARN naming an instance that was never created.
    let missing_identifier = "emu-abr-never-created";
    let missing = format!("arn:aws:rds:{region}:{ACCOUNT}:db:{missing_identifier}");
    // The START resolves that source instance, so it is refused the modelled not-found fault.
    let case = "a start on a source instance that does not exist";
    let refused = expect_error(
        case,
        start_replication(&router, destination, &missing, &[]).await,
        404,
        NOT_FOUND,
    );
    assert_eq!(
        text(&refused, "Message"),
        format!("Requested DB Instance {missing_identifier} not found in source region {region}"),
        "case {case:?}: the refusal must name the instance the source ARN carries and the \
         Region that ARN points at, got {refused}"
    );
    // The STOP resolves no source instance at all: what it asks is whether this Region holds a
    // replication from the instance that ARN names, and an instance that does not exist holds
    // no more of one than an idle instance does — so the answer is the state fault, worded over
    // the ARN the request carried.
    let case = "a stop on a source instance that does not exist";
    let refused = expect_error(
        case,
        stop_replication(&router, destination, &missing).await,
        400,
        INVALID_STATE,
    );
    assert_eq!(
        text(&refused, "Message"),
        format!("DB Instance {missing} is not replicating to the current region."),
        "case {case:?}: the refusal must name the ARN the request carried and state that \
         nothing is replicating here, got {refused}"
    );

    // A value that is not the ARN of a DB instance at all: a bare identifier, an ARN of
    // another resource type of this service, and an ARN of another service.
    for named in [
        "emu-abr-never-created",
        &format!("arn:aws:rds:{region}:{ACCOUNT}:cluster:emu-abr-cluster"),
        &format!("arn:aws:ec2:{region}:{ACCOUNT}:instance:i-0123456789abcdef0"),
    ] {
        for (case, response) in [
            (
                format!("a start whose SourceDBInstanceArn is {named}"),
                start_replication(&router, destination, named, &[]).await,
            ),
            (
                format!("a stop whose SourceDBInstanceArn is {named}"),
                stop_replication(&router, destination, named).await,
            ),
        ] {
            let refused = expect_error(&case, response, 400, INVALID_PARAMETER_VALUE);
            assert_eq!(
                text(&refused, "Message"),
                format!(
                    "The parameter SourceDBInstanceArn must name the ARN of a DB instance; \
                     got {named}."
                ),
                "case {case:?}: the refusal must state what the member accepts, got {refused}"
            );
        }
    }

    // A source instance in the Region of the call itself: backups are replicated to a
    // DIFFERENT Region, and the one the instance is in already reports its `active` backup.
    let case = "a start that would replicate into the source's own Region";
    let (_, same_region_arn) =
        available_instance(&router, case, region, "emu-abr-same-region", "postgres", 1).await;
    let refused = expect_error(
        case,
        start_replication(&router, region, &same_region_arn, &[]).await,
        400,
        INVALID_PARAMETER_VALUE,
    );
    assert_eq!(
        text(&refused, "Message"),
        format!(
            "The source DB instance {same_region_arn} is located in the Region this request \
             was made to. Automated backups are replicated to a different AWS Region."
        ),
        "case {case:?}: the refusal must name the source ARN and state the rule, got {refused}"
    );

    // A source instance keeping no automated backups has none to replicate: that is what a
    // `BackupRetentionPeriod` of zero means, "to disable automated backups, set this value to
    // 0".
    let case = "a start on a source with automated backups turned off";
    let (_, no_backups_arn) =
        available_instance(&router, case, region, "emu-abr-no-backups", "mysql", 0).await;
    let refused = expect_error(
        case,
        start_replication(&router, destination, &no_backups_arn, &[]).await,
        400,
        INVALID_STATE,
    );
    assert_eq!(
        text(&refused, "Message"),
        AUTOMATED_BACKUPS_NOT_ENABLED_MESSAGE,
        "case {case:?}: the refusal must state that automated backups are off, got {refused}"
    );

    // A source instance that is not up: the one stopped below is in a settled status that is
    // not `available`, and a stopped instance's engine has written nothing since, so there is
    // no new snapshot of it for a replication to start from.
    let case = "a start on a source that is not in the available state";
    let identifier = "emu-abr-stopped";
    let (_, stopped_arn) =
        available_instance(&router, case, region, identifier, "postgres", 1).await;
    expect_ok(
        case,
        call_in(
            &router,
            region,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => identifier],
        )
        .await,
    );
    poll_until(
        case,
        &format!("{identifier} reaching stopped"),
        || describe_instance(&router, region, identifier),
        |status, body| {
            status == 200 && optional_text(body, "DBInstanceStatus").as_deref() == Some("stopped")
        },
    )
    .await;
    let refused = expect_error(
        case,
        start_replication(&router, destination, &stopped_arn, &[]).await,
        400,
        INVALID_STATE,
    );
    assert_eq!(
        text(&refused, "Message"),
        READ_REPLICA_SOURCE_NOT_AVAILABLE_MESSAGE,
        "case {case:?}: the refusal must state that the instance is not available, got \
         {refused}"
    );

    // A stop where no replication was ever enabled: there is none to stop, even though the
    // source instance itself is there.
    let case = "a stop with no replication in force";
    let (_, arn) =
        available_instance(&router, case, region, "emu-abr-never-started", "mysql", 1).await;
    let refused = expect_error(
        case,
        stop_replication(&router, destination, &arn).await,
        400,
        INVALID_STATE,
    );
    assert_eq!(
        text(&refused, "Message"),
        format!("DB Instance {arn} is not replicating to the current region."),
        "case {case:?}: the refusal must name the ARN the request carried, got {refused}"
    );
}
