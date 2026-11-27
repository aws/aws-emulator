// SPDX-License-Identifier: CC0-1.0

//! The semantic checks `CreateRuleGroup` and `UpdateRuleGroup` apply to the rules
//! a request submits, and the refusals they answer a rule group they cannot load
//! with.
//!
//! Six rules, each asserted over a table rather than on one request:
//!
//! * a stateless rule group's `Priority` settings are unique
//!   (`StatelessRule$Priority`: "you must ensure that the priority settings are
//!   unique for the rule group");
//! * every entry of a stateless rule's `Actions` names an action — a standard one
//!   or a custom action the same document declares — and only one of them is
//!   standard (`RuleDefinition$Actions`);
//! * the `RulesSource` member a document carries belongs to the half of the
//!   service the `Type` names (`RulesSource`: "you can use an instance of this for
//!   either stateless rules or stateful rules");
//! * a `Type` outside the values the public API accepts is refused by the
//!   operation that WRITES a rule group, with the modelled client error rather
//!   than a validation clause;
//! * a stateful rule submitted in Suricata flat format is one the engine can load:
//!   a supported action, a signature id, and an id no earlier rule of the same
//!   string already loaded;
//! * a load that did not complete answers the loader's own diagnostics, one per
//!   stage a rule failed at, terminated by the single summary line.
//!
//! Every refusal here must also leave the store untouched, which each table
//! proves by reading the rule group back and then creating it, under the SAME
//! name, with the defect corrected.

use serde_json::{json, Value};

use crate::harness::{error_code, error_message, string_at, Harness};
use crate::rule_group_lifecycle::{create_request, stateless_group, stateless_rule};

/// The modelled client error every refusal in this suite carries
/// (`smithy_query(query="shape",shape="InvalidRequestException")`: client error,
/// HTTP 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The refusal a rule-group type the public API does not accept carries.
const INVALID_TYPE_MESSAGE: &str =
    "The rule group type isn't valid. Enter a valid type, and try again.";

/// A stateless rule group document holding `rules` and declaring `custom` as a
/// custom action, when one is named.
fn stateless_document(rules: Vec<Value>, custom: Option<&str>) -> Value {
    let mut document = stateless_group(rules);
    if let Some(name) = custom {
        document["RulesSource"]["StatelessRulesAndCustomActions"]["CustomActions"] = json!([{
            "ActionName": name,
            "ActionDefinition": { "PublishMetricAction": {
                "Dimensions": [{ "Value": name }],
            }},
        }]);
    }
    document
}

/// One stateless rule at `priority` carrying `actions`.
fn rule_with_actions(priority: i64, actions: Vec<&str>) -> Value {
    json!({
        "Priority": priority,
        "RuleDefinition": { "Actions": actions, "MatchAttributes": {} },
    })
}

/// Assert that `name` names no stored rule group of `group_type`, so a refused
/// request left the store as it was.
async fn assert_nothing_stored(harness: &Harness, case: &str, name: &str, group_type: &str) {
    let (status, body) = harness
        .call(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": name, "Type": group_type }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: a refused create must store nothing, got {status} {body:?}"
    );
}

/// Assert that `request` is refused with `message`, and that it stored nothing.
async fn assert_refused(harness: &Harness, case: &str, request: &Value, message: &str) {
    let (status, body) = harness.call("CreateRuleGroup", request).await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body).as_str()),
        (400, INVALID_REQUEST, message),
        "case {case:?}: expected 400 {INVALID_REQUEST} {message:?}, got {status} {body:?}"
    );
    let name = string_at(request, &["RuleGroupName"]).to_string();
    let group_type = request
        .get("Type")
        .and_then(Value::as_str)
        .unwrap_or("STATELESS")
        .to_string();
    assert_nothing_stored(harness, case, &name, &group_type).await;
}

