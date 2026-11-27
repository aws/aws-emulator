// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACCOUNT-LEVEL RETENTION POLICIES: the `RETENTION_POLICY` type of
//! `PutAccountPolicy` / `DeleteAccountPolicy`, the retention `DescribeLogGroups` then reports
//! for the log groups one applies to, and the `dryRun` preview of a change to them.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a log group's retention is the one stated by the account-level
//! retention policy that applies to it — "an account-level retention policy overrides the
//! retention setting of the log group", and "when more than one retention policy applies to a
//! log group, the policy with the longest retention period is used" — reported beside the name
//! of that policy, and the group's own setting is what it is kept for only while no policy
//! applies to it. Any single retention an issue happens to mention is one row of the tables
//! below, which span the documented retentions, both enforcement modes, the selection criteria
//! that scope a policy, and the three directions a change can move a group's retention in.
//!
//! The suites next door cover the rest of the surface: `tests/all/account_policies.rs` the
//! put / describe / delete round trip every policy type shares, and
//! `tests/all/retention_policy.rs` the retention a client sets on ONE log group with
//! `PutRetentionPolicy`.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    ComAmazonawsCloudwatchlogsLogs20140328State, ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS,
};

use crate::harness::{batch, bring_metering_due, now_ms, Logs};

/// The `policyType` of the policies this suite is about.
const POLICY_TYPE: &str = "RETENTION_POLICY";

/// Milliseconds in one day, the unit `retentionInDays` is expressed in.
const DAY: i64 = 24 * 60 * 60 * 1_000;

/// How far clear of a bound a case that is not testing the bound itself is placed, so no
/// case is decided by how long a request took.
const MARGIN: i64 = 5_000;

/// Every retention an account-level retention policy's document may state. Source:
/// `PutAccountPolicy$policyDocument` — "The number of days to retain log events. Valid values
/// are 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, and
/// 3653."
const ACCOUNT_RETENTIONS: [i64; 17] = [
    30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653,
];

/// What a document states instead of a number of days for log events that are kept for good.
/// Source: `PutAccountPolicy$policyDocument` — "To retain log events indefinitely, specify the
/// string NEVER_EXPIRE."
const NEVER_EXPIRE: &str = "NEVER_EXPIRE";

/// The enforcement modes a document may state. Source: `PutAccountPolicy$policyDocument` —
/// "Specifies which log groups the policy applies to. Valid values are NEW_ONLY and
/// RETROACTIVE."
const ENFORCEMENT_MODES: [&str; 2] = ["NEW_ONLY", "RETROACTIVE"];

/// Retentions a log group can be set on its own with `PutRetentionPolicy`, spanning values
/// shorter than, equal to and longer than the ones a policy states. Source: the model's `Days`
/// shape — "Possible values are: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731,
/// 1096, 1827, 2192, 2557, 2922, 3288, and 3653."
const GROUP_RETENTIONS: [i64; 4] = [1, 90, 400, 3653];

/// Log group names spanning the forms `LogGroupName`'s pattern (`^[\.\-_/#A-Za-z0-9]+$`)
/// admits: a path, a dotted name, a single character, and one carrying every punctuation class
/// the pattern allows.
const NAME_FORMS: [&str; 4] = [
    "/aws/lambda/retention-under-test",
    "service.api.Logs",
    "g",
    "mixed-1_2.3/4#5",
];

/// How many account-level retention policies the account may hold. Source:
/// `PutAccountPolicy` — "Each account can have up to 20 account-level retention policies in
/// each Region."
const MAX_RETENTION_POLICIES: usize = 20;

/// A retention policy document stating `retention` — a number of days or [`NEVER_EXPIRE`] —
/// and the enforcement mode `mode`.
fn document(retention: &Value, mode: &str) -> String {
    json!({"retentionInDays": retention, "enforcementMode": mode}).to_string()
}

/// A request body for one `PutAccountPolicy` of a retention policy.
fn put_body(
    name: &str,
    document: &str,
    criteria: Option<&str>,
    dry_run: bool,
) -> Value {
    let mut body = json!({
        "policyName": name,
        "policyType": POLICY_TYPE,
        "policyDocument": document,
    });
    if let Some(criteria) = criteria {
        body["selectionCriteria"] = json!(criteria);
    }
    if dry_run {
        body["dryRun"] = json!(true);
    }
    body
}

/// A request body for one `DeleteAccountPolicy` of a retention policy.
fn delete_body(name: &str, dry_run: bool) -> Value {
    let mut body = json!({"policyName": name, "policyType": POLICY_TYPE});
    if dry_run {
        body["dryRun"] = json!(true);
    }
    body
}

/// Put one retention policy, which must be accepted, and answer the response.
async fn put(logs: &Logs, name: &str, retention: &Value, mode: &str, criteria: Option<&str>) -> Value {
    logs.ok(
        "PutAccountPolicy",
        &put_body(name, &document(retention, mode), criteria, false),
    )
    .await
}

