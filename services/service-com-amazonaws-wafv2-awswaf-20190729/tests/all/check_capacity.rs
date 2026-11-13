// SPDX-License-Identifier: CC0-1.0

//! `CheckCapacity`, end to end over the wire.
//!
//! `CheckCapacity` "returns the web ACL capacity unit (WCU) requirements for a
//! specified scope and set of rules", and the rule asserted here is: the reported
//! `Capacity` is the cost of the rules in the REQUEST — each rule's statement
//! priced by its kind, a logical statement costing what it nests, a statement that
//! runs a rule group costing that group's own reported capacity — summed over the
//! rules, which is exactly what `WebACL$Capacity` reports for a web ACL holding the
//! same rules. The call REPORTS: it stores nothing and changes nothing a later
//! caller sees.
//!
//! Its counterpart on the save path is `RuleGroup$Capacity`: "when you add or
//! modify the rules in a rule group, WAF enforces this limit", so a
//! `CreateRuleGroup` whose rules need more WCUs than the request declares is
//! refused.
//!
//! Each suite asserts that over a TABLE spanning the class — every statement kind
//! this emulator prices, each nesting form, rule counts, both scopes, and
//! references to groups of different declared capacities — rather than over one
//! request. The harness is the one in [`crate::web_acl_lifecycle`]: every call goes
//! through the router's front door, so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ManagedRuleGroupDescriptor, AWS_MANAGED_RULE_GROUP_VENDOR, MANAGED_RULE_GROUP_CATALOGUE,
    MAX_STATEMENT_NESTING_DEPTH,
};

use crate::managed_rule_groups::managed_statement;
use crate::rule_group_lifecycle::{byte_match_statement, create_rule_group};
use crate::web_acl_lifecycle::{
    create_ip_set, create_web_acl, nested_statement, ordered, rule, sqli_statement, visibility,
    xss_statement, Waf,
};

/// The WCUs one `ByteMatchStatement` over a single field, with one `NONE` text
/// transformation and the `CONTAINS` positional constraint, costs: the recorded
/// corpus prices a web ACL of three such rules at 30, as
/// `state::BYTE_MATCH_SCANNING_WCU` documents. Written out as a number here rather
/// than imported, so the table does not restate the code under test. The whole cost
/// model — the other positional constraints, the sensitivity split and every
/// modifier — is spanned by `crate::wcu_cost_model`.
const BYTE_MATCH_WCU: i64 = 10;

/// The WCUs one `RateBasedStatement` costs before anything it nests: the recorded
/// corpus prices a web ACL holding one bare `IP`-aggregated rate-based rule at 2
/// (`state::RATE_BASED_WCU`). See [`BYTE_MATCH_WCU`].
const RATE_BASED_WCU: i64 = 2;

/// The WCUs a `RateBasedStatement` adds PER custom aggregation key: the same
/// recorded web ACL, with its rate-based rule aggregating on one custom key, reports
/// 32 (`state::RATE_BASED_CUSTOM_KEY_WCU`). See [`BYTE_MATCH_WCU`].
const RATE_BASED_CUSTOM_KEYS_WCU: i64 = 30;

/// The WCUs one `SqliMatchStatement` costs, as the WAF Developer Guide's
/// rule-statement list states it and as
/// `crate::web_acl_lifecycle::web_acl_capacity_is_derived_from_the_stored_rules`
/// already pins it for `WebACL$Capacity`.
const SQLI_WCU: i64 = 20;

/// The WCUs one `XssMatchStatement` costs. See [`SQLI_WCU`].
const XSS_WCU: i64 = 40;

/// The WCUs one `RegexMatchStatement` costs. See [`SQLI_WCU`].
const REGEX_MATCH_WCU: i64 = 3;

/// The WCUs one of the cheap statements costs — size constraint, geo match, IP set
/// reference, label match and ASN match. See [`SQLI_WCU`].
const CHEAP_WCU: i64 = 1;

