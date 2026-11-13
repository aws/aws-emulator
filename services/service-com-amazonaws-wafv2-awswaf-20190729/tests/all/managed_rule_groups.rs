// SPDX-License-Identifier: CC0-1.0

//! Running a VENDOR-published managed rule group from a web ACL rule, end to end
//! over the wire.
//!
//! The rule asserted here: a rule statement that runs a rule group is saved when
//! the group it names exists for the kind of reference it is — for a
//! `ManagedRuleGroupStatement` that means the `(VendorName, Name)` pair is one the
//! service publishes in its managed-rule-group catalogue — and the saved web ACL's
//! derived `Capacity` includes that group's capacity. A pair the catalogue does not
//! list names nothing that exists and is refused with the modelled
//! `WAFNonexistentItemException`, exactly as a reference to a missing IP set or
//! rule group is.
//!
//! Every row of the tables below is driven through the router's front door with the
//! harness in [`crate::web_acl_lifecycle`], so what these tests observe is what an
//! SDK observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ManagedRuleGroupDescriptor, MANAGED_RULE_GROUP_CATALOGUE,
};

use crate::web_acl_lifecycle::{create_web_acl, sqli_statement, visibility, Waf};

/// A `ManagedRuleGroupStatement` for `(vendor, name)`.
pub(crate) fn managed_statement(vendor: &str, name: &str) -> Value {
    json!({
        "ManagedRuleGroupStatement": { "VendorName": vendor, "Name": name }
    })
}

/// A rule that RUNS a rule group: such a rule carries `OverrideAction` rather than
/// `Action` — "you can only use this for rule statements that reference a rule
/// group, like `RuleGroupReferenceStatement` and `ManagedRuleGroupStatement`"
/// (`OverrideAction`'s own doc).
fn managed_rule(name: &str, priority: i64, statement: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// Every catalogued managed rule group can be run from a web ACL rule: the create
/// succeeds, the statement reads back verbatim through `GetWebACL`, and the web
/// ACL's derived `Capacity` is the group's published capacity.
///
/// The table is the WHOLE catalogue
/// (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`, the WAF Developer Guide's
/// "AWS Managed Rules rule groups list"), one row per published group, so the
/// groups no recorded request mentions are covered beside the one it does.
#[tokio::test]
async fn every_catalogued_managed_rule_group_can_be_run_from_a_web_acl_rule() {
    let waf = Waf::new();
    for (index, group) in MANAGED_RULE_GROUP_CATALOGUE.iter().enumerate() {
        let ManagedRuleGroupDescriptor {
            vendor_name,
            name: group_name,
            label_slug: _,
            capacity,
            product_title: _,
            product_description: _,
            is_versioning_supported: _,
            is_advanced_managed_rule_set: _,
            versions: _,
            expired_versions: _,
            rules: _,
            token_labels: _,
        } = *group;
        let case = format!("{vendor_name}/{group_name}");
        let acl_name = format!("managed-acl-{index}");
        let statement = managed_statement(vendor_name, group_name);
        let (acl_id, _arn, _lock) = create_web_acl(
            &waf,
            "REGIONAL",
            &acl_name,
            json!([managed_rule("managed", 0, statement.clone())]),
        )
        .await;
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &acl_name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Rules"][0]["Statement"], statement,
            "case {case:?}: the stored managed-rule statement must read back \
             verbatim, got {got}"
        );
        assert_eq!(
            got["WebACL"]["Capacity"],
            json!(capacity),
            "case {case:?}: expected the group's published capacity {capacity}, \
             got {}",
            got["WebACL"]["Capacity"]
        );
    }
}