/// Create one log group, and set the retention it holds of its own when the row gives one.
async fn create_group(logs: &Logs, name: &str, own_retention: Option<i64>) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name})).await;
    if let Some(days) = own_retention {
        logs.ok(
            "PutRetentionPolicy",
            &json!({"logGroupName": name, "retentionInDays": days}),
        )
        .await;
    }
}

/// The `retentionInDays` and `retentionPolicyName` `DescribeLogGroups` reports for one log
/// group — the pair the rule under test is about.
async fn reported_retention(logs: &Logs, name: &str) -> (Option<i64>, Option<String>) {
    let group = logs
        .describe_group(name)
        .await
        .unwrap_or_else(|| panic!("case {name}: the log group must exist"));
    (
        group.get("retentionInDays").and_then(Value::as_i64),
        group
            .get("retentionPolicyName")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// Bring the retention every account-level retention policy the account holds states DUE, so
/// the next read reports what those policies put on the log groups they select.
///
/// "A retention policy takes effect [a span] after you create it. If you update a policy, the
/// new retention period takes effect [a span] after the update, and the previous retention
/// period stays in effect until then." A suite whose subject is WHAT a policy does moves the
/// clock that switch hangs off rather than waiting for it — each policy's effective instant is
/// pushed the whole span into the past — and the next call over the wire resolves the retention
/// exactly as it would for any client. That the switch is owed at all is asserted against the
/// clock in [`a_put_leaves_the_retention_in_effect_until_the_new_one_falls_due`].
fn bring_retention_effect_due(logs: &Logs) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    for policy in store.account_policies.values_mut() {
        if let Some(effect) = policy.retention.as_mut() {
            effect.effective_at_ms = effect
                .effective_at_ms
                .saturating_sub(ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS);
        }
    }
}

/// Move one stored retention policy's clock back by `millis`: the instant it was created,
/// which a `NEW_ONLY` enforcement mode and the tie between two equal retentions are measured
/// against, and the instant the retention it states takes effect.
///
/// A case about which of two instants came first cannot be left to how fast the host runs two
/// calls, so the suite states the order in the store and asserts what a client then reads.
fn backdate_policy(logs: &Logs, name: &str, millis: i64) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let policy = store
        .account_policies
        .get_mut(&(POLICY_TYPE.to_string(), name.to_string()))
        .unwrap_or_else(|| panic!("the policy {name} is stored"));
    policy.created_time_ms = policy.created_time_ms.saturating_sub(millis);
    if let Some(effect) = policy.retention.as_mut() {
        effect.effective_at_ms = effect.effective_at_ms.saturating_sub(millis);
    }
}

/// Move one log group's creation instant back by `millis`, so a case about a group that
/// already existed when a policy was created does not depend on how fast the host ran the two
/// calls. See [`backdate_policy`].
fn backdate_group(logs: &Logs, name: &str, millis: i64) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let group = store
        .log_groups
        .get_mut(name)
        .unwrap_or_else(|| panic!("the log group {name} is stored"));
    group.creation_time_ms = group.creation_time_ms.saturating_sub(millis);
}

/// The policies `DescribeAccountPolicies` reports of this type, by name.
async fn held_names(logs: &Logs) -> Vec<String> {
    let page = logs
        .ok("DescribeAccountPolicies", &json!({"policyType": POLICY_TYPE}))
        .await;
    page.get("accountPolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeAccountPolicies must return accountPolicies, got {page}"))
        .iter()
        .filter_map(|policy| policy.get("policyName").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The counts one `dryRunResult` reports, in the order the members are read below: the log
/// groups matched, those whose retention would grow, shrink and stay, and those that could not
/// be analysed.
type DryRunCounts = (i64, i64, i64, i64, i64);

/// The `dryRunResult` of one response, as the counts it reports.
fn dry_run_counts(case: &str, response: &Value) -> DryRunCounts {
    let result = response
        .get("dryRunResult")
        .unwrap_or_else(|| panic!("case {case}: expected a dryRunResult, got {response}"));
    let count = |member: &str| {
        result
            .get(member)
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case}: a dryRunResult carries {member}, got {result}"))
    };
    (
        count("totalMatchedLogGroups"),
        count("increasedRetentionLogGroupCount"),
        count("decreasedRetentionLogGroupCount"),
        count("unchangedRetentionLogGroupCount"),
        count("failedAnalysisCount"),
    )
}

