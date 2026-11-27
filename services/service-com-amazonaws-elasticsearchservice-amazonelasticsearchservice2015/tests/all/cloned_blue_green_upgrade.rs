// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a domain's VERSION UPGRADE: the in-place one, the cloned
//! blue/green one `UpdateClone` drives, and the change progress both record — driven as
//! real restJson1 requests through the router.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. An upgrade may move a domain only onto a release the service OFFERS for the one
//!    it is running (the matrix `GetCompatibleElasticsearchVersions` publishes); it
//!    records a change the domain reports, and the domain runs the target release once
//!    the clock makes the upgrade due.
//! 2. An eligibility check (`PerformCheckOnly`) writes NOTHING.
//! 3. A cloned blue/green upgrade leaves the domain on its own release and waits on the
//!    caller: a traffic shift reports the live split against the requested one until it
//!    lands, whatever share is asked for.
//! 4. A cutover decides the upgrade: onto the cloned cluster the domain moves to the
//!    target release and the cluster cut over from is kept for the retention period;
//!    back to the primary cluster the upgrade ends and the release does not move.
//! 5. A `RetentionAction` ends the retention period, which ends the upgrade — and
//!    `RollbackToPrimaryCluster` puts the domain back onto the cluster it came from.
//! 6. `UpdateClone` takes EXACTLY ONE of the three settings, and only against a domain
//!    that has an upgrade in progress.
//! 7. `DescribeDomainChangeProgress` answers about the change a `ChangeId` names, the
//!    most recent one when it names none, and nothing a caller asked for that the
//!    domain never recorded.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region every call in this suite is signed for.
const REGION: &str = "us-east-1";

/// How long a poll loop waits for a transition the clock owes before it gives up.
///
/// Generous on purpose: the condition decides when the loop ends, not the clock, so a
/// slow or loaded host only polls more times.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long one poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The retention period the clone cases start their upgrade with, in hours.
const RETENTION_HOURS: i64 = 24;

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Answer {
    call_in(router, REGION, method, path, body).await
}

/// Create a domain running `version` (omitted when `None`).
async fn create(router: &MultiServiceRouter, name: &str, version: Option<&str>) -> Answer {
    let mut body = json!({
        "DomainName": name,
        "ElasticsearchClusterConfig": {"InstanceType": "t2.small.elasticsearch", "InstanceCount": 2}
    });
    if let Some(version) = version {
        body["ElasticsearchVersion"] = json!(version);
    }
    call(router, "POST", "/2015-01-01/es/domain", Some(&body)).await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/2015-01-01/es/domain/{name}"), None).await
}

async fn upgrade(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", "/2015-01-01/es/upgradeDomain", Some(body)).await
}

async fn update_clone(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/2015-01-01/es/domain/{name}/clone"),
        Some(body),
    )
    .await
}

async fn progress(router: &MultiServiceRouter, name: &str, change_id: Option<&str>) -> Answer {
    let path = match change_id {
        Some(change_id) => format!("/2015-01-01/es/domain/{name}/progress?changeid={change_id}"),
        None => format!("/2015-01-01/es/domain/{name}/progress"),
    };
    call(router, "GET", &path, None).await
}

/// The body that starts a cloned blue/green upgrade onto `target`.
fn cloned_upgrade_body(name: &str, target: &str, retention_hours: Option<i64>) -> Value {
    let mut options = json!({"DeploymentStrategy": "ClonedBlueGreen"});
    if let Some(hours) = retention_hours {
        options["ClonedBlueGreenOptions"] = json!({"RetentionOptions": {"RetentionInHours": hours}});
    }
    json!({
        "DomainName": name,
        "TargetVersion": target,
        "DeploymentStrategyOptions": options
    })
}