/// A web ACL's `Capacity` is the SUM over its rules, whatever mix of managed
/// groups and ordinary statements they hold, and a managed statement's own
/// `ScopeDownStatement` is priced beside the group it narrows.
///
/// The rows differ in how many managed groups the web ACL runs and in what else
/// its rules carry; each expectation is composed from the catalogue rather than
/// written as a literal.
#[tokio::test]
async fn capacity_sums_the_published_capacity_of_every_managed_group_run() {
    /// The published capacity of a catalogued group, by name.
    fn capacity_of(name: &str) -> i64 {
        MANAGED_RULE_GROUP_CATALOGUE
            .iter()
            .find(|group| group.name == name)
            .map(|group| group.capacity)
            .unwrap_or_else(|| panic!("{name} must be in the managed catalogue"))
    }
    /// The WCUs one `SqliMatchStatement` costs, as
    /// `web_acl_capacity_is_derived_from_the_stored_rules` pins it.
    const SQLI_WCU: i64 = 20;

    let common = capacity_of("AWSManagedRulesCommonRuleSet");
    let reputation = capacity_of("AWSManagedRulesAmazonIpReputationList");
    let known_bad = capacity_of("AWSManagedRulesKnownBadInputsRuleSet");

    let cases: Vec<(&str, Value, i64)> = vec![
        (
            "one managed group",
            json!([managed_rule(
                "common",
                0,
                managed_statement("AWS", "AWSManagedRulesCommonRuleSet")
            )]),
            common,
        ),
        (
            "two managed groups",
            json!([
                managed_rule(
                    "common",
                    0,
                    managed_statement("AWS", "AWSManagedRulesCommonRuleSet")
                ),
                managed_rule(
                    "reputation",
                    1,
                    managed_statement("AWS", "AWSManagedRulesAmazonIpReputationList")
                ),
            ]),
            common + reputation,
        ),
        (
            "a managed group beside an ordinary statement",
            crate::web_acl_lifecycle::ordered(json!([
                managed_rule(
                    "known-bad",
                    0,
                    managed_statement("AWS", "AWSManagedRulesKnownBadInputsRuleSet")
                ),
                crate::web_acl_lifecycle::rule("sqli", sqli_statement()),
            ])),
            known_bad + SQLI_WCU,
        ),
        (
            "a managed group narrowed by a scope-down statement",
            json!([managed_rule(
                "scoped",
                0,
                json!({
                    "ManagedRuleGroupStatement": {
                        "VendorName": "AWS",
                        "Name": "AWSManagedRulesCommonRuleSet",
                        "ScopeDownStatement": sqli_statement(),
                    }
                })
            )]),
            common + SQLI_WCU,
        ),
    ];

    let waf = Waf::new();
    for (index, (case, rules, expected)) in cases.into_iter().enumerate() {
        let acl_name = format!("managed-capacity-{index}");
        let (acl_id, _arn, _lock) = create_web_acl(&waf, "REGIONAL", &acl_name, rules).await;
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &acl_name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Capacity"],
            json!(expected),
            "case {case:?}: expected Capacity {expected}, got {}",
            got["WebACL"]["Capacity"]
        );
    }
}