/// The `topLogGroupsImpacted` of one response as `(logGroupName, currentRetention,
/// proposedRetention)` triples, in the order reported.
fn impacted(case: &str, response: &Value) -> Vec<(String, String, String)> {
    let result = response
        .get("dryRunResult")
        .unwrap_or_else(|| panic!("case {case}: expected a dryRunResult, got {response}"));
    result
        .get("topLogGroupsImpacted")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: a dryRunResult carries topLogGroupsImpacted, got {result}"))
        .iter()
        .map(|impact| {
            let member = |name: &str| {
                impact
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: an impact carries {name}, got {impact}"))
                    .to_string()
            };
            (
                member("logGroupName"),
                member("currentRetention"),
                member("proposedRetention"),
            )
        })
        .collect()
}

/// Every retention a policy's document may state, and both enforcement modes, replace the
/// retention set on the log groups the policy applies to — including a group whose own
/// retention is LONGER than the policy's ("An account-level retention policy replaces the
/// retention period set on a log group, including when the log group's own retention period is
/// longer") — and the group reports the policy that decided it.
#[tokio::test]
async fn every_documented_account_retention_replaces_the_group_setting() {
    // The documented catalogue, plus the string that stands for a retention that never
    // expires, crossed with the retentions a group may hold of its own and the forms a log
    // group name takes: neither the value, nor the group's own setting, nor the name shape can
    // be what makes a case pass.
    let mut rows: Vec<(Value, Option<i64>)> = ACCOUNT_RETENTIONS
        .iter()
        .enumerate()
        .map(|(index, days)| {
            let own = match index % 5 {
                0 => None,
                other => Some(GROUP_RETENTIONS[other - 1]),
            };
            (json!(days), own)
        })
        .collect();
    rows.push((json!(NEVER_EXPIRE), None));
    rows.push((json!(NEVER_EXPIRE), Some(GROUP_RETENTIONS[0])));
    rows.push((json!(NEVER_EXPIRE), Some(GROUP_RETENTIONS[3])));

    for (index, (retention, own)) in rows.iter().enumerate() {
        let mode = ENFORCEMENT_MODES[index % ENFORCEMENT_MODES.len()];
        let case = format!("retention {retention} mode {mode} own {own:?}");
        let logs = Logs::new();
        let group = format!("{}/case-{index}", NAME_FORMS[index % NAME_FORMS.len()]);
        let name = format!("retain-{index}");

        create_group(&logs, &group, *own).await;
        assert_eq!(
            reported_retention(&logs, &group).await,
            (*own, None),
            "case {case}: before any policy a group reports its own retention and no policy name"
        );

        let put_at = now_ms();
        let stored = put(&logs, &name, retention, mode, None).await;
        assert_eq!(
            stored
                .get("accountPolicy")
                .and_then(|policy| policy.get("policyDocument"))
                .and_then(Value::as_str),
            Some(document(retention, mode).as_str()),
            "case {case}: a put that is not a dry run reports the policy it stored, got {stored}"
        );
        assert_eq!(
            stored.get("dryRunResult"),
            None,
            "case {case}: a put that is not a dry run reports no dryRunResult, got {stored}"
        );

        // What the policy states takes effect a span after the put, so a read the clock has
        // not reached that instant by is still answered the retention in effect before it.
        let (reported, policy_name) = reported_retention(&logs, &group).await;
        if now_ms() < put_at + ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS {
            assert_eq!(
                (reported, policy_name),
                (*own, None),
                "case {case}: until the policy takes effect the group reports its own retention"
            );
        }

        // A `NEW_ONLY` policy reaches "only log groups that are created after you create the
        // policy", so for that mode the store states that this policy is the older of the
        // two; which groups each mode reaches is the subject of the case below.
        if mode == "NEW_ONLY" {
            backdate_policy(&logs, &name, DAY);
        }
        bring_retention_effect_due(&logs);
        let expected = retention.as_i64();
        assert_eq!(
            reported_retention(&logs, &group).await,
            (expected, Some(name.clone())),
            "case {case}: expected the policy's retention and its name once it is in effect"
        );

        // The same policy is what `DescribeAccountPolicies` reports, so what decided the
        // group's retention is a policy the account holds rather than a figure this read made
        // up.
        assert_eq!(
            held_names(&logs).await,
            vec![name.clone()],
            "case {case}: the account holds exactly the policy that was put"
        );
    }
}