/// A `RegexMatchStatement` over the URI path.
fn regex_match_statement() -> Value {
    json!({
        "RegexMatchStatement": {
            "RegexString": "^/admin",
            "FieldToMatch": { "UriPath": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
        }
    })
}

/// A `GeoMatchStatement`. `CountryCodes` takes "two-character country codes … from
/// the alpha-2 country ISO codes of the ISO 3166 international standard".
fn geo_match_statement() -> Value {
    json!({ "GeoMatchStatement": { "CountryCodes": ["US", "CN"] } })
}

/// A `LabelMatchStatement` over one label.
fn label_match_statement() -> Value {
    json!({ "LabelMatchStatement": { "Scope": "LABEL", "Key": "upstream-label" } })
}

/// A `SizeConstraintStatement`: "look for query strings that are longer than 100
/// bytes", the example its own doc gives.
fn size_constraint_statement() -> Value {
    json!({
        "SizeConstraintStatement": {
            "FieldToMatch": { "QueryString": {} },
            "ComparisonOperator": "GT",
            "Size": 100,
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
        }
    })
}

/// An `AsnMatchStatement`. `AsnList` is `length 1..100` of Autonomous System
/// Numbers (`smithy_query(query="shape",shape="AsnMatchStatement")`).
fn asn_match_statement() -> Value {
    json!({ "AsnMatchStatement": { "AsnList": [64512] } })
}

/// A `RateBasedStatement`, aggregating on `IP` or on one custom key.
fn rate_based_statement(custom_keys: bool) -> Value {
    if custom_keys {
        json!({
            "RateBasedStatement": {
                "Limit": 2000,
                "AggregateKeyType": "CUSTOM_KEYS",
                "CustomKeys": [{ "HTTPMethod": {} }],
            }
        })
    } else {
        json!({ "RateBasedStatement": { "Limit": 2000, "AggregateKeyType": "IP" } })
    }
}

/// A `ManagedRuleGroupStatement` naming the `(VendorName, Name)` pair of a
/// catalogued managed rule group.
fn aws_managed_statement(name: &str) -> Value {
    // One definition of the fixture JSON, in `crate::managed_rule_groups`; every group
    // in the catalogue this service publishes carries the AWS vendor name.
    managed_statement(AWS_MANAGED_RULE_GROUP_VENDOR, name)
}

/// The catalogued managed rule group `name`, selected BY NAME so a catalogue that
/// grows or shrinks fails here with a diagnosis rather than an index panic.
fn catalogued(name: &str) -> &'static ManagedRuleGroupDescriptor {
    MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .find(|group| group.name == name)
        .unwrap_or_else(|| {
            panic!("case {name:?}: the published catalogue must list this managed rule group")
        })
}