/// RULE: within one stateless rule group, two rules may not share a `Priority`;
/// the request is refused with the duplicated value in the parameter brackets and
/// the rule's input path as context, nothing is stored, and the same name is still
/// free for the corrected request.
///
/// The rows span where the clash sits (the first two rules, a later pair, a rule
/// repeating the first after an intervening one), how many rules the group
/// carries (2, 3, 4), the duplicated value (the lowest the model admits, a
/// middling one, the highest) and a three-way clash.
#[tokio::test]
async fn stateless_rule_priorities_must_be_unique_within_the_rule_group() {
    // (case, the priorities the rules carry, the one that is duplicated)
    let rows: &[(&str, &[i64], i64)] = &[
        ("the-first-pair", &[10, 10], 10),
        ("a-later-pair", &[1, 20, 20], 20),
        ("repeating-the-first", &[7, 300, 7], 7),
        ("the-lowest-priority", &[1, 1], 1),
        ("the-highest-priority", &[65535, 2, 65535], 65535),
        ("three-rules-at-one-priority", &[5, 5, 5], 5),
        ("a-clash-among-four", &[1, 2, 3, 2], 2),
    ];

    let harness = Harness::new();
    for (case, priorities, duplicated) in rows {
        let rules: Vec<Value> = priorities
            .iter()
            .map(|priority| stateless_rule(*priority, "aws:pass", json!({})))
            .collect();
        let expected = format!(
            "Priority has duplicate values, parameter: [{duplicated}], context: \
             StatelessRulesAndCustomActions.StatelessRules[Priority={duplicated}]"
        );
        let request = create_request(
            case,
            "STATELESS",
            100,
            &json!({ "RuleGroup": stateless_document(rules, None) }),
        );
        assert_refused(&harness, case, &request, &expected).await;

        // The corrected request takes the name the refused one did not: every
        // priority distinct, so nothing clashes.
        let corrected: Vec<Value> = (1..=priorities.len() as i64)
            .map(|priority| stateless_rule(priority, "aws:pass", json!({})))
            .collect();
        let created = harness
            .ok(
                "CreateRuleGroup",
                &create_request(
                    case,
                    "STATELESS",
                    100,
                    &json!({ "RuleGroup": stateless_document(corrected, None) }),
                ),
            )
            .await;
        assert_eq!(
            string_at(&created, &["RuleGroupResponse", "RuleGroupName"]),
            *case,
            "case {case:?}: expected the corrected request to take the free name, got {created:?}"
        );
    }
}

