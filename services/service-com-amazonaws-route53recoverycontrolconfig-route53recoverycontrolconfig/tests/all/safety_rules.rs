// SPDX-License-Identifier: CC0-1.0

//! Safety rules, end to end over HTTP: created in a control panel, read back, listed,
//! updated and deleted.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.
//!
//! The rules under test, each asserted over a table that spans what it generalises over
//! rather than the one request that exposed it:
//!
//! * a created safety rule is PERSISTED: every member of the created rule is the one the
//!   request asked for, and a later read projects the same record. The table spans both
//!   kinds of rule, every `RuleType` the model declares, both values of `Inverted`,
//!   several thresholds, control lists of one to three entries and several wait periods;
//! * a safety rule's controls are the routing controls of the ONE control panel it is
//!   scoped to, so a control of another panel is refused like one that is not stored;
//! * a control panel's rules are listed together in creation order, each under the `Rule`
//!   member its own kind fills, and paged by `MaxResults`/`NextToken`;
//! * the rule configuration is stored as the request sends it: the control plane does not
//!   couple the threshold to how many controls the rule names, so a threshold below, at
//!   and above that number is served and read back unchanged;
//! * an update changes the name and the wait period of the rule it addresses and nothing
//!   else, and is refused for the other kind of rule, for an ARN holding no rule, for a
//!   name another rule of the same control panel already holds, and — by the front door
//!   ahead of it, because such a request names no resource for the call to be authorized
//!   against — for an update carrying neither kind;
//! * a deleted rule is gone, and neither the control panel that scopes a rule nor a
//!   routing control the rule references is deletable while the rule is there.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{answered, call, enc, error_code, panel_with_controls, router};

/// The values `RuleType` declares, in the order the model declares them
/// (`smithy_query(query="shape",shape="RuleType")`).
const RULE_TYPES: &[&str] = &["ATLEAST", "AND", "OR"];

/// The `Status` a resource reports before its deployment is due, of the set
/// `smithy_query(query="shape",shape="Status")` declares.
const STATUS_PENDING: &str = "PENDING";

/// The account id every emulated ARN carries, which is also every resource's `Owner`.
const ACCOUNT: &str = "000000000000";

/// The ARN prefix AWS documents for recovery-control resources.
const ARN_PREFIX: &str = "arn:aws:route53-recovery-control::000000000000:";

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="ValidationException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The sentence a request carrying both kinds of rule is refused with: one fixed sentence
/// with no part taken from the request.
fn invalid_rule() -> String {
    "Invalid Rule provided.".to_string()
}

/// The fixed sentence an ARN that resolves to no safety rule is refused with.
fn not_found(arn: &str) -> String {
    format!("No record corresponding to safety rule Arn({arn})")
}

/// The sentence a create that references a resource this service does not hold is refused
/// with.
fn does_not_exist(resource: &str, arn: &str) -> String {
    format!("{resource} {arn} does not exist.")
}

/// The sentence an update addressed to a rule of the other kind is refused with.
fn wrong_kind(arn: &str, stored: &str, requested: &str) -> String {
    format!("Safety Rule {arn} is a {stored} rule, not a {requested} rule.")
}

/// The sentence a delete is refused with while the addressed resource still owns, or is
/// referenced by, dependents.
fn cannot_delete(resource: &str, dependents: &str) -> String {
    format!("Cannot delete a {resource} with {dependents}.")
}

/// The sentence a create or a rename whose name is already taken in its control panel is
/// refused with: the colliding rule's own id, then the name the request asked for.
fn already_exists(existing_arn: &str, name: &str) -> String {
    let existing_id = existing_arn
        .rsplit_once('/')
        .map_or(existing_arn, |(_, id)| id);
    format!("A safety rule with the specified name already exists: {existing_id}, {name}")
}

/// A `RuleConfig` body.
fn rule_config(rule_type: &str, inverted: bool, threshold: i32) -> Value {
    json!({ "Inverted": inverted, "Threshold": threshold, "Type": rule_type })
}

/// Read one safety rule back, answering the whole `DescribeSafetyRule` body.
async fn describe(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/safetyrule/{}", enc(arn)), None).await;
    assert_eq!(
        status, 200,
        "case describe {arn}: expected 200, got {status} {body:?}"
    );
    body
}