/// A rule whose top-level statement RUNS a whole rule group — customer-owned or
/// vendor-published. Such a rule carries an `OverrideAction` rather than an
/// `Action` ("you can only use this for rule statements that reference a rule
/// group", `Rule$OverrideAction`).
fn group_running_rule(name: &str, priority: i64, statement: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// `CheckCapacity` for `rules` in `scope`; answers the reported `Capacity`.
async fn check_capacity(waf: &Waf, scope: &str, rules: &Value) -> i64 {
    let answered = waf
        .ok("CheckCapacity", json!({ "Scope": scope, "Rules": rules }))
        .await;
    answered["Capacity"]
        .as_i64()
        .unwrap_or_else(|| panic!("CheckCapacity must report a numeric Capacity, got {answered}"))
}

/// The `Capacity` a web ACL created with `rules` reports, read back through
/// `GetWebACL`.
async fn web_acl_capacity(waf: &Waf, scope: &str, name: &str, rules: &Value) -> i64 {
    let (id, _arn, _lock) = create_web_acl(waf, scope, name, rules.clone()).await;
    let got = waf
        .ok("GetWebACL", json!({ "Name": name, "Scope": scope, "Id": id }))
        .await;
    got["WebACL"]["Capacity"]
        .as_i64()
        .unwrap_or_else(|| panic!("GetWebACL must report a numeric Capacity, got {got}"))
}

/// One priced rule list: the rules and the WCUs they cost.
struct CapacityCase {
    case: &'static str,
    rules: Value,
    expected: i64,
}

/// Every statement kind this emulator prices, each nesting form, and rule lists of
/// several lengths — one row each. The expected value is built from the published
/// per-statement costs, so a row's total cannot come out right by accident.
fn capacity_cases() -> Vec<CapacityCase> {
    vec![
        CapacityCase {
            case: "no rules at all",
            rules: json!([]),
            expected: 0,
        },
        CapacityCase {
            case: "one byte match",
            rules: json!([rule("a", byte_match_statement())]),
            expected: BYTE_MATCH_WCU,
        },
        CapacityCase {
            case: "one SQLi match",
            rules: json!([rule("a", sqli_statement())]),
            expected: SQLI_WCU,
        },
        CapacityCase {
            case: "one XSS match",
            rules: json!([rule("a", xss_statement())]),
            expected: XSS_WCU,
        },
        CapacityCase {
            case: "one regex match",
            rules: json!([rule("a", regex_match_statement())]),
            expected: REGEX_MATCH_WCU,
        },
        CapacityCase {
            case: "one geo match",
            rules: json!([rule("a", geo_match_statement())]),
            expected: CHEAP_WCU,
        },
        CapacityCase {
            case: "one label match",
            rules: json!([rule("a", label_match_statement())]),
            expected: CHEAP_WCU,
        },
        CapacityCase {
            case: "one size constraint",
            rules: json!([rule("a", size_constraint_statement())]),
            expected: CHEAP_WCU,
        },
        CapacityCase {
            case: "one ASN match",
            rules: json!([rule("a", asn_match_statement())]),
            expected: CHEAP_WCU,
        },
        CapacityCase {
            case: "one bare rate-based statement",
            rules: json!([rule("a", rate_based_statement(false))]),
            expected: RATE_BASED_WCU,
        },
        CapacityCase {
            case: "one rate-based statement aggregating on a custom key",
            rules: json!([rule("a", rate_based_statement(true))]),
            expected: RATE_BASED_WCU + RATE_BASED_CUSTOM_KEYS_WCU,
        },
        CapacityCase {
            case: "three rules of the same kind, summed per rule",
            rules: ordered(json!([
                rule("a", byte_match_statement()),
                rule("b", byte_match_statement()),
                rule("c", byte_match_statement()),
            ])),
            expected: 3 * BYTE_MATCH_WCU,
        },
        CapacityCase {
            case: "four rules of different kinds, summed per rule",
            rules: ordered(json!([
                rule("a", geo_match_statement()),
                rule("b", sqli_statement()),
                rule("c", xss_statement()),
                rule("d", regex_match_statement()),
            ])),
            expected: CHEAP_WCU + SQLI_WCU + XSS_WCU + REGEX_MATCH_WCU,
        },
        CapacityCase {
            case: "an AndStatement costs what it nests",
            rules: json!([rule(
                "a",
                json!({
                    "AndStatement": {
                        "Statements": [byte_match_statement(), sqli_statement()]
                    }
                })
            )]),
            expected: BYTE_MATCH_WCU + SQLI_WCU,
        },
        CapacityCase {
            case: "an OrStatement costs what it nests",
            rules: json!([rule(
                "a",
                json!({
                    "OrStatement": {
                        "Statements": [geo_match_statement(), xss_statement()]
                    }
                })
            )]),
            expected: CHEAP_WCU + XSS_WCU,
        },
        CapacityCase {
            case: "a NotStatement costs what it nests",
            rules: json!([rule(
                "a",
                json!({ "NotStatement": { "Statement": xss_statement() } })
            )]),
            expected: XSS_WCU,
        },
        CapacityCase {
            case: "a rate-based statement costs its scope-down too",
            rules: json!([rule(
                "a",
                json!({
                    "RateBasedStatement": {
                        "Limit": 1000,
                        "AggregateKeyType": "IP",
                        "ScopeDownStatement": xss_statement(),
                    }
                })
            )]),
            expected: RATE_BASED_WCU + XSS_WCU,
        },
        CapacityCase {
            case: "a statement nested at the maximum depth is still priced",
            rules: json!([rule(
                "a",
                nested_statement(MAX_STATEMENT_NESTING_DEPTH, sqli_statement())
            )]),
            expected: SQLI_WCU,
        },
        CapacityCase {
            case: "two managed rule groups beside two rules of other kinds",
            // `ordered` assigns each rule its position as `Priority`, which is what
            // makes the four priorities distinct; the numbers passed to
            // `group_running_rule` below are the positions it will set anyway.
            rules: ordered(json!([
                group_running_rule(
                    "core",
                    0,
                    aws_managed_statement("AWSManagedRulesCommonRuleSet")
                ),
                group_running_rule(
                    "reputation",
                    1,
                    aws_managed_statement("AWSManagedRulesAmazonIpReputationList")
                ),
                rule("scan", byte_match_statement()),
                rule("rate", rate_based_statement(false)),
            ])),
            expected: catalogued("AWSManagedRulesCommonRuleSet").capacity
                + catalogued("AWSManagedRulesAmazonIpReputationList").capacity
                + BYTE_MATCH_WCU
                + RATE_BASED_WCU,
        },
    ]
}

/// `CheckCapacity` "returns the web ACL capacity unit (WCU) requirements for a
/// specified scope and set of rules": each rule's statement priced by its kind,
/// nestings costing what they nest, summed over the rules.
///
/// Every row runs in BOTH values `Scope` declares, because the price of a rule list
/// that references nothing is a property of the rules.
#[tokio::test]
async fn check_capacity_prices_every_statement_kind_and_sums_over_the_rules() {
    let waf = Waf::new();
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        for row in capacity_cases() {
            let case = format!("{scope}: {}", row.case);
            let actual = check_capacity(&waf, scope, &row.rules).await;
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected Capacity {}, got {actual}",
                row.expected
            );
        }
    }
}