/// RULE: every entry of a stateless rule's `Actions` must name an action — one of
/// the standard actions or a custom action the same `StatelessRulesAndCustomActions`
/// declares — and a list may name only one standard action; the refusal names the
/// offending value and the rule's own input path, located by that rule's priority.
///
/// The rows span: a stateful action's name, a token in no namespace, a custom
/// action no document declares, one declared under another name, the offending
/// rule's position in the group, and two standard actions in one list.
#[tokio::test]
async fn a_stateless_rules_actions_must_name_an_action() {
    // (case, the rules, the declared custom action, the stem, the offending
    //  value, the priority of the rule it sits on)
    type Row<'a> = (&'a str, Vec<Value>, Option<&'a str>, &'a str, &'a str, i64);
    let rows: Vec<Row> = vec![
        (
            "a-stateful-actions-name",
            vec![rule_with_actions(10, vec!["aws:alert"])],
            None,
            "is invalid",
            "aws:alert",
            10,
        ),
        (
            "no-namespace-at-all",
            vec![rule_with_actions(1, vec!["pass"])],
            None,
            "is invalid",
            "pass",
            1,
        ),
        (
            "a-custom-action-nothing-declares",
            vec![rule_with_actions(4, vec!["aws:pass", "MyMetrics"])],
            None,
            "is invalid",
            "MyMetrics",
            4,
        ),
        (
            "a-custom-action-declared-under-another-name",
            vec![rule_with_actions(4, vec!["aws:pass", "MyMetrics"])],
            Some("OtherMetrics"),
            "is invalid",
            "MyMetrics",
            4,
        ),
        (
            "the-second-rule-of-the-group",
            vec![
                rule_with_actions(1, vec!["aws:pass"]),
                rule_with_actions(2, vec!["aws:reject"]),
            ],
            None,
            "is invalid",
            "aws:reject",
            2,
        ),
        (
            "two-standard-actions-at-once",
            vec![rule_with_actions(9, vec!["aws:pass", "aws:drop"])],
            None,
            "cannot exist together",
            "aws:pass, aws:drop",
            9,
        ),
    ];

    let harness = Harness::new();
    for (case, rules, custom, stem, offending, priority) in rows {
        let expected = format!(
            "Actions {stem}, parameter: [{offending}], context: \
             StatelessRulesAndCustomActions.StatelessRules[Priority={priority}].RuleDefinition.Actions"
        );
        let request = create_request(
            case,
            "STATELESS",
            100,
            &json!({ "RuleGroup": stateless_document(rules, custom) }),
        );
        assert_refused(&harness, case, &request, &expected).await;
    }

    // The other side of the rule: a standard action and a custom action the same
    // document declares are accepted together, and the rule group is stored.
    let created = harness
        .ok(
            "CreateRuleGroup",
            &create_request(
                "declared-custom-action",
                "STATELESS",
                100,
                &json!({
                    "RuleGroup": stateless_document(
                        vec![rule_with_actions(4, vec!["aws:pass", "MyMetrics"])],
                        Some("MyMetrics"),
                    ),
                }),
            ),
        )
        .await;
    assert_eq!(
        string_at(&created, &["RuleGroupResponse", "RuleGroupName"]),
        "declared-custom-action",
        "case \"declared-custom-action\": expected the rule group created, got {created:?}"
    );
    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "declared-custom-action", "Type": "STATELESS" }),
        )
        .await;
    assert_eq!(
        described["RuleGroup"]["RulesSource"]["StatelessRulesAndCustomActions"]["StatelessRules"]
            [0]["RuleDefinition"]["Actions"],
        json!(["aws:pass", "MyMetrics"]),
        "case \"declared-custom-action\": expected the actions stored as given, got {described:?}"
    );
}

/// RULE: a document's `RulesSource` member must belong to the half of the service
/// its `Type` names; one on the wrong half is refused with that member named and
/// its input path as context, and no value from the document is quoted back.
///
/// The rows span every `RulesSource` member against a type of the other half, and
/// both stateful types.
#[tokio::test]
async fn a_rules_source_on_the_wrong_half_of_the_service_is_refused() {
    let stateful_rules = json!([{
        "Action": "PASS",
        "Header": {
            "Protocol": "TCP",
            "Source": "ANY",
            "SourcePort": "ANY",
            "Direction": "ANY",
            "Destination": "ANY",
            "DestinationPort": "ANY",
        },
        "RuleOptions": [{ "Keyword": "sid", "Settings": ["1"] }],
    }]);
    let stateless_source = stateless_group(vec![stateless_rule(1, "aws:pass", json!({}))])
        ["RulesSource"]
        .clone();

    // (case, the type, the rules source, the member the refusal names)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "stateless-rules-in-a-stateful-group",
            "STATEFUL",
            stateless_source.clone(),
            "StatelessRulesAndCustomActions",
        ),
        (
            "stateless-rules-in-a-stateful-domain-group",
            "STATEFUL_DOMAIN",
            stateless_source,
            "StatelessRulesAndCustomActions",
        ),
        (
            "stateful-rules-in-a-stateless-group",
            "STATELESS",
            json!({ "StatefulRules": stateful_rules }),
            "StatefulRules",
        ),
        (
            "a-rules-string-in-a-stateless-group",
            "STATELESS",
            json!({ "RulesString": "pass tcp any any -> any any (sid:1;)" }),
            "RulesString",
        ),
        (
            "a-domain-list-in-a-stateless-group",
            "STATELESS",
            json!({ "RulesSourceList": {
                "Targets": [".example.test"],
                "TargetTypes": ["TLS_SNI"],
                "GeneratedRulesType": "ALLOWLIST",
            }}),
            "RulesSourceList",
        ),
    ];

    let harness = Harness::new();
    for (case, group_type, rules_source, member) in rows {
        let expected = format!("{member} is invalid, context: RulesSource.{member}");
        let request = create_request(
            case,
            group_type,
            100,
            &json!({ "RuleGroup": { "RulesSource": rules_source } }),
        );
        assert_refused(&harness, case, &request, &expected).await;
    }
}

