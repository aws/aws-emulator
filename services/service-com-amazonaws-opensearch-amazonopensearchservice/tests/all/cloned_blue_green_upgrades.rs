// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a CLONED BLUE/GREEN upgrade: the `UpgradeDomain` that starts one,
//! the `UpdateClone` calls that manage it, and the `DescribeDomainChangeProgress` read that
//! reports its state.
//!
//! Every case drives the real wire path — a signed `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness is this crate's own
//! ([`super::domain_lifecycle`]); this suite adds no second copy of it.
//!
//! The RULES under test, each asserted over a SET of inputs rather than over one call:
//!
//! * an `UpgradeDomain` carrying the cloned blue/green deployment strategy echoes the
//!   strategy it was asked for and starts an upgrade whose state the progress read reports
//!   as `ClonedBlueGreenUpgradeDetails`, on the change that upgrade began — and an upgrade
//!   that asks for no strategy reports no such details at all;
//! * `UpdateClone` moves that upgrade and nothing else: a traffic shift is `InProgress` with
//!   the split still where it was, and the clock moves the split to the share the request
//!   asked for; a cutover carries all read traffic to the cluster it cuts over to; and a
//!   retention action ends the retention period and with it the upgrade;
//! * exactly one of the three option blocks is managed per request, and a domain with no
//!   such upgrade in progress has nothing for the call to manage.
//!
//! A case that waits for the clock polls for the CONDITION
//! ([`super::domain_lifecycle::poll_until`]) rather than sleeping for the span, so the same
//! case passes on a fast host and on a loaded one.

use serde_json::{json, Value};

use super::domain_lifecycle::{error_code, poll_until, Answer, Emulator, PREFIX};

/// The error code every refused input of this service travels under.
const VALIDATION: &str = "ValidationException";

/// The error code this service's generic envelope travels under.
const BASE: &str = "BaseException";

/// The error code its missing-resource refusals travel under.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The status this service answers every missing-resource refusal on.
const NOT_FOUND_STATUS: u16 = 404;

/// `DeploymentStrategyOverride`'s only value
/// (`smithy_query(query="shape",shape="DeploymentStrategyOverride")`).
const CLONED_BLUE_GREEN: &str = "ClonedBlueGreen";

/// `TrafficStrategy`'s only value.
const SHIFT_READ_TRAFFIC_PERCENT: &str = "ShiftReadTrafficPercent";

/// `CutoverTarget`'s two values.
const CLONED_CLUSTER: &str = "ClonedCluster";
/// See [`CLONED_CLUSTER`].
const PRIMARY_CLUSTER: &str = "PrimaryCluster";

/// `CutoverStatus` / `TrafficSplitStatus` while a move is under way, and once it has landed.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const COMPLETED: &str = "Completed";

/// `RetentionAction`'s two values.
const ROLLBACK_TO_PRIMARY_CLUSTER: &str = "RollbackToPrimaryCluster";
/// See [`ROLLBACK_TO_PRIMARY_CLUSTER`].
const DELETE_PRIMARY_CLUSTER: &str = "DeletePrimaryCluster";

/// The engine version every domain of this suite is created on, and the one it is upgraded
/// to: a target `GetCompatibleVersions` offers for that source.
const SOURCE_VERSION: &str = "OpenSearch_1.3";
/// See [`SOURCE_VERSION`].
const TARGET_VERSION: &str = "OpenSearch_2.3";

/// Create a domain on [`SOURCE_VERSION`] and wait until it reports nothing in flight, which
/// is the condition an upgrade of it needs.
async fn settled_domain(emulator: &Emulator, name: &str) {
    let created = emulator
        .create(json!({ "DomainName": name, "EngineVersion": SOURCE_VERSION }))
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    super::domain_lifecycle::settled_domain(emulator, name).await;
}

/// `UpgradeDomain`: `POST /2021-01-01/opensearch/upgradeDomain`.
async fn upgrade(emulator: &Emulator, name: &str, strategy: Option<Value>) -> Answer {
    let mut body = json!({ "DomainName": name, "TargetVersion": TARGET_VERSION });
    if let Some(strategy) = strategy {
        body["DeploymentStrategyOptions"] = strategy;
    }
    emulator
        .call("POST", &format!("{PREFIX}/opensearch/upgradeDomain"), Some(body))
        .await
}

/// The `DeploymentStrategyOptions` an upgrade asks a cloned blue/green deployment for, with
/// the retention period for the cluster it will cut over from.
fn cloned_blue_green(retention_in_hours: Option<i64>) -> Value {
    let mut strategy = json!({ "DeploymentStrategy": CLONED_BLUE_GREEN });
    if let Some(hours) = retention_in_hours {
        strategy["ClonedBlueGreenOptions"] =
            json!({ "RetentionOptions": { "RetentionInHours": hours } });
    }
    strategy
}