/// The capacity `CheckCapacity` reports for a rule list is the `Capacity` a web ACL
/// holding that same rule list reports: "you can use this to check the capacity
/// requirements for the rules you want to use in a RuleGroup or WebACL", so the two
/// are one derivation, not two.
///
/// The rows are the whole priced class above, plus the reference-bearing statements
/// a web ACL may hold.
#[tokio::test]
async fn check_capacity_equals_the_capacity_a_web_acl_with_the_same_rules_reports() {
    let waf = Waf::new();
    let (_ip_set_id, ip_set_arn, _ip_lock) = create_ip_set(&waf, "REGIONAL", "equality-ips").await;
    let (_group_id, group_arn, _group_lock) =
        create_rule_group(&waf, "REGIONAL", "equality-group", 42, json!([])).await;

    let mut cases = capacity_cases();
    cases.push(CapacityCase {
        case: "an IP set reference",
        rules: json!([rule(
            "a",
            json!({ "IPSetReferenceStatement": { "ARN": &ip_set_arn } })
        )]),
        expected: CHEAP_WCU,
    });
    cases.push(CapacityCase {
        case: "a rule group reference beside a byte match",
        rules: ordered(json!([
            group_running_rule(
                "a",
                0,
                json!({ "RuleGroupReferenceStatement": { "ARN": &group_arn } })
            ),
            rule("b", byte_match_statement()),
        ])),
        expected: 42 + BYTE_MATCH_WCU,
    });
    cases.push(CapacityCase {
        case: "a managed rule group reference",
        rules: json!([group_running_rule(
            "a",
            0,
            aws_managed_statement(catalogued("AWSManagedRulesAdminProtectionRuleSet").name)
        )]),
        expected: catalogued("AWSManagedRulesAdminProtectionRuleSet").capacity,
    });

    for (index, row) in cases.into_iter().enumerate() {
        let case = row.case;
        let reported = check_capacity(&waf, "REGIONAL", &row.rules).await;
        let name = format!("equality-acl-{index}");
        let stored = web_acl_capacity(&waf, "REGIONAL", &name, &row.rules).await;
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected CheckCapacity {}, got {reported}",
            row.expected
        );
        assert_eq!(
            stored, reported,
            "case {case:?}: WebACL.Capacity must equal CheckCapacity, got {stored} and {reported}"
        );
    }
}