/// RULE: on an operation that WRITES a rule group, a `Type` outside the values the
/// public API accepts is the operation's own refusal — the modelled client error
/// with the constant for an invalid rule-group type — while the same value on an
/// operation that only picks which rule groups to report stays the boundary's
/// validation refusal.
///
/// The rows span the three values the service's own enumeration carries beside the
/// public three, a value no enumeration carries, two accepted values in the wrong
/// case, and the empty string — each on the create and on the listing, plus the
/// update path and a request that breaks a constraint the create does declare
/// alongside the type.
#[tokio::test]
async fn a_type_the_api_does_not_accept_is_refused_as_an_invalid_rule_group_type() {
    let rows: &[&str] = &[
        "FIREWALL",
        "IPS",
        "IPS_DOMAIN",
        "STATEFULL",
        "Stateful",
        "stateless",
        "",
    ];

    let harness = Harness::new();
    for group_type in rows {
        let (status, body) = harness
            .call(
                "CreateRuleGroup",
                &json!({
                    "RuleGroupName": "typed-group",
                    "Type": group_type,
                    "Capacity": 10,
                    "RuleGroup": stateless_group(vec![stateless_rule(1, "aws:pass", json!({}))]),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body).as_str()),
            (400, INVALID_REQUEST, INVALID_TYPE_MESSAGE),
            "case {group_type:?}: expected 400 {INVALID_REQUEST} {INVALID_TYPE_MESSAGE:?}, got \
             {status} {body:?}"
        );

        // The same value on the listing, which takes the member to pick which rule
        // groups to report: there it is a constraint on the input, and the
        // framework's own refusal stands.
        let (status, body) = harness
            .call("ListRuleGroups", &json!({ "Type": group_type }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {group_type:?}: expected the listing to keep its validation refusal, got \
             {status} {body:?}"
        );
    }

    // A request that ALSO breaks a constraint the create does declare never reaches
    // the operation, so it is answered with that constraint's clause — the type,
    // being no constraint of this input, is not among the clauses.
    let (status, body) = harness
        .call(
            "CreateRuleGroup",
            &json!({
                "RuleGroupName": "typed-group",
                "Type": "IPS",
                "Capacity": 10,
                "Description": "d".repeat(513),
                "RuleGroup": stateless_group(vec![stateless_rule(1, "aws:pass", json!({}))]),
            }),
        )
        .await;
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string();
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"an invalid type beside another violation\": expected the declared constraint's \
         refusal, got {status} {body:?}"
    );
    assert_eq!(
        message,
        "1 validation error detected: Value at 'description' failed to satisfy constraint: \
         Member must have length less than or equal to 512",
        "case \"an invalid type beside another violation\": expected the one declared \
         constraint stated, got {message:?}"
    );

    // The update of a rule group writes the type too, so it answers the same
    // refusal — and the rule group it names survives the refused request.
    harness
        .shared_rule_group("updated-group", "STATELESS", 10)
        .await;
    let (status, body) = harness
        .call(
            "UpdateRuleGroup",
            &json!({
                "RuleGroupName": "updated-group",
                "Type": "IPS",
                "UpdateToken": "00000000-0000-4000-8000-000000000000",
                "RuleGroup": stateless_group(vec![stateless_rule(1, "aws:pass", json!({}))]),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body).as_str()),
        (400, INVALID_REQUEST, INVALID_TYPE_MESSAGE),
        "case \"the update path\": expected 400 {INVALID_REQUEST} {INVALID_TYPE_MESSAGE:?}, got \
         {status} {body:?}"
    );
    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "updated-group", "Type": "STATELESS" }),
        )
        .await;
    assert_eq!(
        string_at(&described, &["RuleGroupResponse", "RuleGroupName"]),
        "updated-group",
        "case \"the update path\": expected the stored rule group untouched, got {described:?}"
    );
}

