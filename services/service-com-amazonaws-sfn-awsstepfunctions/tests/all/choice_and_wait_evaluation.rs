// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how the Amazon States Language's `Choice` comparators and
//! `Wait` members are EVALUATED at run time, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`.
//!
//! Four rules are under test, each over a table whose rows differ along what the rule
//! generalises over rather than along the one input an issue cited:
//!
//! 1. A typed comparator is TYPE-STRICT and never coerces: it matches only a candidate
//!    whose JSON type is the comparator's own, and a candidate of any other type simply
//!    does not match, so evaluation falls through to the next rule and finally to the
//!    state's `Default`. The rows cross every comparator family (`String*`, `Numeric*`,
//!    `BooleanEquals`, `Timestamp*`, `StringMatches`, each in its `*Path` form too) with
//!    every JSON type a candidate can have (string, number, boolean, null, array,
//!    object), and run the mismatch inside `And`/`Or`/`Not` as well.
//! 2. A timestamp comparator compares the INSTANTS its operands denote, not their
//!    lexical text: the whole ISO 8601 instant grammar is read, a numeric UTC offset
//!    included, so two spellings of one instant compare equal and a lexically larger
//!    string can denote the earlier instant.
//! 3. A `Choice` rule's condition path that resolves to nothing fails the execution with
//!    `States.Runtime`, and the cause names THAT path, so the failing operand is told
//!    from every other path the state reads.
//! 4. A state member whose value is not one the member accepts fails with a cause that
//!    names the member THE DEFINITION WROTE — `SecondsPath` where the definition carries
//!    a `SecondsPath` — and never the other member of the pair.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds and the `Choice` definition is
//! `execution_interpreter`'s; `tests/all.rs` is this binary's crate root, so both are
//! reached through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{MAX_WAIT_SECONDS, RUNTIME_ERROR};

use crate::execution_interpreter::choice_definition;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};

/// What the service states about a path a `Choice` rule's condition could not resolve,
/// beside the path itself.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a sibling
/// suite asserting the same diagnosis reads it from here rather than carrying a second
/// copy of the sentence.
pub(crate) const CHOICE_CONDITION_DIAGNOSIS: &str =
    "The choice state's condition path references an invalid value.";

/// Start one execution per row of a `Choice` table and report which branch each took:
/// `"rule"` when the rule matched, `"default"` when evaluation fell through to the
/// state's `Default` ([`choice_definition`]).
async fn branches_taken(prefix: &str, rows: &[(&str, Value, Value)]) -> Vec<String> {
    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, (_, rule, input)) in rows.iter().enumerate() {
        let definition = choice_definition(rule.clone());
        let machine = create_machine(&emulator, &format!("{prefix}-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", &input.to_string()).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let mut taken = Vec::with_capacity(rows.len());
    for ((case, _, _), execution) in rows.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: a Choice over a value it can decide about succeeds rather than \
             failing, got {described:?}"
        );
        let output = described.get("output").and_then(Value::as_str).unwrap_or("");
        taken.push(output.trim_matches('"').to_string());
    }
    taken
}

