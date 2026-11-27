// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal every CROSS-REGION operation of this service words when
//! the SOURCE resource its request named cannot be found.
//!
//! The rule under test: an operation whose source is named by an ARN looks that source up in
//! the Region the ARN itself carries, so its not-found refusal names BOTH the resource asked
//! for and that Region — `Requested <resource> not found in source region <region>` — rather
//! than the Region-less sentence the calls that address a resource in the Region of the call
//! answer. An ARN carrying no Region names no Region other than the one the request was
//! signed for, which is then the Region the source was looked for in.
//!
//! The operations the rule covers are the ones that look their source up BY ARN:
//! `StartDBInstanceAutomatedBackupsReplication` (the source DB instance, whose properties it
//! has to read to replicate them), and `CopyDBSnapshot` and `CopyDBClusterSnapshot` (the
//! source snapshot), which word it through the same template.
//! `StopDBInstanceAutomatedBackupsReplication` is NOT one of them: it asks only whether the
//! Region of the call holds a replication from the instance the ARN names, so it looks no
//! source instance up and answers its own state refusal for an ARN naming nothing — which is
//! what `stop_backup_replication_source_resolution` and
//! `stop_db_instance_automated_backups_replication_region` pin.
//!
//! The table below spans what the rule generalises over: the Region the source ARN points at,
//! the Region the call itself was made to, the identifier the ARN carries, and an ARN with no
//! Region segment at all. One case proves the
//! Region in the sentence is the ARN's rather than any Region the identifier does exist in: an
//! instance is created in one Region and then addressed by an ARN naming another. A separate
//! test pins the other half of the Region-less rule — such an ARN is RESOLVED in the Region of
//! the call, so an instance that does exist there is found.
//!
//! Every call goes over the wire through `db_instances::emulator`, as a real awsQuery form
//! POST carrying the SigV4 credential scope the router keys on, and the assertions read the
//! XML the emulator answers with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call_in, expect_error, expect_ok, text};

/// The account id every generated emulator presents, which the ARN forms below are built
/// with.
const ACCOUNT: &str = "000000000000";

/// The `awsQueryError` code of `DBInstanceNotFoundFault`, as the model publishes it.
const NOT_FOUND: &str = "DBInstanceNotFound";

/// The `awsQueryError` code of `DBSnapshotNotFoundFault`, which a copy answers for a source
/// snapshot it cannot find.
const SNAPSHOT_NOT_FOUND: &str = "DBSnapshotNotFound";

/// The `awsQueryError` code of `DBClusterSnapshotNotFoundFault`, which the model publishes as
/// the fault's own name.
const CLUSTER_SNAPSHOT_NOT_FOUND: &str = "DBClusterSnapshotNotFoundFault";

/// The Query API error code Amazon RDS answers a value a parameter does not accept with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The `awsQueryError` code of `InvalidDBInstanceStateFault`.
const INVALID_STATE: &str = "InvalidDBInstanceState";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// One row of the table: a source DB instance ARN that resolves to nothing, and the Region
/// the refusal must say it was looked for in.
struct SourceCase {
    case: &'static str,
    /// The Region the call is made to — the destination of the replication.
    request_region: &'static str,
    /// The `SourceDBInstanceArn` the request carries.
    source_arn: String,
    /// The identifier the sentence must name.
    identifier: &'static str,
    /// The Region the sentence must name.
    source_region: &'static str,
}

/// A DB instance ARN, as `SourceDBInstanceArn` documents one
/// (`arn:aws:rds:us-west-2:123456789012:db:mydatabase`).
fn db_instance_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:db:{identifier}")
}

/// `StartDBInstanceAutomatedBackupsReplication` in `destination`.
async fn start_replication(
    router: &MultiServiceRouter,
    destination: &str,
    source_arn: &str,
) -> (u16, String) {
    call_in(
        router,
        destination,
        "StartDBInstanceAutomatedBackupsReplication",
        &params!["SourceDBInstanceArn" => source_arn],
    )
    .await
}