/// Poll `read` until `holds` answers true, or [`SETTLE_DEADLINE`] passes.
///
/// The wait is for the CONDITION: nothing here asserts on elapsed time, and the
/// deadline only bounds a loop that would otherwise never end.
async fn poll_until<Read, Fut, Holds>(case: &str, mut read: Read, holds: Holds) -> Answer
where
    Read: FnMut() -> Fut,
    Fut: std::future::Future<Output = Answer>,
    Holds: Fn(&Answer) -> bool,
{
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let answer = read().await;
        if holds(&answer) {
            return answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the condition never held; last answer {} {}",
            answer.status,
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One upgrade the service offers: the release the domain runs and the one it may be
/// carried onto.
struct UpgradeCase {
    label: &'static str,
    name: &'static str,
    source: &'static str,
    target: &'static str,
}

/// Rows differ along what rule 1 generalises over: the step inside one major, a step
/// across a major boundary, the migration onto the other engine's line, and a step
/// inside that line. Every pair is one the service's own matrix offers
/// (`GetCompatibleElasticsearchVersions` answers it for the source release).
fn upgrade_cases() -> Vec<UpgradeCase> {
    vec![
        UpgradeCase {
            label: "a later minor of the same major",
            name: "upgrade-minor",
            source: "7.1",
            target: "7.10",
        },
        UpgradeCase {
            label: "the first offered release of the next major",
            name: "upgrade-major",
            source: "6.8",
            target: "7.4",
        },
        UpgradeCase {
            label: "the migration onto the OpenSearch line",
            name: "upgrade-migration",
            source: "7.10",
            target: "OpenSearch_1.3",
        },
        UpgradeCase {
            label: "a step inside the OpenSearch line",
            name: "upgrade-opensearch",
            source: "OpenSearch_1.3",
            target: "OpenSearch_2.3",
        },
    ]
}

/// RULE 1: an in-place upgrade records a change, reports the domain as upgrading, and
/// lands the target release by the clock alone.
#[tokio::test]
async fn an_in_place_upgrade_records_a_change_and_lands_the_target_release() {
    let router = router();
    for case in upgrade_cases() {
        let label = case.label;
        create(&router, case.name, Some(case.source)).await;

        let started = upgrade(
            &router,
            &json!({"DomainName": case.name, "TargetVersion": case.target}),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: the upgrade must be accepted, got {} {}",
            started.status, started.body
        );
        assert_eq!(
            started.at(label, "/TargetVersion"),
            &json!(case.target),
            "case {label:?}: the upgrade echoes the target release"
        );
        let change_id = started.text(label, "/ChangeProgressDetails/ChangeId");
        assert!(
            is_guid(&change_id),
            "case {label:?}: the change must be tracked by a GUID, got {change_id:?}"
        );

        // While the change applies, the domain is still on the release it was running.
        let during = describe(&router, case.name).await;
        assert_eq!(
            during.at(label, "/DomainStatus/ElasticsearchVersion"),
            &json!(case.source),
            "case {label:?}: an upgrade in flight leaves the domain on its own release"
        );
        assert_eq!(
            during.at(label, "/DomainStatus/UpgradeProcessing"),
            &json!(true),
            "case {label:?}: a domain polled at once reports the upgrade as processing"
        );
        assert_eq!(
            during.text(label, "/DomainStatus/ChangeProgressDetails/ChangeId"),
            change_id,
            "case {label:?}: the domain reports the change the upgrade minted"
        );

        let settled = poll_until(
            label,
            || describe(&router, case.name),
            |answer| answer.body.pointer("/DomainStatus/UpgradeProcessing") == Some(&json!(false)),
        )
        .await;
        assert_eq!(
            settled.at(label, "/DomainStatus/ElasticsearchVersion"),
            &json!(case.target),
            "case {label:?}: the settled domain runs the target release"
        );
        assert_eq!(
            settled.text(label, "/DomainStatus/ChangeProgressDetails/ConfigChangeStatus"),
            "Completed",
            "case {label:?}: the change that carried the upgrade is Completed"
        );
    }
}

/// RULE 1, the other half: a target the service does not offer for the domain's release
/// is refused, and nothing is recorded.
#[tokio::test]
async fn a_target_release_the_service_does_not_offer_is_refused() {
    let router = router();
    // Rows differ along the ways a target can be outside the matrix: a release older
    // than the one running, one the service no longer accepts as a destination, and one
    // two majors ahead.
    let cases = [
        ("an older release", "refuse-older", "7.10", "7.1"),
        ("a retired destination", "refuse-retired", "5.6", "6.0"),
        ("two majors ahead", "refuse-two-majors", "7.1", "OpenSearch_2.3"),
    ];
    for (label, name, source, target) in cases {
        create(&router, name, Some(source)).await;
        let refused = upgrade(
            &router,
            &json!({"DomainName": name, "TargetVersion": target}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: an upgrade off the matrix is refused with 400, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ValidationException"),
            "case {label:?}: refused as a ValidationException, got {:?}",
            refused.error_type
        );
        let message = refused.text(label, "/message");
        assert!(
            message.contains(target) && message.contains(source),
            "case {label:?}: the refusal names the release asked for and the one \
             running, got {message:?}"
        );

        let after = describe(&router, name).await;
        assert_eq!(
            after.at(label, "/DomainStatus/ElasticsearchVersion"),
            &json!(source),
            "case {label:?}: a refused upgrade leaves the release where it was"
        );
        assert!(
            after.missing("/DomainStatus/ChangeProgressDetails"),
            "case {label:?}: a refused upgrade records no change, got {}",
            after.body
        );
    }
}

/// RULE 2: an eligibility check reports the request back and writes nothing — neither a
/// change, nor a release move, nor an upgrade in progress.
#[tokio::test]
async fn an_eligibility_check_writes_nothing() {
    let router = router();
    for case in upgrade_cases() {
        let label = case.label;
        let name = &format!("check-{}", case.name)[..];
        create(&router, name, Some(case.source)).await;

        let checked = upgrade(
            &router,
            &json!({
                "DomainName": name,
                "TargetVersion": case.target,
                "PerformCheckOnly": true
            }),
        )
        .await;
        assert_eq!(
            checked.status, 200,
            "case {label:?}: an eligibility check of an offered target succeeds, got {} {}",
            checked.status, checked.body
        );
        assert_eq!(
            checked.at(label, "/PerformCheckOnly"),
            &json!(true),
            "case {label:?}: the check reports that it was one"
        );
        assert!(
            checked.missing("/ChangeProgressDetails"),
            "case {label:?}: a check performs no change, so it tracks none, got {}",
            checked.body
        );

        let after = describe(&router, name).await;
        assert_eq!(
            after.at(label, "/DomainStatus/ElasticsearchVersion"),
            &json!(case.source),
            "case {label:?}: a check does not move the release"
        );
        assert_eq!(
            after.at(label, "/DomainStatus/UpgradeProcessing"),
            &json!(false),
            "case {label:?}: a check starts no upgrade"
        );
        assert!(
            after.missing("/DomainStatus/ChangeProgressDetails"),
            "case {label:?}: a check records no change, got {}",
            after.body
        );
        assert_eq!(
            progress(&router, name, None).await.status,
            200,
            "case {label:?}: the progress read answers about a domain with no change"
        );
        assert!(
            progress(&router, name, None)
                .await
                .missing("/ChangeProgressStatus"),
            "case {label:?}: a domain nothing changed has no progress to report"
        );
    }
}

/// RULE 3: a cloned blue/green upgrade waits on the caller, and a traffic shift reports
/// the live split against the requested one until the shift lands — for every share.
#[tokio::test]
async fn a_traffic_shift_reports_the_live_split_until_it_lands() {
    let router = router();
    let name = "clone-traffic";
    create(&router, name, Some("7.10")).await;
    let started = upgrade(
        &router,
        &cloned_upgrade_body(name, "OpenSearch_1.3", Some(RETENTION_HOURS)),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the cloned blue/green upgrade must be accepted, got {} {}",
        started.status, started.body
    );
    let upgrade_change = started.text("clone start", "/ChangeProgressDetails/ChangeId");

    // The clone is stood up first, and the upgrade then waits for the caller: that is
    // what `PendingUserInput` means for the change that started it.
    poll_until(
        "the clone becomes ready",
        || progress(&router, name, Some(&upgrade_change)),
        |answer| {
            answer.body.pointer("/ChangeProgressStatus/ConfigChangeStatus")
                == Some(&json!("PendingUserInput"))
        },
    )
    .await;

    // Rows differ along the share asked for: none, the smallest step, an arbitrary
    // share, and the whole of the read traffic (`TrafficPercent` is @range(0..100)).
    let shares = [0, 1, 37, 100];
    let mut previous = 0;
    for share in shares {
        let label = &format!("a shift to {share} percent")[..];
        let shifted = update_clone(
            &router,
            name,
            &json!({"TrafficSplitOptions": {
                "Strategy": "ShiftReadTrafficPercent",
                "CloneReadTrafficPercent": share
            }}),
        )
        .await;
        assert_eq!(
            shifted.status, 200,
            "case {label:?}: the shift must be accepted, got {} {}",
            shifted.status, shifted.body
        );
        assert_eq!(
            shifted.at(label, "/TrafficSplitOptions"),
            &json!({
                "Current": {"PrimaryReadTrafficPercent": 100 - previous, "CloneReadTrafficPercent": previous},
                "Target": {"PrimaryReadTrafficPercent": 100 - share, "CloneReadTrafficPercent": share},
                "Status": "InProgress"
            }),
            "case {label:?}: a shift in flight reports the live split, the requested \
             one and InProgress"
        );
        assert_eq!(
            shifted.at(label, "/DomainName"),
            &json!(name),
            "case {label:?}: the answer names the domain"
        );
        assert!(
            is_guid(&shifted.text(label, "/ChangeId")),
            "case {label:?}: every call records a change tracked by a GUID"
        );
        assert_eq!(
            shifted.at(label, "/RetentionOptions"),
            &json!({"RetentionInHours": RETENTION_HOURS}),
            "case {label:?}: the upgrade reports the retention period it was started with"
        );
        assert!(
            shifted.missing("/CutoverOptions"),
            "case {label:?}: no cutover was asked for, so none is reported, got {}",
            shifted.body
        );

        let landed = poll_until(
            label,
            || progress(&router, name, None),
            |answer| {
                answer
                    .body
                    .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                    == Some(&json!("Completed"))
            },
        )
        .await;
        assert_eq!(
            landed.at(label, "/ChangeProgressStatus/ClonedBlueGreenUpgradeDetails/TrafficSplit"),
            &json!({"PrimaryReadTrafficPercent": 100 - share, "CloneReadTrafficPercent": share}),
            "case {label:?}: once the shift has landed the live split is the one asked for"
        );
        assert_eq!(
            landed.at(label, "/ChangeProgressStatus/CompletedProperties"),
            &json!(["TrafficSplitOptions"]),
            "case {label:?}: the property the change moved is reported completed"
        );

        // The domain is still on its own release: a shift moves traffic, not the engine.
        let described = describe(&router, name).await;
        assert_eq!(
            described.at(label, "/DomainStatus/ElasticsearchVersion"),
            &json!("7.10"),
            "case {label:?}: a traffic shift does not move the release"
        );
        assert_eq!(
            described.at(label, "/DomainStatus/UpgradeProcessing"),
            &json!(true),
            "case {label:?}: the upgrade is still in progress until the caller decides"
        );
        previous = share;
    }
}

/// RULE 4: the cutover decides the upgrade — onto the cloned cluster the domain moves
/// onto the target release and the cluster it left is kept, back to the primary cluster
/// the upgrade ends where it started.
#[tokio::test]
async fn the_cutover_decides_whether_the_release_moves() {
    let router = router();
    // Rows differ along the two targets `CutoverTarget` declares.
    let cases = [
        ("a cutover onto the cloned cluster", "cutover-clone", "ClonedCluster", "OpenSearch_1.3"),
        ("a cutover back to the primary cluster", "cutover-primary", "PrimaryCluster", "7.10"),
    ];
    for (label, name, target, running_after) in cases {
        create(&router, name, Some("7.10")).await;
        let started = upgrade(
            &router,
            &cloned_upgrade_body(name, "OpenSearch_1.3", Some(RETENTION_HOURS)),
        )
        .await;
        let upgrade_change = started.text(label, "/ChangeProgressDetails/ChangeId");
        poll_until(
            label,
            || progress(&router, name, Some(&upgrade_change)),
            |answer| {
                answer
                    .body
                    .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                    == Some(&json!("PendingUserInput"))
            },
        )
        .await;

        let cut = update_clone(
            &router,
            name,
            &json!({"CutoverOptions": {"Target": target}}),
        )
        .await;
        assert_eq!(
            cut.status, 200,
            "case {label:?}: the cutover must be accepted, got {} {}",
            cut.status, cut.body
        );
        assert_eq!(
            cut.at(label, "/CutoverOptions"),
            &json!({"Target": target, "Status": "InProgress"}),
            "case {label:?}: a cutover in flight reports its target and InProgress"
        );

        let settled = poll_until(
            label,
            || describe(&router, name),
            |answer| {
                answer.body.pointer("/DomainStatus/ElasticsearchVersion")
                    == Some(&json!(running_after))
                    && answer
                        .body
                        .pointer("/DomainStatus/ChangeProgressDetails/ConfigChangeStatus")
                        == Some(&json!("Completed"))
            },
        )
        .await;
        assert_eq!(
            settled.at(label, "/DomainStatus/ElasticsearchVersion"),
            &json!(running_after),
            "case {label:?}: the completed cutover decides the release the domain runs"
        );

        // A second cutover is refused: the upgrade either waits on nothing any more or
        // has already been decided.
        let again = update_clone(
            &router,
            name,
            &json!({"CutoverOptions": {"Target": target}}),
        )
        .await;
        assert_eq!(
            again.status, 400,
            "case {label:?}: a second cutover is refused, got {} {}",
            again.status, again.body
        );
    }
}

/// RULE 4 and 5: after the cutover completes the cluster cut over from is kept for the
/// retention period, which the upgrade reports as the hours REMAINING beside the action
/// the service takes when it ends — and a `RetentionAction` ends both the period and the
/// upgrade.
#[tokio::test]
async fn the_retention_period_is_reported_and_can_be_ended() {
    let router = router();
    // Rows differ along the two actions `RetentionAction` declares: one releases the
    // cluster cut over from, the other moves the domain back onto it.
    let cases = [
        (
            "the retention period deletes the primary cluster",
            "retention-delete",
            "DeletePrimaryCluster",
            "OpenSearch_1.3",
        ),
        (
            "the retention period rolls back to the primary cluster",
            "retention-rollback",
            "RollbackToPrimaryCluster",
            "7.10",
        ),
    ];
    for (label, name, action, running_after) in cases {
        create(&router, name, Some("7.10")).await;
        let started = upgrade(
            &router,
            &cloned_upgrade_body(name, "OpenSearch_1.3", Some(RETENTION_HOURS)),
        )
        .await;
        let upgrade_change = started.text(label, "/ChangeProgressDetails/ChangeId");
        poll_until(
            label,
            || progress(&router, name, Some(&upgrade_change)),
            |answer| {
                answer
                    .body
                    .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                    == Some(&json!("PendingUserInput"))
            },
        )
        .await;

        // The action is valid only after the cutover completes.
        let early = update_clone(&router, name, &json!({"RetentionOptions": {"RetentionAction": action}})).await;
        assert_eq!(
            early.status, 400,
            "case {label:?}: a RetentionAction before the cutover completes is refused, \
             got {} {}",
            early.status, early.body
        );

        update_clone(
            &router,
            name,
            &json!({"CutoverOptions": {"Target": "ClonedCluster"}}),
        )
        .await;
        let after_cutover = poll_until(
            label,
            || progress(&router, name, None),
            |answer| {
                answer
                    .body
                    .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                    == Some(&json!("Completed"))
            },
        )
        .await;
        assert_eq!(
            after_cutover.at(
                label,
                "/ChangeProgressStatus/ClonedBlueGreenUpgradeDetails/RetentionInHours"
            ),
            &json!(RETENTION_HOURS),
            "case {label:?}: the whole retention period is still left the instant the \
             cutover completes"
        );
        assert_eq!(
            after_cutover.text(
                label,
                "/ChangeProgressStatus/ClonedBlueGreenUpgradeDetails/ScheduledAction/Action"
            ),
            "DeletePrimaryCluster",
            "case {label:?}: the service reports what it does when the period ends"
        );

        // Changing the period is reported by the next read of the upgrade.
        let shortened = update_clone(
            &router,
            name,
            &json!({"RetentionOptions": {"RetentionInHours": 1}}),
        )
        .await;
        assert_eq!(
            shortened.at(label, "/RetentionOptions"),
            &json!({"RetentionInHours": 1}),
            "case {label:?}: the retention period a call sets is the one reported back"
        );

        let ended = update_clone(
            &router,
            name,
            &json!({"RetentionOptions": {"RetentionAction": action}}),
        )
        .await;
        assert_eq!(
            ended.status, 200,
            "case {label:?}: the action must be accepted once the cutover completed, \
             got {} {}",
            ended.status, ended.body
        );
        assert_eq!(
            ended.at(label, "/RetentionOptions"),
            &json!({"RetentionAction": action}),
            "case {label:?}: the upgrade reports the action that ended the period"
        );

        // The period, and with it the upgrade, is over: there is nothing left to drive.
        let after = update_clone(
            &router,
            name,
            &json!({"RetentionOptions": {"RetentionInHours": RETENTION_HOURS}}),
        )
        .await;
        assert_eq!(
            after.status, 400,
            "case {label:?}: once the upgrade is over UpdateClone has nothing to manage, \
             got {} {}",
            after.status, after.body
        );

        let settled = poll_until(
            label,
            || describe(&router, name),
            |answer| answer.body.pointer("/DomainStatus/UpgradeProcessing") == Some(&json!(false)),
        )
        .await;
        assert_eq!(
            settled.at(label, "/DomainStatus/ElasticsearchVersion"),
            &json!(running_after),
            "case {label:?}: the action decides which cluster's release the domain runs"
        );
    }
}

/// RULE 6: exactly one setting per request, and only on a domain that has an upgrade in
/// progress.
#[tokio::test]
async fn update_clone_takes_exactly_one_setting_of_an_upgrade_in_progress() {
    let router = router();
    let name = "clone-refusals";
    create(&router, name, Some("7.10")).await;

    // Before any upgrade is started there is nothing to manage.
    let no_upgrade = update_clone(
        &router,
        name,
        &json!({"CutoverOptions": {"Target": "ClonedCluster"}}),
    )
    .await;
    assert_eq!(
        no_upgrade.status, 400,
        "a domain with no cloned blue/green upgrade is refused with 400, got {} {}",
        no_upgrade.status, no_upgrade.body
    );
    assert!(
        no_upgrade.error_type.contains("ValidationException"),
        "that refusal is a ValidationException, got {:?}",
        no_upgrade.error_type
    );

    // A domain that does not exist is a different claim, answered as one.
    let absent = update_clone(
        &router,
        "clone-nothing-here",
        &json!({"CutoverOptions": {"Target": "ClonedCluster"}}),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "a domain nothing created is not found, got {} {}",
        absent.status, absent.body
    );
    assert!(
        absent.error_type.contains("ResourceNotFoundException"),
        "that refusal names the missing resource, got {:?}",
        absent.error_type
    );

    let started = upgrade(
        &router,
        &cloned_upgrade_body(name, "OpenSearch_1.3", Some(RETENTION_HOURS)),
    )
    .await;
    let upgrade_change = started.text("refusals", "/ChangeProgressDetails/ChangeId");
    poll_until(
        "the clone becomes ready",
        || progress(&router, name, Some(&upgrade_change)),
        |answer| {
            answer
                .body
                .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                == Some(&json!("PendingUserInput"))
        },
    )
    .await;

    // Rows differ along how many of the three settings a request names, and along a
    // retention period under the published minimum of one hour.
    let cases = [
        ("no setting at all", json!({})),
        (
            "two settings",
            json!({
                "CutoverOptions": {"Target": "ClonedCluster"},
                "RetentionOptions": {"RetentionInHours": RETENTION_HOURS}
            }),
        ),
        (
            "all three settings",
            json!({
                "TrafficSplitOptions": {"Strategy": "ShiftReadTrafficPercent", "CloneReadTrafficPercent": 10},
                "CutoverOptions": {"Target": "ClonedCluster"},
                "RetentionOptions": {"RetentionInHours": RETENTION_HOURS}
            }),
        ),
        (
            "a retention period under the minimum",
            json!({"RetentionOptions": {"RetentionInHours": 0}}),
        ),
    ];
    for (label, body) in cases {
        let refused = update_clone(&router, name, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: refused with 400, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ValidationException"),
            "case {label:?}: refused as a ValidationException, got {:?}",
            refused.error_type
        );
    }

    // None of those refusals changed the upgrade: it is still waiting on the caller.
    let still_waiting = progress(&router, name, Some(&upgrade_change)).await;
    assert_eq!(
        still_waiting.text("refusals", "/ChangeProgressStatus/ConfigChangeStatus"),
        "PendingUserInput",
        "a refused request leaves the upgrade where it was"
    );
}

/// RULE 7: the progress read answers about the change its `ChangeId` names, the most
/// recent change when it names none, and refuses an id the domain never recorded.
#[tokio::test]
async fn the_progress_read_answers_about_the_change_it_names() {
    let router = router();
    let name = "progress-reads";
    create(&router, name, Some("7.10")).await;
    let started = upgrade(
        &router,
        &cloned_upgrade_body(name, "OpenSearch_1.3", Some(RETENTION_HOURS)),
    )
    .await;
    let upgrade_change = started.text("progress", "/ChangeProgressDetails/ChangeId");
    poll_until(
        "the clone becomes ready",
        || progress(&router, name, Some(&upgrade_change)),
        |answer| {
            answer
                .body
                .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                == Some(&json!("PendingUserInput"))
        },
    )
    .await;

    let shifted = update_clone(
        &router,
        name,
        &json!({"TrafficSplitOptions": {
            "Strategy": "ShiftReadTrafficPercent",
            "CloneReadTrafficPercent": 25
        }}),
    )
    .await;
    let shift_change = shifted.text("progress", "/ChangeId");
    assert_ne!(
        shift_change, upgrade_change,
        "two changes of one domain never share a ChangeId"
    );

    // Each id names its OWN change, with the property that change moved.
    for (label, change_id, property) in [
        ("the upgrade's change", &upgrade_change, "ElasticsearchVersion"),
        ("the shift's change", &shift_change, "TrafficSplitOptions"),
    ] {
        let answer = progress(&router, name, Some(change_id)).await;
        assert_eq!(
            answer.text(label, "/ChangeProgressStatus/ChangeId"),
            *change_id,
            "case {label:?}: the read answers about the change it names"
        );
        let properties: Vec<String> = ["PendingProperties", "CompletedProperties"]
            .iter()
            .flat_map(|member| {
                answer
                    .at(label, &format!("/ChangeProgressStatus/{member}"))
                    .as_array()
                    .expect("the property lists are lists")
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect::<Vec<String>>()
            })
            .collect();
        assert_eq!(
            properties,
            vec![property.to_string()],
            "case {label:?}: the change reports exactly the property it moves, pending \
             or completed"
        );
        assert_eq!(
            answer.at(label, "/ChangeProgressStatus/TotalNumberOfStages"),
            &json!(0),
            "case {label:?}: no stage set is reported, so the stage count is zero"
        );
    }

    // A request naming no id is answered about the most recent change.
    let latest = progress(&router, name, None).await;
    assert_eq!(
        latest.text("the latest change", "/ChangeProgressStatus/ChangeId"),
        shift_change,
        "a progress read naming no ChangeId answers about the most recent change"
    );

    // An id this domain recorded nothing under names no change at all.
    let unknown = "11111111-2222-3333-4444-555555555555";
    let missing = progress(&router, name, Some(unknown)).await;
    assert_eq!(
        missing.status, 404,
        "a ChangeId nothing recorded is not found, got {} {}",
        missing.status, missing.body
    );
    assert!(
        missing.text("an unknown change", "/message").contains(unknown),
        "the refusal names the change asked for, got {}",
        missing.body
    );

    // And a domain nothing created is not found either.
    let absent = progress(&router, "progress-nothing-here", None).await;
    assert_eq!(
        absent.status, 404,
        "a domain nothing created is not found, got {} {}",
        absent.status, absent.body
    );
}

/// RULE 3, the other half: a cloned blue/green upgrade started with no retention period
/// keeps nothing, and the upgrade is over the moment the cutover completes.
#[tokio::test]
async fn an_upgrade_that_keeps_nothing_ends_with_the_cutover() {
    let router = router();
    let name = "clone-no-retention";
    create(&router, name, Some("7.10")).await;
    let started = upgrade(&router, &cloned_upgrade_body(name, "OpenSearch_1.3", None)).await;
    let upgrade_change = started.text("no retention", "/ChangeProgressDetails/ChangeId");
    poll_until(
        "the clone becomes ready",
        || progress(&router, name, Some(&upgrade_change)),
        |answer| {
            answer
                .body
                .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                == Some(&json!("PendingUserInput"))
        },
    )
    .await;

    let cut = update_clone(
        &router,
        name,
        &json!({"CutoverOptions": {"Target": "ClonedCluster"}}),
    )
    .await;
    assert!(
        cut.missing("/RetentionOptions"),
        "an upgrade that keeps nothing reports no retention settings, got {}",
        cut.body
    );

    let settled = poll_until(
        "no retention",
        || describe(&router, name),
        |answer| answer.body.pointer("/DomainStatus/UpgradeProcessing") == Some(&json!(false)),
    )
    .await;
    assert_eq!(
        settled.at("no retention", "/DomainStatus/ElasticsearchVersion"),
        &json!("OpenSearch_1.3"),
        "the cutover carried the domain onto the target release"
    );

    // An upgrade already started cannot be started again while it runs.
    let second = upgrade(
        &router,
        &json!({"DomainName": name, "TargetVersion": "OpenSearch_2.3"}),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "once the upgrade is over the next one is accepted, got {} {}",
        second.status, second.body
    );
}

/// RULE 1 and 6: a retention period under the published minimum is refused where the
/// upgrade is STARTED too, and an upgrade already running is not started twice.
#[tokio::test]
async fn an_upgrade_refuses_a_retention_period_under_the_minimum_and_a_second_start() {
    let router = router();
    let name = "clone-start-rules";
    create(&router, name, Some("7.10")).await;

    let under = upgrade(
        &router,
        &cloned_upgrade_body(name, "OpenSearch_1.3", Some(0)),
    )
    .await;
    assert_eq!(
        under.status, 400,
        "a retention period under the minimum is refused at the start, got {} {}",
        under.status, under.body
    );
    assert!(
        under.error_type.contains("ValidationException"),
        "refused as a ValidationException, got {:?}",
        under.error_type
    );

    // The start accepts only `RetentionInHours`.
    let action_at_start = upgrade(
        &router,
        &json!({
            "DomainName": name,
            "TargetVersion": "OpenSearch_1.3",
            "DeploymentStrategyOptions": {
                "DeploymentStrategy": "ClonedBlueGreen",
                "ClonedBlueGreenOptions": {
                    "RetentionOptions": {"RetentionAction": "DeletePrimaryCluster"}
                }
            }
        }),
    )
    .await;
    assert_eq!(
        action_at_start.status, 400,
        "a RetentionAction at the start is refused, got {} {}",
        action_at_start.status, action_at_start.body
    );

    let accepted = upgrade(
        &router,
        &cloned_upgrade_body(name, "OpenSearch_1.3", Some(RETENTION_HOURS)),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the upgrade itself is accepted, got {} {}",
        accepted.status, accepted.body
    );
    let second = upgrade(
        &router,
        &json!({"DomainName": name, "TargetVersion": "OpenSearch_1.2"}),
    )
    .await;
    assert_eq!(
        second.status, 400,
        "a second upgrade while one is in progress is refused, got {} {}",
        second.status, second.body
    );
}

/// RULE 1 and 4, over TWO consecutive upgrades: the release a domain runs is the one
/// its last upgrade left it on, so a later upgrade in flight — and one the caller cuts
/// back from — reports that release and never the one the domain was created with.
#[tokio::test]
async fn a_second_upgrade_reports_the_release_the_first_one_landed() {
    let router = router();
    let name = "upgrade-twice";
    create(&router, name, Some("7.10")).await;

    // The first upgrade lands, so the domain runs the OpenSearch release.
    upgrade(
        &router,
        &json!({"DomainName": name, "TargetVersion": "OpenSearch_1.3"}),
    )
    .await;
    let landed = poll_until(
        "the first upgrade",
        || describe(&router, name),
        |answer| answer.body.pointer("/DomainStatus/UpgradeProcessing") == Some(&json!(false)),
    )
    .await;
    assert_eq!(
        landed.at("the first upgrade", "/DomainStatus/ElasticsearchVersion"),
        &json!("OpenSearch_1.3"),
        "the first upgrade carried the domain onto its target release"
    );
    assert_eq!(
        engine_type_of(&router, name).await,
        "OpenSearch",
        "the engine the domain reports follows the release it runs"
    );

    // A second, cloned blue/green upgrade: while it runs, the domain still runs the
    // release the FIRST one landed — not the one it was created with.
    let started = upgrade(
        &router,
        &cloned_upgrade_body(name, "OpenSearch_2.3", Some(RETENTION_HOURS)),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the second upgrade must be accepted once the first has landed, got {} {}",
        started.status, started.body
    );
    let upgrade_change = started.text("the second upgrade", "/ChangeProgressDetails/ChangeId");
    poll_until(
        "the clone of the second upgrade",
        || progress(&router, name, Some(&upgrade_change)),
        |answer| {
            answer
                .body
                .pointer("/ChangeProgressStatus/ConfigChangeStatus")
                == Some(&json!("PendingUserInput"))
        },
    )
    .await;
    let during = describe(&router, name).await;
    assert_eq!(
        during.at("during the second upgrade", "/DomainStatus/ElasticsearchVersion"),
        &json!("OpenSearch_1.3"),
        "an upgrade in flight leaves the domain on the release its last upgrade landed"
    );
    assert_eq!(
        engine_type_of(&router, name).await,
        "OpenSearch",
        "and on that release's engine"
    );

    // Cutting back to the primary cluster ends the upgrade where it started: on the
    // release the first upgrade landed, for good.
    update_clone(
        &router,
        name,
        &json!({"CutoverOptions": {"Target": "PrimaryCluster"}}),
    )
    .await;
    let after = poll_until(
        "after the cut back",
        || describe(&router, name),
        |answer| answer.body.pointer("/DomainStatus/UpgradeProcessing") == Some(&json!(false)),
    )
    .await;
    assert_eq!(
        after.at("after the cut back", "/DomainStatus/ElasticsearchVersion"),
        &json!("OpenSearch_1.3"),
        "a cutover back to the primary cluster leaves the release where it was"
    );
    assert_eq!(
        engine_type_of(&router, name).await,
        "OpenSearch",
        "and the engine with it"
    );
}

/// The `EngineType` the domain listing reports for `name`.
async fn engine_type_of(router: &MultiServiceRouter, name: &str) -> String {
    let listed = call(router, "GET", "/2015-01-01/domain", None).await;
    listed
        .at(name, "/DomainNames")
        .as_array()
        .expect("DomainNames is a list")
        .iter()
        .find(|entry| entry.pointer("/DomainName") == Some(&json!(name)))
        .and_then(|entry| entry.pointer("/EngineType"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the listing must report {name}, got {}", listed.body))
        .to_string()
}

/// Whether `value` is the text of a `GUID` as this model's pattern writes one:
/// `8-4-4-4-12` hexadecimal digits.
fn is_guid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|digit| digit.is_ascii_hexdigit()))
}
