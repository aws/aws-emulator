// SPDX-License-Identifier: CC0-1.0

//! A PROBE of the cluster-launch workflow a client drives WITHIN ITS OWN POLLING BUDGET: the
//! positive control that has to come up and finish, and the launch whose reference resolves to
//! nothing, both read at a client's cadence rather than as fast as the process can.
//!
//! It is named for what it diagnoses, beside this crate's other probes
//! ([`crate::zz_diag_notebook_budget`], [`crate::zz_timeline_probe`]): the RULE it drives is
//! the one [`crate::launch_reference_validation`] states, but here every look is paced like a
//! client's own ([`CLIENT_CADENCE`]) and the whole workflow has ONE budget
//! ([`CLIENT_BUDGET`]) — so a settlement that only arrives for a caller reading in a tight
//! loop, or a cluster whose capacity or shutdown costs more rounds than a client's budget
//! holds, fails here.
//!
//! The two legs are the ones a client really drives, in order, against ONE emulator:
//!
//! * the positive control — a multi-primary cluster, launched in a subnet Amazon EC2 holds
//!   under a role IAM holds — reaches launched capacity, and then the shutdown a client asks
//!   for reaches a terminal state: such a cluster is locked against a shutdown it did not mean
//!   ([`crate::multi_primary_termination_protection`]), so the client clears the lock and
//!   terminates it, which is the sequence Amazon EMR documents for one;
//! * the same request with its `ServiceRole` naming a role IAM holds nothing under settles
//!   `TERMINATED_WITH_ERRORS` with the modelled `VALIDATION_ERROR` reason, holding no
//!   instance.
//!
//! Everything is mounted and built through the harness of that suite, so the two read one
//! emulator and one set of identities rather than a copy each.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::launch_reference_validation::{
    describe, emr, harness, identities, instances_of, launch, multi_primary, subnet,
    ABSENT_SERVICE_ROLE, READY_STATES, SERVICE_ROLE, TERMINAL_STATES, VALIDATION_ERROR,
};

/// The cadence this probe's client reads a cluster at — a client waiting on an AWS status reads
/// seconds apart rather than as fast as it can.
const CLIENT_CADENCE: Duration = Duration::from_secs(1);

/// The budget the WHOLE workflow has, counted once for both legs together: a client that
/// launches a cluster, waits for it, launches another and waits for that one has one window for
/// all of it. Generous beside the spans the two legs are owed, so a loaded host delays this
/// probe rather than failing it, and far short of the test-suite ceiling.
const CLIENT_BUDGET: Duration = Duration::from_secs(90);

/// The cluster `cluster_id` once it reports one of `states`, read at the client's own cadence
/// until the shared budget runs out.
async fn read_until(
    harness: &crate::launch_reference_validation::Harness,
    case: &str,
    cluster_id: &str,
    states: &[&str],
    deadline: Instant,
) -> Value {
    loop {
        let cluster = describe(harness, cluster_id).await;
        let state = cluster["Status"]["State"].as_str().unwrap_or_default();
        if states.contains(&state) {
            return cluster;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: a client reading every {CLIENT_CADENCE:?} must see one of \
             {states:?} inside its budget, got {cluster}"
        );
        ::tokio::time::sleep(CLIENT_CADENCE).await;
    }
}

#[tokio::test]
async fn a_client_reading_at_its_own_cadence_sees_both_launches_settle() {
    let harness = harness();
    identities(&harness).await;
    let held_subnet = subnet(&harness).await;
    let deadline = Instant::now() + CLIENT_BUDGET;

    // The positive control: every reference resolves, so the cluster comes up with its capacity
    // and then finishes, since its launch asked for no cluster that stays up.
    let case = "the positive control";
    let control = launch(
        &harness,
        case,
        held_subnet.as_str(),
        SERVICE_ROLE,
        multi_primary(),
        None,
    )
    .await;
    let up = read_until(&harness, case, &control, &READY_STATES, deadline).await;
    let reported = instances_of(&harness, &control).await;
    assert!(
        !reported.is_empty(),
        "case {case:?}: the cluster that came up must report its capacity, got {up}"
    );
    // A cluster with several primary nodes is locked against a shutdown it did not mean, so the
    // client clears the lock and then asks for the shutdown — the sequence Amazon EMR documents
    // for terminating one.
    emr(
        &harness.router,
        "SetTerminationProtection",
        ::serde_json::json!({ "JobFlowIds": [&control], "TerminationProtected": false }),
    )
    .await;
    emr(
        &harness.router,
        "TerminateJobFlows",
        ::serde_json::json!({ "JobFlowIds": [&control] }),
    )
    .await;
    read_until(&harness, case, &control, &TERMINAL_STATES, deadline).await;

    // The same request, with one reference that resolves to nothing.
    let case = "a service role IAM holds nothing under";
    let refused = launch(
        &harness,
        case,
        held_subnet.as_str(),
        ABSENT_SERVICE_ROLE,
        multi_primary(),
        None,
    )
    .await;
    let settled = read_until(
        &harness,
        case,
        &refused,
        &["TERMINATED_WITH_ERRORS"],
        deadline,
    )
    .await;
    assert_eq!(
        settled["Status"]["StateChangeReason"]["Code"].as_str(),
        Some(VALIDATION_ERROR),
        "case {case:?}: the settled cluster must carry the validation reason, got {settled}"
    );
    let reported = instances_of(&harness, &refused).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: no instance is launched for it, got {reported:?}"
    );
}