/// Of the policies that apply to one log group, the one stating the longest retention decides
/// it, `NEVER_EXPIRE` being the longest of all; between two stating the same retention the one
/// created first is the one reported. A group no policy selects keeps its own retention and
/// reports no policy name.
#[tokio::test]
async fn the_longest_retention_of_the_applying_policies_decides() {
    let logs = Logs::new();
    // Groups spanning the prefixes the policies below select on, each with a retention of its
    // own so an unselected group has something to fall back to.
    let groups: [(&str, Option<i64>); 4] = [
        ("/team/app", Some(1)),
        ("/team/audit/trail", Some(3653)),
        ("/team/tie", None),
        ("/other/app", Some(90)),
    ];
    for (group, own) in groups {
        create_group(&logs, group, own).await;
    }

    // An account-wide baseline, a longer policy over one prefix, a never-expiring one over a
    // narrower prefix inside it, and two policies stating the SAME retention over the same
    // prefix — which the rule for "retention policies can have overlapping selection criteria"
    // admits.
    let policies: [(&str, Value, Option<&str>); 5] = [
        ("baseline", json!(30), None),
        ("team", json!(365), Some(r#"LogGroupNamePrefix IN ["/team"]"#)),
        (
            "audit",
            json!(NEVER_EXPIRE),
            Some(r#"LogGroupNamePrefix IN ["/team/audit"]"#),
        ),
        (
            "tie-later",
            json!(365),
            Some(r#"LogGroupNamePrefix IN ["/team/tie"]"#),
        ),
        (
            "tie-earlier",
            json!(365),
            Some(r#"LogGroupNamePrefix = "/team/tie""#),
        ),
    ];
    for (name, retention, criteria) in &policies {
        put(&logs, name, retention, "RETROACTIVE", *criteria).await;
    }
    // The tie is decided by which policy was created first, which the store states rather than
    // the order two calls happened to land in.
    backdate_policy(&logs, "tie-earlier", DAY);
    bring_retention_effect_due(&logs);

    // (the group, the retention it is left with, the policy reported for it)
    let expected: [(&str, Option<i64>, Option<&str>); 4] = [
        // Two policies apply: the baseline and the longer one over its prefix.
        ("/team/app", Some(365), Some("team")),
        // Three apply, and the one that never expires is the longest of them.
        ("/team/audit/trail", None, Some("audit")),
        // Three apply and two of them state the same longest retention: the earlier one.
        ("/team/tie", Some(365), Some("tie-earlier")),
        // Only the account-wide one applies, and it is shorter than the group's own setting.
        ("/other/app", Some(30), Some("baseline")),
    ];
    for (group, retention, policy_name) in expected {
        assert_eq!(
            reported_retention(&logs, group).await,
            (retention, policy_name.map(str::to_string)),
            "case {group}: expected the longest applying policy to decide the retention"
        );
    }

    // A group outside every scoped prefix once the account-wide policy is gone keeps its own
    // retention: "the log groups that it applied to use their own retention setting, unless
    // another retention policy applies to them."
    logs.ok("DeleteAccountPolicy", &delete_body("baseline", false))
        .await;
    assert_eq!(
        reported_retention(&logs, "/other/app").await,
        (Some(90), None),
        "case /other/app: a group no policy selects reports its own retention and no policy name"
    );
    assert_eq!(
        reported_retention(&logs, "/team/app").await,
        (Some(365), Some("team".to_string())),
        "case /team/app: the policies that still apply are unaffected by the delete"
    );
}

/// A `NEW_ONLY` policy "applies the policy only to log groups that are created after you
/// create the policy. Log groups that already existed when you created the policy keep their
/// own retention period", while a `RETROACTIVE` one applies "to every log group that matches
/// the selection criteria, including log groups that existed before you created the policy".
#[tokio::test]
async fn enforcement_mode_decides_which_existing_log_groups_a_policy_reaches() {
    for mode in ENFORCEMENT_MODES {
        let logs = Logs::new();
        let existed = "/app/existed";
        let created_later = "/app/created-later";
        create_group(&logs, existed, Some(90)).await;

        put(&logs, "retain", &json!(365), mode, None).await;
        create_group(&logs, created_later, Some(90)).await;

        // The policy is a day old and the group that already existed two days old, so which
        // of them came first is stated rather than raced.
        backdate_policy(&logs, "retain", DAY);
        backdate_group(&logs, existed, 2 * DAY);

        let reaches_existing = mode == "RETROACTIVE";
        let expected_existing = if reaches_existing {
            (Some(365), Some("retain".to_string()))
        } else {
            (Some(90), None)
        };
        assert_eq!(
            reported_retention(&logs, existed).await,
            expected_existing,
            "case {mode}: expected {expected_existing:?} for a log group that already existed"
        );
        assert_eq!(
            reported_retention(&logs, created_later).await,
            (Some(365), Some("retain".to_string())),
            "case {mode}: a log group created after the policy is reached whatever the mode"
        );
    }
}

/// A `dryRun` put "validates the request, does not save the policy, and returns the estimated
/// impact in dryRunResult instead of accountPolicy": the counts say how many log groups would
/// have a longer, shorter or unchanged retention, the impacted groups are reported by stored
/// bytes descending, and the account holds no policy afterwards.
#[tokio::test]
async fn a_dry_run_put_reports_the_impact_and_stores_nothing() {
    let logs = Logs::new();
    // One group per direction the change can move a retention in, plus a Delivery-class group
    // the analysis leaves out: "Log groups in the Delivery class are not included."
    let groups: [(&str, Option<i64>); 4] = [
        ("impact-a-short", Some(1)),
        ("impact-b-long", Some(3653)),
        ("impact-c-none", None),
        ("impact-d-same", Some(90)),
    ];
    for (group, own) in groups {
        create_group(&logs, group, own).await;
    }
    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": "impact-e-delivery", "logGroupClass": "DELIVERY"}),
    )
    .await;

    // One group stores more than the others, so the order the impacts are reported in is
    // decided by stored bytes rather than by name: the group given a volume is the LAST of
    // them by name.
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": "impact-d-same", "logStreamName": "stream"}),
    )
    .await;
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": "impact-d-same",
            "logStreamName": "stream",
            "logEvents": batch(&[(now_ms() - MARGIN, "stored volume")]),
        }),
    )
    .await;
    // The volume a group stores is metered on the service's own cycle: bringing the next pass
    // due and then reading the group materialises it, which is what the analysis below reads.
    bring_metering_due(&logs);
    logs.describe_group("impact-d-same").await;

    // (the case, the retention previewed, the expected counts, the expected proposed value)
    let rows: [(&str, Value, DryRunCounts, &str); 2] = [
        // 90 days is longer than one group's retention, shorter than two groups' (a group
        // with no retention of its own never expires, which is the longest there is) and the
        // same as the fourth's.
        ("ninety-days", json!(90), (4, 1, 2, 1, 0), "90"),
        // A policy that never expires is the longest retention there is, so only the group
        // that already never expires is unchanged.
        (
            "never-expire",
            json!(NEVER_EXPIRE),
            (4, 3, 0, 1, 0),
            NEVER_EXPIRE,
        ),
    ];
    for (case, retention, counts, proposed) in &rows {
        let response = logs
            .ok(
                "PutAccountPolicy",
                &put_body(
                    "previewed",
                    &document(retention, "RETROACTIVE"),
                    None,
                    true,
                ),
            )
            .await;
        assert_eq!(
            response.get("accountPolicy"),
            None,
            "case {case}: a dry run reports the impact instead of accountPolicy, got {response}"
        );
        assert_eq!(
            dry_run_counts(case, &response),
            *counts,
            "case {case}: expected (total, increased, decreased, unchanged, failed) {counts:?}"
        );

        let reported = impacted(case, &response);
        // The group that stores a volume first, then the rest in name order, each with the
        // retention it is at now and the one the previewed policy would leave it at.
        let expected: Vec<(String, String, String)> = [
            ("impact-d-same", "90"),
            ("impact-a-short", "1"),
            ("impact-b-long", "3653"),
            ("impact-c-none", NEVER_EXPIRE),
        ]
        .iter()
        .map(|(group, current)| {
            (
                (*group).to_string(),
                (*current).to_string(),
                (*proposed).to_string(),
            )
        })
        .collect();
        assert_eq!(
            reported, expected,
            "case {case}: expected the analysed groups by stored bytes then name, each with its current and proposed retention"
        );

        // Nothing was saved: no policy is held, and every group still reports what it did.
        assert!(
            held_names(&logs).await.is_empty(),
            "case {case}: a dry run saves no policy"
        );
        for (group, own) in groups {
            assert_eq!(
                reported_retention(&logs, group).await,
                (own, None),
                "case {case}: a dry run leaves {group} at its own retention"
            );
        }
    }
}

