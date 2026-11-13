// SPDX-License-Identifier: CC0-1.0

//! `UpdateRuleGroup` against stored state, end to end over the wire.
//!
//! The rule asserted here: `UpdateRuleGroup` "completely replaces the mutable
//! specifications that you already have for the rule group with the ones that you
//! provide to this call", so after a successful update `GetRuleGroup` answers
//! EXACTLY what the call described — every member the request carried, a member the
//! request omitted cleared, the server-derived members recomputed from the new
//! rules — with a freshly rotated lock token, while `Name`, `Id`, `ARN`,
//! `LabelNamespace` and the immutable `Capacity` are untouched. A refused update
//! changes nothing at all: not the rules, not the description, not the lock token.
//!
//! "When you create a rule group, you define an immutable capacity limit. If you
//! update a rule group, you must stay within the capacity" — so a rule list costing
//! more WCUs than the group declared is refused, priced by the very report
//! `Capacity`'s doc tells a caller to size against (`CheckCapacity`).
//!
//! Every row is driven through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};

use crate::rule_group_lifecycle::{
    byte_match_statement, create_body, label_match_rule, label_namespace, label_summaries,
    labelled_rule, rule_group_arn,
};
use crate::web_acl_lifecycle::{
    create_ip_set, is_model_uuid, nested_statement, ordered, rule, sqli_statement, visibility,
    xss_statement, Waf, TEST_REGION,
};

/// The `(Name, Scope, Id, LockToken)` a mutating rule-group call addresses: the
/// triple that identifies the group plus the optimistic-lock token it was last read
/// with.
type Addressed<'a> = (&'a str, &'a str, &'a str, &'a str);

/// An `UpdateRuleGroup` body. `Description` and `CustomResponseBodies` are present
/// only when the case supplies them, so an absent member is really absent on the
/// wire.
fn update_body(
    addressed: Addressed<'_>,
    description: Option<&str>,
    rules: &Value,
    custom_response_bodies: Option<&Value>,
    metric: &str,
) -> Value {
    let (name, scope, id, lock_token) = addressed;
    let mut body = json!({
        "Name": name,
        "Scope": scope,
        "Id": id,
        "LockToken": lock_token,
        "VisibilityConfig": visibility(metric),
        "Rules": rules,
    });
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    if let Some(bodies) = custom_response_bodies {
        body["CustomResponseBodies"] = bodies.clone();
    }
    body
}

/// A rule whose statement references the IP set `arn`.
fn ip_set_rule(name: &str, arn: &str) -> Value {
    rule(
        name,
        json!({ "IPSetReferenceStatement": { "ARN": arn } }),
    )
}

/// `GetRuleGroup` for `(scope, name, id)`, as a 200.
async fn get_rule_group(waf: &Waf, scope: &str, name: &str, id: &str) -> Value {
    waf.ok(
        "GetRuleGroup",
        json!({ "Name": name, "Scope": scope, "Id": id }),
    )
    .await
}

/// The WCUs this service publishes for `rules`, as `CheckCapacity` reports them —
/// the report `RuleGroup$Capacity`'s doc tells a caller to size the limit against
/// ("you can check the capacity for a set of rules using CheckCapacity"), so no
/// per-statement figure is written out here.
async fn published_capacity(waf: &Waf, scope: &str, rules: &Value) -> i64 {
    let checked = waf
        .ok("CheckCapacity", json!({ "Scope": scope, "Rules": rules }))
        .await;
    checked["Capacity"]
        .as_i64()
        .unwrap_or_else(|| panic!("CheckCapacity must answer a Capacity, got {checked}"))
}

/// One whole-replacement case: what the rule group held, and what the update
/// submits.
struct Replacement {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The scope the group lives in; it is a namespace, and immutable.
    scope: &'static str,
    /// The capacity the group is created with; immutable thereafter.
    capacity: i64,
    /// The rules, description and custom response bodies the group is created with.
    rules_before: Value,
    description_before: Option<&'static str>,
    bodies_before: Option<Value>,
    /// The rules, description and custom response bodies the update submits; `None`
    /// means the update omits the member.
    rules_after: Value,
    description_after: Option<&'static str>,
    bodies_after: Option<Value>,
    /// What `AvailableLabels` must report after the update: the labels the NEW rules
    /// add.
    available_labels: Vec<&'static str>,
    /// What `ConsumedLabels` must report after the update: the labels the NEW rules
    /// match against.
    consumed_labels: Vec<&'static str>,
}

