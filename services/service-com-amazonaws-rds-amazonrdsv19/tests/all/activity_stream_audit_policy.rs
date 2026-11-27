// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyActivityStream`: the audit policy state of a DB instance's
//! database activity stream, and the `DBInstance$ActivityStreamPolicyStatus` the instance's
//! own describe reports it as.
//!
//! The rule under test: `ModifyActivityStream` writes the audit policy state the request
//! names (`locked` when it names none) on the DB instance its `ResourceArn` ARN resolves to
//! in the request's Region, reports that policy's status beside the stream's own status, and
//! leaves every later read of the instance reporting what the last accepted call wrote — a
//! change of state reported as the transitional `ActivityStreamPolicyStatus` member until
//! this service's clock settles it, and a call asking for the state the policy is already in
//! or already moving to taken as the no-op it is. A `ResourceArn` that names no DB instance
//! of this account in the request's Region is `DBInstanceNotFoundFault` (HTTP 404), an
//! instance whose engine has no database activity stream or that is not in a state a
//! modification can be taken in is `InvalidDBInstanceStateFault` (HTTP 400), and a value
//! that is no Amazon RDS DB instance ARN of this Region at all is refused on the member that
//! carried it —
//! `smithy_query(query="operation", operation="ModifyActivityStream")` for the faults and
//! their statuses, and `smithy_query(query="shape", shape="ActivityStreamPolicyStatus")` for
//! the four states a policy is reported in.
//!
//! The tables span what the rule generalises over: both engine families the operation is
//! published for and several of their editions, an engine named in mixed case, each direction
//! of the change, the same state asked for twice (the state is a setting, not a step), a
//! request that names no `AuditPolicyState` at all (the documented default), the engine
//! families with no activity stream (an RDS Custom edition of a served family among them),
//! the statuses that make an instance unmodifiable, and the forms a `ResourceArn` can take
//! that resolve to nothing (another account, another partition, another resource type, no ARN
//! at all, and no value at all), and a `ResourceArn` naming another REGION, which is refused
//! on the member rather than reported absent — the Region pairs and resource kinds that rule
//! covers are the table in `activity_stream_resource_arn_region`.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with, and every wait is the poll loop a client
//! writes (`lifecycle_poll_loops::poll_until`) rather than a sleep.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    ACTIVITY_STREAM_ENGINE_MESSAGE, DB_INSTANCE_STATUS_AVAILABLE, DB_INSTANCE_STATUS_CREATING,
    DB_INSTANCE_STATUS_STOPPED, MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, REGION,
};
use crate::lifecycle_poll_loops::poll_until;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second Region, so the REGIONAL scope of the instance a `ResourceArn` resolves to is
/// observable.
const OTHER_REGION: &str = "eu-west-1";

// The members of `ActivityStreamPolicyStatus` (`smithy_query(query="shape",
// shape="ActivityStreamPolicyStatus")`: locked | unlocked | locking-policy |
// unlocking-policy), written out here rather than taken from the code under test. The first
// two are also the members of `AuditPolicyState`, which is what a request asks for.
/// The audit policy is read-only, which is the state `ModifyActivityStream` documents as its
/// default.
const LOCKED: &str = "locked";
/// The audit policy is read/write.
const UNLOCKED: &str = "unlocked";
/// The policy is on its way to [`LOCKED`].
const LOCKING: &str = "locking-policy";
/// The policy is on its way to [`UNLOCKED`].
const UNLOCKING: &str = "unlocking-policy";

/// The `stopped` member of `ActivityStreamStatus` (`smithy_query(query="shape",
/// shape="ActivityStreamStatus")`: started | starting | stopped | stopping) — the status of
/// the activity stream of an instance no stream was started for.
const STREAM_STOPPED: &str = "stopped";

/// The `arn:<partition>:rds:<region>:<account>:db:<identifier>` form Amazon RDS gives a DB
/// instance ("Constructing an ARN for Amazon RDS"), written out here rather than taken from
/// the code under test.
fn instance_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:db:{identifier}")
}

/// One instance the modification is driven against.
struct EngineCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    class: &'static str,
}