/// RULE: `Capacity` is sufficient when it covers what the submitted rules
/// consume, and nothing else — there is no floor of its own: a capacity below the
/// consumed units is refused with the capacity the rules REQUIRE in the parameter
/// brackets, whatever the number the request asked for.
///
/// The rows span a capacity of zero, a negative one, one short of what the rules
/// consume, and (on the other side) exactly the consumed units.
#[tokio::test]
async fn capacity_is_sufficient_when_it_covers_what_the_rules_consume() {
    // Three rules, each with no match criteria: "a rule with no criteria
    // specified in any of its match settings has a capacity requirement of 1".
    let rules: Vec<Value> = (1..=3)
        .map(|priority| stateless_rule(priority, "aws:pass", json!({})))
        .collect();
    let required = 3;

    // (case, the capacity the request asks for)
    let rows: &[(&str, i64)] = &[
        ("zero", 0),
        ("negative", -1),
        ("one-short", required - 1),
        ("far-short", 1),
    ];

    let harness = Harness::new();
    let expected = format!(
        "StatelessRulesAndCustomActions capacity exceeded, parameter: [{required}], context: \
         RulesSource.StatelessRulesAndCustomActions"
    );
    for (case, capacity) in rows {
        let request = create_request(
            case,
            "STATELESS",
            *capacity,
            &json!({ "RuleGroup": stateless_group(rules.clone()) }),
        );
        assert_refused(&harness, case, &request, &expected).await;
    }

    let created = harness
        .ok(
            "CreateRuleGroup",
            &create_request(
                "exactly-enough",
                "STATELESS",
                required,
                &json!({ "RuleGroup": stateless_group(rules.clone()) }),
            ),
        )
        .await;
    assert_eq!(
        (
            created["RuleGroupResponse"]["Capacity"].clone(),
            created["RuleGroupResponse"]["ConsumedCapacity"].clone()
        ),
        (json!(required), json!(required)),
        "case \"exactly-enough\": expected the capacity accepted and reported, got {created:?}"
    );
}