/// `UpdateClone`: `POST /2021-01-01/opensearch/domain/{DomainName}/clone`.
async fn update_clone(emulator: &Emulator, name: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{name}/clone"),
            Some(body),
        )
        .await
}

/// `DescribeDomainChangeProgress`: `GET /2021-01-01/opensearch/domain/{DomainName}/progress`,
/// with the `changeid` query member when a case names a change.
async fn progress(emulator: &Emulator, name: &str, change_id: &str) -> Value {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/progress?changeid={change_id}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainChangeProgress({name}, {change_id}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ChangeProgressStatus"].clone()
}

/// Start a cloned blue/green upgrade of a fresh domain and hand back the `ChangeId` of the
/// change it began — the change `DescribeDomainChangeProgress` reports the upgrade against.
async fn given_cloned_upgrade(
    emulator: &Emulator,
    name: &str,
    retention_in_hours: Option<i64>,
) -> String {
    settled_domain(emulator, name).await;
    let started = upgrade(emulator, name, Some(cloned_blue_green(retention_in_hours))).await;
    assert_eq!(
        started.status, 200,
        "case UpgradeDomain({name}, cloned blue/green): expected 200, got {} with {:?}",
        started.status, started.body
    );
    started.body["ChangeProgressDetails"]["ChangeId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case UpgradeDomain({name}, cloned blue/green): expected the change it began to \
                 be named, got {:?}",
                started.body
            )
        })
        .to_string()
}

/// The `(primary, clone)` shares of read traffic the progress read reports.
fn reported_split(status: &Value) -> (i64, i64) {
    let split = &status["ClonedBlueGreenUpgradeDetails"]["TrafficSplit"];
    (
        split["PrimaryReadTrafficPercent"].as_i64().unwrap_or(-1),
        split["CloneReadTrafficPercent"].as_i64().unwrap_or(-1),
    )
}