/// A typed comparator matches only a candidate of its own JSON type: a differently
/// typed value is not matched, is not coerced, and does not fail the execution — the
/// rule falls through to the `Default`.
#[tokio::test]
async fn a_typed_comparator_matches_only_a_value_of_its_own_type() {
    // (case, rule, input) — the branch each row must take is in `expected` below.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "NumericEquals against the same number",
            json!({"Variable":"$.n","NumericEquals":10,"Next":"Then"}),
            json!({"n":10}),
        ),
        (
            "NumericEquals against the numeral as a string",
            json!({"Variable":"$.n","NumericEquals":10,"Next":"Then"}),
            json!({"n":"10"}),
        ),
        (
            "NumericLessThan against a numeral as a string",
            json!({"Variable":"$.n","NumericLessThan":10,"Next":"Then"}),
            json!({"n":"5"}),
        ),
        (
            "NumericGreaterThanEquals against a boolean",
            json!({"Variable":"$.n","NumericGreaterThanEquals":0,"Next":"Then"}),
            json!({"n":true}),
        ),
        (
            "NumericEquals against null",
            json!({"Variable":"$.n","NumericEquals":0,"Next":"Then"}),
            json!({"n":null}),
        ),
        (
            "NumericEquals against an array",
            json!({"Variable":"$.n","NumericEquals":1,"Next":"Then"}),
            json!({"n":[1]}),
        ),
        (
            "NumericEquals against an object",
            json!({"Variable":"$.n","NumericEquals":1,"Next":"Then"}),
            json!({"n":{"value":1}}),
        ),
        (
            "StringEquals against the same string",
            json!({"Variable":"$.s","StringEquals":"10","Next":"Then"}),
            json!({"s":"10"}),
        ),
        (
            "StringEquals against the number the string spells",
            json!({"Variable":"$.s","StringEquals":"10","Next":"Then"}),
            json!({"s":10}),
        ),
        (
            "StringLessThan against a number",
            json!({"Variable":"$.s","StringLessThan":"b","Next":"Then"}),
            json!({"s":1}),
        ),
        (
            "StringMatches against a number the pattern would match as text",
            json!({"Variable":"$.s","StringMatches":"1*","Next":"Then"}),
            json!({"s":10}),
        ),
        (
            "StringEquals against a boolean",
            json!({"Variable":"$.s","StringEquals":"true","Next":"Then"}),
            json!({"s":true}),
        ),
        (
            "BooleanEquals against the same boolean",
            json!({"Variable":"$.b","BooleanEquals":true,"Next":"Then"}),
            json!({"b":true}),
        ),
        (
            "BooleanEquals against the word as a string",
            json!({"Variable":"$.b","BooleanEquals":true,"Next":"Then"}),
            json!({"b":"true"}),
        ),
        (
            "BooleanEquals against the number one",
            json!({"Variable":"$.b","BooleanEquals":true,"Next":"Then"}),
            json!({"b":1}),
        ),
        (
            "TimestampEquals against the same instant",
            json!({"Variable":"$.t","TimestampEquals":"2026-01-01T00:00:00Z","Next":"Then"}),
            json!({"t":"2026-01-01T00:00:00Z"}),
        ),
        (
            "TimestampEquals against epoch seconds",
            json!({"Variable":"$.t","TimestampEquals":"2026-01-01T00:00:00Z","Next":"Then"}),
            json!({"t":1_767_225_600_i64}),
        ),
        (
            "TimestampLessThan against a string that denotes no instant",
            json!({"Variable":"$.t","TimestampLessThan":"2026-01-01T00:00:00Z","Next":"Then"}),
            json!({"t":"yesterday"}),
        ),
        (
            "NumericEqualsPath whose operand resolves to a string",
            json!({"Variable":"$.n","NumericEqualsPath":"$.limit","Next":"Then"}),
            json!({"n":10,"limit":"10"}),
        ),
        (
            "NumericEqualsPath whose operand resolves to the same number",
            json!({"Variable":"$.n","NumericEqualsPath":"$.limit","Next":"Then"}),
            json!({"n":10,"limit":10}),
        ),
        (
            "StringEqualsPath whose variable resolves to a number",
            json!({"Variable":"$.s","StringEqualsPath":"$.other","Next":"Then"}),
            json!({"s":10,"other":"10"}),
        ),
        (
            "an And whose second condition is differently typed",
            json!({"And":[
                {"Variable":"$.s","StringEquals":"go"},
                {"Variable":"$.n","NumericEquals":10}
            ],"Next":"Then"}),
            json!({"s":"go","n":"10"}),
        ),
        (
            "an Or whose other condition still matches",
            json!({"Or":[
                {"Variable":"$.n","NumericEquals":10},
                {"Variable":"$.s","StringEquals":"go"}
            ],"Next":"Then"}),
            json!({"s":"go","n":"10"}),
        ),
        (
            "a Not around a differently typed condition",
            json!({"Not":{"Variable":"$.n","NumericEquals":10},"Next":"Then"}),
            json!({"n":"10"}),
        ),
    ];
    // Only a candidate of the comparator's own type matches; every type mismatch falls
    // through to the `Default`, and `Not` of a mismatch therefore holds.
    let expected = [
        "rule", "default", "default", "default", "default", "default", "default", "rule",
        "default", "default", "default", "default", "rule", "default", "default", "rule",
        "default", "default", "default", "rule", "default", "default", "rule", "rule",
    ];
    assert_eq!(
        rows.len(),
        expected.len(),
        "every row states the branch it must take"
    );

    let taken = branches_taken("type-strict", &rows).await;
    for (((case, rule, input), expected), actual) in rows.iter().zip(expected).zip(taken) {
        assert_eq!(
            actual, expected,
            "case {case:?}: rule {rule} over input {input} must take the {expected:?} branch, \
             got {actual:?}"
        );
    }
}