/// A statement that runs a rule group costs that group's OWN reported capacity:
/// `RuleGroup$Capacity` is "the web ACL capacity units (WCUs) required for this
/// rule group", fixed at creation, and a managed group's cost is the capacity its
/// vendor publishes for it.
///
/// The expected value is read back out of the service (`GetRuleGroup`, the
/// catalogue the service publishes) rather than written as a literal, so the rule
/// is asserted against what the referenced group reports. One row per declared
/// capacity, per number of references, per scope, and for a vendor-published group
/// beside a customer-owned one.
#[tokio::test]
async fn a_rule_group_reference_costs_the_referenced_groups_own_reported_capacity() {
    let waf = Waf::new();
    // Three declared capacities: the published minimum, an arbitrary middle value
    // and a large one, so no total can come out right by accident.
    let declared = [1_i64, 73, 500];
    let mut arns: Vec<String> = Vec::new();
    for (index, capacity) in declared.iter().enumerate() {
        let name = format!("priced-group-{index}");
        let (id, arn, _lock) = create_rule_group(&waf, "REGIONAL", &name, *capacity, json!([])).await;
        // What the group itself reports is what a reference to it must cost.
        let got = waf
            .ok(
                "GetRuleGroup",
                json!({ "Name": &name, "Scope": "REGIONAL", "Id": id }),
            )
            .await;
        assert_eq!(
            got["RuleGroup"]["Capacity"].as_i64(),
            Some(*capacity),
            "case {name:?}: the group must report the capacity it was created with, got {got}"
        );
        arns.push(arn);
    }

    let reference = |arn: &str| json!({ "RuleGroupReferenceStatement": { "ARN": arn } });
    let cases: Vec<(String, Value, i64)> = vec![
        (
            "one reference to the smallest group".to_string(),
            json!([group_running_rule("a", 0, reference(&arns[0]))]),
            declared[0],
        ),
        (
            "one reference to the middle group".to_string(),
            json!([group_running_rule("a", 0, reference(&arns[1]))]),
            declared[1],
        ),
        (
            "one reference to the largest group".to_string(),
            json!([group_running_rule("a", 0, reference(&arns[2]))]),
            declared[2],
        ),
        (
            "three references, one per group".to_string(),
            json!([
                group_running_rule("a", 0, reference(&arns[0])),
                group_running_rule("b", 1, reference(&arns[1])),
                group_running_rule("c", 2, reference(&arns[2])),
            ]),
            declared[0] + declared[1] + declared[2],
        ),
        (
            "the same group referenced twice costs twice".to_string(),
            json!([
                group_running_rule("a", 0, reference(&arns[1])),
                group_running_rule("b", 1, reference(&arns[1])),
            ]),
            2 * declared[1],
        ),
        (
            "a customer-owned reference beside a vendor-published one".to_string(),
            json!([
                group_running_rule("a", 0, reference(&arns[2])),
                group_running_rule(
                    "b",
                    1,
                    aws_managed_statement(catalogued("AWSManagedRulesAmazonIpReputationList").name)
                ),
            ]),
            declared[2] + catalogued("AWSManagedRulesAmazonIpReputationList").capacity,
        ),
        (
            "a reference beside statements of its own".to_string(),
            ordered(json!([
                group_running_rule("a", 0, reference(&arns[1])),
                rule("b", sqli_statement()),
                rule("c", geo_match_statement()),
            ])),
            declared[1] + SQLI_WCU + CHEAP_WCU,
        ),
    ];

    for (case, rules, expected) in cases {
        let actual = check_capacity(&waf, "REGIONAL", &rules).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected Capacity {expected}, got {actual}"
        );
    }

    // A scope is a namespace, so the same assertion holds for a group of the other
    // scope, priced in that scope.
    let (_cf_id, cf_arn, _cf_lock) =
        create_rule_group(&waf, "CLOUDFRONT", "priced-group-cf", 123, json!([])).await;
    let cf_rules = json!([group_running_rule("a", 0, reference(&cf_arn))]);
    let actual = check_capacity(&waf, "CLOUDFRONT", &cf_rules).await;
    assert_eq!(
        actual, 123,
        "case \"a CLOUDFRONT group referenced in CLOUDFRONT scope\": expected Capacity 123, \
         got {actual}"
    );

    // Every managed rule group the service publishes is priced at the capacity the
    // catalogue states for it — one row per catalogue entry, so the rule is not
    // asserted on one hand-picked group.
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let rules = json!([group_running_rule("a", 0, aws_managed_statement(group.name))]);
        let actual = check_capacity(&waf, "REGIONAL", &rules).await;
        assert_eq!(
            actual, group.capacity,
            "case {:?}: expected the catalogued capacity {}, got {actual}",
            group.name, group.capacity
        );
    }
}