/// RULE: an `UpgradeDomain` carrying the cloned blue/green deployment strategy echoes the
/// strategy it was asked for and starts an upgrade whose state the progress read reports on
/// the change that upgrade began — all read traffic still on the primary cluster, the
/// retention period as the request asked for it, and no scheduled action before a cutover
/// has happened. An upgrade that asks for no strategy reports no such details at all.
///
/// The rows differ along what the rule generalises over: the retention period the request
/// asks for — two values, including the published minimum of one hour — and a request that
/// asks for the strategy without one.
#[tokio::test]
async fn an_upgrade_asking_for_a_cloned_deployment_reports_the_clone_on_its_change() {
    let emulator = Emulator::new();

    for (index, retention_in_hours) in [Some(24i64), Some(1i64), None].into_iter().enumerate() {
        let name = format!("cloned-start-{index}");
        let case = format!("UpgradeDomain({name}, retention {retention_in_hours:?})");
        settled_domain(&emulator, &name).await;
        let started = upgrade(
            &emulator,
            &name,
            Some(cloned_blue_green(retention_in_hours)),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            started.status, started.body
        );
        // The strategy the request asked for, echoed beside the rest of what it asked for.
        assert_eq!(
            (
                started.body["DeploymentStrategyOptions"]["DeploymentStrategy"].as_str(),
                started.body["DeploymentStrategyOptions"]["ClonedBlueGreenOptions"]
                    ["RetentionOptions"]["RetentionInHours"]
                    .as_i64(),
                started.body["TargetVersion"].as_str(),
            ),
            (Some(CLONED_BLUE_GREEN), retention_in_hours, Some(TARGET_VERSION)),
            "case {case}: expected the strategy echoed, got {:?}",
            started.body
        );

        let change_id = started.body["ChangeProgressDetails"]["ChangeId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let status = progress(&emulator, &name, &change_id).await;
        assert_eq!(
            reported_split(&status),
            (100, 0),
            "case {case}: expected all read traffic still on the primary cluster, got {:?}",
            status["ClonedBlueGreenUpgradeDetails"]
        );
        assert_eq!(
            status["ClonedBlueGreenUpgradeDetails"]["RetentionInHours"].as_i64(),
            retention_in_hours,
            "case {case}: expected the retention period the request asked for, got {:?}",
            status["ClonedBlueGreenUpgradeDetails"]
        );
        // "Members that don't apply are omitted": the retention period runs from the cutover,
        // so nothing is scheduled before one has completed.
        assert!(
            status["ClonedBlueGreenUpgradeDetails"]
                .get("ScheduledAction")
                .is_none(),
            "case {case}: expected no scheduled action before a cutover, got {:?}",
            status["ClonedBlueGreenUpgradeDetails"]
        );
    }

    // An upgrade that asks for no deployment strategy clones nothing, so its change reports
    // no such details and `UpdateClone` has nothing to manage on that domain.
    let plain = "plain-upgrade-domain";
    settled_domain(&emulator, plain).await;
    let started = upgrade(&emulator, plain, None).await;
    assert_eq!(
        started.status, 200,
        "case UpgradeDomain({plain}, no strategy): expected 200, got {} with {:?}",
        started.status, started.body
    );
    assert!(
        started.body.get("DeploymentStrategyOptions").is_none(),
        "case UpgradeDomain({plain}, no strategy): expected no strategy echoed, got {:?}",
        started.body
    );
    let change_id = started.body["ChangeProgressDetails"]["ChangeId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let status = progress(&emulator, plain, &change_id).await;
    assert!(
        status.get("ClonedBlueGreenUpgradeDetails").is_none(),
        "case UpgradeDomain({plain}, no strategy): expected no cloned blue/green details, got \
         {status:?}"
    );
}

/// RULE: `UpdateClone` shifting read traffic reports the shift as `InProgress` with the split
/// still where it was and the share the request asked for as the target, and the clock moves
/// the split to that share — which is what the progress read then reports.
///
/// The rows are the shares a request can ask for: both published bounds of `TrafficPercent`
/// (`@range(0, 100)`, from `smithy_query(query="shape",shape="TrafficPercent")`) and two
/// shares between them.
#[tokio::test]
async fn a_traffic_shift_moves_the_split_the_clock_owes_it() {
    let emulator = Emulator::new();
    let name = "traffic-shift-domain";
    let change_id = given_cloned_upgrade(&emulator, name, Some(12)).await;

    let mut held = 0i64;
    for asked in [25i64, 100, 50, 0] {
        let case = format!("UpdateClone({name}, CloneReadTrafficPercent {asked})");
        let shifted = update_clone(
            &emulator,
            name,
            json!({
                "TrafficSplitOptions": {
                    "Strategy": SHIFT_READ_TRAFFIC_PERCENT,
                    "CloneReadTrafficPercent": asked,
                },
            }),
        )
        .await;
        assert_eq!(
            shifted.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            shifted.status, shifted.body
        );
        let split = &shifted.body["TrafficSplitOptions"];
        assert_eq!(
            (
                shifted.body["DomainName"].as_str(),
                split["Current"]["CloneReadTrafficPercent"].as_i64(),
                split["Target"]["CloneReadTrafficPercent"].as_i64(),
                split["Target"]["PrimaryReadTrafficPercent"].as_i64(),
                split["Status"].as_str(),
            ),
            (
                Some(name),
                Some(held),
                Some(asked),
                Some(100 - asked),
                Some(IN_PROGRESS)
            ),
            "case {case}: expected the shift in flight from {held} to {asked}, got {:?}",
            shifted.body
        );
        assert_eq!(
            shifted.body["ChangeId"].as_str().map(str::len),
            Some(36),
            "case {case}: expected the change it recorded to be named, got {:?}",
            shifted.body
        );

        // The clock owes the shift: once it falls due the split a read reports is the share
        // the request asked for.
        poll_until(&case, || async {
            reported_split(&progress(&emulator, name, &change_id).await) == (100 - asked, asked)
        })
        .await;

        // Asking for the share now in force moves nothing, so the shift it reports is
        // already complete and the split stays where it is.
        let repeated = update_clone(
            &emulator,
            name,
            json!({
                "TrafficSplitOptions": {
                    "Strategy": SHIFT_READ_TRAFFIC_PERCENT,
                    "CloneReadTrafficPercent": asked,
                },
            }),
        )
        .await;
        assert_eq!(
            (
                repeated.status,
                repeated.body["TrafficSplitOptions"]["Status"].as_str(),
                repeated.body["TrafficSplitOptions"]["Current"]["CloneReadTrafficPercent"]
                    .as_i64(),
            ),
            (200, Some(COMPLETED), Some(asked)),
            "case {case} repeated once it had landed: expected {COMPLETED} and the split in \
             force, got {} with {:?}",
            repeated.status,
            repeated.body
        );
        held = asked;
    }
}

/// RULE: a cutover carries ALL read traffic to the cluster it cuts over to, and a cutover to
/// the cloned cluster starts the retention period for the cluster the domain cut over from —
/// which is when the service schedules the action it takes if the caller does not act.
#[tokio::test]
async fn a_cutover_carries_the_read_traffic_and_schedules_the_retention_action() {
    let emulator = Emulator::new();
    let name = "cutover-domain";
    let retention_in_hours = 36i64;
    let change_id = given_cloned_upgrade(&emulator, name, Some(retention_in_hours)).await;

    let cut = update_clone(
        &emulator,
        name,
        json!({ "CutoverOptions": { "Target": CLONED_CLUSTER } }),
    )
    .await;
    assert_eq!(
        (
            cut.status,
            cut.body["CutoverOptions"]["Target"].as_str(),
            cut.body["CutoverOptions"]["Status"].as_str(),
            cut.body["RetentionOptions"]["RetentionInHours"].as_i64(),
        ),
        (
            200,
            Some(CLONED_CLUSTER),
            Some(IN_PROGRESS),
            Some(retention_in_hours)
        ),
        "case UpdateClone({name}, cutover to the clone): got {} with {:?}",
        cut.status,
        cut.body
    );

    poll_until(&format!("the cutover of {name} falling due"), || async {
        reported_split(&progress(&emulator, name, &change_id).await) == (0, 100)
    })
    .await;
    let status = progress(&emulator, name, &change_id).await;
    let scheduled = &status["ClonedBlueGreenUpgradeDetails"]["ScheduledAction"];
    assert_eq!(
        scheduled["Action"].as_str(),
        Some(DELETE_PRIMARY_CLUSTER),
        "case the action scheduled after the cutover: got {:?}",
        status["ClonedBlueGreenUpgradeDetails"]
    );
    assert!(
        scheduled["ScheduledTime"].as_f64().is_some_and(|at| at > 0.0),
        "case the action scheduled after the cutover: expected the instant it is taken, got {:?}",
        status["ClonedBlueGreenUpgradeDetails"]
    );

    // Ending the retention period with an action ends the upgrade with it, and the action
    // decides which cluster serves the domain: a rollback puts the read traffic back on the
    // cluster the domain started on.
    let ended = update_clone(
        &emulator,
        name,
        json!({ "RetentionOptions": { "RetentionAction": ROLLBACK_TO_PRIMARY_CLUSTER } }),
    )
    .await;
    assert_eq!(
        (
            ended.status,
            ended.body["RetentionOptions"]["RetentionAction"].as_str(),
            ended.body["CutoverOptions"]["Target"].as_str(),
        ),
        (200, Some(ROLLBACK_TO_PRIMARY_CLUSTER), Some(PRIMARY_CLUSTER)),
        "case UpdateClone({name}, ending the retention period): got {} with {:?}",
        ended.status,
        ended.body
    );
    poll_until(&format!("the rollback of {name} falling due"), || async {
        reported_split(&progress(&emulator, name, &change_id).await) == (100, 0)
    })
    .await;
    let status = progress(&emulator, name, &change_id).await;
    assert!(
        status["ClonedBlueGreenUpgradeDetails"]
            .get("ScheduledAction")
            .is_none()
            && status["ClonedBlueGreenUpgradeDetails"]
                .get("RetentionInHours")
                .is_none(),
        "case {name} after the retention period ended: expected neither a period nor an action \
         scheduled, got {:?}",
        status["ClonedBlueGreenUpgradeDetails"]
    );

    // The upgrade is over, so there is nothing left for `UpdateClone` to manage.
    let afterwards = update_clone(
        &emulator,
        name,
        json!({ "CutoverOptions": { "Target": CLONED_CLUSTER } }),
    )
    .await;
    assert_eq!(
        (afterwards.status, error_code(&afterwards).as_str()),
        (400, BASE),
        "case UpdateClone({name}) after the upgrade concluded: expected 400 {BASE}, got {} with \
         {:?}",
        afterwards.status,
        afterwards.body
    );
}

/// RULE: `UpdateClone` manages exactly one of the three option blocks per request, on a
/// domain that HAS a cloned blue/green upgrade in progress — and each refusal is the class
/// for what is wrong: the request itself, or the domain it was addressed to.
#[tokio::test]
async fn update_clone_refuses_what_it_cannot_manage() {
    let emulator = Emulator::new();
    let managed = "managed-clone-domain";
    given_cloned_upgrade(&emulator, managed, Some(6)).await;

    let traffic = json!({
        "Strategy": SHIFT_READ_TRAFFIC_PERCENT,
        "CloneReadTrafficPercent": 10,
    });
    // (case, the body, the status, the code)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        ("no option at all", json!({}), 400, VALIDATION),
        (
            "a traffic shift and a cutover",
            json!({
                "TrafficSplitOptions": traffic,
                "CutoverOptions": { "Target": CLONED_CLUSTER },
            }),
            400,
            VALIDATION,
        ),
        (
            "a cutover and a retention period",
            json!({
                "CutoverOptions": { "Target": PRIMARY_CLUSTER },
                "RetentionOptions": { "RetentionInHours": 2 },
            }),
            400,
            VALIDATION,
        ),
        (
            "all three",
            json!({
                "TrafficSplitOptions": traffic,
                "CutoverOptions": { "Target": CLONED_CLUSTER },
                "RetentionOptions": { "RetentionAction": DELETE_PRIMARY_CLUSTER },
            }),
            400,
            VALIDATION,
        ),
    ];
    for (case, body, status, code) in rows {
        let answer = update_clone(&emulator, managed, body).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (status, code),
            "case UpdateClone({case}): expected {status} {code}, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(
                "Specify exactly one of TrafficSplitOptions, CutoverOptions, or RetentionOptions"
            ),
            "case UpdateClone({case}): got {:?}",
            answer.body
        );
    }

    // A domain this account holds none of is a missing resource; a domain that holds no such
    // upgrade — never upgraded, or upgraded without the strategy — is the generic envelope,
    // because the DOMAIN is there.
    let plain = "unmanaged-clone-domain";
    settled_domain(&emulator, plain).await;
    let fresh = "never-upgraded-domain";
    settled_domain(&emulator, fresh).await;
    let upgraded = upgrade(&emulator, plain, None).await;
    assert_eq!(
        upgraded.status, 200,
        "case UpgradeDomain({plain}, no strategy): expected 200, got {} with {:?}",
        upgraded.status, upgraded.body
    );
    // (case, the domain addressed, the status, the code, the message)
    let rows: Vec<(&str, &str, u16, &str, String)> = vec![
        (
            "a domain nobody created",
            "absent-clone-domain",
            NOT_FOUND_STATUS,
            NOT_FOUND,
            "Domain not found: absent-clone-domain".to_string(),
        ),
        (
            "a domain no upgrade has been performed on",
            fresh,
            400,
            BASE,
            format!("No cloned blue/green upgrade is in progress on domain: {fresh}"),
        ),
        (
            "a domain upgraded without the strategy",
            plain,
            400,
            BASE,
            format!("No cloned blue/green upgrade is in progress on domain: {plain}"),
        ),
    ];
    for (case, domain, status, code, message) in rows {
        let answer = update_clone(
            &emulator,
            domain,
            json!({ "CutoverOptions": { "Target": CLONED_CLUSTER } }),
        )
        .await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (status, code),
            "case UpdateClone({case}): expected {status} {code}, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(message.as_str()),
            "case UpdateClone({case}): expected {message:?}, got {:?}",
            answer.body
        );
    }
}