/// `UpdateRuleGroup` replaces the whole mutable specification, and `GetRuleGroup`
/// answers what was submitted.
///
/// The rows differ along everything the replacement generalises over: how many
/// rules there are before and after (fewer, more, none at all), what their
/// statements are (match, label match, IP set reference, nested logical), the ORDER
/// of the same two rules, whether a `Description` and `CustomResponseBodies` are
/// carried at all, the metric the `VisibilityConfig` names, and the scope. The
/// members WAF derives server-side are asserted with the rest: a `Description` the
/// update omits reads back as `""`, `Rules` is a list even when empty, and
/// `AvailableLabels` / `ConsumedLabels` are recomputed from the NEW rules — while
/// `Name`, `Id`, `ARN`, `LabelNamespace` and `Capacity` are unchanged.
#[tokio::test]
async fn update_rule_group_replaces_the_whole_mutable_specification() {
    let waf = Waf::new();
    // A rule group's references resolve only within its own scope, so each scope
    // gets its own IP set to reference.
    let (_regional_ips_id, regional_ips_arn, _regional_ips_lock) =
        create_ip_set(&waf, "REGIONAL", "update-rg-ips").await;
    let (_global_ips_id, global_ips_arn, _global_ips_lock) =
        create_ip_set(&waf, "CLOUDFRONT", "update-rg-ips").await;
    let custom_bodies = json!({
        "too-many": { "ContentType": "TEXT_PLAIN", "Content": "slow down" }
    });
    let other_bodies = json!({
        "go-away": { "ContentType": "TEXT_PLAIN", "Content": "no" }
    });

    let cases = vec![
        Replacement {
            case: "one rule replaces two, and the description is dropped",
            scope: "REGIONAL",
            capacity: 30,
            rules_before: ordered(json!([
                rule("first", byte_match_statement()),
                rule("second", sqli_statement()),
            ])),
            description_before: Some("two rules"),
            bodies_before: None,
            rules_after: json!([rule("only", byte_match_statement())]),
            description_after: None,
            bodies_after: None,
            available_labels: vec![],
            consumed_labels: vec![],
        },
        Replacement {
            case: "rules are added where there were none, and one adds a label",
            scope: "REGIONAL",
            capacity: 10,
            rules_before: json!([]),
            description_before: None,
            bodies_before: None,
            rules_after: json!([labelled_rule(
                "adds",
                byte_match_statement(),
                &["tagged"]
            )]),
            description_after: Some("now has a rule"),
            bodies_after: None,
            available_labels: vec!["tagged"],
            consumed_labels: vec![],
        },
        Replacement {
            case: "every rule is removed",
            scope: "REGIONAL",
            capacity: 20,
            rules_before: json!([rule("sqli", sqli_statement())]),
            description_before: Some("one rule"),
            bodies_before: None,
            rules_after: json!([]),
            description_after: Some("emptied"),
            bodies_after: None,
            available_labels: vec![],
            consumed_labels: vec![],
        },
        Replacement {
            case: "the same two rules in the opposite order",
            scope: "REGIONAL",
            capacity: 11,
            rules_before: ordered(json!([
                rule("match", byte_match_statement()),
                ip_set_rule("ips", &regional_ips_arn),
            ])),
            description_before: Some("ordered"),
            bodies_before: None,
            rules_after: ordered(json!([
                ip_set_rule("ips", &regional_ips_arn),
                rule("match", byte_match_statement()),
            ])),
            description_after: Some("ordered"),
            bodies_after: None,
            available_labels: vec![],
            consumed_labels: vec![],
        },
        Replacement {
            case: "a label-matching rule replaces a label-adding one",
            scope: "REGIONAL",
            capacity: 10,
            rules_before: json!([labelled_rule(
                "producer",
                byte_match_statement(),
                &["produced"]
            )]),
            description_before: None,
            bodies_before: None,
            rules_after: json!([label_match_rule("consumer", "upstream-label")]),
            description_after: None,
            bodies_after: None,
            available_labels: vec![],
            consumed_labels: vec!["upstream-label"],
        },
        Replacement {
            case: "a nested logical statement replaces a flat one",
            scope: "REGIONAL",
            capacity: 10,
            rules_before: json!([rule("flat", byte_match_statement())]),
            description_before: Some("flat"),
            bodies_before: None,
            rules_after: json!([rule(
                "nested",
                nested_statement(
                    3,
                    json!({ "IPSetReferenceStatement": { "ARN": &regional_ips_arn } })
                )
            )]),
            description_after: Some("nested three deep"),
            bodies_after: None,
            available_labels: vec![],
            consumed_labels: vec![],
        },
        Replacement {
            case: "an IP set reference and custom response bodies are added, in CLOUDFRONT scope",
            scope: "CLOUDFRONT",
            capacity: 11,
            rules_before: json!([rule("match", byte_match_statement())]),
            description_before: Some("global"),
            bodies_before: None,
            rules_after: ordered(json!([
                rule("match", byte_match_statement()),
                ip_set_rule("ips", &global_ips_arn),
            ])),
            description_after: Some("global with an IP set"),
            bodies_after: Some(custom_bodies.clone()),
            available_labels: vec![],
            consumed_labels: vec![],
        },
        Replacement {
            case: "custom response bodies are replaced by another set",
            scope: "REGIONAL",
            capacity: 10,
            rules_before: json!([rule("match", byte_match_statement())]),
            description_before: None,
            bodies_before: Some(custom_bodies.clone()),
            rules_after: json!([rule("match", byte_match_statement())]),
            description_after: None,
            bodies_after: Some(other_bodies.clone()),
            available_labels: vec![],
            consumed_labels: vec![],
        },
        Replacement {
            case: "custom response bodies are dropped when the update omits them",
            scope: "REGIONAL",
            capacity: 10,
            rules_before: json!([rule("match", byte_match_statement())]),
            description_before: Some("has bodies"),
            bodies_before: Some(custom_bodies.clone()),
            rules_after: json!([rule("match", byte_match_statement())]),
            description_after: Some("no bodies"),
            bodies_after: None,
            available_labels: vec![],
            consumed_labels: vec![],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let name = format!("updated-rg-{index}");
        let created = waf
            .ok(
                "CreateRuleGroup",
                create_body(
                    &name,
                    row.scope,
                    row.capacity,
                    row.description_before,
                    &row.rules_before,
                    row.bodies_before.as_ref(),
                ),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let lock = created["Summary"]["LockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The update names a different metric, so the replaced `VisibilityConfig` is
        // observable rather than coincidentally equal to the created one.
        let metric = format!("{name}-after");
        let updated = waf
            .ok(
                "UpdateRuleGroup",
                update_body(
                    (&name, row.scope, &id, &lock),
                    row.description_after,
                    &row.rules_after,
                    row.bodies_after.as_ref(),
                    &metric,
                ),
            )
            .await;
        let next_lock = updated["NextLockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_model_uuid(&next_lock),
            "case {case:?}: NextLockToken must have the modelled form, got {updated}"
        );
        assert_ne!(
            next_lock, lock,
            "case {case:?}: a successful update must rotate the lock token, got {updated}"
        );

        let expected_arn = rule_group_arn(row.scope, &name, &id);
        let mut expected = json!({
            "Name": &name,
            "Id": &id,
            "ARN": &expected_arn,
            // Immutable: this call carries no member that could change it.
            "Capacity": row.capacity,
            "Description": row.description_after.unwrap_or_default(),
            "Rules": row.rules_after.clone(),
            "VisibilityConfig": visibility(&metric),
            "LabelNamespace": label_namespace(&name),
            "AvailableLabels": label_summaries(&row.available_labels),
            "ConsumedLabels": label_summaries(&row.consumed_labels),
        });
        if let Some(bodies) = row.bodies_after.as_ref() {
            expected["CustomResponseBodies"] = bodies.clone();
        }

        let got = get_rule_group(&waf, row.scope, &name, &id).await;
        assert_eq!(
            got["RuleGroup"], expected,
            "case {case:?}: expected the submitted specification, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&next_lock),
            "case {case:?}: GetRuleGroup must report the rotated lock token, got {got}"
        );
        // The update did not re-mint the ARN, so the by-ARN read answers the same
        // group.
        let by_arn = waf
            .ok("GetRuleGroup", json!({ "ARN": &expected_arn }))
            .await;
        assert_eq!(
            by_arn, got,
            "case {case:?}: both addressing forms must answer the same group, got \
             {by_arn} and {got}"
        );

        // The token the caller just spent no longer works, and the refused retry
        // leaves the replacement in place.
        waf.refused(
            "UpdateRuleGroup",
            update_body(
                (&name, row.scope, &id, &lock),
                Some("a stale retry"),
                &json!([]),
                None,
                &name,
            ),
            "WAFOptimisticLockException",
            case,
        )
        .await;
        let again = get_rule_group(&waf, row.scope, &name, &id).await;
        assert_eq!(
            again["RuleGroup"], expected,
            "case {case:?}: a refused update must change nothing, got {again}"
        );
        assert_eq!(
            again["LockToken"],
            json!(&next_lock),
            "case {case:?}: a refused update must not rotate the lock token, got {again}"
        );
    }
}

/// A refused `UpdateRuleGroup` changes nothing: not the rules, not the description,
/// not the lock token.
///
/// One row per way the request can be wrong — a triple that names no stored group
/// (wrong name, wrong id, wrong scope), a stale lock token, rules costing more than
/// the immutable capacity, a reference to an IP set that does not exist, a rule
/// group reference and a managed rule group reference (neither of which may sit
/// inside a rule group), and a statement nested one level past the published bound
/// — each with the error code the model gives that failure.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_rule_group_untouched() {
    let waf = Waf::new();
    let name = "refused-rg";
    let rules_before = json!([rule("sqli", sqli_statement())]);
    let created = waf
        .ok(
            "CreateRuleGroup",
            create_body(name, "REGIONAL", 20, Some("kept"), &rules_before, None),
        )
        .await;
    let id = created["Summary"]["Id"].as_str().expect("Summary.Id");
    let lock = created["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken");
    let arn = rule_group_arn("REGIONAL", name, id);
    let stale = "11111111-2222-3333-4444-555555555555";
    let absent_id = "66666666-7777-8888-9999-aaaaaaaaaaaa";
    // A well-formed wafv2 IP set ARN that was never minted here: the shape of ARN a
    // reference carries, naming nothing this service holds.
    let absent_ips_arn = format!(
        "arn:aws:wafv2:{TEST_REGION}:{}:regional/ipset/absent/{absent_id}",
        engine::EMULATOR_ACCOUNT_ID
    );
    let replacement = json!([rule("replacement", byte_match_statement())]);

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a name that names no rule group",
            update_body(
                ("no-such-rg", "REGIONAL", id, lock),
                None,
                &replacement,
                None,
                "metric",
            ),
            "WAFNonexistentItemException",
        ),
        (
            "an id that names no rule group",
            update_body(
                (name, "REGIONAL", absent_id, lock),
                None,
                &replacement,
                None,
                "metric",
            ),
            "WAFNonexistentItemException",
        ),
        (
            "the group of the other scope",
            update_body(
                (name, "CLOUDFRONT", id, lock),
                None,
                &replacement,
                None,
                "metric",
            ),
            "WAFNonexistentItemException",
        ),
        (
            "a lock token that was never issued",
            update_body(
                (name, "REGIONAL", id, stale),
                None,
                &replacement,
                None,
                "metric",
            ),
            "WAFOptimisticLockException",
        ),
        (
            "rules costing more WCUs than the immutable capacity",
            update_body(
                (name, "REGIONAL", id, lock),
                None,
                &json!([rule("xss", xss_statement())]),
                None,
                "metric",
            ),
            "WAFInvalidParameterException",
        ),
        (
            "a reference to an IP set that does not exist",
            update_body(
                (name, "REGIONAL", id, lock),
                None,
                &json!([ip_set_rule("ips", &absent_ips_arn)]),
                None,
                "metric",
            ),
            "WAFNonexistentItemException",
        ),
        (
            "a rule group reference, which cannot sit inside a rule group",
            update_body(
                (name, "REGIONAL", id, lock),
                None,
                &json!([rule(
                    "reference",
                    json!({ "RuleGroupReferenceStatement": { "ARN": &arn } })
                )]),
                None,
                "metric",
            ),
            "WAFInvalidParameterException",
        ),
        (
            "a managed rule group reference, which cannot sit inside a rule group",
            update_body(
                (name, "REGIONAL", id, lock),
                None,
                &json!([rule(
                    "managed",
                    json!({
                        "ManagedRuleGroupStatement": {
                            "VendorName": "AWS",
                            "Name": "AWSManagedRulesCommonRuleSet",
                        }
                    })
                )]),
                None,
                "metric",
            ),
            "WAFInvalidParameterException",
        ),
        (
            "a statement nested one level past the published bound",
            update_body(
                (name, "REGIONAL", id, lock),
                None,
                &json!([rule(
                    "too-deep",
                    nested_statement(
                        service_com_amazonaws_wafv2_awswaf_20190729::state::MAX_STATEMENT_NESTING_DEPTH
                            + 1,
                        byte_match_statement(),
                    )
                )]),
                None,
                "metric",
            ),
            "WAFInvalidParameterException",
        ),
    ];

    for (case, request, code) in cases {
        waf.refused("UpdateRuleGroup", request, code, case).await;
        let got = get_rule_group(&waf, "REGIONAL", name, id).await;
        assert_eq!(
            got["RuleGroup"]["Rules"], rules_before,
            "case {case:?}: a refused update must not replace the rules, got {got}"
        );
        assert_eq!(
            got["RuleGroup"]["Description"],
            json!("kept"),
            "case {case:?}: a refused update must not change the description, got {got}"
        );
        assert_eq!(
            got["RuleGroup"]["VisibilityConfig"],
            visibility(name),
            "case {case:?}: a refused update must not change the visibility config, \
             got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(lock),
            "case {case:?}: a refused update must not rotate the lock token, got {got}"
        );
    }
}