/// The engines the operation is published for — "This operation is supported for RDS for
/// Oracle and Microsoft SQL Server" — across editions of each family and two instance
/// classes.
const AUDIT_POLICY_ENGINES: &[EngineCase] = &[
    EngineCase {
        case: "an oracle-se2 instance",
        identifier: "emu-das-oracle-se2",
        engine: "oracle-se2",
        class: "db.m5.large",
    },
    EngineCase {
        case: "an oracle-ee instance",
        identifier: "emu-das-oracle-ee",
        engine: "oracle-ee",
        class: "db.t3.medium",
    },
    EngineCase {
        case: "a sqlserver-ex instance",
        identifier: "emu-das-sqlserver-ex",
        engine: "sqlserver-ex",
        class: "db.t3.medium",
    },
    EngineCase {
        case: "a sqlserver-ee instance named in mixed case",
        identifier: "EMU-DAS-SQLServer-EE",
        engine: "sqlserver-ee",
        class: "db.m5.large",
    },
    EngineCase {
        // `Engine` is accepted case-insensitively and stored as the request spelled it, so
        // the engine-family rule has to hold for a create that named the engine in mixed
        // case too.
        case: "an instance created with its engine named in mixed case",
        identifier: "emu-das-oracle-mixed-engine",
        engine: "Oracle-SE2",
        class: "db.m5.large",
    },
];

/// The engines with no database activity stream for this operation to modify.
const ENGINES_WITHOUT_AN_AUDIT_POLICY: &[EngineCase] = &[
    EngineCase {
        case: "a mysql instance",
        identifier: "emu-das-mysql",
        engine: "mysql",
        class: "db.t3.small",
    },
    EngineCase {
        case: "a postgres instance",
        identifier: "emu-das-postgres",
        engine: "postgres",
        class: "db.m5.large",
    },
    EngineCase {
        case: "a mariadb instance",
        identifier: "emu-das-mariadb",
        engine: "mariadb",
        class: "db.t3.medium",
    },
    EngineCase {
        // An RDS CUSTOM engine of a family the operation otherwise serves: RDS Custom does
        // not support database activity streams, so an instance of one has no audit policy
        // either.
        case: "a custom-sqlserver-ee instance",
        identifier: "emu-das-custom-sqlserver",
        engine: "custom-sqlserver-ee",
        class: "db.m5.large",
    },
];

/// `CreateDBInstance` with the members every row of this suite carries.
async fn create_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    engine: &str,
    class: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", identifier),
            ("Engine", engine),
            ("DBInstanceClass", class),
            ("AllocatedStorage", "20"),
            ("MasterUsername", "emuadmin"),
        ],
    )
    .await
}

/// `ModifyActivityStream` for `arn`, asking for `state` where the row names one.
async fn modify(
    router: &MultiServiceRouter,
    arn: &str,
    state: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<(&str, &str)> = vec![("ResourceArn", arn)];
    if let Some(state) = state {
        params.push(("AuditPolicyState", state));
    }
    call(router, "ModifyActivityStream", &params).await
}