/// A rule list whose references price above `i64::MAX` is reported, not a panic.
///
/// `RuleGroup$Capacity` is modelled `range 1..` and is stored verbatim, so a caller
/// can declare a group whose capacity is near the maximum a `long` holds and then
/// reference several such groups. `ConsumedCapacity` is `range 0..`, so the sum
/// saturates at that maximum: a capacity above any limit, never a wrapped negative
/// one and never a refused request.
#[tokio::test]
async fn check_capacity_saturates_instead_of_overflowing_on_huge_declared_capacities() {
    let waf = Waf::new();
    let reference = |arn: &str| json!({ "RuleGroupReferenceStatement": { "ARN": arn } });
    let mut arns: Vec<String> = Vec::new();
    for index in 0..3 {
        let name = format!("huge-group-{index}");
        let (_id, arn, _lock) =
            create_rule_group(&waf, "REGIONAL", &name, i64::MAX, json!([])).await;
        arns.push(arn);
    }

    let cases: Vec<(&str, Value, i64)> = vec![
        (
            "one reference at the maximum a long holds",
            json!([group_running_rule("a", 0, reference(&arns[0]))]),
            i64::MAX,
        ),
        (
            "two references, each at that maximum",
            json!([
                group_running_rule("a", 0, reference(&arns[0])),
                group_running_rule("b", 1, reference(&arns[1])),
            ]),
            i64::MAX,
        ),
        (
            "three references and a statement of its own",
            ordered(json!([
                group_running_rule("a", 0, reference(&arns[0])),
                group_running_rule("b", 1, reference(&arns[1])),
                group_running_rule("c", 2, reference(&arns[2])),
                rule("d", sqli_statement()),
            ])),
            i64::MAX,
        ),
    ];

    for (case, rules, expected) in cases {
        let actual = check_capacity(&waf, "REGIONAL", &rules).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the saturated Capacity {expected}, got {actual}"
        );
    }
}

/// `CheckCapacity` REPORTS: it writes nothing a later caller can see.
///
/// The same call is made repeatedly, with references, before and after the reads it
/// could have disturbed: the referenced group still reports what it was created
/// with, the name a web ACL could take is still free, and every answer is the same
/// value.
#[tokio::test]
async fn check_capacity_reports_without_changing_anything_a_later_caller_sees() {
    let waf = Waf::new();
    let (group_id, group_arn, group_lock) =
        create_rule_group(&waf, "REGIONAL", "untouched-group", 64, json!([])).await;
    let rules = ordered(json!([
        group_running_rule(
            "a",
            0,
            json!({ "RuleGroupReferenceStatement": { "ARN": &group_arn } })
        ),
        rule("b", sqli_statement()),
    ]));
    let expected = 64 + SQLI_WCU;

    let before = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": "untouched-group", "Scope": "REGIONAL", "Id": &group_id }),
        )
        .await;
    let listed_before = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;

    for attempt in 0..3 {
        let actual = check_capacity(&waf, "REGIONAL", &rules).await;
        assert_eq!(
            actual, expected,
            "case \"attempt {attempt}\": a repeated report must answer {expected}, got {actual}"
        );
    }

    let after = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": "untouched-group", "Scope": "REGIONAL", "Id": &group_id }),
        )
        .await;
    assert_eq!(
        after, before,
        "case \"the referenced rule group\": CheckCapacity must change nothing, got {after}"
    );
    assert_eq!(
        after["LockToken"].as_str(),
        Some(group_lock.as_str()),
        "case \"the referenced rule group\": the lock token must not move, got {after}"
    );
    let listed_after = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
    assert_eq!(
        listed_after, listed_before,
        "case \"the web ACLs of the scope\": CheckCapacity must create none, got {listed_after}"
    );

    // And the names it was asked about are still free: a save may still take them.
    let (_acl_id, _acl_arn, _acl_lock) =
        create_web_acl(&waf, "REGIONAL", "untouched-group", rules.clone()).await;
}