/// RULE: a stateful rule submitted in Suricata flat format must be one the engine
/// can load — its action is one the service implements, it assigns a signature id,
/// and the id it assigns is assigned by no EARLIER rule of the same string — and a
/// rule group carrying one that cannot be loaded is refused with the rule as the
/// service loaded it and the reason.
///
/// A defect the service reads in ONE rule's own text is reported with that rule
/// named beside the reason, and the rules are not loaded. A string whose rules
/// each read cleanly is loaded, and a failed load answers the loader's own
/// diagnostics instead: per rule it could not load, in load order, the distinct
/// diagnostic of each stage that rule failed at, and then the single summary line
/// that terminates a failed load.
///
/// The rows span: an action the service does not implement; a signature id
/// swallowed by an option value left unterminated; no signature id at all; an id
/// two rules share; the same rule written twice; three rules sharing one id; a
/// shared id beside a unique one; and every rule loading cleanly. Each row is
/// submitted BOTH through `Rules` and through `RuleGroup.RulesSource.RulesString`,
/// the two ways the same rules reach the service.
#[tokio::test]
async fn a_stateful_rule_in_suricata_flat_format_must_be_one_the_engine_can_load() {
    /// The refusal a rule whose own text the service could not read carries.
    fn unloadable(rule: &str, reason: &str) -> String {
        format!("stateful rule is invalid, rule: {rule}, reason: {reason}")
    }

    // (case, the rules string, the refusal it must carry)
    let rows: Vec<(&str, &str, Option<String>)> = vec![
        (
            "an-action-the-service-does-not-implement",
            r#"allow tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1;)"#,
            Some(unloadable(
                r#"allow tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1; gid:1234567890;)"#,
                "Action allow is not supported",
            )),
        ),
        (
            "a-signature-id-swallowed-by-an-unterminated-option",
            r#"pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443" sid:1001; rev:1;)"#,
            Some(unloadable(
                r#"pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443" sid:1001; rev:1; gid:1234567890;)"#,
                "sid not assigned",
            )),
        ),
        (
            "no-signature-id-at-all",
            r#"pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; rev:1;)"#,
            Some(unloadable(
                r#"pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; rev:1; gid:1234567890;)"#,
                "sid not assigned",
            )),
        ),
        (
            // The rule that loaded the id first is loaded; the one repeating it
            // is what the load could not load.
            "a-signature-id-two-rules-share",
            "pass tcp any any -> any 443 (sid:1001; rev:1;)\n\
             drop tcp any any -> any 80 (sid:1001; rev:1;)",
            Some(format!(
                "{}\n{LOAD_FAILED}",
                rejected("drop tcp any any -> any 80 (sid:1001; rev:1; gid:1234567890;)"),
            )),
        ),
        (
            // The same rule written twice: the second copy is the one the load
            // could not load.
            "a-repeated-rule",
            "pass tcp 10.1.0.0/16 any -> any 443 (msg:\"egress 443\"; sid:1001; rev:1;)\n\
             pass tcp 10.1.0.0/16 any -> any 443 (msg:\"egress 443\"; sid:1001; rev:1;)",
            Some(format!(
                "{}\n{LOAD_FAILED}",
                rejected(concat!(
                    r#"pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; "#,
                    "rev:1; gid:1234567890;)",
                )),
            )),
        ),
        (
            "three-rules-sharing-an-id",
            "pass tcp any any -> any 443 (sid:7; rev:1;)\n\
             drop tcp any any -> any 80 (sid:7; rev:1;)\n\
             reject tcp any any -> any 25 (sid:7; rev:1;)",
            Some(format!(
                "{}\n{}\n{LOAD_FAILED}",
                rejected("drop tcp any any -> any 80 (sid:7; rev:1; gid:1234567890;)"),
                rejected("reject tcp any any -> any 25 (sid:7; rev:1; gid:1234567890;)"),
            )),
        ),
        (
            // One repeated id beside one that is not: only the rule repeating an
            // already loaded id is reported.
            "one-shared-id-beside-a-unique-one",
            "pass tcp any any -> any 443 (sid:11; rev:1;)\n\
             pass udp any any -> any 53 (sid:12; rev:1;)\n\
             drop tcp any any -> any 80 (sid:11; rev:1;)",
            Some(format!(
                "{}\n{LOAD_FAILED}",
                rejected("drop tcp any any -> any 80 (sid:11; rev:1; gid:1234567890;)"),
            )),
        ),
        (
            "every-rule-loads",
            "# a comment, which carries no rule\n\
             pass tcp any any -> any 443 (sid:1; rev:1;)\n\
             alert udp any any -> any 53 (msg:\"dns\"; sid:2;)\n\
             reject tcp any any -> any 25 (sid:3;)",
            None,
        ),
    ];

    let harness = Harness::new();
    for (case, rules, refusal) in rows {
        for (variant, specification) in [
            ("rules", json!({ "Rules": rules })),
            (
                "rules-string",
                json!({ "RuleGroup": { "RulesSource": { "RulesString": rules } } }),
            ),
        ] {
            let case = format!("{case}-{variant}");
            let request = create_request(&case, "STATEFUL", 100, &specification);
            match &refusal {
                Some(message) => assert_refused(&harness, &case, &request, message).await,
                None => {
                    let created = harness.ok("CreateRuleGroup", &request).await;
                    assert_eq!(
                        created["RuleGroupResponse"]["ConsumedCapacity"],
                        json!(3),
                        "case {case:?}: expected the three rules to consume three units, got \
                         {created:?}"
                    );
                    let described = harness
                        .ok(
                            "DescribeRuleGroup",
                            &json!({ "RuleGroupName": case, "Type": "STATEFUL" }),
                        )
                        .await;
                    assert_eq!(
                        described["RuleGroup"]["RulesSource"]["RulesString"],
                        json!(rules),
                        "case {case:?}: expected the rules read back as given, got {described:?}"
                    );
                }
            }
        }
    }
}

