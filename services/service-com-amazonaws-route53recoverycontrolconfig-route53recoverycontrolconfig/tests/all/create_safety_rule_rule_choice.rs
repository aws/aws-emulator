// SPDX-License-Identifier: CC0-1.0

//! `CreateSafetyRule`'s rule choice, over HTTP: a request carries EXACTLY ONE of the two
//! rule members, and a refused create leaves the control panel exactly as it was.
//!
//! The rules under test, each asserted over a table that spans what it generalises over
//! rather than the one request that exposed it:
//!
//! * a safety-rule write names the resource the call is authorized against INSIDE one of
//!   its two optional rule members, so a request that carries neither of them — or carries
//!   one with no ARN in it — names no resource at all and is refused by the front door that
//!   takes that decision — HTTP 401, `x-amzn-errortype: UnauthorizedException`, body
//!   exactly `{"message":"Unauthorized"}` — ahead of anything the operation itself says
//!   about the request, for the update (`AssertionRuleUpdate` / `GatingRuleUpdate`) as much
//!   as for the create;
//! * a request carrying BOTH members DOES name a resource, so it reaches the operation,
//!   where the two members are the alternatives of ONE choice: it is refused with
//!   `ValidationException` (400) and one fixed sentence that interpolates no part of the
//!   request — whatever else the request carries, and whether or not the members it
//!   carries would be valid on their own;
//! * a refused create persists NOTHING: the control panel's safety rules are the same
//!   after the refusal as before it, and the refusal consumes neither the request's
//!   `ClientToken` nor the name it asked for;
//! * a request carrying exactly one member succeeds, answers under that member alone, and
//!   the created rule appears in the control panel's list under its own kind key with the
//!   submitted name.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{answered, call, enc, error_code, panel_with_controls, router};

/// The one sentence a create whose rule choice is wrong is refused with. It is fixed text:
/// no rule name, control-panel ARN or member name is interpolated into it.
const INVALID_RULE: &str = "Invalid Rule provided.";

/// The status the front door refuses a request it has no resource to authorize the caller
/// against with, the wire error code that refusal names in `x-amzn-errortype`, and the
/// whole sentence it states — none of them a shape this service's model declares
/// (`smithy_query(query="shape",shape="UnauthorizedException")` reports no such shape),
/// because the layer that answers is not the service.
const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");

/// The values `RuleType` declares, in the order the model declares them
/// (`smithy_query(query="shape",shape="RuleType")`).
const RULE_TYPES: &[&str] = &["ATLEAST", "AND", "OR"];

/// An id in the 8-4-4-4-12 form these resources carry, which nothing in the store holds.
const UNSTORED_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="ValidationException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// A `RuleConfig` body.
fn rule_config(rule_type: &str, inverted: bool, threshold: i32) -> Value {
    json!({ "Inverted": inverted, "Threshold": threshold, "Type": rule_type })
}

/// A `NewAssertionRule` body over `controls`.
fn assertion_member(
    panel_arn: &str,
    controls: &[String],
    name: &str,
    rule_type: &str,
    wait_period_ms: i32,
) -> Value {
    json!({
        "AssertedControls": controls,
        "ControlPanelArn": panel_arn,
        "Name": name,
        "RuleConfig": rule_config(rule_type, false, controls.len() as i32),
        "WaitPeriodMs": wait_period_ms
    })
}

/// A `NewGatingRule` body gating `gating_controls` over `target_controls`.
fn gating_member(
    panel_arn: &str,
    gating_controls: &[String],
    target_controls: &[String],
    name: &str,
    rule_type: &str,
    wait_period_ms: i32,
) -> Value {
    json!({
        "ControlPanelArn": panel_arn,
        "GatingControls": gating_controls,
        "Name": name,
        "RuleConfig": rule_config(rule_type, false, gating_controls.len() as i32),
        "TargetControls": target_controls,
        "WaitPeriodMs": wait_period_ms
    })
}

/// Send one `CreateSafetyRule` request and answer `(status, body)`.
async fn create_rule(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/safetyrule", Some(body)).await
}

