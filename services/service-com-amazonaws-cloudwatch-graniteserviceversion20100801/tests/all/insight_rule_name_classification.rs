// SPDX-License-Identifier: CC0-1.0

//! Cover for how the batch Contributor Insights rule state operations judge ONE `RuleNames`
//! entry: `EnableInsightRules` and `DisableInsightRules`, read back through
//! `DescribeInsightRules`.
//!
//! The rule under test: **an entry of a batch's rule-name array that is not a rule NAME —
//! outside the 1..128 length or the printable-ASCII pattern `InsightRuleName` declares
//! (`smithy_query(query="shape",shape="InsightRuleName")`) — is reported as that entry's own
//! `Failures` entry of the invalid-parameter class on an HTTP 200, not as a refusal of the
//! request; every other name in the same batch is still resolved, so a held rule still moves
//! and a legal-but-unknown name is still reported as the resource that is not there.** "If the
//! operation could not process one of the rules, the following data is returned for each of
//! those rules." (`PartialFailure`) — the per-entry report IS the mechanism, and neither
//! operation models a refusal for the shape of one entry
//! (`smithy_query(query="operation",operation="EnableInsightRules,DisableInsightRules")`
//! declares only `InvalidParameterValueException`, `LimitExceededException` and
//! `MissingRequiredParameterException`).
//!
//! Rows differ along what the rule generalises over: which of the two operations is called,
//! which way the entry misses the grammar (no characters, more characters than the maximum, a
//! control byte, the delete byte, a byte above ASCII), where in the batch the entry sits, how
//! many unusable entries one request carries, and — on the other side of the same bounds — the
//! shortest, the longest and the punctuation-carrying names that ARE legal.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The two batch operations that move a rule's state, and the state each moves a rule to.
const STATE_OPERATIONS: [(&str, &str); 2] = [
    ("EnableInsightRules", "ENABLED"),
    ("DisableInsightRules", "DISABLED"),
];

/// A rule definition in the Contributor Insights rule syntax, as `PutInsightRule` takes it.
fn definition() -> String {
    json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    })
    .to_string()
}

/// Create the rule `name`, in the state `rule_state`.
async fn given_rule(router: &MultiServiceRouter, name: &str, rule_state: &str) {
    ok(
        router,
        "PutInsightRule",
        &json!({
            "RuleName": name,
            "RuleDefinition": definition(),
            "RuleState": rule_state,
        }),
    )
    .await;
}