/// The line that terminates a load that did not complete.
const LOAD_FAILED: &str = "stateful rule is invalid, reason: Loading signatures failed.";

/// The diagnostics a failed load answers for one rule it could not load: the
/// duplicate signature, then the loader's parse error for that same rule, each
/// naming the rule as the service loaded it.
fn rejected(rule: &str) -> String {
    format!(
        "stateful rule is invalid, reason: Duplicate signature \"{rule}\"\n\
         stateful rule is invalid, reason: error parsing signature \"{rule}\""
    )
}

/// The whole refusal a load answers when it could not load `rejected_rules` —
/// the rules as the service loaded them, in load order — or `None` when it
/// loaded everything: each rejected rule's diagnostics ([`rejected`]), then the
/// one summary line.
fn load_diagnostics(rejected_rules: &[&str]) -> Option<String> {
    (!rejected_rules.is_empty()).then(|| {
        let per_rule: Vec<String> = rejected_rules.iter().copied().map(rejected).collect();
        format!("{}\n{LOAD_FAILED}", per_rule.join("\n"))
    })
}

/// Assert that `answered` — the message an operation ANSWERED a failed load
/// with — lists each diagnostic once and ends with the single summary line.
fn assert_distinct_diagnostics_terminated_by_one_summary(case: &str, answered: &str) {
    let lines: Vec<&str> = answered.lines().collect();
    let mut distinct = lines.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        (
            distinct.len(),
            lines.last().copied(),
            lines.iter().filter(|line| **line == LOAD_FAILED).count(),
        ),
        (lines.len(), Some(LOAD_FAILED), 1),
        "case {case:?}: expected distinct diagnostics terminated by one summary, got {lines:?}"
    );
}

