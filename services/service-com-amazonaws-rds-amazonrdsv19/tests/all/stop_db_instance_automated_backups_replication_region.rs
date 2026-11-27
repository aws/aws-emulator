// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT `StopDBInstanceAutomatedBackupsReplication` resolves: the
//! replication of automated backups INTO the Region the call was made to, named by the
//! request's `SourceDBInstanceArn`.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper this
//! test target has, as a real awsQuery form POST carrying the SigV4 credential scope the router
//! keys on — each call signed for the Region it belongs to, which is what makes a source Region
//! and a destination Region two different Regions of one emulator. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: an operation that acts on a relationship a Region holds resolves
//! THAT relationship and nothing else. So a stop that names no replication into the Region of
//! the call is refused the one modelled state fault,
//! `InvalidDBInstanceStateFault`, awsQuery code `InvalidDBInstanceState`, HTTP 400, worded
//! from the ARN the request carried, whether the replication was never enabled, has already
//! been stopped, runs into another Region, or the ARN names an instance no Region holds at
//! all — while the START leg, which has to read the source instance's own properties before
//! it can replicate them, answers the modelled `DBInstanceNotFoundFault` (awsQuery code
//! `DBInstanceNotFound`, HTTP 404) for that last ARN: the two legs resolve different things,
//! which is why one ARN earns two answers.
//!
//! The table below spans what the rule generalises over: an instance that exists and never
//! replicated, a replication already stopped, a replication in force into ANOTHER Region, the
//! Region segment of the ARN, and the engine family of the source. The positive rows show the
//! same resolution answering a 200 — including for an ARN whose identifier is cased
//! differently from the stored one — so the refusal is not a blanket one.
//!
//! Nothing here waits out a span: every state a transition this service owes leads to is
//! reached by polling a REPORTING call through `lifecycle_poll_loops::poll_until`.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_automated_backup_replication::{
    available_instance, describe_backups_of, start_replication, stop_replication, the_backup,
};
use crate::db_instances::{emulator, expect_error, expect_ok, optional_text, text};
use crate::lifecycle_poll_loops::poll_until;

/// The account id every generated emulator presents, which the ARN forms below are read
/// against.
const ACCOUNT: &str = "000000000000";

/// The `awsQueryError` code of `InvalidDBInstanceStateFault`, as the model publishes it — the
/// code this operation answers when the Region of the call holds no replication to stop.
const INVALID_STATE: &str = "InvalidDBInstanceState";

/// The `awsQueryError` code of `DBInstanceNotFoundFault`, which the START leg answers for a
/// source instance it cannot resolve.
const NOT_FOUND: &str = "DBInstanceNotFound";

/// The status a replicated automated backup reports once it has its first snapshot.
const ACTIVE: &str = "active";

/// The status it reports "after backup replication is stopped".
const RETAINED: &str = "retained";

/// The sentence AWS refuses a stop that names no replication into the Region of the call with:
/// fixed text either side of the one variable part, the request's `SourceDBInstanceArn`,
/// reproduced whole.
fn not_replicating_here(source_arn: &str) -> String {
    format!("DB Instance {source_arn} is not replicating to the current region.")
}

/// A DB instance ARN of this service's own form for `identifier` in `region`.
fn db_instance_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:db:{identifier}")
}

/// Start a replication of `source_arn`'s backups into `destination` and poll them to `active`,
/// so the stop under test acts on a replication that is fully in force.
async fn replication_in_force(
    router: &MultiServiceRouter,
    case: &str,
    destination: &str,
    source_arn: &str,
    resource_id: &str,
) {
    expect_ok(
        case,
        start_replication(router, destination, source_arn, &[]).await,
    );
    poll_until(
        case,
        "the replicated backups reaching active",
        || describe_backups_of(router, destination, resource_id),
        |status, body| status == 200 && optional_text(body, "Status").as_deref() == Some(ACTIVE),
    )
    .await;
}

/// RULE: a stop whose source instance IS resolved and that names no replication into the
/// Region of the call is refused HTTP 400 `InvalidDBInstanceState`, worded from the ARN the
/// request carried — however the Region of the call came to hold no such replication.
#[tokio::test]
async fn a_stop_with_no_replication_into_this_region_is_refused_invalid_state() {
    let router = emulator();
    let destination = "us-east-1";

    // Row 1: an existing instance whose automated backups were never replicated anywhere.
    let case = "an instance that never replicated";
    let (_, never_started) = available_instance(
        &router,
        case,
        "us-west-2",
        "emu-stop-abr-never",
        "postgres",
        3,
    )
    .await;

    // Row 2: an instance whose replication into this Region was started and then STOPPED, so a
    // repeat stop finds the backups retained rather than replicating.
    let case = "a replication already stopped";
    let (stopped_id, already_stopped) =
        available_instance(&router, case, "us-west-2", "emu-stop-abr-twice", "mysql", 2).await;
    replication_in_force(&router, case, destination, &already_stopped, &stopped_id).await;
    expect_ok(
        case,
        stop_replication(&router, destination, &already_stopped).await,
    );

    // Row 3: an instance replicating into a DIFFERENT destination Region, the stop issued in
    // this one — the question is about the Region of the call.
    let case = "a replication in force into another Region";
    let (other_id, other_destination) = available_instance(
        &router,
        case,
        "us-west-2",
        "emu-stop-abr-elsewhere",
        "oracle-ee",
        1,
    )
    .await;
    replication_in_force(&router, case, "sa-east-1", &other_destination, &other_id).await;

    let table: &[(&str, &str)] = &[
        (
            "an instance that exists and never replicated",
            &never_started,
        ),
        ("a replication already stopped", &already_stopped),
        (
            "a replication in force into another Region",
            &other_destination,
        ),
    ];
    for (case, source_arn) in table {
        let refused = expect_error(
            case,
            stop_replication(&router, destination, source_arn).await,
            400,
            INVALID_STATE,
        );
        assert_eq!(
            text(&refused, "Message"),
            not_replicating_here(source_arn),
            "case {case:?}: the refusal must name the ARN the request carried, got {refused}"
        );
    }

    // The replication row 3 set up elsewhere is untouched by the refusals above: the Region
    // that holds it can still stop it.
    let case = "the replication in another Region after the refusals";
    let stopped = expect_ok(
        case,
        stop_replication(&router, "sa-east-1", &other_destination).await,
    );
    assert_eq!(
        text(&the_backup(case, &stopped), "Status"),
        RETAINED,
        "case {case:?}: the Region holding the replication stops it, got {stopped}"
    );
}

