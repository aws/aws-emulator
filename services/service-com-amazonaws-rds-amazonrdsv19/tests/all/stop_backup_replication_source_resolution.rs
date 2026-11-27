// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SOURCE `StopDBInstanceAutomatedBackupsReplication` does NOT
//! resolve: whatever instance the request's `SourceDBInstanceArn` names, the question is
//! asked of the Region the call was made to.
//!
//! The rule under test: a stop asks one question of that Region — is the instance the ARN
//! names replicating its automated backups INTO this Region? — so the answer is found in that
//! Region's own records and no source DB instance is looked up anywhere. An ARN naming an
//! instance no Region holds is therefore answered exactly as an idle one is: a Region holds no
//! replication from an instance that does not exist, which is the modelled
//! `InvalidDBInstanceStateFault` (awsQuery code `InvalidDBInstanceState`, HTTP 400), worded
//! `DB Instance <SourceDBInstanceArn> is not replicating to the current region.` — fixed text
//! whose one variable part is the request's ARN, reproduced whole. The leg that has to READ
//! the source instance's own properties to replicate them, the START, is the one that answers
//! `DBInstanceNotFoundFault` for such an ARN (`cross_region_source_not_found_sentences` pins
//! that sentence, and `stop_db_instance_automated_backups_replication_region` drives the two
//! legs over one ARN).
//!
//! The rows span what the rule generalises over: the Region the ARN points at against the
//! Region of the call, an identifier that exists in ANOTHER Region than the ARN names (so no
//! Region's instance list is consulted), an ARN with no Region segment at all, the casing of
//! the ARN's Region, the identifier's own shape, and an instance that exists and replicates
//! nothing. The controls on the other side of the rule keep it honest: a replication that IS
//! in force here is SERVED and retains the backups it had taken, and a value that is no DB
//! instance ARN at all is still the parameter refusal it always was.
//!
//! Every call goes over the wire through `db_instances::emulator`, as a real awsQuery form
//! POST carrying the SigV4 credential scope the router keys on, and the assertions read the
//! XML the emulator answers with.
//!
//! Nothing here waits out a span: the one state a transition this service owes leads to is
//! reached by polling a REPORTING call through `lifecycle_poll_loops::poll_until`.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_automated_backup_replication::{
    available_instance, describe_backups_of, start_replication, stop_replication, the_backup,
};
use crate::db_instances::{emulator, expect_error, expect_ok, optional_text, text};
use crate::lifecycle_poll_loops::poll_until;

/// The account id every generated emulator presents, which the ARN forms below are built with.
const ACCOUNT: &str = "000000000000";

/// The `awsQueryError` code of `InvalidDBInstanceStateFault`, the one refusal a stop that has
/// nothing to stop answers.
const INVALID_STATE: &str = "InvalidDBInstanceState";

/// The Query API error code Amazon RDS answers a value a parameter does not accept with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The status a replicated automated backup reports once it has its first snapshot.
const ACTIVE: &str = "active";

/// The status it reports "after backup replication is stopped".
const RETAINED: &str = "retained";

/// A DB instance ARN of this service's own form, as `SourceDBInstanceArn` documents one.
fn db_instance_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:db:{identifier}")
}

/// The sentence a stop that has nothing to stop is refused with: fixed text around the ARN
/// the request carried.
fn not_replicating(source_arn: &str) -> String {
    format!("DB Instance {source_arn} is not replicating to the current region.")
}

/// One row: the `SourceDBInstanceArn` a stop carries, and the Region it is stopped from.
struct NothingToStop {
    case: &'static str,
    /// The Region the stop is made to — the destination whose replication it would stop.
    request_region: &'static str,
    source_arn: String,
}