/// A rule list `CheckCapacity` cannot price is refused, not priced at zero.
///
/// One row per way a rule list can be unpriceable: a statement that runs a rule
/// group the service does not hold in this scope (unknown ARN, the other scope's
/// group, an uncatalogued vendor group, an entity kind this emulator has no store
/// for), which is the modelled `WAFNonexistentItemException`; and a rule list WAF
/// does not accept at all — a rule-group-running statement reached through a
/// nesting, or a tree one level past the published nesting bound — which is the
/// modelled `WAFInvalidParameterException`.
#[tokio::test]
async fn check_capacity_refuses_a_rule_list_it_cannot_price() {
    let waf = Waf::new();
    let (_id, regional_arn, _lock) =
        create_rule_group(&waf, "REGIONAL", "refusal-group", 17, json!([])).await;
    let (_cf_id, cloudfront_arn, _cf_lock) =
        create_rule_group(&waf, "CLOUDFRONT", "refusal-group-cf", 19, json!([])).await;
    let unknown_arn = format!(
        "arn:aws:wafv2:us-west-2:{}:regional/rulegroup/absent/11111111-2222-3333-4444-555555555555",
        engine::EMULATOR_ACCOUNT_ID
    );

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a reference to a rule group that was never created",
            json!([group_running_rule(
                "a",
                0,
                json!({ "RuleGroupReferenceStatement": { "ARN": &unknown_arn } })
            )]),
            "WAFNonexistentItemException",
        ),
        (
            "a reference to a rule group of the other scope",
            json!([group_running_rule(
                "a",
                0,
                json!({ "RuleGroupReferenceStatement": { "ARN": &cloudfront_arn } })
            )]),
            "WAFNonexistentItemException",
        ),
        (
            "a managed rule group the catalogue does not list",
            json!([group_running_rule(
                "a",
                0,
                aws_managed_statement("AWSManagedRulesNoSuchRuleSet")
            )]),
            "WAFNonexistentItemException",
        ),
        (
            "a regex pattern set reference, an entity kind nothing can create yet",
            json!([rule(
                "a",
                json!({
                    "RegexPatternSetReferenceStatement": {
                        "ARN": &unknown_arn,
                        "FieldToMatch": { "UriPath": {} },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                    }
                })
            )]),
            "WAFNonexistentItemException",
        ),
        (
            "a rule group reference nested inside another statement",
            json!([rule(
                "a",
                json!({
                    "NotStatement": {
                        "Statement": { "RuleGroupReferenceStatement": { "ARN": &regional_arn } }
                    }
                })
            )]),
            "WAFInvalidParameterException",
        ),
        (
            "a tree one level past the nesting bound",
            json!([rule(
                "a",
                nested_statement(MAX_STATEMENT_NESTING_DEPTH + 1, sqli_statement())
            )]),
            "WAFInvalidParameterException",
        ),
    ];

    for (case, rules, code) in cases {
        waf.refused(
            "CheckCapacity",
            json!({ "Scope": "REGIONAL", "Rules": rules }),
            code,
            case,
        )
        .await;
    }

    // A refusal costs the caller nothing: the next priceable list is still priced.
    let priceable = json!([group_running_rule(
        "a",
        0,
        json!({ "RuleGroupReferenceStatement": { "ARN": &regional_arn } })
    )]);
    let actual = check_capacity(&waf, "REGIONAL", &priceable).await;
    assert_eq!(
        actual, 17,
        "case \"after the refusals\": expected Capacity 17, got {actual}"
    );
}