/// RULE: the two legs of backup replication resolve DIFFERENT things, so one and the same ARN
/// naming an instance that does not exist earns two answers: the start, which has to read that
/// instance, is refused 404 `DBInstanceNotFound` with the Region-qualified sentence, and the
/// stop, which asks only what the Region of the call holds, is refused 400
/// `InvalidDBInstanceState` with its own sentence over the ARN the request carried.
#[tokio::test]
async fn the_start_and_the_stop_legs_answer_one_unresolvable_arn_for_what_each_resolves() {
    let router = emulator();
    let destination = "us-east-1";
    // The rows differ in the Region the ARN points at and in the identifier it carries, which
    // is everything an unresolvable ARN varies in.
    let table: &[(&str, &str, &str)] = &[
        ("a source Region west of the call", "us-west-2", "emu-stop-abr-two-legs"),
        ("a source Region in another partition's geography", "ap-southeast-2", "emu-stop-abr-2l-2"),
        ("a one-label identifier", "eu-central-1", "db"),
    ];
    for (case, source_region, identifier) in table {
        let arn = db_instance_arn(source_region, identifier);
        let expected =
            format!("Requested DB Instance {identifier} not found in source region {source_region}");
        let refused_start = expect_error(
            case,
            start_replication(&router, destination, &arn, &[]).await,
            404,
            NOT_FOUND,
        );
        assert_eq!(
            text(&refused_start, "Message"),
            expected,
            "case {case:?}: the start resolves the source instance, got {refused_start}"
        );
        let refused_stop = expect_error(
            case,
            stop_replication(&router, destination, &arn).await,
            400,
            INVALID_STATE,
        );
        assert_eq!(
            text(&refused_stop, "Message"),
            not_replicating_here(&arn),
            "case {case:?}: the stop answers for the replication this Region holds none of, \
             got {refused_stop}"
        );
    }
}

/// RULE: the same resolution ANSWERS a stop whose ARN names a replication into the Region of
/// the call — read off the ARN, so an identifier or a Region segment cased differently from the
/// stored one names it too ("this parameter isn't case-sensitive" is how this service resolves
/// every name) — and the backups it retains are reported from then on.
#[tokio::test]
async fn a_replication_in_force_is_stopped_by_the_arn_that_names_it() {
    let router = emulator();
    let destination = "eu-west-2";
    let table: &[(&str, &str, &str, &str)] = &[
        (
            "the ARN as the instance reports it",
            "emu-stop-abr-asis",
            "postgres",
            "as-is",
        ),
        (
            "the same ARN with its identifier upper-cased",
            "emu-stop-abr-cased",
            "mariadb",
            "upper",
        ),
        (
            "the same ARN with its Region segment upper-cased",
            "emu-stop-abr-region-cased",
            "mysql",
            "upper-region",
        ),
    ];
    for (case, identifier, engine, casing) in table {
        let (resource_id, reported_arn) =
            available_instance(&router, case, "us-west-2", identifier, engine, 1).await;
        replication_in_force(&router, case, destination, &reported_arn, &resource_id).await;
        let named = match *casing {
            "upper" => db_instance_arn("us-west-2", &identifier.to_ascii_uppercase()),
            "upper-region" => db_instance_arn("US-WEST-2", identifier),
            _ => reported_arn.clone(),
        };

        let stopped = expect_ok(case, stop_replication(&router, destination, &named).await);
        assert_eq!(
            text(&the_backup(case, &stopped), "Status"),
            RETAINED,
            "case {case:?}: the stop retains the backups it had taken, got {stopped}"
        );
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
        // And the replication is gone: a second stop, by either spelling, has none to stop.
        for named in [named, reported_arn] {
            let refused = expect_error(
                case,
                stop_replication(&router, destination, &named).await,
                400,
                INVALID_STATE,
            );
            assert_eq!(
                text(&refused, "Message"),
                not_replicating_here(&named),
                "case {case:?}: a stop after the stop names no replication, got {refused}"
            );
        }
    }
}