/// The safety rules a control panel holds, as `(kind key, Name)` in list order — the
/// observable state a refused create must leave untouched.
async fn listed(router: &MultiServiceRouter, panel_arn: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "GET",
        &format!(
            "/controlpanel/{}/safetyrules?MaxResults=1000",
            enc(panel_arn)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list {panel_arn}: expected 200, got {status} {body:?}"
    );
    body["SafetyRules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            let kind = if rule["ASSERTION"].is_object() {
                "ASSERTION"
            } else if rule["GATING"].is_object() {
                "GATING"
            } else {
                "NEITHER"
            };
            (
                kind.to_string(),
                rule[kind]["Name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The ARN of a control panel this service does not hold, in the shape of a real one.
fn unstored_panel(panel_arn: &str) -> String {
    let (prefix, _) = panel_arn
        .rsplit_once('/')
        .expect("a control panel ARN names its resource after a '/'");
    format!("{prefix}/{UNSTORED_ID}")
}

/// One refused request: what makes its rule choice wrong, and the body that carries it.
struct Refused {
    case: String,
    body: Value,
}

/// The requests whose rule choice is wrong because they carry BOTH members — spanning
/// what else the request carries and whether the members would stand on their own.
///
/// A request carrying NEITHER member is not here: it names no resource for the call to be
/// authorized against, so the front door refuses it ahead of this operation
/// ([`a_write_naming_no_resource_is_refused_by_the_front_door`]).
fn refused_rows(panel_arn: &str, controls: &[String]) -> Vec<Refused> {
    let one = controls[..1].to_vec();
    let two = controls[..2].to_vec();
    let absent = unstored_panel(panel_arn);
    let unstored_control = format!("{panel_arn}/routingcontrol/{UNSTORED_ID}");

    let mut rows = vec![
        Refused {
            case: "both members, each valid on its own".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &one, "both-assertion", "ATLEAST", 5000),
                "GatingRule": gating_member(panel_arn, &one, &one, "both-gating", "AND", 5000)
            }),
        },
        Refused {
            case: "both members under one and the same name".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &two, "both-same-name", "ATLEAST", 5000),
                "GatingRule": gating_member(panel_arn, &one, &two, "both-same-name", "OR", 5000)
            }),
        },
        Refused {
            case: "both members, the gating one over an unstored routing control".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &one, "both-unstored-control", "ATLEAST", 5000),
                "GatingRule": gating_member(panel_arn, std::slice::from_ref(&unstored_control), &one, "both-unstored-gating", "AND", 5000)
            }),
        },
        Refused {
            case: "both members, both scoped to an unstored control panel".to_string(),
            body: json!({
                "AssertionRule": assertion_member(&absent, &one, "both-absent-panel", "ATLEAST", 5000),
                "GatingRule": gating_member(&absent, &one, &one, "both-absent-gating", "AND", 5000)
            }),
        },
        Refused {
            case: "both members, with a ClientToken and Tags".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &two, "both-token-assertion", "OR", 60000),
                "GatingRule": gating_member(panel_arn, &two, &one, "both-token-gating", "ATLEAST", 1),
                "ClientToken": "rule-choice-both",
                "Tags": { "suite": "rule-choice" }
            }),
        },
    ];

    // The choice is wrong whatever rule configuration the two members carry, so every
    // declared rule type appears on a both-members row too.
    for rule_type in RULE_TYPES {
        rows.push(Refused {
            case: format!("both members, both of rule type {rule_type}"),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &one, &format!("both-{rule_type}-assertion"), rule_type, 5000),
                "GatingRule": gating_member(panel_arn, &one, &two, &format!("both-{rule_type}-gating"), rule_type, 5000)
            }),
        });
    }
    rows
}