/// Create one safety rule, answering the whole `CreateSafetyRule` body.
async fn create_rule(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/safetyrule", Some(body)).await
}

/// The thresholds to try against a rule that evaluates `evaluated` controls: the lowest
/// value, one, the published upper bound (the number of controls itself), one past it and
/// one well past it — each distinct value once.
fn thresholds(evaluated: usize) -> Vec<i32> {
    let count = evaluated as i32;
    let mut all = vec![0, 1, count, count + 1, count + 5];
    all.sort_unstable();
    all.dedup();
    all
}

/// One created-rule row: which kind, with which rule configuration and controls.
struct Case {
    label: String,
    /// `"ASSERTION"` or `"GATING"` — the member of `Rule` this kind fills.
    kind: &'static str,
    rule_type: &'static str,
    inverted: bool,
    threshold: i32,
    /// How many of the panel's routing controls the rule evaluates.
    evaluated: usize,
    wait_period_ms: i32,
}

/// RULE: a created safety rule is persisted as asked — control panel, name, wait period,
/// rule configuration and controls — and `DescribeSafetyRule` projects the same record,
/// under the member of the kind created and no other.
#[tokio::test]
async fn a_created_safety_rule_is_persisted_and_read_back() {
    let router = router();
    // Four controls: three to evaluate and one more to be a gating rule's target, so the
    // target list is demonstrably its own member and not a copy of the evaluated one.
    let (panel_arn, controls) = panel_with_controls(&router, "persisted", 4).await;

    // Rows differ along every value the projection is read off: the kind of rule, the rule
    // type, `Inverted`, the threshold, how many controls the rule evaluates and the wait
    // period.
    let mut cases: Vec<Case> = Vec::new();
    for kind in ["ASSERTION", "GATING"] {
        for (index, rule_type) in RULE_TYPES.iter().enumerate() {
            let evaluated = index + 1;
            cases.push(Case {
                label: format!("{kind} {rule_type} over {evaluated}"),
                kind,
                rule_type,
                inverted: index % 2 == 0,
                threshold: evaluated as i32,
                evaluated,
                wait_period_ms: 5000 + (index as i32) * 1500,
            });
        }
    }
    // The threshold a rule evaluating every control can still be met by one control, and
    // the wait period AWS documents as the default
    // (`smithy_query(query="doc",shape="NewAssertionRule",member="WaitPeriodMs")`).
    cases.push(Case {
        label: "ASSERTION ATLEAST 1 of 3".to_string(),
        kind: "ASSERTION",
        rule_type: "ATLEAST",
        inverted: false,
        threshold: 1,
        evaluated: 3,
        wait_period_ms: 5000,
    });
    cases.push(Case {
        label: "GATING OR with a zero threshold".to_string(),
        kind: "GATING",
        rule_type: "OR",
        inverted: true,
        threshold: 0,
        evaluated: 2,
        wait_period_ms: 1,
    });

    for case in &cases {
        let name = case.label.replace(' ', "-");
        let evaluated: Vec<String> = controls[..case.evaluated].to_vec();
        let targets: Vec<String> = controls[controls.len() - 1..].to_vec();
        let config = rule_config(case.rule_type, case.inverted, case.threshold);
        let body = if case.kind == "ASSERTION" {
            json!({
                "AssertionRule": {
                    "AssertedControls": evaluated,
                    "ControlPanelArn": panel_arn,
                    "Name": name,
                    "RuleConfig": config,
                    "WaitPeriodMs": case.wait_period_ms
                },
                "Tags": { "suite": "safety-rules" }
            })
        } else {
            json!({
                "GatingRule": {
                    "ControlPanelArn": panel_arn,
                    "GatingControls": evaluated,
                    "Name": name,
                    "RuleConfig": config,
                    "TargetControls": targets,
                    "WaitPeriodMs": case.wait_period_ms
                },
                "Tags": { "suite": "safety-rules" }
            })
        };

        let (status, created) = create_rule(&router, body).await;
        assert_eq!(
            status, 200,
            "case {}: expected CreateSafetyRule 200, got {status} {created:?}",
            case.label
        );
        let other = if case.kind == "ASSERTION" {
            "GatingRule"
        } else {
            "AssertionRule"
        };
        assert_eq!(
            created[other],
            Value::Null,
            "case {}: a rule of one kind fills only its own member, got {:?}",
            case.label,
            created[other]
        );

        let projected = if case.kind == "ASSERTION" {
            created["AssertionRule"].clone()
        } else {
            created["GatingRule"].clone()
        };
        let arn = projected["SafetyRuleArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&format!("{panel_arn}/safetyrule/")),
            "case {}: expected an ARN nested under the control panel's, got {arn:?}",
            case.label
        );
        assert!(
            arn.starts_with(ARN_PREFIX),
            "case {}: expected a recovery-control ARN, got {arn:?}",
            case.label
        );
        assert_eq!(
            projected["ControlPanelArn"],
            json!(panel_arn),
            "case {}: expected the panel the request named, got {:?}",
            case.label,
            projected["ControlPanelArn"]
        );
        assert_eq!(
            projected["Name"],
            json!(name),
            "case {}: expected the requested name, got {:?}",
            case.label,
            projected["Name"]
        );
        assert_eq!(
            projected["WaitPeriodMs"],
            json!(case.wait_period_ms),
            "case {}: expected the requested wait period, got {:?}",
            case.label,
            projected["WaitPeriodMs"]
        );
        assert_eq!(
            projected["RuleConfig"],
            rule_config(case.rule_type, case.inverted, case.threshold),
            "case {}: expected the requested rule configuration, got {:?}",
            case.label,
            projected["RuleConfig"]
        );
        assert_eq!(
            projected["Owner"],
            json!(ACCOUNT),
            "case {}: expected owner {ACCOUNT}, got {:?}",
            case.label,
            projected["Owner"]
        );
        assert_eq!(
            projected["Status"],
            json!(STATUS_PENDING),
            "case {}: a freshly created rule is {STATUS_PENDING}, got {:?}",
            case.label,
            projected["Status"]
        );
        if case.kind == "ASSERTION" {
            assert_eq!(
                projected["AssertedControls"],
                json!(evaluated),
                "case {}: expected the asserted controls as sent, got {:?}",
                case.label,
                projected["AssertedControls"]
            );
        } else {
            assert_eq!(
                projected["GatingControls"],
                json!(evaluated),
                "case {}: expected the gating controls as sent, got {:?}",
                case.label,
                projected["GatingControls"]
            );
            assert_eq!(
                projected["TargetControls"],
                json!(targets),
                "case {}: expected the target controls as sent, got {:?}",
                case.label,
                projected["TargetControls"]
            );
        }

        // The round trip: the read answers from the stored record, so the whole body is
        // the created one again — only the derived `Status` may have moved on.
        let read = describe(&router, &arn).await;
        let mut expected = created.clone();
        let mut read_normalised = read.clone();
        for body in [&mut expected, &mut read_normalised] {
            for member in ["AssertionRule", "GatingRule"] {
                if body[member].is_object() {
                    body[member]["Status"] = Value::Null;
                }
            }
        }
        assert_eq!(
            read_normalised, expected,
            "case {}: expected DescribeSafetyRule to project the created record, got {read:?}",
            case.label
        );
    }
}