/// The one `<DBInstance>` subtree `DescribeDBInstances` reports for `identifier`.
async fn describe_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    let found = subtrees(&body, "DBInstance");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBInstance for {identifier}, got {} in {body}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// What the instance's own describe reports for its activity stream's audit policy, which is
/// absent for an instance no modification has set one on.
async fn reported_policy(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> Option<String> {
    let reported = describe_instance(router, case, identifier).await;
    optional_text(&reported, "ActivityStreamPolicyStatus")
}

/// Poll the instance's own describe until it reports `expected` for the policy, the way a
/// client waits for a state this service's clock owes.
async fn poll_until_policy(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    expected: &str,
) {
    poll_until(
        case,
        &format!("the audit policy of {identifier} reaching {expected}"),
        || async {
            call(
                router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await
        },
        |status, body| {
            status == 200
                && subtrees(body, "DBInstance")
                    .first()
                    .and_then(|instance| optional_text(instance, "ActivityStreamPolicyStatus"))
                    .is_some_and(|reported| reported == expected)
        },
    )
    .await;
}

/// One accepted modification: assert the response reports `expected_policy` and the stream's
/// own status, and that the configuration of a stream nothing started is absent from it.
async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    asked_for: Option<&str>,
    expected_policy: &str,
) {
    let body = expect_ok(case, modify(router, arn, asked_for).await);
    let policy = text(&body, "PolicyStatus");
    assert_eq!(
        policy, expected_policy,
        "case {case:?}: expected ModifyActivityStream asking for {asked_for:?} to report \
         PolicyStatus {expected_policy:?}, got {policy:?} in {body}"
    );
    let status = text(&body, "Status");
    assert_eq!(
        status, STREAM_STOPPED,
        "case {case:?}: expected the stream itself to stay {STREAM_STOPPED:?}, got \
         {status:?} in {body}"
    );
    for absent in ["Mode", "KmsKeyId", "KinesisStreamName"] {
        let reported = optional_text(&body, absent);
        assert!(
            reported.is_none(),
            "case {case:?}: no activity stream was started, so {absent} must be absent, got \
             {reported:?} in {body}"
        );
    }
}

#[tokio::test]
async fn each_published_engine_locks_and_unlocks_its_audit_policy_and_the_instance_reads_it_back()
{
    let router = emulator();

    for row in AUDIT_POLICY_ENGINES {
        expect_ok(
            row.case,
            create_instance(&router, REGION, row.identifier, row.engine, row.class).await,
        );
    }
    for row in AUDIT_POLICY_ENGINES {
        poll_until(
            row.case,
            &format!("{} reaching {DB_INSTANCE_STATUS_AVAILABLE}", row.identifier),
            || async {
                call(
                    &router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", row.identifier)],
                )
                .await
            },
            |status, body| {
                status == 200 && text(body, "DBInstanceStatus") == DB_INSTANCE_STATUS_AVAILABLE
            },
        )
        .await;
    }

    for (index, row) in AUDIT_POLICY_ENGINES.iter().enumerate() {
        let case = row.case;
        // "This parameter is stored as a lowercase string", so that is the identifier the
        // instance's own ARN carries.
        let stored = row.identifier.to_ascii_lowercase();
        let arn = instance_arn(REGION, &stored);

        // An instance no modification has reached reports the stream stopped and no policy
        // status at all: there is no policy state to report until a call sets one.
        let created = describe_instance(&router, case, &stored).await;
        let stream = text(&created, "ActivityStreamStatus");
        assert_eq!(
            stream, STREAM_STOPPED,
            "case {case:?}: expected the created instance to report ActivityStreamStatus \
             {STREAM_STOPPED:?}, got {stream:?}"
        );
        let before = optional_text(&created, "ActivityStreamPolicyStatus");
        assert!(
            before.is_none(),
            "case {case:?}: expected the created instance to report no \
             ActivityStreamPolicyStatus, got {before:?}"
        );

        // Unlocking is a change, so it is reported as the transition it is and settles on
        // the state that was asked for.
        accepted(&router, case, &arn, Some(UNLOCKED), UNLOCKING).await;
        poll_until_policy(&router, case, &stored, UNLOCKED).await;

        // The same state asked for again is no transition at all: the policy state is a
        // setting, so the call is taken and reports where the policy stands.
        accepted(&router, case, &arn, Some(UNLOCKED), UNLOCKED).await;
        let still = reported_policy(&router, case, &stored).await;
        assert_eq!(
            still.as_deref(),
            Some(UNLOCKED),
            "case {case:?}: expected the repeated call to leave the policy {UNLOCKED:?}, got \
             {still:?}"
        );

        // And the same rule in the other direction.
        accepted(&router, case, &arn, Some(LOCKED), LOCKING).await;
        poll_until_policy(&router, case, &stored, LOCKED).await;
        accepted(&router, case, &arn, Some(LOCKED), LOCKED).await;

        // A call that names no `AuditPolicyState` asks for the documented default
        // ("to either locked (default) or unlocked"): it is a no-op on a locked policy and
        // locks an unlocked one.
        accepted(&router, case, &arn, None, LOCKED).await;
        accepted(&router, case, &arn, Some(UNLOCKED), UNLOCKING).await;
        poll_until_policy(&router, case, &stored, UNLOCKED).await;
        accepted(&router, case, &arn, None, LOCKING).await;
        poll_until_policy(&router, case, &stored, LOCKED).await;

        // The write reached the instance the ARN named and no other: every row driven
        // before this one still reports the state its own last call left.
        for earlier in &AUDIT_POLICY_ENGINES[..index] {
            let untouched =
                reported_policy(&router, earlier.case, &earlier.identifier.to_ascii_lowercase())
                    .await;
            assert_eq!(
                untouched.as_deref(),
                Some(LOCKED),
                "case {case:?}: expected the modification to leave {} alone, which its own \
                 last call left {LOCKED:?}, got {untouched:?}",
                earlier.identifier
            );
        }
    }
}

#[tokio::test]
async fn a_resource_arn_that_names_no_db_instance_in_the_region_is_not_found() {
    let router = emulator();

    // One instance in each Region, so a miss is a miss because of what the ARN names rather
    // than because the store is empty.
    expect_ok(
        "setup",
        create_instance(&router, REGION, "emu-das-here", "oracle-se2", "db.m5.large").await,
    );
    expect_ok(
        "setup",
        create_instance(
            &router,
            OTHER_REGION,
            "emu-das-elsewhere",
            "oracle-se2",
            "db.m5.large",
        )
        .await,
    );
    poll_until(
        "setup",
        "emu-das-here reaching available",
        || async {
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "emu-das-here")],
            )
            .await
        },
        |status, body| {
            status == 200 && text(body, "DBInstanceStatus") == DB_INSTANCE_STATUS_AVAILABLE
        },
    )
    .await;

    // (case, the `ResourceArn` the refused call carries, the DB instance that ARN names)
    //
    // Every row names the Region the request is signed for: an ARN naming ANOTHER Region is
    // not a missing instance but a value this endpoint could not act on, which the loop after
    // this one asserts.
    let misses = [
        (
            "an instance that was never created",
            instance_arn(REGION, "emu-das-absent"),
            "emu-das-absent",
        ),
        (
            "an existing instance's ARN under another account",
            format!("arn:aws:rds:{REGION}:210987654321:db:emu-das-here"),
            "emu-das-here",
        ),
        (
            "an instance ARN in another partition",
            format!("arn:aws-us-gov:rds:{REGION}:{ACCOUNT}:db:emu-das-here"),
            "emu-das-here",
        ),
    ];

    for (case, arn, named) in &misses {
        for asked_for in [None, Some(LOCKED), Some(UNLOCKED)] {
            let refused = expect_error(
                case,
                modify(&router, arn, asked_for).await,
                404,
                "DBInstanceNotFound",
            );
            let message = text(&refused, "Message");
            let expected = format!("DBInstance not found: {named}");
            assert_eq!(
                message, expected,
                "case {case:?} asking for {asked_for:?}: expected the wording {expected:?}, \
                 got {message:?}"
            );
        }
    }

    // The same identifiers under ANOTHER Region are not misses at all: Amazon RDS serves one
    // Region per endpoint, so an ARN whose Region is not the endpoint's names nothing this
    // call could act on and is refused on the member, before the lookup — the pairs of
    // Regions and resource kinds that rule covers are the table in
    // `activity_stream_resource_arn_region`.
    for (case, arn) in [
        (
            "an existing instance's ARN with another Region in it",
            instance_arn(OTHER_REGION, "emu-das-here"),
        ),
        (
            "an instance that exists only in another Region",
            instance_arn(OTHER_REGION, "emu-das-elsewhere"),
        ),
    ] {
        for asked_for in [None, Some(LOCKED), Some(UNLOCKED)] {
            let refused = expect_error(
                case,
                modify(&router, &arn, asked_for).await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&refused, "Message");
            // The sentence Amazon RDS refuses an ARN parameter of another Region with: its
            // fixed text, with the ARN as submitted, the member, the Region the endpoint
            // serves and the Region in the ARN as the variable parts.
            let expected = format!(
                "The provided ARN ({arn}) is invalid for this parameter (ResourceArn). \
                 Expected region = {REGION}, actual region = {OTHER_REGION}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} asking for {asked_for:?}: expected the wording {expected:?}, \
                 got {message:?}"
            );
        }
    }

    // The instance that IS in the request's Region still serves the call, so the refusals
    // above are about the ARN and not about the operation.
    accepted(
        &router,
        "the instance in the request's Region",
        &instance_arn(REGION, "emu-das-here"),
        Some(UNLOCKED),
        UNLOCKING,
    )
    .await;
}