/// RULE: the two rule members of a create are the alternatives of one choice, so a request
/// carrying both of them — which names the resource the call is authorized against, and so
/// reaches the operation — is refused `ValidationException` (400) with one fixed sentence,
/// and the refusal persists nothing.
#[tokio::test]
async fn a_create_carrying_both_rule_members_is_refused_and_persists_nothing() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "rule-choice", 2).await;

    // A control panel with no rule yet, and the same panel once it holds one: a refusal
    // must neither create a rule nor disturb the rule that is already there.
    for phase in [
        "an empty control panel",
        "a control panel already holding one rule",
    ] {
        if phase == "a control panel already holding one rule" {
            let (status, answer) = create_rule(
                &router,
                json!({
                    "AssertionRule": assertion_member(
                        &panel_arn, &controls[..1], "rule-choice-baseline", "ATLEAST", 5000,
                    )
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case the baseline rule: expected 200, got {status} {answer:?}"
            );
        }

        let before = listed(&router, &panel_arn).await;
        let expected_before = if phase == "an empty control panel" {
            Vec::new()
        } else {
            vec![(
                "ASSERTION".to_string(),
                "rule-choice-baseline".to_string(),
            )]
        };
        assert_eq!(
            before, expected_before,
            "case {phase}: expected {expected_before:?} before any refusal, got {before:?}"
        );

        for row in refused_rows(&panel_arn, &controls) {
            let (status, answer) = create_rule(&router, row.body.clone()).await;
            assert_eq!(
                status, 400,
                "case {phase} / {}: expected 400, got {status} {answer:?}",
                row.case
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ValidationException",
                "case {phase} / {}: expected ValidationException, got {code:?} in {answer:?}",
                row.case
            );
            let actual = message(&answer);
            assert_eq!(
                actual, INVALID_RULE,
                "case {phase} / {}: expected {INVALID_RULE:?}, got {actual:?}",
                row.case
            );

            let after = listed(&router, &panel_arn).await;
            assert_eq!(
                after, before,
                "case {phase} / {}: a refused create persists nothing, expected {before:?}, got \
                 {after:?}",
                row.case
            );
        }
    }
}

/// RULE: a create carrying exactly one rule member succeeds, answers under that member
/// alone, and the rule it created is listed under its own kind key with the submitted name.
#[tokio::test]
async fn a_create_carrying_exactly_one_rule_member_is_served_and_listed_under_its_kind() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "one-member", 3).await;

    // Rows: both kinds, every declared rule type, and control lists of one to three
    // entries, so the served choice is not a property of one request shape.
    let mut rows: Vec<(String, &str, Value)> = Vec::new();
    for (index, rule_type) in RULE_TYPES.iter().enumerate() {
        let evaluated = controls[..=index].to_vec();
        let assertion_name = format!("one-assertion-{rule_type}");
        rows.push((
            assertion_name.clone(),
            "ASSERTION",
            json!({
                "AssertionRule": assertion_member(
                    &panel_arn, &evaluated, &assertion_name, rule_type, 5000 + index as i32,
                )
            }),
        ));
        let gating_name = format!("one-gating-{rule_type}");
        rows.push((
            gating_name.clone(),
            "GATING",
            json!({
                "GatingRule": gating_member(
                    &panel_arn,
                    &evaluated,
                    &controls[..1],
                    &gating_name,
                    rule_type,
                    5000 + index as i32,
                )
            }),
        ));
    }

    let mut expected: Vec<(String, String)> = Vec::new();
    for (name, kind, body) in &rows {
        let (status, answer) = create_rule(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {name}: expected 200, got {status} {answer:?}"
        );

        // The response fills the member of the kind created and leaves the other empty.
        let (filled, empty) = if *kind == "ASSERTION" {
            ("AssertionRule", "GatingRule")
        } else {
            ("GatingRule", "AssertionRule")
        };
        let arn = answer[filled]["SafetyRuleArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&format!("{panel_arn}/safetyrule/")),
            "case {name}: expected a safety-rule ARN under {panel_arn}, got {arn:?}"
        );
        let served_name = answer[filled]["Name"].as_str().unwrap_or_default();
        assert_eq!(
            served_name, name,
            "case {name}: expected the submitted name, got {served_name:?}"
        );
        assert!(
            answer[empty].is_null(),
            "case {name}: expected no {empty} in the answer, got {:?}",
            answer[empty]
        );

        // And the control panel's list gains exactly that one rule, under its kind key.
        expected.push((kind.to_string(), name.clone()));
        let after = listed(&router, &panel_arn).await;
        assert_eq!(
            after, expected,
            "case {name}: expected {expected:?} listed, got {after:?}"
        );
    }
}

