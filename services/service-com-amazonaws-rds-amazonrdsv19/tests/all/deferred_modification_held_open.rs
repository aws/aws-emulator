// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a deferred `ModifyDBInstance` HOLDING: a change the caller did not ask
//! to apply immediately stays pending for as long as the instance's preferred maintenance
//! window is shut, read after read.
//!
//! The rule under test: `ApplyImmediately` is "disabled by default" and "if this parameter is
//! disabled, changes to the DB instance are applied during the next maintenance window", while
//! `DBInstance$PendingModifiedValues` "contains changes that will be applied during the next
//! maintenance window". So a setting that structure carries a member for is published as
//! pending and the instance goes on reporting the value it RUNS under — on every read taken
//! while no occurrence of its window is open, not merely on the first one: the entry neither
//! applies itself at a time no window names nor disappears, because it is the statement of
//! what is scheduled that a client reconciles desired configuration against. The instance also
//! goes on reporting the `PreferredMaintenanceWindow` it was created with, which is the
//! schedule the deferral is against.
//!
//! The sibling suites hold the other halves of this rule: `pending_modified_values.rs` pins
//! WHICH settings are deferred and what a modification that names the live value withdraws,
//! and `parked_modification_window.rs` varies the SHAPE of the window around the instant the
//! service reads it. This one holds the window shut and keeps READING, which is what a client
//! that gates a deployment on `PendingModifiedValues` does, and then moves the window open on
//! one row so the holding is shown to be the window's doing rather than a change that can
//! never land.
//!
//! The rows differ along what the rule generalises over — the engine family and storage type
//! the instance runs on, and the kind of setting staged (an integer size, a compute class, a
//! boolean deployment option): none of which changes that the live value stays and the staged
//! one is published. Every call goes over the wire through `db_instances::emulator`, and every
//! wait is a poll for the CONDITION the client waits on.

use std::time::Instant;

use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;

use crate::db_instances::{call, emulator, expect_ok, optional_text};
use crate::pending_modified_values::{pending, poll_instance, window_days_ahead, window_that_started};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The storage every row's instance is created with, in gibibytes.
const CREATED_STORAGE: &str = "20";

/// The compute class every row's instance is created in.
const CREATED_CLASS: &str = "db.t3.micro";

/// How long a row goes on reading the instance while its window is shut: several times over
/// the span this service owes a transition it has accepted, so a read taken after every
/// instant the clock could have made something due is included. The row asserts the state
/// each read reports, never how long the reads took or how many there were — a slower host
/// simply takes fewer of them.
fn observation_span() -> std::time::Duration {
    STATUS_TRANSITION_DELAY * 8
}

/// The gap between two reads of the same loop.
const READ_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// One row: the instance, the engine and storage it runs on, and the one setting a deferred
/// modification stages on it.
struct HeldCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    master_username: &'static str,
    /// The storage type the instance is created on.
    storage_type: &'static str,
    /// The member the deferred modification carries the new value in, which is also the member
    /// of `PendingModifiedValues` it is published under.
    member: &'static str,
    /// The value the modification asks for.
    staged: &'static str,
    /// The member of `DBInstance` the LIVE value is reported under, and what the instance
    /// reports there before the modification.
    live_member: &'static str,
    live: &'static str,
}

const HELD: &[HeldCase] = &[
    HeldCase {
        // The storage change a deployment defers to the window: the live size must stay, since
        // applying it outside the window is a storage operation nobody scheduled.
        case: "the allocated storage of a mysql instance on gp2",
        identifier: "emu-held-mysql-storage",
        engine: "mysql",
        master_username: "myadmin",
        storage_type: "gp2",
        member: "AllocatedStorage",
        staged: "30",
        live_member: "AllocatedStorage",
        live: CREATED_STORAGE,
    },
    HeldCase {
        case: "the compute class of a postgres instance on gp3",
        identifier: "emu-held-postgres-class",
        engine: "postgres",
        master_username: "pgadmin",
        storage_type: "gp3",
        member: "DBInstanceClass",
        staged: "db.t3.small",
        live_member: "DBInstanceClass",
        live: CREATED_CLASS,
    },
    HeldCase {
        case: "the Multi-AZ deployment option of a mariadb instance on gp2",
        identifier: "emu-held-mariadb-multi-az",
        engine: "mariadb",
        master_username: "mariaadmin",
        storage_type: "gp2",
        member: "MultiAZ",
        staged: "true",
        live_member: "MultiAZ",
        live: "false",
    },
];