#[tokio::test]
async fn a_value_that_is_no_db_instance_arn_is_refused_on_the_member() {
    let router = emulator();
    expect_ok(
        "setup",
        create_instance(&router, REGION, "emu-das-member", "oracle-se2", "db.m5.large").await,
    );
    poll_until(
        "setup",
        "emu-das-member reaching available",
        || async {
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "emu-das-member")],
            )
            .await
        },
        |status, body| {
            status == 200 && text(body, "DBInstanceStatus") == DB_INSTANCE_STATUS_AVAILABLE
        },
    )
    .await;

    // Every form that carries no Amazon RDS DB instance ARN: the bare identifier, an ARN of
    // another RDS resource type, an ARN of another service, and a value that is no ARN.
    let unusable = [
        "emu-das-member".to_string(),
        instance_arn(REGION, "emu-das-member").replace(":db:", ":cluster:"),
        instance_arn(REGION, "emu-das-member").replace(":db:", ":snapshot:"),
        format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance:i-0123456789abcdef0"),
        "not-an-arn-at-all".to_string(),
    ];
    for value in &unusable {
        let refused = expect_error(
            value,
            modify(&router, value, Some(UNLOCKED)).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&refused, "Message");
        let expected = format!("Invalid resource name: {value}");
        assert_eq!(
            message, expected,
            "case {value:?}: expected the wording {expected:?}, got {message:?}"
        );
    }

    // And a call that names no instance at all: the member absent, and the member present
    // but empty.
    let no_instance = [
        ("ResourceArn absent", vec![("AuditPolicyState", UNLOCKED)]),
        (
            "ResourceArn empty",
            vec![("ResourceArn", ""), ("AuditPolicyState", UNLOCKED)],
        ),
    ];
    for (case, params) in &no_instance {
        let refused = expect_error(
            case,
            call(&router, "ModifyActivityStream", params).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&refused, "Message");
        let expected = "The parameter ResourceArn must be provided and must not be blank.";
        assert_eq!(
            message, expected,
            "case {case:?}: expected the wording {expected:?}, got {message:?}"
        );
    }

    // None of the refused calls wrote anything: the instance reports no policy status.
    let reported = reported_policy(&router, "the named instance", "emu-das-member").await;
    assert!(
        reported.is_none(),
        "expected the refused calls to leave the instance without an \
         ActivityStreamPolicyStatus, got {reported:?}"
    );
}