/// RULE: a control panel's safety rules are listed together in creation order, each under
/// the `Rule` member its own kind fills, and a page is cut by `MaxResults` with a
/// `NextToken` that resumes after it.
#[tokio::test]
async fn safety_rules_of_both_kinds_are_listed_in_creation_order() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "listed", 2).await;

    // Interleaved kinds, so the order under test is creation order and not kind order.
    let plan: &[(&str, &str)] = &[
        ("ASSERTION", "listed-assertion-1"),
        ("GATING", "listed-gating-1"),
        ("ASSERTION", "listed-assertion-2"),
        ("GATING", "listed-gating-2"),
    ];
    let mut created: Vec<(&str, String)> = Vec::new();
    for (kind, name) in plan {
        let body = if *kind == "ASSERTION" {
            json!({
                "AssertionRule": {
                    "AssertedControls": [controls[0]],
                    "ControlPanelArn": panel_arn,
                    "Name": name,
                    "RuleConfig": rule_config("ATLEAST", false, 1),
                    "WaitPeriodMs": 5000
                }
            })
        } else {
            json!({
                "GatingRule": {
                    "ControlPanelArn": panel_arn,
                    "GatingControls": [controls[0]],
                    "Name": name,
                    "RuleConfig": rule_config("AND", false, 1),
                    "TargetControls": [controls[1]],
                    "WaitPeriodMs": 5000
                }
            })
        };
        let (status, answer) = create_rule(&router, body).await;
        assert_eq!(
            status, 200,
            "case create {name}: expected 200, got {status} {answer:?}"
        );
        let member = if *kind == "ASSERTION" {
            "AssertionRule"
        } else {
            "GatingRule"
        };
        created.push((
            kind,
            answer[member]["SafetyRuleArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }

    // Rows: a page smaller than the list, each exact size, and one past it. `MaxResults`
    // is modelled `range 1..1000` (`smithy_query(query="operation",
    // operation="ListSafetyRules")`), so 1 is its published minimum.
    for page_size in [1_usize, 2, 3, 4, 5] {
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut path = format!(
                "/controlpanel/{}/safetyrules?MaxResults={page_size}",
                enc(&panel_arn)
            );
            if let Some(token) = &next {
                path.push_str(&format!("&NextToken={}", enc(token)));
            }
            let (status, body) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size}: expected 200, got {status} {body:?}"
            );
            let rules = body["SafetyRules"].as_array().cloned().unwrap_or_default();
            assert!(
                rules.len() <= page_size,
                "case MaxResults={page_size}: a page holds at most that many, got {}",
                rules.len()
            );
            for rule in &rules {
                let kind = if rule["ASSERTION"].is_object() {
                    "ASSERTION"
                } else {
                    "GATING"
                };
                let other = if kind == "ASSERTION" {
                    "GATING"
                } else {
                    "ASSERTION"
                };
                assert!(
                    rule[kind].is_object() && rule[other].is_null(),
                    "case MaxResults={page_size}: a listed rule fills exactly one member, got \
                     {rule:?}"
                );
                walked.push((
                    kind.to_string(),
                    rule[kind]["SafetyRuleArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ));
            }
            next = body["NextToken"].as_str().map(str::to_string);
            if next.is_none() {
                break;
            }
        }
        let expected: Vec<(String, String)> = created
            .iter()
            .map(|(kind, arn)| ((*kind).to_string(), arn.clone()))
            .collect();
        assert_eq!(
            walked, expected,
            "case MaxResults={page_size}: expected every rule once, in creation order, got \
             {walked:?}"
        );
    }

    // A list of a control panel that holds no rule is an empty page, not a refusal.
    let (empty_panel, _) = panel_with_controls(&router, "empty", 0).await;
    let (status, body) = call(
        &router,
        "GET",
        &format!("/controlpanel/{}/safetyrules", enc(&empty_panel)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case a panel with no rules: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["SafetyRules"],
        json!([]),
        "case a panel with no rules: expected an empty page, got {:?}",
        body["SafetyRules"]
    );
}

/// RULE: the rule configuration is stored as the request sends it. The control plane does
/// not couple `RuleConfig$Threshold` to how many controls the rule names — the threshold is
/// how many of them must be set, and it is the DATA plane that evaluates that — so a
/// threshold below, at and above the number of controls is served and read back unchanged.
#[tokio::test]
async fn the_rule_configuration_is_stored_as_sent_whatever_the_threshold() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "threshold", 3).await;

    // Rows: both kinds, control lists of one to three entries, and every threshold from
    // the lowest to one past the number of controls and well past it.
    let mut stored = 0_usize;
    for kind in ["ASSERTION", "GATING"] {
        for evaluated in 1..=3_usize {
            for threshold in thresholds(evaluated) {
                let label = format!("{kind} threshold {threshold} over {evaluated} controls");
                let name = format!("threshold-{kind}-{evaluated}-{threshold}");
                let set: Vec<String> = controls[..evaluated].to_vec();
                let config = rule_config("ATLEAST", false, threshold);
                let member = if kind == "ASSERTION" {
                    "AssertionRule"
                } else {
                    "GatingRule"
                };
                let body = if kind == "ASSERTION" {
                    json!({
                        "AssertionRule": {
                            "AssertedControls": set,
                            "ControlPanelArn": panel_arn,
                            "Name": name,
                            "RuleConfig": config,
                            "WaitPeriodMs": 5000
                        }
                    })
                } else {
                    json!({
                        "GatingRule": {
                            "ControlPanelArn": panel_arn,
                            "GatingControls": set,
                            "Name": name,
                            "RuleConfig": config,
                            "TargetControls": [controls[0]],
                            "WaitPeriodMs": 5000
                        }
                    })
                };
                let (status, answer) = create_rule(&router, body).await;
                assert_eq!(
                    status, 200,
                    "case {label}: the control plane serves any threshold, got {status} \
                     {answer:?}"
                );
                assert_eq!(
                    answer[member]["RuleConfig"], config,
                    "case {label}: expected the configuration as sent, got {:?}",
                    answer[member]["RuleConfig"]
                );
                stored += 1;

                // And the read answers the same configuration, so it was stored rather
                // than echoed.
                let arn = answer[member]["SafetyRuleArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let read = describe(&router, &arn).await;
                assert_eq!(
                    read[member]["RuleConfig"], config,
                    "case {label}: expected the stored configuration, got {:?}",
                    read[member]["RuleConfig"]
                );
            }
        }
    }

    // Every one of them is in the control panel's own list.
    let (status, body) = call(
        &router,
        "GET",
        &format!(
            "/controlpanel/{}/safetyrules?MaxResults=1000",
            enc(&panel_arn)
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "case list: expected 200, got {status} {body:?}");
    let listed = body["SafetyRules"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        stored,
        "case after the whole table: expected every created rule stored, got {}",
        listed.len()
    );
}

/// RULE: a create that only REFERENCES a resource this service does not hold is refused on
/// its input with `ValidationException` (400) naming that resource, and a request carrying
/// both kinds of rule is refused the same way.
#[tokio::test]
async fn a_create_naming_an_unstored_resource_or_both_kinds_is_refused() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "referenced", 1).await;
    let unstored_panel = format!("{ARN_PREFIX}controlpanel/00000000-0000-0000-0000-000000000000");
    let unstored_control = format!("{panel_arn}/routingcontrol/00000000-0000-0000-0000-000000000000");

    let assertion = |panel: &str, control: &str, name: &str| {
        json!({
            "AssertionRule": {
                "AssertedControls": [control],
                "ControlPanelArn": panel,
                "Name": name,
                "RuleConfig": rule_config("ATLEAST", false, 1),
                "WaitPeriodMs": 5000
            }
        })
    };
    let gating = |panel: &str, gating_control: &str, target: &str, name: &str| {
        json!({
            "GatingRule": {
                "ControlPanelArn": panel,
                "GatingControls": [gating_control],
                "Name": name,
                "RuleConfig": rule_config("AND", false, 1),
                "TargetControls": [target],
                "WaitPeriodMs": 5000
            }
        })
    };

    // (case, body, the sentence it must be refused with)
    let rows: Vec<(String, Value, String)> = vec![
        (
            "an assertion rule in an unstored control panel".to_string(),
            assertion(&unstored_panel, &controls[0], "unstored-panel"),
            does_not_exist("Control Panel", &unstored_panel),
        ),
        (
            "a gating rule in an unstored control panel".to_string(),
            gating(&unstored_panel, &controls[0], &controls[0], "unstored-panel-gating"),
            does_not_exist("Control Panel", &unstored_panel),
        ),
        (
            "an assertion rule over an unstored routing control".to_string(),
            assertion(&panel_arn, &unstored_control, "unstored-control"),
            does_not_exist("Routing Control", &unstored_control),
        ),
        (
            "a gating rule whose TARGET control is unstored".to_string(),
            gating(&panel_arn, &controls[0], &unstored_control, "unstored-target"),
            does_not_exist("Routing Control", &unstored_control),
        ),
        (
            "both kinds of rule at once".to_string(),
            json!({
                "AssertionRule": assertion(&panel_arn, &controls[0], "both-assertion")["AssertionRule"],
                "GatingRule": gating(&panel_arn, &controls[0], &controls[0], "both-gating")["GatingRule"]
            }),
            invalid_rule(),
        ),
    ];

    for (case, body, expected) in &rows {
        let (status, answer) = create_rule(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            &actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // A name already taken inside the control panel that scopes it is refused the same
    // way: 400, not a transient class a retry could win.
    let (status, answer) = create_rule(
        &router,
        assertion(&panel_arn, &controls[0], "taken-name"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the first rule under a name: expected 200, got {status} {answer:?}"
    );
    let taken_arn = answer["AssertionRule"]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, answer) =
        create_rule(&router, assertion(&panel_arn, &controls[0], "taken-name")).await;
    assert_eq!(
        status, 400,
        "case a second rule under the same name: expected 400, got {status} {answer:?}"
    );
    let actual = message(&answer);
    assert_eq!(
        actual,
        already_exists(&taken_arn, "taken-name"),
        "case a second rule under the same name: expected the already-exists sentence, got \
         {actual:?}"
    );

    // A routing control of ANOTHER control panel is not referenceable either: a rule's
    // controls are the controls of the panel it is scoped to.
    let (other_panel, other_controls) = panel_with_controls(&router, "other", 1).await;
    for (case, body, refused_arn) in [
        (
            "an asserted control of another control panel",
            assertion(&panel_arn, &other_controls[0], "foreign-asserted"),
            other_controls[0].clone(),
        ),
        (
            "a gating control of another control panel",
            gating(&panel_arn, &other_controls[0], &controls[0], "foreign-gating"),
            other_controls[0].clone(),
        ),
        (
            "a target control of another control panel",
            gating(&panel_arn, &controls[0], &other_controls[0], "foreign-target"),
            other_controls[0].clone(),
        ),
        (
            // And the other way round, so the rule is about the panel the rule names and
            // not about which panel came first.
            "a control of the first panel, in a rule scoped to the second",
            assertion(&other_panel, &controls[0], "foreign-reversed"),
            controls[0].clone(),
        ),
    ] {
        let (status, answer) = create_rule(&router, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            actual,
            does_not_exist("Routing Control", &refused_arn),
            "case {case}: expected the does-not-exist sentence, got {actual:?}"
        );
    }
}

/// RULE: an update moves the rule it addresses onto the name and wait period it carries
/// and changes nothing else; the kinds do not cross, and an ARN holding no rule is
/// `ResourceNotFoundException` (404).
#[tokio::test]
async fn an_update_persists_the_name_and_wait_period_of_the_rule_it_addresses() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "updated", 2).await;

    let assertion_body = json!({
        "AssertionRule": {
            "AssertedControls": [controls[0], controls[1]],
            "ControlPanelArn": panel_arn,
            "Name": "updated-assertion",
            "RuleConfig": rule_config("ATLEAST", false, 2),
            "WaitPeriodMs": 5000
        }
    });
    let gating_body = json!({
        "GatingRule": {
            "ControlPanelArn": panel_arn,
            "GatingControls": [controls[0]],
            "Name": "updated-gating",
            "RuleConfig": rule_config("AND", true, 1),
            "TargetControls": [controls[1]],
            "WaitPeriodMs": 5000
        }
    });
    let (status, assertion) = create_rule(&router, assertion_body).await;
    assert_eq!(status, 200, "case create assertion: got {status} {assertion:?}");
    let assertion_arn = assertion["AssertionRule"]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, gating) = create_rule(&router, gating_body).await;
    assert_eq!(status, 200, "case create gating: got {status} {gating:?}");
    let gating_arn = gating["GatingRule"]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: both kinds, and wait periods spanning a shorter one, a longer one and the
    // value the rule already carries (which changes nothing).
    let rows: Vec<(&str, &str, &str, &str, i32)> = vec![
        ("assertion renamed and slowed", "ASSERTION", &assertion_arn, "assertion-renamed", 60_000),
        ("assertion hurried", "ASSERTION", &assertion_arn, "assertion-renamed", 1),
        ("assertion unchanged", "ASSERTION", &assertion_arn, "assertion-renamed", 1),
        ("gating renamed and slowed", "GATING", &gating_arn, "gating-renamed", 7_500),
        ("gating hurried", "GATING", &gating_arn, "gating-renamed", 250),
    ];

    for (case, kind, arn, name, wait_period_ms) in &rows {
        let member = if *kind == "ASSERTION" {
            "AssertionRule"
        } else {
            "GatingRule"
        };
        let update_member = if *kind == "ASSERTION" {
            "AssertionRuleUpdate"
        } else {
            "GatingRuleUpdate"
        };
        let before = describe(&router, arn).await;
        let (status, updated) = call(
            &router,
            "PUT",
            "/safetyrule",
            Some(json!({
                update_member: {
                    "Name": name,
                    "SafetyRuleArn": arn,
                    "WaitPeriodMs": wait_period_ms
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateSafetyRule 200, got {status} {updated:?}"
        );
        assert_eq!(
            updated[member]["Name"],
            json!(name),
            "case {case}: expected the requested name, got {:?}",
            updated[member]["Name"]
        );
        assert_eq!(
            updated[member]["WaitPeriodMs"],
            json!(wait_period_ms),
            "case {case}: expected the requested wait period, got {:?}",
            updated[member]["WaitPeriodMs"]
        );
        // Everything an update does not touch is the stored value still.
        for untouched in ["RuleConfig", "ControlPanelArn", "SafetyRuleArn", "Owner"] {
            assert_eq!(
                updated[member][untouched], before[member][untouched],
                "case {case}: an update changes only the name and the wait period, but \
                 {untouched} became {:?}",
                updated[member][untouched]
            );
        }
        // And the read answers the update, so it was persisted rather than echoed.
        let read = describe(&router, arn).await;
        assert_eq!(
            read[member]["Name"],
            json!(name),
            "case {case}: expected the stored name, got {:?}",
            read[member]["Name"]
        );
        assert_eq!(
            read[member]["WaitPeriodMs"],
            json!(wait_period_ms),
            "case {case}: expected the stored wait period, got {:?}",
            read[member]["WaitPeriodMs"]
        );
    }

    // The kinds do not cross, and neither does an ARN holding no rule.
    let unstored = format!("{panel_arn}/safetyrule/00000000-0000-0000-0000-000000000000");
    let refusals: Vec<(String, Value, u16, &str, String)> = vec![
        (
            "a gating update addressed to an assertion rule".to_string(),
            json!({ "GatingRuleUpdate": { "Name": "crossed", "SafetyRuleArn": assertion_arn, "WaitPeriodMs": 5000 } }),
            400,
            "ValidationException",
            wrong_kind(&assertion_arn, "ASSERTION", "GATING"),
        ),
        (
            "an assertion update addressed to a gating rule".to_string(),
            json!({ "AssertionRuleUpdate": { "Name": "crossed", "SafetyRuleArn": gating_arn, "WaitPeriodMs": 5000 } }),
            400,
            "ValidationException",
            wrong_kind(&gating_arn, "GATING", "ASSERTION"),
        ),
        (
            "an update addressed to an ARN holding no rule".to_string(),
            json!({ "AssertionRuleUpdate": { "Name": "absent", "SafetyRuleArn": unstored, "WaitPeriodMs": 5000 } }),
            404,
            "ResourceNotFoundException",
            not_found(&unstored),
        ),
        (
            "an update carrying neither kind".to_string(),
            json!({}),
            401,
            "UnauthorizedException",
            // An update carries the ARN it addresses INSIDE one of its two optional
            // members, so a request carrying neither names no resource for the call to be
            // authorized against and is refused by the front door ahead of this operation
            // (`tests/all/create_safety_rule_rule_choice.rs` spans that rule).
            "Unauthorized".to_string(),
        ),
        (
            "a rename onto a name the panel's other rule holds".to_string(),
            json!({ "AssertionRuleUpdate": { "Name": "gating-renamed", "SafetyRuleArn": assertion_arn, "WaitPeriodMs": 5000 } }),
            400,
            "ValidationException",
            already_exists(&gating_arn, "gating-renamed"),
        ),
    ];

    for (case, body, expected_status, expected_code, expected_message) in &refusals {
        // The error code is read off `x-amzn-errortype`, which every refusal here names it
        // in — a modelled error of this service and the front door's own refusal alike;
        // only the modelled ones also carry it in the body's `__type`.
        let (status, code, answer) = answered(&router, "PUT", "/safetyrule", Some(body.clone())).await;
        assert_eq!(
            status, *expected_status,
            "case {case}: expected {expected_status}, got {status} {answer:?}"
        );
        assert_eq!(
            code, *expected_code,
            "case {case}: expected {expected_code}, got {code:?} in {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            &actual, expected_message,
            "case {case}: expected {expected_message:?}, got {actual:?}"
        );
    }
}

/// RULE: a deleted safety rule is gone — a later read of its ARN is refused with the
/// not-found sentence — and a control panel is only deletable once the rules scoped to it
/// are gone.
#[tokio::test]
async fn a_deleted_safety_rule_is_gone_and_blocks_its_panel_until_it_is() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "deleted", 1).await;

    let mut arns: Vec<String> = Vec::new();
    for (kind, name) in [("ASSERTION", "deleted-assertion"), ("GATING", "deleted-gating")] {
        let body = if kind == "ASSERTION" {
            json!({
                "AssertionRule": {
                    "AssertedControls": [controls[0]],
                    "ControlPanelArn": panel_arn,
                    "Name": name,
                    "RuleConfig": rule_config("ATLEAST", false, 1),
                    "WaitPeriodMs": 5000
                }
            })
        } else {
            json!({
                "GatingRule": {
                    "ControlPanelArn": panel_arn,
                    "GatingControls": [controls[0]],
                    "Name": name,
                    "RuleConfig": rule_config("OR", false, 1),
                    "TargetControls": [controls[0]],
                    "WaitPeriodMs": 5000
                }
            })
        };
        let (status, created) = create_rule(&router, body).await;
        assert_eq!(status, 200, "case create {name}: got {status} {created:?}");
        let member = if kind == "ASSERTION" {
            "AssertionRule"
        } else {
            "GatingRule"
        };
        arns.push(
            created[member]["SafetyRuleArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // The panel still holds both rules, so it is not deletable yet — and neither is the
    // routing control the rules reference.
    let (status, refused) = call(
        &router,
        "DELETE",
        &format!("/controlpanel/{}", enc(&panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case a panel still holding rules: expected 400, got {status} {refused:?}"
    );
    // Its only routing control is the one both rules name, so the dependent rule is what
    // the refusal reports — the panel's routing controls are what a panel delete reports.
    let (status, refused) = call(
        &router,
        "DELETE",
        &format!("/routingcontrol/{}", enc(&controls[0])),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case a routing control a rule references: expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case a routing control a rule references: expected ValidationException, got \
         {refused:?}"
    );
    let actual = message(&refused);
    assert_eq!(
        actual,
        cannot_delete("Routing Control", "Safety Rules"),
        "case a routing control a rule references: expected the cannot-delete sentence, got \
         {actual:?}"
    );

    for arn in &arns {
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/safetyrule/{}", enc(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case delete {arn}: expected 200, got {status} {deleted:?}"
        );

        // Gone: the read of the same ARN is refused with the fixed sentence, and so is a
        // second delete.
        for (case, method) in [("read", "GET"), ("second delete", "DELETE")] {
            let (status, gone) = call(
                &router,
                method,
                &format!("/safetyrule/{}", enc(arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case} of the deleted {arn}: expected 404, got {status} {gone:?}"
            );
            assert_eq!(
                error_code(&gone),
                "ResourceNotFoundException",
                "case {case} of the deleted {arn}: expected ResourceNotFoundException, got \
                 {gone:?}"
            );
            let actual = message(&gone);
            assert_eq!(
                actual,
                not_found(arn),
                "case {case} of the deleted {arn}: expected the not-found sentence, got \
                 {actual:?}"
            );
        }
    }

    // With both rules gone the panel's own list is empty again, and the routing control
    // they referenced is deletable.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/controlpanel/{}/safetyrules", enc(&panel_arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "case list after deletes: got {status} {body:?}");
    assert_eq!(
        body["SafetyRules"],
        json!([]),
        "case list after deletes: expected an empty page, got {:?}",
        body["SafetyRules"]
    );
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/routingcontrol/{}", enc(&controls[0])),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the routing control once no rule references it: expected 200, got {status} \
         {deleted:?}"
    );
}