/// Create one row's instance with a maintenance window days away, and wait until it reports
/// itself `available` — the state a modification needs it in.
async fn provision(router: &smithy_server_runtime::MultiServiceRouter, row: &HeldCase, window: &str) {
    expect_ok(
        row.case,
        call(
            router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => row.identifier,
                "Engine" => row.engine,
                "DBInstanceClass" => CREATED_CLASS,
                "AllocatedStorage" => CREATED_STORAGE,
                "StorageType" => row.storage_type,
                "PubliclyAccessible" => false,
                "PreferredMaintenanceWindow" => window,
                "MasterUsername" => row.master_username,
                "MasterUserPassword" => "a-strong-password-1",
            ],
        )
        .await,
    );
    let created = poll_instance(
        router,
        row.case,
        &format!("{} reaching available", row.identifier),
        row.identifier,
        |instance| optional_text(instance, "DBInstanceStatus").as_deref() == Some("available"),
    )
    .await;
    // The instance keeps the schedule the create asked for, which is what the deferral below
    // is against.
    let reported = optional_text(&created, "PreferredMaintenanceWindow");
    assert_eq!(
        reported.as_deref(),
        Some(window),
        "case {:?}: expected the instance to report the window it was created with, got \
         {reported:?}",
        row.case
    );
    // And the live value the row stages a change to is the one it claims.
    let running = optional_text(&created, row.live_member);
    assert_eq!(
        running.as_deref(),
        Some(row.live),
        "case {:?}: expected the created instance to report {} {:?}, got {running:?}",
        row.case,
        row.live_member,
        row.live
    );
}

/// Read the instance until the observation span is over, asserting on EVERY read that the live
/// value is untouched, the staged one is published as pending and the instance is still
/// `available` — and answer how many reads were taken.
async fn assert_held_while_shut(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &HeldCase,
) -> usize {
    let until = Instant::now() + observation_span();
    let mut reads = 0usize;
    loop {
        let instance = poll_instance(
            router,
            row.case,
            &format!("{} answering a read", row.identifier),
            row.identifier,
            |_| true,
        )
        .await;
        reads += 1;
        let status = optional_text(&instance, "DBInstanceStatus");
        assert_eq!(
            status.as_deref(),
            Some("available"),
            "case {:?}: a deferred modification takes the instance out of service for nothing, \
             so it stays available, got {status:?} on read {reads}",
            row.case
        );
        let live = optional_text(&instance, row.live_member);
        assert_eq!(
            live.as_deref(),
            Some(row.live),
            "case {:?}: expected the live {} to stay {:?} outside the window, got {live:?} on \
             read {reads}",
            row.case,
            row.live_member,
            row.live
        );
        let staged = pending(&instance).unwrap_or_else(|| {
            panic!(
                "case {:?}: the deferred change must stay in PendingModifiedValues, got \
                 {instance} on read {reads}",
                row.case
            )
        });
        let waiting = optional_text(&staged, row.member);
        assert_eq!(
            waiting.as_deref(),
            Some(row.staged),
            "case {:?}: expected {} {:?} published as pending, got {waiting:?} on read {reads}",
            row.case,
            row.member,
            row.staged
        );
        if Instant::now() >= until {
            return reads;
        }
        tokio::time::sleep(READ_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_deferred_modification_stays_pending_while_the_window_is_shut() {
    let router = emulator();
    // A window whose next occurrence is days away, so no occurrence of it is open while the row
    // runs and nothing staged against it is due ([`window_days_ahead`] builds it from the
    // instant the row runs at, so the fixture is the same case whenever the suite runs).
    let window = window_days_ahead();

    for row in HELD {
        provision(&router, row, &window).await;

        // A modification the caller did NOT ask to apply immediately.
        expect_ok(
            row.case,
            call(
                &router,
                "ModifyDBInstance",
                &params![
                    "DBInstanceIdentifier" => row.identifier,
                    "ApplyImmediately" => false,
                    row.member => row.staged,
                ],
            )
            .await,
        );

        let reads = assert_held_while_shut(&router, row).await;
        assert!(
            reads > 1,
            "case {:?}: the holding must be observed on more than one read, got {reads}",
            row.case
        );
    }
}

#[tokio::test]
async fn moving_the_window_open_applies_the_change_that_was_held() {
    let router = emulator();
    // The storage row, whose live value is the one a client must not see change outside the
    // window — and must see change once the window the instance keeps is open.
    let row = &HELD[0];
    let shut = window_days_ahead();
    provision(&router, row, &shut).await;
    expect_ok(
        row.case,
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => row.identifier,
                "ApplyImmediately" => false,
                row.member => row.staged,
            ],
        )
        .await,
    );
    assert_held_while_shut(&router, row).await;

    // The window the instance KEEPS is what decides when a staged change lands, so a
    // modification that moves it to an occurrence that is open — one that has started and not
    // yet ended, built from the instant this row runs at — makes the held change due.
    let open = window_that_started();
    expect_ok(
        row.case,
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => row.identifier,
                "PreferredMaintenanceWindow" => open,
            ],
        )
        .await,
    );
    let applied = poll_instance(
        &router,
        row.case,
        "the held change being applied inside the window",
        row.identifier,
        |instance| optional_text(instance, row.live_member).as_deref() == Some(row.staged),
    )
    .await;
    let live = optional_text(&applied, row.live_member);
    assert_eq!(
        live.as_deref(),
        Some(row.staged),
        "case {:?}: expected the held change applied once the window was open, got {live:?}",
        row.case
    );
    assert!(
        pending(&applied).is_none(),
        "case {:?}: nothing stays pending once the window applied it, got {applied}",
        row.case
    );
}