/// An update must "stay within the capacity" the group declared at creation: a rule
/// list is saved exactly when the WCUs it costs fit, and refused one WCU past it.
///
/// One row per statement shape, each driven at every bound of the range the limit
/// defines: one WCU below the cost (refused), exactly at it (saved) and one above it
/// (saved). The cost is not written out here — it is read from `CheckCapacity`, the
/// report `Capacity`'s own doc tells a caller to size the limit against, so the save
/// is accepted exactly when the capacity the caller could have checked fits.
#[tokio::test]
async fn an_update_must_stay_within_the_groups_immutable_capacity() {
    let waf = Waf::new();
    let (_ips_id, ips_arn, _ips_lock) = create_ip_set(&waf, "REGIONAL", "capacity-rg-ips").await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "one byte-match rule",
            json!([rule("byte", byte_match_statement())]),
        ),
        ("one SQLi rule", json!([rule("sqli", sqli_statement())])),
        ("one XSS rule", json!([rule("xss", xss_statement())])),
        (
            "two label-match rules",
            ordered(json!([
                label_match_rule("first", "one-label"),
                label_match_rule("second", "another-label"),
            ])),
        ),
        (
            "a byte match and an IP set reference",
            ordered(json!([
                rule("byte", byte_match_statement()),
                ip_set_rule("ips", &ips_arn),
            ])),
        ),
    ];

    for (index, (row, rules)) in rows.iter().enumerate() {
        let cost = published_capacity(&waf, "REGIONAL", rules).await;
        assert!(
            cost >= 2,
            "case {row:?}: the row needs a cost of at least 2 WCUs to have a capacity \
             below it, got {cost}"
        );
        for (offset, saved) in [(-1_i64, false), (0, true), (1, true)] {
            let capacity = cost + offset;
            let case = format!("{row} at capacity {capacity} for a cost of {cost}");
            let name = format!("capacity-rg-{index}-{}", capacity);
            let created = waf
                .ok(
                    "CreateRuleGroup",
                    create_body(&name, "REGIONAL", capacity, None, &json!([]), None),
                )
                .await;
            let id = created["Summary"]["Id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let lock = created["Summary"]["LockToken"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            let request = update_body(
                (&name, "REGIONAL", &id, &lock),
                None,
                rules,
                None,
                &format!("{name}-after"),
            );
            if saved {
                waf.ok("UpdateRuleGroup", request).await;
                let got = get_rule_group(&waf, "REGIONAL", &name, &id).await;
                assert_eq!(
                    got["RuleGroup"]["Rules"], *rules,
                    "case {case:?}: rules that fit the capacity must be saved, got {got}"
                );
                assert_eq!(
                    got["RuleGroup"]["Capacity"],
                    json!(capacity),
                    "case {case:?}: an update must not change the declared capacity, \
                     got {got}"
                );
            } else {
                // The capacity is a parameter of the saved group, so a rule list
                // that does not fit it is the invalid parameter, carrying the cost
                // the rules were priced at; see
                // `crate::rule_group_capacity_refusal`.
                waf.refused(
                    "UpdateRuleGroup",
                    request,
                    "WAFInvalidParameterException",
                    &case,
                )
                .await;
                let got = get_rule_group(&waf, "REGIONAL", &name, &id).await;
                assert_eq!(
                    got["RuleGroup"]["Rules"],
                    json!([]),
                    "case {case:?}: rules above the capacity must not be stored, got \
                     {got}"
                );
                assert_eq!(
                    got["LockToken"],
                    json!(&lock),
                    "case {case:?}: a refused update must not rotate the lock token, \
                     got {got}"
                );
            }
        }
    }
}