#[tokio::test]
async fn an_engine_with_no_database_activity_stream_is_refused_on_its_engine() {
    let router = emulator();

    for row in ENGINES_WITHOUT_AN_AUDIT_POLICY {
        expect_ok(
            row.case,
            create_instance(&router, REGION, row.identifier, row.engine, row.class).await,
        );
    }
    for row in ENGINES_WITHOUT_AN_AUDIT_POLICY {
        poll_until(
            row.case,
            &format!("{} reaching {DB_INSTANCE_STATUS_AVAILABLE}", row.identifier),
            || async {
                call(
                    &router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", row.identifier)],
                )
                .await
            },
            |status, body| {
                status == 200 && text(body, "DBInstanceStatus") == DB_INSTANCE_STATUS_AVAILABLE
            },
        )
        .await;
    }

    for row in ENGINES_WITHOUT_AN_AUDIT_POLICY {
        let case = row.case;
        let arn = instance_arn(REGION, row.identifier);
        // The instance really is available, so the refusal below is its ENGINE answering
        // rather than a status the call would have to wait out.
        let status = text(
            &describe_instance(&router, case, row.identifier).await,
            "DBInstanceStatus",
        );
        assert_eq!(
            status, DB_INSTANCE_STATUS_AVAILABLE,
            "case {case:?}: expected the instance to be {DB_INSTANCE_STATUS_AVAILABLE} when \
             the call arrives, got {status:?}"
        );

        for asked_for in [None, Some(LOCKED), Some(UNLOCKED)] {
            let refused = expect_error(
                case,
                modify(&router, &arn, asked_for).await,
                400,
                "InvalidDBInstanceState",
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message, ACTIVITY_STREAM_ENGINE_MESSAGE,
                "case {case:?} asking for {asked_for:?}: expected the wording \
                 {ACTIVITY_STREAM_ENGINE_MESSAGE:?}, got {message:?}"
            );
        }

        let reported = reported_policy(&router, case, row.identifier).await;
        assert!(
            reported.is_none(),
            "case {case:?}: expected the refused calls to write nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_instance_that_is_not_available_is_refused_until_it_is() {
    let router = emulator();

    // An instance whose stop has finished, settled before the row below creates the one
    // that is still coming up, so both blocking statuses are met without waiting between
    // them.
    expect_ok(
        "a stopped instance",
        create_instance(&router, REGION, "emu-das-stopped", "oracle-se2", "db.m5.large").await,
    );
    poll_until(
        "a stopped instance",
        "emu-das-stopped reaching available",
        || async {
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "emu-das-stopped")],
            )
            .await
        },
        |status, body| {
            status == 200 && text(body, "DBInstanceStatus") == DB_INSTANCE_STATUS_AVAILABLE
        },
    )
    .await;
    expect_ok(
        "a stopped instance",
        call(
            &router,
            "StopDBInstance",
            &[("DBInstanceIdentifier", "emu-das-stopped")],
        )
        .await,
    );
    poll_until(
        "a stopped instance",
        "emu-das-stopped reaching stopped",
        || async {
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "emu-das-stopped")],
            )
            .await
        },
        |status, body| {
            status == 200 && text(body, "DBInstanceStatus") == DB_INSTANCE_STATUS_STOPPED
        },
    )
    .await;

    expect_ok(
        "an instance still coming up",
        create_instance(
            &router,
            REGION,
            "emu-das-creating",
            "sqlserver-ex",
            "db.t3.medium",
        )
        .await,
    );

    let blocked = [
        (
            "an instance still coming up",
            "emu-das-creating",
            DB_INSTANCE_STATUS_CREATING,
        ),
        (
            "a stopped instance",
            "emu-das-stopped",
            DB_INSTANCE_STATUS_STOPPED,
        ),
    ];
    for (case, identifier, status) in blocked {
        let reported = text(
            &describe_instance(&router, case, identifier).await,
            "DBInstanceStatus",
        );
        assert_eq!(
            reported, status,
            "case {case:?}: expected the instance to report {status:?} when the call arrives, \
             got {reported:?}"
        );
        let arn = instance_arn(REGION, identifier);
        let refused = expect_error(
            case,
            modify(&router, &arn, Some(UNLOCKED)).await,
            400,
            "InvalidDBInstanceState",
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message, MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE,
            "case {case:?}: expected the wording {MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE:?}, \
             got {message:?}"
        );
        let policy = reported_policy(&router, case, identifier).await;
        assert!(
            policy.is_none(),
            "case {case:?}: expected the refused call to write nothing, got {policy:?}"
        );
    }

    // The refusal was about the STATE: once the clock has settled the instance that was
    // coming up, the same call is taken.
    poll_until(
        "an instance still coming up",
        "emu-das-creating reaching available",
        || async {
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "emu-das-creating")],
            )
            .await
        },
        |status, body| {
            status == 200 && text(body, "DBInstanceStatus") == DB_INSTANCE_STATUS_AVAILABLE
        },
    )
    .await;
    accepted(
        &router,
        "an instance still coming up",
        &instance_arn(REGION, "emu-das-creating"),
        Some(UNLOCKED),
        UNLOCKING,
    )
    .await;
    poll_until_policy(
        &router,
        "an instance still coming up",
        "emu-das-creating",
        UNLOCKED,
    )
    .await;
}