/// RULE: a refused create consumes nothing — neither the `ClientToken` it carried nor the
/// name it asked for, so the very same request minus the wrong member is served.
#[tokio::test]
async fn a_refused_create_consumes_neither_its_client_token_nor_its_name() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "consumes", 2).await;
    let one = controls[..1].to_vec();

    // Rows: the member the retry keeps, and the token the refused request carried.
    for (case, token, kind, retry) in [
        (
            "an assertion rule after a both-members refusal",
            "consumes-token-assertion",
            "ASSERTION",
            json!({
                "AssertionRule": assertion_member(&panel_arn, &one, "consumes-assertion", "ATLEAST", 5000)
            }),
        ),
        (
            "a gating rule after a both-members refusal",
            "consumes-token-gating",
            "GATING",
            json!({
                "GatingRule": gating_member(&panel_arn, &one, &one, "consumes-gating", "AND", 5000)
            }),
        ),
    ] {
        let name = retry[if kind == "ASSERTION" {
            "AssertionRule"
        } else {
            "GatingRule"
        }]["Name"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The refused request carries the token AND the name the retry will ask for.
        let mut both = retry.clone();
        let other = if kind == "ASSERTION" {
            json!(gating_member(&panel_arn, &one, &one, &name, "OR", 5000))
        } else {
            json!(assertion_member(&panel_arn, &one, &name, "OR", 5000))
        };
        both[if kind == "ASSERTION" {
            "GatingRule"
        } else {
            "AssertionRule"
        }] = other;
        both["ClientToken"] = json!(token);

        let (status, answer) = create_rule(&router, both).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            actual, INVALID_RULE,
            "case {case}: expected {INVALID_RULE:?}, got {actual:?}"
        );

        // Same token, same name, one member: served, because the refusal took neither.
        let mut served = retry.clone();
        served["ClientToken"] = json!(token);
        let (status, answer) = create_rule(&router, served).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the retry to be served 200, got {status} {answer:?}"
        );

        let listed_now = listed(&router, &panel_arn).await;
        assert!(
            listed_now.contains(&(kind.to_string(), name.clone())),
            "case {case}: expected ({kind}, {name}) listed, got {listed_now:?}"
        );
    }
}

/// A body carrying each member of `members` with a null value: the member is present and
/// names nothing, which is the same as not carrying it.
fn null_members(members: &[&str]) -> Value {
    let mut object = serde_json::Map::new();
    for member in members {
        object.insert((*member).to_string(), Value::Null);
    }
    Value::Object(object)
}

/// A body carrying `carrier` with `carried` inside it.
fn carrying(carrier: &str, carried: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(carrier.to_string(), carried);
    Value::Object(object)
}

/// An `AssertionRuleUpdate` / `GatingRuleUpdate` body addressing the rule `rule_arn`.
fn rule_update(rule_arn: &str, name: &str, wait_period_ms: i32) -> Value {
    json!({ "Name": name, "SafetyRuleArn": rule_arn, "WaitPeriodMs": wait_period_ms })
}

/// The members each safety-rule write carries the ARN of the resource it acts on inside,
/// with the method that write is sent by — the two operations of this model whose target
/// travels in members the model leaves optional
/// (`smithy_query(query="operation",operation="CreateSafetyRule,UpdateSafetyRule")`).
const SAFETY_RULE_WRITES: [(&str, &str, [&str; 2], &str); 2] = [
    (
        "CreateSafetyRule",
        "POST",
        ["AssertionRule", "GatingRule"],
        "ControlPanelArn",
    ),
    (
        "UpdateSafetyRule",
        "PUT",
        ["AssertionRuleUpdate", "GatingRuleUpdate"],
        "SafetyRuleArn",
    ),
];