/// A `dryRun` delete "does not delete the policy and returns the estimated impact in
/// dryRunResult"; the delete that is not a dry run then gives each log group back its own
/// retention setting.
#[tokio::test]
async fn a_dry_run_delete_reports_the_impact_and_deletes_nothing() {
    let logs = Logs::new();
    let groups: [(&str, Option<i64>); 4] = [
        ("revert-a-short", Some(1)),
        ("revert-b-long", Some(3653)),
        ("revert-c-none", None),
        ("revert-d-same", Some(90)),
    ];
    for (group, own) in groups {
        create_group(&logs, group, own).await;
    }
    put(&logs, "retain", &json!(90), "RETROACTIVE", None).await;
    bring_retention_effect_due(&logs);
    for (group, _) in groups {
        assert_eq!(
            reported_retention(&logs, group).await.0,
            Some(90),
            "case {group}: the policy under test is in effect before the preview"
        );
    }

    let response = logs
        .ok("DeleteAccountPolicy", &delete_body("retain", true))
        .await;
    // Taking the policy away moves each group back to its own setting: one longer, two
    // shorter (a group with no retention of its own never expires) and one unchanged.
    assert_eq!(
        dry_run_counts("delete", &response),
        (4, 2, 1, 1, 0),
        "case delete: expected (total, increased, decreased, unchanged, failed) (4, 2, 1, 1, 0)"
    );
    assert_eq!(
        impacted("delete", &response),
        vec![
            ("revert-a-short".to_string(), "90".to_string(), "1".to_string()),
            (
                "revert-b-long".to_string(),
                "90".to_string(),
                "3653".to_string()
            ),
            (
                "revert-c-none".to_string(),
                "90".to_string(),
                NEVER_EXPIRE.to_string()
            ),
            ("revert-d-same".to_string(), "90".to_string(), "90".to_string()),
        ],
        "case delete: expected each group's current retention and the one it would revert to"
    );

    // The policy is still there, and still decides every group's retention.
    assert_eq!(
        held_names(&logs).await,
        vec!["retain".to_string()],
        "case delete: a dry run deletes no policy"
    );
    for (group, _) in groups {
        assert_eq!(
            reported_retention(&logs, group).await,
            (Some(90), Some("retain".to_string())),
            "case {group}: a dry run leaves the policy in effect"
        );
    }

    // The delete that is not a dry run does what the preview said it would.
    let deleted = logs
        .ok("DeleteAccountPolicy", &delete_body("retain", false))
        .await;
    assert_eq!(
        deleted.get("dryRunResult"),
        None,
        "case delete: a delete that is not a dry run reports no dryRunResult, got {deleted}"
    );
    for (group, own) in groups {
        assert_eq!(
            reported_retention(&logs, group).await,
            (own, None),
            "case {group}: after the delete the group uses its own retention setting"
        );
    }
}

