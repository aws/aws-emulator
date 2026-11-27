// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the way SEVERAL `UpdateLaunchConfigurationTemplate` calls
//! on one template combine, and for the read that reports the result
//! (`DescribeLaunchConfigurationTemplates`).
//!
//! The single-update rules (a supplied setting replaces the stored one, an absent one is
//! left alone, an unknown id is refused, a value outside a modelled enumeration is
//! refused) are pinned in `tests/all/launch_configuration_template_update.rs`; this
//! suite pins what that leaves open — how a SEQUENCE of updates, each supplying a
//! different part of the template, adds up:
//!
//! 1. Updates that supply disjoint sets of settings MERGE: after a run of them the
//!    template reports every setting any of them wrote, each with the value its own
//!    update carried, and nothing else — so no update discards a setting it never
//!    mentioned.
//! 2. That merge does not depend on the order the disjoint updates arrive in: the same
//!    groups applied back to front leave the same template, member for member, beside
//!    the id and ARN the service assigned each one.
//! 3. Disjoint updates that are in flight at the same time all land: every one answers
//!    200 and the following read reports every setting they wrote.
//!
//! The rows are PARTITIONS of the operation's settable members, so the rule is asserted
//! over the ways one template's settings can be split across calls (nine single-setting
//! updates, halves, thirds, one-and-the-rest, all at once) rather than over one recorded
//! pair of requests.