/// RULE: a safety-rule write names the resource the call is authorized against only inside
/// one of its two optional members, so a request that carries neither of them, or carries
/// one with no ARN in it, names no resource at all and is refused by the front door that
/// takes that decision — HTTP 401, `x-amzn-errortype: UnauthorizedException`, body exactly
/// `{"message":"Unauthorized"}` — and nothing it asked for runs.
///
/// The rows span both dimensions the rule generalises over: WHICH write names no resource
/// (the create and the update), and HOW the request fails to name one (nothing else in the
/// body, a `ClientToken`, `Tags`, both, either member present but null, a member present
/// carrying nothing, carrying everything but its ARN, carrying an empty ARN, not a
/// structure at all, a body that is not an object, and no body whatsoever).
#[tokio::test]
async fn a_safety_rule_write_naming_no_resource_is_refused_by_the_front_door() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "target", 1).await;

    // One stored rule, so every row can be shown to have changed nothing: the panel's list
    // and the rule's own members are read back after each refusal.
    let (status, created) = create_rule(
        &router,
        json!({
            "AssertionRule": assertion_member(
                &panel_arn, &controls[..1], "target-baseline", "ATLEAST", 5000,
            )
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the baseline rule: expected 200, got {status} {created:?}"
    );
    let rule_arn = created["AssertionRule"]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let before = listed(&router, &panel_arn).await;
    let (expected_status, expected_code, expected_message) = UNAUTHORIZED;

    let mut rows: Vec<(String, &str, Option<Value>)> = Vec::new();
    for (operation, method, carriers, arn) in SAFETY_RULE_WRITES {
        rows.push((
            format!("{operation}: an otherwise empty request"),
            method,
            Some(json!({})),
        ));
        rows.push((
            format!("{operation}: only a ClientToken"),
            method,
            Some(json!({ "ClientToken": "target-nameless" })),
        ));
        rows.push((
            format!("{operation}: only Tags"),
            method,
            Some(json!({ "Tags": { "suite": "target" } })),
        ));
        rows.push((
            format!("{operation}: a ClientToken and Tags"),
            method,
            Some(json!({
                "ClientToken": "target-nameless-extras",
                "Tags": { "suite": "target" }
            })),
        ));
        rows.push((
            format!("{operation}: {} present but null", carriers[0]),
            method,
            Some(null_members(&carriers[..1])),
        ));
        rows.push((
            format!("{operation}: both members present but null"),
            method,
            Some(null_members(&carriers)),
        ));
        // A carrier the request DOES send, but with no ARN in it: the member names no
        // resource either, whatever else it carries.
        rows.push((
            format!("{operation}: {} carrying nothing", carriers[0]),
            method,
            Some(carrying(carriers[0], json!({}))),
        ));
        rows.push((
            format!("{operation}: {} without its {arn}", carriers[1]),
            method,
            Some(carrying(
                carriers[1],
                json!({ "Name": "target-no-arn", "WaitPeriodMs": 5000 }),
            )),
        ));
        rows.push((
            format!("{operation}: {} with an empty {arn}", carriers[0]),
            method,
            Some(carrying(carriers[0], carrying(arn, json!("")))),
        ));
        rows.push((
            format!("{operation}: {} that is not a structure", carriers[0]),
            method,
            Some(carrying(carriers[0], json!(1))),
        ));
        rows.push((
            format!("{operation}: a body that is not an object"),
            method,
            Some(json!([])),
        ));
        rows.push((format!("{operation}: no body at all"), method, None));
    }

    for (case, method, body) in &rows {
        let (status, code, answer) = answered(&router, method, "/safetyrule", body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case}: expected {expected_status} {expected_code}, got {status} {code:?} \
             {answer:?}"
        );
        // The sentence and nothing else: not one of this service's modelled errors, whose
        // bodies carry a `__type` and travel at the model's own statuses.
        assert_eq!(
            answer,
            json!({ "message": expected_message }),
            "case {case}: expected the body {:?}, got {answer:?}",
            json!({ "message": expected_message })
        );

        let after = listed(&router, &panel_arn).await;
        assert_eq!(
            after, before,
            "case {case}: expected the panel's rules unchanged at {before:?}, got {after:?}"
        );
        let (status, read) = call(
            &router,
            "GET",
            &format!("/safetyrule/{}", enc(&rule_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the stored rule still readable, got {status} {read:?}"
        );
        assert_eq!(
            (
                read["AssertionRule"]["Name"].clone(),
                read["AssertionRule"]["WaitPeriodMs"].clone()
            ),
            (json!("target-baseline"), json!(5000)),
            "case {case}: expected the stored rule unchanged, got {read:?}"
        );
    }
}

/// RULE: naming the resource is all that stage reads — a safety-rule write that carries an
/// ARN in either member reaches the operation and gets the operation's own answer, whether
/// the resource it named is stored, is not stored, is named by a value that is not an ARN at
/// all, or is named twice over.
#[tokio::test]
async fn a_safety_rule_write_that_names_a_resource_reaches_the_operation() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "named", 1).await;
    let one = controls[..1].to_vec();
    let absent_panel = unstored_panel(&panel_arn);

    let (status, created) = create_rule(
        &router,
        json!({
            "AssertionRule": assertion_member(&panel_arn, &one, "named-baseline", "ATLEAST", 5000)
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the baseline rule: expected 200, got {status} {created:?}"
    );
    let rule_arn = created["AssertionRule"]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let absent_rule = format!("{panel_arn}/safetyrule/{UNSTORED_ID}");

    // (case, method, body, expected status, expected error code — empty for a served call)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a create naming a stored control panel",
            "POST",
            json!({
                "AssertionRule": assertion_member(&panel_arn, &one, "named-served", "ATLEAST", 5000)
            }),
            200,
            "",
        ),
        (
            "a create naming a control panel this account does not hold",
            "POST",
            json!({
                "AssertionRule": assertion_member(&absent_panel, &one, "named-absent", "ATLEAST", 5000)
            }),
            400,
            "ValidationException",
        ),
        (
            "a create naming a control panel through both members",
            "POST",
            json!({
                "AssertionRule": assertion_member(&panel_arn, &one, "named-both-assertion", "ATLEAST", 5000),
                "GatingRule": gating_member(&panel_arn, &one, &one, "named-both-gating", "AND", 5000)
            }),
            400,
            "ValidationException",
        ),
        (
            "an update naming a stored rule",
            "PUT",
            json!({ "AssertionRuleUpdate": rule_update(&rule_arn, "named-renamed", 6000) }),
            200,
            "",
        ),
        (
            "an update naming an ARN holding no rule",
            "PUT",
            json!({ "AssertionRuleUpdate": rule_update(&absent_rule, "named-absent", 6000) }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an update naming a rule through both members",
            "PUT",
            json!({
                "AssertionRuleUpdate": rule_update(&rule_arn, "named-both", 6000),
                "GatingRuleUpdate": rule_update(&rule_arn, "named-both", 6000)
            }),
            400,
            "ValidationException",
        ),
        (
            // A value in the ARN slot names what the caller asked to act on, so its FORM is
            // the layers-behind-the-door's to refuse, not this stage's.
            "an update naming a rule by a value that is no ARN",
            "PUT",
            json!({ "AssertionRuleUpdate": rule_update("not-an-arn", "named-malformed", 6000) }),
            400,
            "",
        ),
    ];

    for (case, method, body, expected_status, expected_code) in &rows {
        let (status, code, answer) =
            answered(&router, method, "/safetyrule", Some(body.clone())).await;
        assert_eq!(
            status, *expected_status,
            "case {case}: expected {expected_status}, got {status} {code:?} {answer:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                code, *expected_code,
                "case {case}: expected {expected_code}, got {code:?} {answer:?}"
            );
        }
    }
}