/// Timestamp comparators compare the instants their operands denote: the ISO 8601
/// instant grammar is read in full, a numeric UTC offset included, and the comparison
/// is of instants rather than of the strings they are written as.
#[tokio::test]
async fn timestamp_comparators_compare_the_instants_the_values_denote() {
    // Rows differ along which side carries the offset, its sign, whether the two
    // spellings denote one instant, the sub-second precision, and whether the lexical
    // order of the two strings agrees with the order of the instants.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a positive offset and the same instant in UTC are equal",
            json!({"Variable":"$.when","TimestampEquals":"2024-03-01T12:00:00Z","Next":"Then"}),
            json!({"when":"2024-03-01T13:00:00+01:00"}),
        ),
        (
            "a positive offset is before a later UTC instant",
            json!({"Variable":"$.when","TimestampLessThan":"2024-03-01T12:30:00Z","Next":"Then"}),
            json!({"when":"2024-03-01T13:00:00+01:00"}),
        ),
        (
            "a positive offset is not after the instant it equals",
            json!({"Variable":"$.when","TimestampGreaterThan":"2024-03-01T12:00:00Z","Next":"Then"}),
            json!({"when":"2024-03-01T13:00:00+01:00"}),
        ),
        (
            "a negative offset and the same instant in UTC are equal",
            json!({"Variable":"$.when","TimestampEquals":"2024-03-01T12:00:00Z","Next":"Then"}),
            json!({"when":"2024-03-01T07:00:00-05:00"}),
        ),
        (
            "an offset on the operand side is read the same way",
            json!({"Variable":"$.when","TimestampEquals":"2024-03-01T14:00:00+02:00","Next":"Then"}),
            json!({"when":"2024-03-01T12:00:00Z"}),
        ),
        (
            "an offset on both sides, the same instant",
            json!({"Variable":"$.when","TimestampEquals":"2024-03-01T08:00:00-04:00","Next":"Then"}),
            json!({"when":"2024-03-01T13:00:00+01:00"}),
        ),
        (
            "a lexically larger string that denotes the earlier instant is less than",
            json!({"Variable":"$.when","TimestampLessThan":"2024-03-01T00:00:00Z","Next":"Then"}),
            json!({"when":"2024-03-01T01:00:00+05:00"}),
        ),
        (
            "sub-second precision is part of the instant",
            json!({"Variable":"$.when","TimestampGreaterThan":"2024-03-01T12:00:00Z","Next":"Then"}),
            json!({"when":"2024-03-01T12:00:00.500Z"}),
        ),
        (
            "a TimestampGreaterThanEqualsPath operand carrying an offset",
            json!({"Variable":"$.when","TimestampGreaterThanEqualsPath":"$.since","Next":"Then"}),
            json!({"when":"2024-03-01T12:00:00Z","since":"2024-03-01T13:00:00+01:00"}),
        ),
        (
            "an offset of zero spelled as a numeric offset",
            json!({"Variable":"$.when","TimestampEquals":"2024-03-01T12:00:00Z","Next":"Then"}),
            json!({"when":"2024-03-01T12:00:00+00:00"}),
        ),
    ];
    let expected = [
        "rule", "rule", "default", "rule", "rule", "rule", "rule", "rule", "rule", "rule",
    ];
    assert_eq!(
        rows.len(),
        expected.len(),
        "every row states the branch it must take"
    );

    let taken = branches_taken("instants", &rows).await;
    for (((case, rule, input), expected), actual) in rows.iter().zip(expected).zip(taken) {
        assert_eq!(
            actual, expected,
            "case {case:?}: rule {rule} over input {input} must take the {expected:?} branch, \
             got {actual:?}"
        );
    }
}