use serde_json::{json, Map, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::ops::types::{
    LAUNCH_DISPOSITION_VALUES, RECOVERY_MODE_VALUES,
    TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::launch_configuration_template_update::{
    create_template, read_back, reported, text, update, SETTINGS,
};
use crate::replication_configuration_template_lifecycle::drs;

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One value per settable member of `UpdateLaunchConfigurationTemplate`, each inside the
/// member's modelled domain (the enumerated ones from the value sets the model generated:
/// `ops::types::{LAUNCH_DISPOSITION_VALUES, TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES,
/// RECOVERY_MODE_VALUES}`). The members are the ones
/// `smithy_query(query="operation",operation="UpdateLaunchConfigurationTemplate")` lists
/// beside the id the request addresses the template by.
fn settings() -> Vec<(&'static str, Value)> {
    let values: Vec<(&'static str, Value)> = vec![
        (
            "launchDisposition",
            json!(LAUNCH_DISPOSITION_VALUES[LAUNCH_DISPOSITION_VALUES.len() - 1]),
        ),
        (
            "targetInstanceTypeRightSizingMethod",
            json!(
                TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES
                    [TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES.len() - 1]
            ),
        ),
        (
            "recoveryMode",
            json!(RECOVERY_MODE_VALUES[RECOVERY_MODE_VALUES.len() - 1]),
        ),
        ("copyPrivateIp", json!(true)),
        ("copyTags", json!(false)),
        ("licensing", json!({"osByol": true})),
        (
            "exportBucketArn",
            json!(format!("arn:aws:s3:::drs-exports-{ACCOUNT}")),
        ),
        ("postLaunchEnabled", json!(true)),
        ("launchIntoSourceInstance", json!(false)),
    ];
    assert_eq!(
        values.len(),
        SETTINGS.len(),
        "every settable member of the operation must have a value, got {values:?}"
    );
    values
}

/// The ways the settings of one template can be split across a run of updates: each row
/// is a PARTITION of `settings()`'s indices, so every group is disjoint from every other
/// and the groups together carry every setting exactly once.
fn partitions() -> Vec<(&'static str, Vec<Vec<usize>>)> {
    let count = SETTINGS.len();
    vec![
        (
            "one setting per update",
            (0..count).map(|index| vec![index]).collect(),
        ),
        (
            "two disjoint halves",
            vec![(0..count / 2).collect(), (count / 2..count).collect()],
        ),
        (
            "three disjoint groups",
            vec![
                (0..count / 3).collect(),
                (count / 3..2 * count / 3).collect(),
                (2 * count / 3..count).collect(),
            ],
        ),
        (
            "one setting, then every other",
            vec![vec![0], (1..count).collect()],
        ),
        ("every setting in one update", vec![(0..count).collect()]),
    ]
}

/// The update body that writes the settings `group` names.
fn patch_for(group: &[usize], all: &[(&'static str, Value)]) -> Value {
    let mut patch = Map::new();
    for index in group {
        let (member, value) = &all[*index];
        patch.insert((*member).to_string(), value.clone());
    }
    Value::Object(patch)
}

/// What the rule says the template reports: the record the create left behind (its id,
/// ARN and tag set) carrying exactly the settings written so far.
fn expected_record(created: &Value, written: &Map<String, Value>) -> Value {
    let mut expected = created
        .as_object()
        .expect("the created template is a JSON object")
        .clone();
    for (member, value) in written {
        expected.insert(member.clone(), value.clone());
    }
    Value::Object(expected)
}

/// A reported template without the two members the service assigns per template, so two
/// templates written by the same settings can be compared.
fn without_identity(template: &Value) -> Value {
    let mut stripped = template
        .as_object()
        .expect("a reported template is a JSON object")
        .clone();
    stripped.remove("launchConfigurationTemplateID");
    stripped.remove("arn");
    Value::Object(stripped)
}

/// Apply `groups` in order to a template created with no setting at all, asserting after
/// each update that the answer and a following read report every setting written so far
/// and no other; answer the template as it finally stands.
async fn apply_groups(
    router: &MultiServiceRouter,
    case: &str,
    groups: &[Vec<usize>],
    all: &[(&'static str, Value)],
) -> Value {
    let created = create_template(router, &json!({"tags": {"env": "dr"}})).await;
    let id = text(&created, "launchConfigurationTemplateID");
    let mut written = Map::new();

    for (step, group) in groups.iter().enumerate() {
        let patch = patch_for(group, all);
        let answer = update(router, &id, &patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} step {step}: expected 200, got {answer:?}"
        );

        for (member, value) in patch.as_object().expect("a JSON object patch") {
            written.insert(member.clone(), value.clone());
        }
        let expected = expected_record(&created, &written);
        assert_eq!(
            reported(&answer),
            expected,
            "case {case:?} step {step}: expected the update to report {expected}, got {answer:?}"
        );

        let read = read_back(router, &id).await;
        assert_eq!(
            read, expected,
            "case {case:?} step {step}: expected the read to report {expected}, got {read}"
        );
    }

    read_back(router, &id).await
}

// ---------------------------------------------------------------------------
// Rule 1 — disjoint updates merge
// ---------------------------------------------------------------------------

/// Updates supplying disjoint sets of settings merge: after the run every setting any
/// update wrote is reported with the value that update carried, the settings not yet
/// written are still absent at each step, and the tag set and identity the create left
/// behind survive.
#[tokio::test]
async fn disjoint_updates_merge_into_one_template() {
    let router = drs();
    let all = settings();

    for (case, groups) in partitions() {
        let final_record = apply_groups(&router, case, &groups, &all).await;

        for (member, value) in &all {
            assert_eq!(
                final_record.get(*member),
                Some(value),
                "case {case:?}: expected {member} to report {value}, got {final_record}"
            );
        }
        assert_eq!(
            final_record.get("tags"),
            Some(&json!({"env": "dr"})),
            "case {case:?}: no update carries tags, so the created set must survive, \
             got {final_record}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — the merge does not depend on the order
// ---------------------------------------------------------------------------

/// The same disjoint groups applied back to front leave the same template: what a read
/// reports is a function of the settings in force, not of the order the updates arrived
/// in.
#[tokio::test]
async fn the_order_of_disjoint_updates_does_not_change_the_template() {
    let router = drs();
    let all = settings();

    for (case, groups) in partitions() {
        let forwards = apply_groups(&router, case, &groups, &all).await;

        let mut reversed = groups.clone();
        reversed.reverse();
        let backwards = apply_groups(&router, case, &reversed, &all).await;

        assert_eq!(
            without_identity(&backwards),
            without_identity(&forwards),
            "case {case:?}: the reversed run left {backwards}, the forward run {forwards}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — disjoint updates in flight at the same time
// ---------------------------------------------------------------------------

/// Disjoint updates that are in flight at the same time all land: each answers 200, and
/// the read that follows them reports every setting they wrote.
#[tokio::test]
async fn disjoint_updates_in_flight_together_all_land() {
    let router = drs();
    let all = settings();
    let created = create_template(&router, &json!({})).await;
    let id = text(&created, "launchConfigurationTemplateID");

    // Three disjoint thirds of the settings, each written by its own in-flight call.
    let third = SETTINGS.len() / 3;
    let first = patch_for(&(0..third).collect::<Vec<_>>(), &all);
    let second = patch_for(&(third..2 * third).collect::<Vec<_>>(), &all);
    let last = patch_for(&(2 * third..SETTINGS.len()).collect::<Vec<_>>(), &all);

    let (a, b, c) = tokio::join!(
        update(&router, &id, &first),
        update(&router, &id, &second),
        update(&router, &id, &last),
    );
    for (case, answer) in [("first third", &a), ("second third", &b), ("last third", &c)] {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
    }

    let read = read_back(&router, &id).await;
    for (member, value) in &all {
        assert_eq!(
            read.get(*member),
            Some(value),
            "expected {member} to report {value} after the concurrent updates, got {read}"
        );
    }
    assert_eq!(
        text(&read, "launchConfigurationTemplateID"),
        id,
        "the concurrent updates must not move the template, got {read}"
    );
    assert_eq!(
        text(&read, "arn"),
        text(&created, "arn"),
        "the concurrent updates must not change the ARN, got {read}"
    );
}