/// What the retention policies refuse: a document that does not state the two attributes one
/// must include, a retention outside the documented catalogue or below the minimum an
/// account-level policy admits, an enforcement mode outside the two documented ones, a
/// selection criteria filter other than a log group name prefix, and a `dryRun` asked of a
/// policy type that states no retention. A refused put stores nothing.
#[tokio::test]
async fn documents_and_requests_the_rules_do_not_admit_are_refused() {
    let logs = Logs::new();

    // Each row breaks one sentence of `PutAccountPolicy$policyDocument` for this type.
    let refused: Vec<(&str, String)> = vec![
        ("neither-attribute", json!({}).to_string()),
        ("without-the-enforcement-mode", json!({"retentionInDays": 90}).to_string()),
        (
            "without-the-retention",
            json!({"enforcementMode": "RETROACTIVE"}).to_string(),
        ),
        // A day count the model's `Days` shape admits for ONE log group but that is below the
        // minimum an account-level policy states: "Account-level retention policies require a
        // minimum retention period of 30 days."
        ("below-the-minimum", document(&json!(14), "RETROACTIVE")),
        ("the-shortest-group-retention", document(&json!(1), "RETROACTIVE")),
        ("one-day-below-the-minimum", document(&json!(29), "RETROACTIVE")),
        // Outside the catalogue on either side of a published value, and past its ends.
        ("between-two-published-values", document(&json!(100), "RETROACTIVE")),
        ("one-below-a-published-value", document(&json!(364), "RETROACTIVE")),
        ("one-above-a-published-value", document(&json!(366), "RETROACTIVE")),
        ("one-past-the-longest", document(&json!(3654), "RETROACTIVE")),
        ("zero", document(&json!(0), "RETROACTIVE")),
        ("negative", document(&json!(-1), "RETROACTIVE")),
        // The one string the member admits is NEVER_EXPIRE, spelled exactly.
        ("a-misspelled-never-expire", document(&json!("NEVER"), "RETROACTIVE")),
        (
            "a-lower-case-never-expire",
            document(&json!("never_expire"), "RETROACTIVE"),
        ),
        ("a-retention-written-as-a-string", document(&json!("90"), "RETROACTIVE")),
        // The enforcement mode is one of the two documented values, spelled exactly.
        ("a-lower-case-enforcement-mode", document(&json!(90), "retroactive")),
        ("an-unknown-enforcement-mode", document(&json!(90), "ALL")),
        ("an-empty-enforcement-mode", document(&json!(90), "")),
        ("not-json", "this is not a policy".to_string()),
        ("an-array", json!([{"retentionInDays": 90}]).to_string()),
    ];
    for (case, document) in &refused {
        let code = logs
            .err("PutAccountPolicy", &put_body(case, document, None, false))
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: expected InvalidParameterException for {document}, got {code}"
        );
        assert!(
            !held_names(&logs).await.contains(&(*case).to_string()),
            "case {case}: a refused put stores nothing"
        );
    }

    // Every published retention and both modes ARE admitted, so the refusals above are the
    // catalogue and not a blanket rejection. One name throughout, so each put is an update of
    // the same policy rather than one more against the quota.
    let mut admitted: Vec<Value> = ACCOUNT_RETENTIONS.iter().map(|days| json!(days)).collect();
    admitted.push(json!(NEVER_EXPIRE));
    for retention in &admitted {
        for mode in ENFORCEMENT_MODES {
            let stored = put(&logs, "admitted", retention, mode, None).await;
            assert_eq!(
                stored
                    .get("accountPolicy")
                    .and_then(|policy| policy.get("policyDocument"))
                    .and_then(Value::as_str),
                Some(document(retention, mode).as_str()),
                "case retention {retention} mode {mode}: expected the document stored verbatim, got {stored}"
            );
        }
    }

    // "Retention policies support log group name prefixes only": the prefix filter in either
    // form it is written in, and no other field.
    for criteria in [
        r#"LogGroupNamePrefix IN ["/aws/one"]"#,
        r#"LogGroupNamePrefix = "/aws/two""#,
        r#"LogGroupNamePrefix NOT IN ["/aws/three"]"#,
    ] {
        logs.ok(
            "PutAccountPolicy",
            &put_body("admitted", &document(&json!(90), "RETROACTIVE"), Some(criteria), false),
        )
        .await;
    }
    for criteria in [
        r#"LogGroupName IN ["/aws/one"]"#,
        r#"LogGroupName NOT IN ["/aws/one"]"#,
        r#"DataSourceName IN ["amazon_vpc"] AND DataSourceType IN ["flow"]"#,
        "LogGroupNamePrefix",
    ] {
        let code = logs
            .err(
                "PutAccountPolicy",
                &put_body(
                    "criteria-case",
                    &document(&json!(90), "RETROACTIVE"),
                    Some(criteria),
                    false,
                ),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case criteria {criteria}: expected InvalidParameterException, got {code}"
        );
    }

    // "Unlike other account-level policy types, retention policies can have overlapping
    // selection criteria": two of them over prefixes one inside the other are both admitted,
    // where a type whose rule is "no two of them can use the same or overlapping log group
    // name prefixes" refuses the second.
    let outer = r#"LogGroupNamePrefix IN ["/aws/overlap"]"#;
    let inner = r#"LogGroupNamePrefix IN ["/aws/overlap/inside"]"#;
    for (name, criteria) in [("overlap-outer", outer), ("overlap-inner", inner)] {
        logs.ok(
            "PutAccountPolicy",
            &put_body(name, &document(&json!(365), "RETROACTIVE"), Some(criteria), false),
        )
        .await;
    }
    let transformer = |name: &str, criteria: &str| {
        json!({
            "policyName": name,
            "policyType": "TRANSFORMER_POLICY",
            "policyDocument": json!([{"parseJSON": {}}]).to_string(),
            "selectionCriteria": criteria,
        })
    };
    logs.ok("PutAccountPolicy", &transformer("transform-outer", outer))
        .await;
    let code = logs
        .err("PutAccountPolicy", &transformer("transform-inner", inner))
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case a transformer policy over a prefix another of its own type already covers: expected InvalidParameterException, got {code}"
    );

    // "Specifying dryRun is valid only when you specify RETENTION_POLICY for policyType."
    let other_types: [(&str, String); 2] = [
        (
            "TRANSFORMER_POLICY",
            json!([{"parseJSON": {}}]).to_string(),
        ),
        (
            "METRIC_EXTRACTION_POLICY",
            json!({"EmbeddedMetricFormat": {"Status": "Disabled"}}).to_string(),
        ),
    ];
    for (policy_type, policy_document) in &other_types {
        let code = logs
            .err(
                "PutAccountPolicy",
                &json!({
                    "policyName": "dry-run-case",
                    "policyType": policy_type,
                    "policyDocument": policy_document,
                    "dryRun": true,
                }),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case a dry run of a {policy_type}: expected InvalidParameterException, got {code}"
        );
        let code = logs
            .err(
                "DeleteAccountPolicy",
                &json!({"policyName": "dry-run-case", "policyType": policy_type, "dryRun": true}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case a dry run delete of a {policy_type}: expected InvalidParameterException, got {code}"
        );
    }
}