/// One row of the condition-path table: the `Choice` state's rules, the input they are
/// evaluated over, and the path the cause must name.
struct PathRow {
    case: &'static str,
    rules: Value,
    input: &'static str,
    /// The path that cannot be resolved, which the cause names.
    named: &'static str,
    /// A path the same state also reads and that resolves, which the cause must NOT
    /// name — none where the state reads only the failing one.
    not_named: Option<&'static str>,
}

/// A `Choice` rule's condition path that resolves to nothing fails the execution, and
/// the cause names that path beside the service's choice-condition sentence.
#[tokio::test]
async fn an_unresolvable_choice_condition_path_is_named_in_the_cause() {
    // Rows differ along which operand owns the failing path (a rule's `Variable`, a
    // `*Path` operand, a condition nested in `And`/`Not`), the path's own form (a
    // member, a bracketed member, a nested member), which rule of several owns it, and
    // whether the state also reads a path that does resolve.
    let rows: Vec<PathRow> = vec![
        PathRow {
            case: "a rule's Variable",
            rules: json!([{"Variable":"$.status","StringEquals":"done","Next":"Then"}]),
            input: r#"{"other":1}"#,
            named: "$.status",
            not_named: None,
        },
        PathRow {
            case: "a *Path operand beside a Variable that resolves",
            rules: json!([{"Variable":"$.value","NumericLessThanPath":"$.limit","Next":"Then"}]),
            input: r#"{"value":1}"#,
            named: "$.limit",
            not_named: Some("$.value"),
        },
        PathRow {
            case: "a bracketed member",
            rules: json!([{"Variable":"$['limit']","NumericEquals":1,"Next":"Then"}]),
            input: r#"{"value":1}"#,
            named: "$['limit']",
            not_named: None,
        },
        PathRow {
            case: "a nested member the input stops short of",
            rules: json!([{"Variable":"$.expected.count","NumericEquals":1,"Next":"Then"}]),
            input: r#"{"expected":{}}"#,
            named: "$.expected.count",
            not_named: None,
        },
        PathRow {
            case: "the second rule of several, the first having been decided",
            rules: json!([
                {"Variable":"$.kind","StringEquals":"alpha","Next":"Then"},
                {"Variable":"$.missing","StringEquals":"beta","Next":"Then"},
            ]),
            input: r#"{"kind":"omega"}"#,
            named: "$.missing",
            not_named: Some("$.kind"),
        },
        PathRow {
            case: "a condition nested in an And",
            rules: json!([{"And":[
                {"Variable":"$.kind","StringEquals":"alpha"},
                {"Variable":"$.absent","NumericEquals":1},
            ],"Next":"Then"}]),
            input: r#"{"kind":"alpha"}"#,
            named: "$.absent",
            not_named: Some("$.kind"),
        },
        PathRow {
            case: "a condition nested in a Not",
            rules: json!([{"Not":{"Variable":"$.absent","StringEquals":"alpha"},"Next":"Then"}]),
            input: r#"{"kind":"alpha"}"#,
            named: "$.absent",
            not_named: None,
        },
        PathRow {
            case: "a path into the context object the execution carries no value for",
            rules: json!([{"Variable":"$$.Map.Item.Value","NumericEquals":1,"Next":"Then"}]),
            input: "{}",
            named: "$$.Map.Item.Value",
            not_named: None,
        },
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        // Every row declares a `Default`, so a rule that merely did not match would
        // SUCCEED: a failure here is the condition that could not be evaluated.
        let definition = json!({
            "StartAt": "Route",
            "States": {
                "Route": {
                    "Type": "Choice",
                    "Choices": row.rules.clone(),
                    "Default": "Else",
                },
                "Then": { "Type": "Pass", "Result": "rule", "End": true },
                "Else": { "Type": "Pass", "Result": "default", "End": true },
            },
        })
        .to_string();
        let machine = create_machine(&emulator, &format!("condition-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", row.input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
        );
        let cause = string(&described, "cause");
        assert!(
            cause.contains(row.named),
            "case {case:?}: expected the cause to name the path {:?} that failed to resolve, got \
             {cause:?}",
            row.named
        );
        assert!(
            cause.contains(CHOICE_CONDITION_DIAGNOSIS),
            "case {case:?}: expected the cause to state {CHOICE_CONDITION_DIAGNOSIS:?}, got \
             {cause:?}"
        );
        assert!(
            cause.contains("'Route'"),
            "case {case:?}: expected the cause to name the Choice state, got {cause:?}"
        );
        if let Some(resolved) = row.not_named {
            assert!(
                !cause.contains(resolved),
                "case {case:?}: expected the cause to name only the path that failed, not \
                 {resolved:?}, got {cause:?}"
            );
        }
    }
}

/// A state member's failure names the member the definition wrote.
#[tokio::test]
async fn a_state_members_failure_names_the_member_the_definition_wrote() {
    // Rows cross the member pairs a definition carries ONE of — a `Wait` state's
    // duration (`Seconds`/`SecondsPath`, `Timestamp`/`TimestampPath`) and a `Map`
    // state's concurrency (`MaxConcurrency`/`MaxConcurrencyPath`), which is a member
    // the issue did not cite — with every way their value can fail to be one the member
    // accepts AT RUN TIME: a path resolving to a negative figure, a non-numeric string,
    // a fraction, a figure past the longest wait, a string that denotes no instant, a
    // value of the wrong JSON type, and a path the input carries no value for. A figure
    // the definition WRITES outside the bound the language documents for it is refused
    // at the create instead (`definition_static_rules` is that rule), so no row here
    // writes one. (The member catalog is the Amazon States Language's own for each
    // state type.) A `Map` state's concurrency that WAS read and is negative is reported
    // under the bound itself whichever member named it, so that class is
    // `map_concurrency_runtime_range`'s rather than this table's.
    let rows: Vec<(&str, Value, String, &str, &str)> = vec![
        (
            "SecondsPath resolving to a negative figure",
            json!({"Type":"Wait","SecondsPath":"$.d","End":true}),
            r#"{"d":-1}"#.to_string(),
            "SecondsPath",
            "Seconds",
        ),
        (
            "SecondsPath resolving to a non-numeric string",
            json!({"Type":"Wait","SecondsPath":"$.d","End":true}),
            r#"{"d":"abc"}"#.to_string(),
            "SecondsPath",
            "Seconds",
        ),
        (
            "SecondsPath resolving to a fraction",
            json!({"Type":"Wait","SecondsPath":"$.d","End":true}),
            r#"{"d":1.5}"#.to_string(),
            "SecondsPath",
            "Seconds",
        ),
        (
            "SecondsPath resolving to a figure past the longest wait",
            json!({"Type":"Wait","SecondsPath":"$.d","End":true}),
            format!(r#"{{"d":{}}}"#, MAX_WAIT_SECONDS + 1),
            "SecondsPath",
            "Seconds",
        ),
        (
            "SecondsPath the input carries no value for",
            json!({"Type":"Wait","SecondsPath":"$.d","End":true}),
            "{}".to_string(),
            "SecondsPath",
            "Seconds",
        ),
        (
            "Seconds written as a string",
            json!({"Type":"Wait","Seconds":"5","End":true}),
            "{}".to_string(),
            "Seconds",
            "SecondsPath",
        ),
        (
            "TimestampPath resolving to a string that denotes no instant",
            json!({"Type":"Wait","TimestampPath":"$.t","End":true}),
            r#"{"t":"yesterday"}"#.to_string(),
            "TimestampPath",
            "Timestamp",
        ),
        (
            "TimestampPath resolving to a number",
            json!({"Type":"Wait","TimestampPath":"$.t","End":true}),
            r#"{"t":1}"#.to_string(),
            "TimestampPath",
            "Timestamp",
        ),
        (
            "TimestampPath the input carries no value for",
            json!({"Type":"Wait","TimestampPath":"$.t","End":true}),
            "{}".to_string(),
            "TimestampPath",
            "Timestamp",
        ),
        (
            "Timestamp written as a string that denotes no instant",
            json!({"Type":"Wait","Timestamp":"yesterday","End":true}),
            "{}".to_string(),
            "Timestamp",
            "TimestampPath",
        ),
        (
            "a Map state's MaxConcurrencyPath resolving to a fraction",
            map_state(json!({"MaxConcurrencyPath":"$.c"})),
            r#"{"items":[1],"c":1.5}"#.to_string(),
            "MaxConcurrencyPath",
            "MaxConcurrency",
        ),
        (
            "a Map state's MaxConcurrencyPath the input carries no value for",
            map_state(json!({"MaxConcurrencyPath":"$.c"})),
            r#"{"items":[1]}"#.to_string(),
            "MaxConcurrencyPath",
            "MaxConcurrency",
        ),
        (
            "a Map state's MaxConcurrency written as a string",
            map_state(json!({"MaxConcurrency":"2"})),
            r#"{"items":[1]}"#.to_string(),
            "MaxConcurrency",
            "MaxConcurrencyPath",
        ),
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, (_, state, input, _, _)) in rows.iter().enumerate() {
        let definition =
            json!({ "StartAt": "W", "States": { "W": state.clone() } }).to_string();
        let machine =
            create_machine(&emulator, &format!("state-member-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", input).await);
    }

    for ((case, _, _, member, other), execution) in rows.iter().zip(started) {
        let described = settled(&emulator, case, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
        );
        let cause = string(&described, "cause");
        assert!(
            cause.contains(member),
            "case {case:?}: expected the cause to name the member {member:?} the definition \
             wrote, got {cause:?}"
        );
        // The names of a pair overlap (`Seconds` is a prefix of `SecondsPath`), so what
        // is checked is that the OTHER member is not named once the one the definition
        // wrote has been taken out of the sentence.
        assert!(
            !cause.replace(member, "").contains(other),
            "case {case:?}: expected the cause to name {member:?} alone, not the other member \
             {other:?} of the pair, got {cause:?}"
        );
    }
}

/// The record of `execution_arn` once it has left `RUNNING`, looked for again until it
/// has or a generous deadline passes.
///
/// The wait is for the CONDITION — the execution settling — so a host that settles one
/// quickly spends none of the deadline and a loaded one is not cut short: nothing here
/// measures how fast the host runs.
async fn settled(emulator: &Emulator, case: &str, execution_arn: &str) -> Value {
    /// How often the execution is looked at again, and how long it is waited for at all.
    const POLL: Duration = Duration::from_millis(25);
    const DEADLINE: Duration = Duration::from_secs(120);

    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = describe_execution(emulator, execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the execution to settle, got {described:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// A `Map` state over `$.items`, carrying the members one row declares: the failure
/// under test is the state reading its own member, so the iterations themselves are a
/// single `Pass`.
fn map_state(members: Value) -> Value {
    let mut state = json!({
        "Type": "Map",
        "ItemsPath": "$.items",
        "ItemProcessor": {
            "StartAt": "P",
            "States": { "P": { "Type": "Pass", "End": true } },
        },
        "End": true,
    });
    let into = state.as_object_mut().expect("an object");
    for (member, value) in members.as_object().expect("an object") {
        into.insert(member.clone(), value.clone());
    }
    state
}