/// `StopDBInstanceAutomatedBackupsReplication` in `destination`.
async fn stop_replication(
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

/// The START leg of the automated-backups-replication pair words a source DB instance it
/// cannot find with the Region-qualified sentence, naming the identifier and the Region the
/// request's own `SourceDBInstanceArn` carries.
#[tokio::test]
async fn a_missing_replication_source_is_refused_with_its_arn_region() {
    let router = emulator_with_an_instance_in("us-west-2", "emu-xr-exists-west").await;
    let table: &[SourceCase] = &[
        SourceCase {
            case: "a source in the Region the ARN names, replicated east",
            request_region: "us-east-1",
            source_arn: db_instance_arn("us-west-2", "emu-xr-never-created"),
            identifier: "emu-xr-never-created",
            source_region: "us-west-2",
        },
        SourceCase {
            case: "a source in a European Region, replicated into another",
            request_region: "eu-west-1",
            source_arn: db_instance_arn("eu-central-1", "emu-xr-eu-probe-7"),
            identifier: "emu-xr-eu-probe-7",
            source_region: "eu-central-1",
        },
        SourceCase {
            case: "a source on the other side of the world from the call",
            request_region: "us-east-2",
            source_arn: db_instance_arn("ap-southeast-2", "a"),
            identifier: "a",
            source_region: "ap-southeast-2",
        },
        SourceCase {
            case: "a long identifier carrying digits and hyphens",
            request_region: "ca-central-1",
            source_arn: db_instance_arn(
                "ap-northeast-1",
                "emu-xr-0123456789-abcdefghij-probe-x9z",
            ),
            identifier: "emu-xr-0123456789-abcdefghij-probe-x9z",
            source_region: "ap-northeast-1",
        },
        SourceCase {
            // An ARN with an EMPTY Region segment names no Region other than the one the
            // request itself was signed for, so that is the Region the source is looked
            // for in and the Region the sentence names.
            case: "a source ARN carrying no Region segment",
            request_region: "us-east-1",
            source_arn: format!("arn:aws:rds::{ACCOUNT}:db:emu-xr-region-less"),
            identifier: "emu-xr-region-less",
            source_region: "us-east-1",
        },
        SourceCase {
            // The identifier DOES exist — in us-west-2, created above — so a sentence that
            // reported where the instance is, or dropped the Region, would differ here.
            case: "an existing identifier addressed by an ARN naming another Region",
            request_region: "us-east-1",
            source_arn: db_instance_arn("eu-north-1", "emu-xr-exists-west"),
            identifier: "emu-xr-exists-west",
            source_region: "eu-north-1",
        },
    ];

    for row in table {
        let expected = format!(
            "Requested DB Instance {} not found in source region {}",
            row.identifier, row.source_region
        );
        let case = format!("StartDBInstanceAutomatedBackupsReplication: {}", row.case);
        let refused = expect_error(
            &case,
            start_replication(&router, row.request_region, &row.source_arn).await,
            404,
            NOT_FOUND,
        );
        assert_eq!(
            text(&refused, "Message"),
            expected,
            "case {case:?}: expected {expected:?}, got {refused}"
        );

        // The STOP of the same ARN looks no source instance up, so it answers for the
        // replication this Region does not hold rather than for the instance that is not
        // there: the sentence is its own and quotes the ARN whole.
        let case = format!("StopDBInstanceAutomatedBackupsReplication: {}", row.case);
        let refused = expect_error(
            &case,
            stop_replication(&router, row.request_region, &row.source_arn).await,
            400,
            INVALID_STATE,
        );
        let not_replicating = format!(
            "DB Instance {} is not replicating to the current region.",
            row.source_arn
        );
        assert_eq!(
            text(&refused, "Message"),
            not_replicating,
            "case {case:?}: expected {not_replicating:?}, got {refused}"
        );
    }
}

/// A Region-less source ARN is RESOLVED in the Region of the call, not only reported against
/// it: an instance that does exist there is found, and what the request is then refused with
/// is the rule that applies to a source in the Region of the call rather than a 404.
#[tokio::test]
async fn a_region_less_source_arn_resolves_in_the_region_of_the_call() {
    let region = "us-west-2";
    let identifier = "emu-xr-region-less-exists";
    let router = emulator_with_an_instance_in(region, identifier).await;
    let source_arn = format!("arn:aws:rds::{ACCOUNT}:db:{identifier}");

    // The start finds the instance, and backups are replicated to a DIFFERENT Region, so the
    // source has nowhere to be replicated to.
    let case = "a start whose Region-less ARN names an instance in the Region of the call";
    let refused = expect_error(
        case,
        start_replication(&router, region, &source_arn).await,
        400,
        INVALID_PARAMETER_VALUE,
    );
    let expected = format!(
        "The source DB instance {source_arn} is located in the Region this request was made \
         to. Automated backups are replicated to a different AWS Region."
    );
    assert_eq!(
        text(&refused, "Message"),
        expected,
        "case {case:?}: expected {expected:?}, got {refused}"
    );

    // The stop looks no instance up at all — it asks what the Region of the call holds — so
    // its own sentence names the ARN the request carried.
    let case = "a stop whose Region-less ARN names an instance in the Region of the call";
    let refused = expect_error(
        case,
        stop_replication(&router, region, &source_arn).await,
        400,
        INVALID_STATE,
    );
    let expected = format!("DB Instance {source_arn} is not replicating to the current region.");
    assert_eq!(
        text(&refused, "Message"),
        expected,
        "case {case:?}: expected {expected:?}, got {refused}"
    );
}

/// The copies word their own cross-Region source the same way, through the one template: the
/// snapshot asked for, carried as the request spelled it, and the Region its ARN points at.
#[tokio::test]
async fn a_missing_copy_source_snapshot_is_refused_with_its_arn_region() {
    let router = crate::db_instances::emulator();
    let request_region = "us-east-1";
    for (action, source_member, target_member, code, subject, snapshot_arn) in [
        (
            "CopyDBSnapshot",
            "SourceDBSnapshotIdentifier",
            "TargetDBSnapshotIdentifier",
            SNAPSHOT_NOT_FOUND,
            "snapshot",
            format!("arn:aws:rds:us-west-2:{ACCOUNT}:snapshot:emu-xr-missing-snapshot"),
        ),
        (
            "CopyDBClusterSnapshot",
            "SourceDBClusterSnapshotIdentifier",
            "TargetDBClusterSnapshotIdentifier",
            CLUSTER_SNAPSHOT_NOT_FOUND,
            "snapshot",
            format!("arn:aws:rds:eu-central-1:{ACCOUNT}:cluster-snapshot:emu-xr-missing-cs"),
        ),
    ] {
        let case = format!("{action} whose source ARN names another Region");
        let response = call_in(
            &router,
            request_region,
            action,
            &params![
                source_member => &snapshot_arn,
                target_member => "emu-xr-copy-target",
            ],
        )
        .await;
        let refused = expect_error(&case, response, 404, code);
        let source_region = snapshot_arn.split(':').nth(3).expect("the ARN's Region");
        let expected =
            format!("Requested {subject} {snapshot_arn} not found in source region {source_region}");
        assert_eq!(
            text(&refused, "Message"),
            expected,
            "case {case:?}: expected {expected:?}, got {refused}"
        );
    }
}

/// A router carrying this service, with one DB instance created in `region` so a row can
/// address an identifier that DOES exist by an ARN naming another Region.
async fn emulator_with_an_instance_in(region: &str, identifier: &str) -> MultiServiceRouter {
    let router = crate::db_instances::emulator();
    let case = format!("create {identifier} in {region}");
    expect_ok(
        &case,
        call_in(
            &router,
            region,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => identifier,
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => "emuadmin",
                "MasterUserPassword" => "emu-Secret-99",
                "BackupRetentionPeriod" => 1,
            ],
        )
        .await,
    );
    router
}