/// "Each account can have up to 20 account-level retention policies in each Region": the
/// twentieth is admitted whether the policies are scoped or not, the twenty-first is over the
/// quota, and deleting one gives its slot back.
#[tokio::test]
async fn the_account_holds_twenty_retention_policies() {
    let logs = Logs::new();
    let body = |index: usize| {
        // Every other policy is scoped, because this quota is one number for the type rather
        // than one for the scoped policies and another for the account-wide one.
        let criteria = (index % 2 == 1).then(|| format!(r#"LogGroupNamePrefix IN ["/aws/{index}"]"#));
        put_body(
            &format!("quota-{index:02}"),
            &document(&json!(ACCOUNT_RETENTIONS[index % ACCOUNT_RETENTIONS.len()]), "RETROACTIVE"),
            criteria.as_deref(),
            false,
        )
    };

    for index in 0..MAX_RETENTION_POLICIES {
        logs.ok("PutAccountPolicy", &body(index)).await;
    }
    let held = held_names(&logs).await;
    assert_eq!(
        held.len(),
        MAX_RETENTION_POLICIES,
        "expected the account to hold {MAX_RETENTION_POLICIES} retention policies, got {held:?}"
    );

    let over = body(MAX_RETENTION_POLICIES);
    let code = logs.err("PutAccountPolicy", &over).await;
    assert_eq!(
        code, "LimitExceededException",
        "case policy {MAX_RETENTION_POLICIES}: expected LimitExceededException, got {code}"
    );

    // A put under a name the account already holds is an update, so it is admitted while the
    // quota is full.
    logs.ok("PutAccountPolicy", &body(0)).await;

    logs.ok("DeleteAccountPolicy", &delete_body("quota-00", false))
        .await;
    logs.ok("PutAccountPolicy", &over).await;
    let held = held_names(&logs).await;
    assert_eq!(
        held.len(),
        MAX_RETENTION_POLICIES,
        "expected the released slot to take the next policy, got {held:?}"
    );
}

/// The retention an account-level policy puts on a log group is the one its INGESTION is held
/// to as well: "Events older than 14 days or preceding the log group's retention period are
/// rejected", and the period that binds is the effective one. An event the group's own short
/// retention expires is ingested once a policy states a longer retention for it.
#[tokio::test]
async fn the_effective_retention_binds_what_put_log_events_admits() {
    // Retentions longer than the group's own one-day setting: the longest the catalogue
    // states, a mid-catalogue value, the shortest an account-level policy may state, and the
    // one that never expires.
    for retention in [
        json!(ACCOUNT_RETENTIONS[16]),
        json!(ACCOUNT_RETENTIONS[6]),
        json!(ACCOUNT_RETENTIONS[0]),
        json!(NEVER_EXPIRE),
    ] {
        let logs = Logs::new();
        let group = "ingest-under-policy";
        let stream = "stream";
        create_group(&logs, group, Some(1)).await;
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": stream}),
        )
        .await;

        // Inside the 14-day ingestion window but before the group's own one-day retention.
        let timestamp = now_ms() - DAY - MARGIN;
        let response = logs
            .ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "logEvents": batch(&[(timestamp, "before the group's own retention")]),
                }),
            )
            .await;
        assert_eq!(
            response.get("rejectedLogEventsInfo"),
            Some(&json!({"expiredLogEventEndIndex": 1})),
            "case {retention}: the group's own retention expires the event before the policy, got {response}"
        );

        put(&logs, "retain", &retention, "RETROACTIVE", None).await;
        bring_retention_effect_due(&logs);

        let response = logs
            .ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "logEvents": batch(&[(timestamp, "inside the policy's retention")]),
                }),
            )
            .await;
        assert_eq!(
            response.get("rejectedLogEventsInfo"),
            None,
            "case {retention}: the policy's longer retention admits the same event, got {response}"
        );
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": group, "logStreamName": stream, "startFromHead": true}),
            )
            .await;
        assert_eq!(
            crate::harness::messages(&page),
            vec!["inside the policy's retention".to_string()],
            "case {retention}: exactly the event the policy admitted is readable"
        );
    }
}