/// RULE: "when you start the upgrade, only `RetentionInHours` is accepted" — an
/// `UpgradeDomain` that asks to start a cloned blue/green upgrade with a retention ACTION is
/// refused, and it starts nothing.
///
/// The rows are the two values `RetentionAction` declares, neither of which a start accepts.
#[tokio::test]
async fn starting_a_cloned_upgrade_accepts_only_a_retention_period() {
    let emulator = Emulator::new();

    for (index, action) in [ROLLBACK_TO_PRIMARY_CLUSTER, DELETE_PRIMARY_CLUSTER]
        .into_iter()
        .enumerate()
    {
        let name = format!("retention-action-start-{index}");
        settled_domain(&emulator, &name).await;
        let refused = upgrade(
            &emulator,
            &name,
            Some(json!({
                "DeploymentStrategy": CLONED_BLUE_GREEN,
                "ClonedBlueGreenOptions": { "RetentionOptions": { "RetentionAction": action } },
            })),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, VALIDATION),
            "case UpgradeDomain({name}, RetentionAction {action} at the start): expected 400 \
             {VALIDATION}, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(
                "Only RetentionInHours is accepted in ClonedBlueGreenOptions when you start the \
                 upgrade"
            ),
            "case UpgradeDomain({name}, RetentionAction {action} at the start): got {:?}",
            refused.body
        );
        // Nothing was started: there is no upgrade for `UpdateClone` to manage.
        let unmanaged = update_clone(
            &emulator,
            &name,
            json!({ "CutoverOptions": { "Target": CLONED_CLUSTER } }),
        )
        .await;
        assert_eq!(
            (unmanaged.status, error_code(&unmanaged).as_str()),
            (400, BASE),
            "case UpdateClone({name}) after the refused start: expected 400 {BASE}, got {} with \
             {:?}",
            unmanaged.status,
            unmanaged.body
        );
    }
}