/// `RuleGroup$Capacity` is the limit the rules must fit: "when you add or modify
/// the rules in a rule group, WAF enforces this limit", and "you can check the
/// capacity for a set of rules using CheckCapacity" — so a save is accepted exactly
/// when `CheckCapacity`'s price for its rules is at most the capacity it declares.
///
/// One row per rule list and per position relative to that price: exactly at it,
/// one below, one above, far above, and the published minimum with no rules at all.
/// A refused save stores nothing, which the retry at the required capacity under
/// the same name proves; an accepted one stores the DECLARED capacity, not the
/// price of its rules.
#[tokio::test]
async fn create_rule_group_enforces_the_declared_capacity_against_the_rules_price() {
    let waf = Waf::new();
    let rule_lists: Vec<(&str, Value)> = vec![
        ("no rules", json!([])),
        ("one byte match", json!([rule("a", byte_match_statement())])),
        (
            "a SQLi and an XSS rule",
            ordered(json!([rule("a", sqli_statement()), rule("b", xss_statement())])),
        ),
        (
            "a nested logical statement",
            json!([rule(
                "a",
                json!({
                    "AndStatement": {
                        "Statements": [byte_match_statement(), regex_match_statement()]
                    }
                })
            )]),
        ),
    ];

    for (index, (list_case, rules)) in rule_lists.into_iter().enumerate() {
        // The price the service itself reports for these rules is the limit the save
        // is judged against, so the expectation is never a literal.
        let required = check_capacity(&waf, "REGIONAL", &rules).await;
        // `CapacityUnit` is `range 1..`, so the smallest declarable capacity is 1.
        let offsets: Vec<(&str, i64, bool)> = vec![
            ("one WCU below the price", required - 1, false),
            ("exactly the price", required.max(1), true),
            ("one WCU above the price", required + 1, true),
            ("far above the price", required + 1_000, true),
        ];
        for (offset_index, (offset_case, declared, accepted)) in offsets.into_iter().enumerate() {
            if declared < 1 {
                // Below `CapacityUnit`'s published minimum there is nothing to
                // enforce: the boundary refuses the constraint itself.
                continue;
            }
            let case = format!("{list_case}, {offset_case}");
            let name = format!("enforced-{index}-{offset_index}");
            let body = json!({
                "Name": &name,
                "Scope": "REGIONAL",
                "Capacity": declared,
                "VisibilityConfig": visibility(&name),
                "Rules": &rules,
            });
            if accepted {
                let created = waf.ok("CreateRuleGroup", body).await;
                let id = created["Summary"]["Id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: Summary.Id, got {created}"))
                    .to_string();
                let got = waf
                    .ok(
                        "GetRuleGroup",
                        json!({ "Name": &name, "Scope": "REGIONAL", "Id": id }),
                    )
                    .await;
                assert_eq!(
                    got["RuleGroup"]["Capacity"].as_i64(),
                    Some(declared),
                    "case {case:?}: the stored capacity must be the declared {declared}, got {got}"
                );
            } else {
                // `Capacity` is a parameter of this request, so a rule list that
                // does not fit it is the invalid parameter and not the account
                // quota; see `crate::rule_group_capacity_refusal`, which pins the
                // members that refusal carries.
                waf.refused(
                    "CreateRuleGroup",
                    body,
                    "WAFInvalidParameterException",
                    &case,
                )
                .await;
                // Nothing was stored: the same (Scope, Name) is still free for a save
                // that declares enough capacity.
                let retried = waf
                    .ok(
                        "CreateRuleGroup",
                        json!({
                            "Name": &name,
                            "Scope": "REGIONAL",
                            "Capacity": required,
                            "VisibilityConfig": visibility(&name),
                            "Rules": &rules,
                        }),
                    )
                    .await;
                assert!(
                    retried["Summary"]["Id"].is_string(),
                    "case {case:?}: the refused name must still be free, got {retried}"
                );
            }
        }
    }
}