/// The retention a put states is owed by the clock rather than applied the instant the call
/// returns: a read before it falls due reports the retention that was in effect before the
/// put, and one after it reports the new one. The update case is the one the rule is stated
/// for — "the previous retention period stays in effect until then".
#[tokio::test]
async fn a_put_leaves_the_retention_in_effect_until_the_new_one_falls_due() {
    let logs = Logs::new();
    let group = "owed-transition";
    create_group(&logs, group, Some(1)).await;

    // The first put: until it falls due the group is still at its own retention.
    let put_at = now_ms();
    put(&logs, "retain", &json!(30), "RETROACTIVE", None).await;
    let before = reported_retention(&logs, group).await;
    if now_ms() < put_at + ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS {
        assert_eq!(
            before,
            (Some(1), None),
            "case the created policy: until it takes effect the group reports its own retention"
        );
    }
    bring_retention_effect_due(&logs);
    assert_eq!(
        reported_retention(&logs, group).await,
        (Some(30), Some("retain".to_string())),
        "case the created policy: once it is due the group reports what it states"
    );

    // The update: the retention in effect stays in effect until the update falls due.
    let updated_at = now_ms();
    put(&logs, "retain", &json!(365), "RETROACTIVE", None).await;
    let during = reported_retention(&logs, group).await;
    if now_ms() < updated_at + ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS {
        assert_eq!(
            during,
            (Some(30), Some("retain".to_string())),
            "case the updated policy: the previous retention stays in effect until the update is due"
        );
    }
    bring_retention_effect_due(&logs);
    assert_eq!(
        reported_retention(&logs, group).await,
        (Some(365), Some("retain".to_string())),
        "case the updated policy: once the update is due the group reports the new retention"
    );
}