/// RULE: every ARN the Region of the call holds no replication from is refused the state
/// fault, with the sentence over the ARN the request carried — whatever that ARN names and
/// whichever Region it points at.
#[tokio::test]
async fn a_stop_this_region_holds_no_replication_for_is_refused_the_state_fault() {
    let router = emulator();

    // Two instances that DO exist: one addressed below by an ARN naming another Region, and
    // one that replicates nothing anywhere.
    let case = "the instance that exists";
    let (_, exists_west) =
        available_instance(&router, case, "us-west-2", "emu-stop-src-exists", "postgres", 1).await;
    assert_eq!(
        exists_west,
        db_instance_arn("us-west-2", "emu-stop-src-exists"),
        "case {case:?}: the instance must report the ARN form the rows are built with, got \
         {exists_west}"
    );
    let case = "a source that exists and replicates nothing";
    let (_, idle) =
        available_instance(&router, case, "us-west-2", "emu-stop-src-idle", "mysql", 2).await;

    let table: &[NothingToStop] = &[
        NothingToStop {
            case: "a source west of the call that was never created",
            request_region: "us-east-1",
            source_arn: db_instance_arn("us-west-2", "emu-stop-src-ghost"),
        },
        NothingToStop {
            case: "the same identifier, in another source Region",
            request_region: "us-east-1",
            source_arn: db_instance_arn("eu-central-1", "emu-stop-src-ghost"),
        },
        NothingToStop {
            // The identifier DOES exist — in us-west-2 — so the stop is answered for the
            // replication and not for where an instance happens to be.
            case: "an existing identifier addressed by an ARN naming another Region",
            request_region: "us-east-1",
            source_arn: db_instance_arn("ap-southeast-2", "emu-stop-src-exists"),
        },
        NothingToStop {
            // An ARN with an EMPTY Region segment names no Region other than the one the
            // request was signed for.
            case: "a source ARN carrying no Region segment",
            request_region: "ca-central-1",
            source_arn: format!("arn:aws:rds::{ACCOUNT}:db:emu-stop-src-region-less"),
        },
        NothingToStop {
            case: "a source ARN whose Region segment is upper-cased",
            request_region: "us-east-1",
            source_arn: db_instance_arn("EU-WEST-1", "emu-stop-src-cased"),
        },
        NothingToStop {
            case: "a one-label identifier",
            request_region: "sa-east-1",
            source_arn: db_instance_arn("us-east-2", "db"),
        },
        NothingToStop {
            case: "a source that exists and replicates nothing anywhere",
            request_region: "us-east-1",
            source_arn: idle.clone(),
        },
    ];

    for row in table {
        let expected = not_replicating(&row.source_arn);
        let refused = expect_error(
            row.case,
            stop_replication(&router, row.request_region, &row.source_arn).await,
            400,
            INVALID_STATE,
        );
        assert_eq!(
            text(&refused, "Message"),
            expected,
            "case {:?}: expected {expected:?}, got {refused}",
            row.case
        );
    }

    // The control on the other side of the rule: a replication into the Region of the call IS
    // stopped there, and the backups it had taken are retained — so the refusals above are the
    // question this Region answers and not a blanket refusal.
    let case = "a source replicating into the Region of the call";
    let (resource_id, replicating) = available_instance(
        &router,
        case,
        "eu-west-1",
        "emu-stop-src-replicating",
        "postgres",
        3,
    )
    .await;
    expect_ok(
        case,
        start_replication(&router, "us-east-1", &replicating, &[]).await,
    );
    poll_until(
        case,
        "the replicated backups reaching active",
        || describe_backups_of(&router, "us-east-1", &resource_id),
        |status, body| status == 200 && optional_text(body, "Status").as_deref() == Some(ACTIVE),
    )
    .await;
    let served = expect_ok(
        case,
        stop_replication(&router, "us-east-1", &replicating).await,
    );
    assert_eq!(
        text(&the_backup(case, &served), "Status"),
        RETAINED,
        "case {case:?}: the stop retains the backups it had taken, got {served}"
    );
}

/// RULE (the other side of the member's own reading): a value that is not the ARN of a DB
/// instance states no source at all, so it is still the parameter refusal — the state fault is
/// the answer for an ARN this operation COULD read and found no replication for, not for one
/// it could not read.
#[tokio::test]
async fn a_stop_whose_arn_names_no_db_instance_is_refused_for_the_parameter() {
    let router: MultiServiceRouter = emulator();
    for named in [
        "emu-stop-bare-identifier",
        &format!("arn:aws:rds:us-west-2:{ACCOUNT}:cluster:emu-stop-cluster"),
        &format!("arn:aws:ec2:us-west-2:{ACCOUNT}:instance:i-0123456789abcdef0"),
    ] {
        let case = format!("a stop whose SourceDBInstanceArn is {named}");
        let refused = expect_error(
            &case,
            stop_replication(&router, "us-east-1", named).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        assert_eq!(
            text(&refused, "Message"),
            format!(
                "The parameter SourceDBInstanceArn must name the ARN of a DB instance; got \
                 {named}."
            ),
            "case {case:?}: the refusal must state what the member accepts, got {refused}"
        );
    }
}