/// RULE: a load of a stateful rules string that did not complete is refused with
/// the loader's own diagnostics — in load order, per rule it could not load, the
/// DISTINCT diagnostic of each stage that rule failed at, no diagnostic repeated
/// — terminated by the single summary line reporting that the load failed; the
/// rule that loaded a signature id first is loaded rather than reported, and the
/// stored rule group is left as it was.
///
/// The rows span: one clash; three rules on one signature id; two separate
/// clashing ids; a clash at the end of a longer string carrying a comment and a
/// blank line; and a string every rule of which loads, which answers no
/// diagnostic and so no summary. Each row is driven through BOTH operations that
/// load a rules string, `CreateRuleGroup` and `UpdateRuleGroup`.
#[tokio::test]
async fn a_failed_suricata_load_answers_distinct_diagnostics_and_one_summary() {
    /// A rules string the service loads, which each update replaces.
    const LOADS: &str = "pass tcp any any -> any 443 (sid:900; rev:1;)";

    // (case, the rules string, the rules the load could not load, as loaded)
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "one-clash",
            "alert tcp any any -> any 8080 (msg:\"admin\"; sid:2001; rev:1;)\n\
             alert udp any any -> any 8080 (msg:\"admin\"; sid:2001; rev:1;)",
            vec![concat!(
                r#"alert udp any any -> any 8080 (msg:"admin"; sid:2001; "#,
                "rev:1; gid:1234567890;)",
            )],
        ),
        (
            "three-rules-on-one-id",
            "pass tcp any any -> any 443 (sid:5;)\n\
             drop tcp any any -> any 444 (sid:5;)\n\
             reject tcp any any -> any 445 (sid:5;)",
            vec![
                "drop tcp any any -> any 444 (sid:5; gid:1234567890;)",
                "reject tcp any any -> any 445 (sid:5; gid:1234567890;)",
            ],
        ),
        (
            "two-clashing-ids",
            "pass tcp any any -> any 443 (sid:31; rev:2;)\n\
             pass udp any any -> any 53 (sid:32; rev:2;)\n\
             drop tcp any any -> any 80 (sid:32; rev:2;)\n\
             drop udp any any -> any 69 (sid:31; rev:2;)",
            vec![
                "drop tcp any any -> any 80 (sid:32; rev:2; gid:1234567890;)",
                "drop udp any any -> any 69 (sid:31; rev:2; gid:1234567890;)",
            ],
        ),
        (
            "a-clash-at-the-end-of-a-longer-string",
            "# a comment, which carries no rule\n\
             pass tcp any any -> any 443 (sid:41;)\n\
             alert udp any any -> any 53 (msg:\"dns\"; sid:42;)\n\
             reject tcp any any -> any 25 (sid:43;)\n\
             \n\
             reject tcp any any -> any 26 (sid:43;)",
            vec!["reject tcp any any -> any 26 (sid:43; gid:1234567890;)"],
        ),
        (
            "every-rule-loads",
            "pass tcp any any -> any 443 (sid:51;)\n\
             drop udp any any -> any 53 (sid:52;)",
            vec![],
        ),
    ];

    let harness = Harness::new();
    for (case, rules, rejected_rules) in rows {
        let diagnostics = load_diagnostics(&rejected_rules);

        // The create path: the rules string is loaded before the rule group is
        // stored, so a failed load stores nothing.
        let create = create_request(case, "STATEFUL", 100, &json!({ "Rules": rules }));
        match &diagnostics {
            Some(message) => {
                let (status, body) = harness.call("CreateRuleGroup", &create).await;
                let answered = error_message(&body);
                assert_eq!(
                    (status, error_code(&body).as_str(), answered.as_str()),
                    (400, INVALID_REQUEST, message.as_str()),
                    "case {case:?}: expected the create refused with the load's diagnostics \
                     {message:?}, got {status} {body:?}"
                );
                assert_distinct_diagnostics_terminated_by_one_summary(case, &answered);
                assert_nothing_stored(&harness, case, case, "STATEFUL").await;
            }
            None => {
                harness.ok("CreateRuleGroup", &create).await;
            }
        }

        // The update path: the same load, over a rule group that already holds a
        // rules string the service loaded, which a failed load leaves alone.
        let name = format!("{case}-update");
        let seeded = harness
            .ok(
                "CreateRuleGroup",
                &create_request(&name, "STATEFUL", 100, &json!({ "Rules": LOADS })),
            )
            .await;
        let update = json!({
            "RuleGroupName": name,
            "Type": "STATEFUL",
            "UpdateToken": string_at(&seeded, &["UpdateToken"]),
            "Rules": rules,
        });
        let (status, body) = harness.call("UpdateRuleGroup", &update).await;
        let expected_rules = match &diagnostics {
            Some(message) => {
                let answered = error_message(&body);
                assert_eq!(
                    (status, error_code(&body).as_str(), answered.as_str()),
                    (400, INVALID_REQUEST, message.as_str()),
                    "case {case:?}: expected the update refused with the load's diagnostics, got \
                     {status} {body:?}"
                );
                assert_distinct_diagnostics_terminated_by_one_summary(case, &answered);
                LOADS
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update accepted, got {status} {body:?}"
                );
                rules
            }
        };
        let described = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": name, "Type": "STATEFUL" }),
            )
            .await;
        assert_eq!(
            described["RuleGroup"]["RulesSource"]["RulesString"],
            json!(expected_rules),
            "case {case:?}: expected the stored rules {expected_rules:?}, got {described:?}"
        );
    }
}