/// The `State` `DescribeInsightRules` reports for `name`, or the empty string when it reports no
/// such rule.
async fn state_of(router: &MultiServiceRouter, name: &str) -> String {
    let body = ok(router, "DescribeInsightRules", &json!({})).await;
    body.get("InsightRules")
        .and_then(Value::as_array)
        .and_then(|rules| {
            rules
                .iter()
                .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(name))
                .cloned()
        })
        .as_ref()
        .and_then(|rule| rule.get("State"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `Failures` entries an answer carries, in the order it carried them.
fn failures(body: &Value) -> Vec<Value> {
    body.get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The entry these operations report for a name that is legal and addresses no rule.
fn rule_not_found(rule_name: &str) -> Value {
    json!({
        "FailureResource": rule_name,
        "ExceptionType": "ResourceNotFoundException",
        "FailureCode": "RULE_NAME",
        "FailureDescription": format!("Rule {rule_name} does not exist."),
    })
}

/// The entry they report for an entry that is not a rule name at all, described by `fault`.
fn invalid_rule_name(rule_name: &str, fault: &str) -> Value {
    json!({
        "FailureResource": rule_name,
        "ExceptionType": "InvalidParameterException",
        "FailureCode": "INVALID_RULE_NAME",
        "FailureDescription": fault,
    })
}

/// One unusable `RuleNames` entry: the value, and the fault the entry is described by.
struct IllegalName {
    /// What this case is, named in every assertion message.
    case: &'static str,
    /// The value the request carries, verbatim.
    value: String,
    /// The `FailureDescription` the entry carries.
    fault: String,
}

/// The ways one entry can miss the grammar `InsightRuleName` declares — `length {min 1, max
/// 128}` and `pattern ^[\x20-\x7E]+$` — each row a different one: no characters at all, one
/// character more than the maximum, and four bytes outside the printable-ASCII span (the two
/// control bytes either side of it, the tab and the newline a client's own input can carry, and
/// a character above ASCII).
fn illegal_names() -> Vec<IllegalName> {
    let pattern = r"[\x20-\x7E]+";
    let outside_the_pattern = |case: &'static str, value: String| IllegalName {
        case,
        fault: format!("RuleName \"{value}\" does not match the pattern: {pattern}"),
        value,
    };
    vec![
        IllegalName {
            case: "no characters at all",
            value: String::new(),
            fault: "RuleName cannot be an empty string.".to_string(),
        },
        IllegalName {
            case: "one character more than the maximum",
            value: "n".repeat(129),
            fault: "RuleName cannot be longer than 128 characters.".to_string(),
        },
        outside_the_pattern("a control byte below the span", "\u{1}bad-rule".to_string()),
        outside_the_pattern("the delete byte above the span", "bad\u{7f}".to_string()),
        outside_the_pattern("a tab inside the name", "bad\trule".to_string()),
        outside_the_pattern("a character above ASCII", "règle".to_string()),
    ]
}

#[tokio::test]
async fn an_entry_that_is_no_rule_name_is_reported_about_that_entry_alone() {
    // The batch each row submits: a rule this service holds, a legal name it holds no rule for,
    // and the row's unusable entry LAST — the position the recorded case puts it in.
    for (operation, target_state) in STATE_OPERATIONS {
        for illegal in illegal_names() {
            let case = format!("{operation} / {}", illegal.case);
            let router = build_router();
            // Created in the state the operation does NOT move it to, so the flip is visible.
            let created_state = if target_state == "ENABLED" {
                "DISABLED"
            } else {
                "ENABLED"
            };
            given_rule(&router, "held", created_state).await;

            let answered = ok(
                &router,
                operation,
                &json!({"RuleNames": ["held", "no-such-rule", illegal.value]}),
            )
            .await;

            assert_eq!(
                failures(&answered),
                vec![
                    rule_not_found("no-such-rule"),
                    invalid_rule_name(&illegal.value, &illegal.fault),
                ],
                "case {case:?}: expected the unknown name and the unusable entry classified \
                 independently, got {answered}"
            );
            // The valid member of the same batch is still applied.
            let actual = state_of(&router, "held").await;
            assert_eq!(
                actual, target_state,
                "case {case:?}: expected the held rule in state {target_state:?}, got {actual:?}"
            );
            // The unusable entry created nothing: the account holds the one rule it was given.
            let described = ok(&router, "DescribeInsightRules", &json!({})).await;
            let held: Vec<&str> = described["InsightRules"]
                .as_array()
                .map(|rules| {
                    rules
                        .iter()
                        .map(|rule| rule["Name"].as_str().unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                held,
                vec!["held"],
                "case {case:?}: an entry reported back stores no rule, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn the_position_and_the_number_of_unusable_entries_do_not_change_the_report() {
    // The same three-name batch with the unusable entry first, in the middle and last, then a
    // batch carrying two of them: each entry is reported in REQUEST order, about itself.
    let illegal = "\u{1}bad-rule";
    let fault = format!("RuleName \"{illegal}\" does not match the pattern: [\\x20-\\x7E]+");
    let empty_fault = "RuleName cannot be an empty string.";
    let rows: Vec<(&str, Vec<&str>, Vec<Value>)> = vec![
        (
            "the unusable entry first",
            vec![illegal, "held", "ghost"],
            vec![invalid_rule_name(illegal, &fault), rule_not_found("ghost")],
        ),
        (
            "the unusable entry in the middle",
            vec!["held", illegal, "ghost"],
            vec![invalid_rule_name(illegal, &fault), rule_not_found("ghost")],
        ),
        (
            "the unusable entry last",
            vec!["held", "ghost", illegal],
            vec![rule_not_found("ghost"), invalid_rule_name(illegal, &fault)],
        ),
        (
            "two unusable entries of different classes",
            vec![illegal, "held", ""],
            vec![
                invalid_rule_name(illegal, &fault),
                invalid_rule_name("", empty_fault),
            ],
        ),
        (
            "the same unusable entry twice",
            vec![illegal, illegal],
            vec![
                invalid_rule_name(illegal, &fault),
                invalid_rule_name(illegal, &fault),
            ],
        ),
        (
            "every entry usable",
            vec!["held"],
            Vec::new(),
        ),
    ];

    for (operation, target_state) in STATE_OPERATIONS {
        for (row, named, expected) in &rows {
            let case = format!("{operation} / {row}");
            let router = build_router();
            let created_state = if target_state == "ENABLED" {
                "DISABLED"
            } else {
                "ENABLED"
            };
            given_rule(&router, "held", created_state).await;

            let answered = ok(&router, operation, &json!({"RuleNames": named})).await;

            assert_eq!(
                &failures(&answered),
                expected,
                "case {case:?}: expected one entry per unusable name, in request order, got \
                 {answered}"
            );
            let actual = state_of(&router, "held").await;
            let expected_state = if named.contains(&"held") {
                target_state
            } else {
                created_state
            };
            assert_eq!(
                actual, expected_state,
                "case {case:?}: expected the held rule in state {expected_state:?}, got \
                 {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_inside_the_grammar_is_resolved_rather_than_reported_as_invalid() {
    // The other side of the same bounds: the shortest and the longest name the length admits,
    // and names built from the ends and the punctuation of the printable-ASCII span — each one
    // held, so a legal name must move rather than come back as an entry.
    let longest = "n".repeat(128);
    let ends_of_the_span = format!("{}{}", '\u{20}', '\u{7e}');
    let legal_names: Vec<(&str, &str)> = vec![
        ("one character", "r"),
        ("the longest name the length admits", longest.as_str()),
        ("the two ends of the pattern's span", ends_of_the_span.as_str()),
        ("spaces and punctuation", "My rule (v2) / high-cardinality"),
        ("digits and symbols only", "99.9%_$#@!"),
    ];

    for (operation, target_state) in STATE_OPERATIONS {
        for (case, name) in &legal_names {
            let case = format!("{operation} / {case}");
            let router = build_router();
            let created_state = if target_state == "ENABLED" {
                "DISABLED"
            } else {
                "ENABLED"
            };
            given_rule(&router, name, created_state).await;

            let answered = ok(&router, operation, &json!({"RuleNames": [name]})).await;
            assert_eq!(
                failures(&answered),
                Vec::<Value>::new(),
                "case {case:?}: a legal held name reports no failure, got {answered}"
            );
            let actual = state_of(&router, name).await;
            assert_eq!(
                actual, target_state,
                "case {case:?}: expected state {target_state:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_list_itself_is_what_is_required_of_the_request() {
    // The required-ness is on the LIST: an array carrying no entry is refused as the empty set
    // of states it asks for, and a request that omits the member entirely is refused as the
    // parameter it did not supply — while an array holding ONE unusable entry is served.
    for (operation, _) in STATE_OPERATIONS {
        let router = build_router();

        let (status, body) = call(&router, operation, &json!({"RuleNames": []})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {operation:?}: an empty array is refused as a bad value, got {status} {body}"
        );

        let (status, body) = call(&router, operation, &json!({})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "MissingRequiredParameterException"),
            "case {operation:?}: an absent RuleNames is the missing parameter, got {status} \
             {body}"
        );

        let (status, body) = call(&router, operation, &json!({"RuleNames": [""]})).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: an empty ENTRY is reported per entry, got {status} {body}"
        );
        assert_eq!(
            failures(&body),
            vec![invalid_rule_name("", "RuleName cannot be an empty string.")],
            "case {operation:?}: expected the empty entry reported about itself, got {body}"
        );
    }
}