/// RULE's other side: every operation whose target the model makes the request carry — in a
/// URI label or in a member it declares `@required` — is left to the layers behind that
/// stage, so a body omitting it is NOT answered as a caller the front door has no resource
/// to authorize.
#[tokio::test]
async fn an_operation_whose_target_the_model_requires_is_not_refused_by_that_stage() {
    let router = router();
    let (unauthorized_status, unauthorized_code, _sentence) = UNAUTHORIZED;

    // Every other write of this model that carries an ARN in its body, each sent with a
    // body that names none.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("CreateControlPanel", "POST", "/controlpanel"),
        ("CreateRoutingControl", "POST", "/routingcontrol"),
        ("UpdateCluster", "PUT", "/cluster"),
        ("UpdateControlPanel", "PUT", "/controlpanel"),
        ("UpdateRoutingControl", "PUT", "/routingcontrol"),
    ];

    for (operation, method, path) in &rows {
        let (status, code, answer) = answered(&router, method, path, Some(json!({}))).await;
        assert_ne!(
            status, unauthorized_status,
            "case {operation}: expected a refusal of its own rather than \
             {unauthorized_status}, got {status} {code:?} {answer:?}"
        );
        assert_ne!(
            code, unauthorized_code,
            "case {operation}: expected a refusal of its own rather than \
             {unauthorized_code}, got {status} {code:?} {answer:?}"
        );
    }
}