/// A `(VendorName, Name)` pair the catalogue does not list names nothing that
/// exists, so the save is refused and stores nothing — whichever member is wrong,
/// and whether the save is a create or an update.
#[tokio::test]
async fn a_managed_group_the_catalogue_does_not_list_is_refused() {
    let catalogued = MANAGED_RULE_GROUP_CATALOGUE[0];
    let cases: &[(&str, &str, &str)] = &[
        // (case, VendorName, Name)
        (
            "a name no vendor publishes",
            "AWS",
            "AWSManagedRulesNoSuchRuleSet",
        ),
        (
            "a catalogued name under an unknown vendor",
            "SomeOtherVendor",
            "AWSManagedRulesCommonRuleSet",
        ),
        (
            "a catalogued name whose case does not match",
            "AWS",
            "awsmanagedrulescommonruleset",
        ),
        (
            "the vendor name in the wrong case",
            "aws",
            "AWSManagedRulesCommonRuleSet",
        ),
    ];

    let waf = Waf::new();
    // One web ACL that already runs a catalogued group, to update against.
    let (live_id, _live_arn, live_lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "managed-live",
        json!([managed_rule(
            "managed",
            0,
            managed_statement(catalogued.vendor_name, catalogued.name)
        )]),
    )
    .await;

    for (index, (case, vendor, name)) in cases.iter().enumerate() {
        let acl_name = format!("managed-absent-{index}");
        waf.refused(
            "CreateWebACL",
            json!({
                "Name": &acl_name,
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(&acl_name),
                "Rules": [managed_rule("managed", 0, managed_statement(vendor, name))],
            }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
        let listed = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
        assert!(
            !listed.to_string().contains(acl_name.as_str()),
            "case {case:?}: a refused CreateWebACL must store nothing, got {listed}"
        );

        waf.refused(
            "UpdateWebACL",
            json!({
                "Name": "managed-live",
                "Scope": "REGIONAL",
                "Id": &live_id,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("managed-live"),
                "LockToken": &live_lock,
                "Rules": [managed_rule("managed", 0, managed_statement(vendor, name))],
            }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    // The refused updates left the live web ACL — and its lock token — alone.
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "managed-live", "Scope": "REGIONAL", "Id": &live_id }),
        )
        .await;
    assert_eq!(
        got["LockToken"],
        json!(&live_lock),
        "a refused update must not rotate the lock token, got {got}"
    );
    assert_eq!(
        got["WebACL"]["Rules"][0]["Statement"],
        managed_statement(catalogued.vendor_name, catalogued.name),
        "a refused update must not replace the stored rules, got {got}"
    );
}

/// "You cannot nest a `ManagedRuleGroupStatement`, for example for use inside a
/// `NotStatement` or `OrStatement`. You cannot use a managed rule group inside
/// another rule group. You can only reference a managed rule group as a top-level
/// statement within a rule that you define in a web ACL"
/// (`smithy_query(query="doc",shape="ManagedRuleGroupStatement")`).
///
/// One row per nesting the model has, plus the rule-group save — the same table
/// shape the customer-owned `RuleGroupReferenceStatement` is held to, because the
/// model states the same placement rule for both.
#[tokio::test]
async fn a_managed_rule_group_statement_may_only_be_a_rule_s_top_level_statement() {
    let managed = managed_statement("AWS", "AWSManagedRulesCommonRuleSet");
    let nestings: Vec<(&str, Value)> = vec![
        (
            "inside a NotStatement",
            json!({ "NotStatement": { "Statement": managed.clone() } }),
        ),
        (
            "inside an AndStatement",
            json!({
                "AndStatement": {
                    "Statements": [managed.clone(), sqli_statement()]
                }
            }),
        ),
        (
            "inside an OrStatement",
            json!({
                "OrStatement": {
                    "Statements": [sqli_statement(), managed.clone()]
                }
            }),
        ),
        (
            "two levels down, inside an AndStatement in a NotStatement",
            json!({
                "NotStatement": {
                    "Statement": {
                        "AndStatement": {
                            "Statements": [managed.clone(), sqli_statement()]
                        }
                    }
                }
            }),
        ),
        (
            "as a rate-based statement's scope-down statement",
            json!({
                "RateBasedStatement": {
                    "Limit": 1000,
                    "AggregateKeyType": "IP",
                    "ScopeDownStatement": managed.clone(),
                }
            }),
        ),
    ];

    let waf = Waf::new();
    for (index, (case, statement)) in nestings.into_iter().enumerate() {
        let acl_name = format!("managed-nested-{index}");
        waf.refused(
            "CreateWebACL",
            json!({
                "Name": &acl_name,
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(&acl_name),
                "Rules": [managed_rule("nested", 0, statement)],
            }),
            "WAFInvalidParameterException",
            case,
        )
        .await;
        let listed = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
        assert!(
            !listed.to_string().contains(acl_name.as_str()),
            "case {case:?}: a refused CreateWebACL must store nothing, got {listed}"
        );
    }

    // "You cannot use a managed rule group inside another rule group": even as a
    // rule's own top-level statement, a managed group cannot be run from a rule
    // group.
    let refusal = waf
        .refused(
            "CreateRuleGroup",
            json!({
                "Name": "managed-in-group",
                "Scope": "REGIONAL",
                "Capacity": 1000,
                "VisibilityConfig": visibility("managed-in-group"),
                "Rules": [managed_rule("managed", 0, managed.clone())],
            }),
            "WAFInvalidParameterException",
            "a managed rule group run from a rule group's rule",
        )
        .await;
    assert_eq!(
        refusal["Field"],
        json!("MANAGED_RULE_SET_STATEMENT"),
        "case {:?}: the refusal must name the statement at fault, got {refusal}",
        "a managed rule group run from a rule group's rule"
    );
}
