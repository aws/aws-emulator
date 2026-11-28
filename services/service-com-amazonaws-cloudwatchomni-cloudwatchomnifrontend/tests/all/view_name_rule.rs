// SPDX-License-Identifier: CC0-1.0

//! The rule a view NAME is held to, and the expression a refusal of one advertises.
//!
//! A view's name is a `ViewName`, and that member's `@pattern` is the whole rule: the
//! `view.` prefix, one lowercase alphanumeric character, then up to 250 characters drawn
//! from the trailing class — lowercase alphanumerics, the hyphen, the underscore and the
//! PERIOD. The class is wider than the published snapshot declares, so the shapes carrying
//! the member are declared from this crate's overlay model
//! (`smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`). The same
//! expression is what the constraint-violation message advertises, with the whole-value
//! anchors stripped, so a client that compiles the regex out of that message derives exactly
//! the rule the service enforces.
//!
//! The suite asserts that as a rule rather than as one recorded refusal:
//!
//! * every operation carrying `ViewName` — `CreateView`, `GetView`, `UpdateView`,
//!   `DeleteView` — admits every name inside the class, whatever punctuation it carries and
//!   wherever in the name it carries it, and a created view round-trips through the reader;
//! * a name outside the class is refused with the modelled `ValidationException` (400)
//!   whose message quotes the member's expression, at whichever of those operations carried
//!   it;
//! * the expression the message advertises, compiled as the regex it is, admits exactly the
//!   names the front door admits — the invariant this file exists for, since a narrower
//!   advertised expression makes a local pre-check reject names the service accepts;
//! * the published length bounds decide the names the expression leaves open.
//!
//! The rows differ along where the punctuation sits (after the prefix, among the trailing
//! characters, at the end), along the character that breaks the rule (case, a separator the
//! class excludes, a missing prefix, a separator in the one position the class does not
//! admit one) and along the operation, so none of them is one recorded request restated.
//! Each assertion names its case and the actual value.

use aws_smithy_schema::codec::Codec;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::types::{
    CreateViewOutput, DeleteViewInput, GetViewInput, GetViewOutput, UpdateViewInput,
    UpdateViewOutput, CREATE_VIEW_INPUT_SCHEMA, DELETE_VIEW_INPUT_SCHEMA, GET_VIEW_INPUT_SCHEMA,
    UPDATE_VIEW_INPUT_SCHEMA,
};
use omni::state::VIEW_TYPE_USER;

use super::validation_message_wording::error_envelope;
use super::view_lifecycle::{create_input, Omni};

/// The error code the model declares for a constraint violation, with `@httpError 400`.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status that error carries.
const VALIDATION_STATUS: u16 = 400;

/// The expression a refusal of a bad view name advertises: `ViewName`'s `@pattern` as this
/// crate declares it (`smithy-model/local-additions.json` states
/// `^view\.[a-z0-9][a-z0-9._-]{0,250}$`, widening the snapshot's class by the period) with
/// the whole-value anchors stripped, which is how the shared wording renders a `@pattern`
/// requirement.
const ADVERTISED_EXPRESSION: &str = r"view\.[a-z0-9][a-z0-9._-]{0,250}";

/// The fixed text a `@pattern` requirement is stated with, and the marker a client reads the
/// expression back out of the message at.
const PATTERN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: ";

/// The published bounds of `ViewName`'s `@length`, from the same shape.
const LENGTH_BOUNDS: (usize, usize) = (6, 256);

/// A definition every row shares: one SQL query, so nothing but the name decides the answer.
const DEFINITION: &str = "SELECT service, count(*) FROM logs GROUP BY service";

/// The whole message a value outside the expression at `member` must be refused with.
fn pattern_message(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         {PATTERN_REQUIREMENT}{ADVERTISED_EXPRESSION}"
    )
}

/// Names INSIDE the `ViewName` class.
///
/// The rows differ along which characters of the trailing class the name carries and where:
/// none at all, one hyphen, one underscore, both beside each other, a leading digit, a
/// period among the trailing characters, several periods, a separator in final position, and
/// the longest name the published `@length` admits.
fn admitted_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name the class admits", "view.a".to_string()),
        (
            "one hyphen among the trailing characters",
            "view.sales-weekly".to_string(),
        ),
        ("several hyphens", "view.a-b-c-d".to_string()),
        (
            "an underscore beside a hyphen",
            "view.sales_2026-q1-rollup".to_string(),
        ),
        (
            "a digit first, then a hyphen",
            "view.0-errors-by-service".to_string(),
        ),
        (
            "a period among the trailing characters",
            "view.sales.weekly".to_string(),
        ),
        ("several periods", "view.a.b.c.d".to_string()),
        ("a separator in final position", "view.sales_".to_string()),
        ("a period in final position", "view.sales.".to_string()),
        (
            "the longest name the class admits",
            // The prefix, one alphanumeric, then the 250 trailing characters the
            // expression's `{0,250}` admits: `LENGTH_BOUNDS.1` characters in all.
            format!("view.a{}", "b".repeat(250)),
        ),
    ]
}

/// Names OUTSIDE the class, each within the published length bounds so the pattern is the
/// only constraint they violate.
///
/// The rows differ along WHAT breaks the rule: the prefix (absent, undotted, upper case), the
/// character right after it (a period, an underscore, a hyphen — the class admits all three
/// only from the second trailing character on), and a separator the trailing class excludes
/// wherever it sits.
const REFUSED_NAMES: &[(&str, &str)] = &[
    ("no view. prefix at all", "sales-weekly"),
    ("the prefix without its period", "viewsales-weekly"),
    ("an upper-case prefix", "View.sales"),
    ("an upper-case trailing character", "view.Sales"),
    ("a period directly after the prefix", "view..sales"),
    ("an underscore directly after the prefix", "view._sales"),
    ("a hyphen directly after the prefix", "view.-sales"),
    ("a blank among the trailing characters", "view.sales weekly"),
    ("a slash", "view.sales/weekly"),
    ("a plus", "view.sales+weekly"),
    ("upper case throughout", "VIEW.SALES"),
    ("a leading blank", " view.sales"),
];

/// Decode `body` as the CBOR response of a successful call.
fn decoded<T: DeserializableStruct>(case: &str, operation: &str, body: &[u8]) -> T {
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(body);
    T::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: the {operation} response does not deserialize: {error:?}")
    })
}

/// Require that `operation` refuses `name` as a pattern violation, and answer with the
/// message it stated.
async fn refusal_message(omni: &Omni, case: &str, operation: &str, name: &str) -> String {
    let (status, body) = match operation {
        "CreateView" => {
            omni.over_the_wire(
                operation,
                CREATE_VIEW_INPUT_SCHEMA,
                &create_input(name, DEFINITION),
            )
            .await
        }
        "GetView" => {
            omni.over_the_wire(
                operation,
                GET_VIEW_INPUT_SCHEMA,
                &GetViewInput {
                    name: name.to_string(),
                },
            )
            .await
        }
        "UpdateView" => {
            omni.over_the_wire(
                operation,
                UPDATE_VIEW_INPUT_SCHEMA,
                &UpdateViewInput {
                    name: name.to_string(),
                    definition: Some(DEFINITION.to_string()),
                    description: None,
                },
            )
            .await
        }
        "DeleteView" => {
            omni.over_the_wire(
                operation,
                DELETE_VIEW_INPUT_SCHEMA,
                &DeleteViewInput {
                    name: name.to_string(),
                },
            )
            .await
        }
        other => panic!("case {case:?}: no row drives {other}"),
    };
    let (code, message) = error_envelope(case, &body);
    assert_eq!(
        (status, code.as_str()),
        (VALIDATION_STATUS, VALIDATION_EXCEPTION),
        "case {case:?}: {operation} must answer {VALIDATION_STATUS} {VALIDATION_EXCEPTION}, got \
         {status} {code:?}: {message:?}"
    );
    message
}

#[tokio::test]
async fn every_operation_carrying_a_view_name_admits_the_whole_class() {
    let omni = Omni::new();
    for (case, name) in admitted_names() {
        assert!(
            (LENGTH_BOUNDS.0..=LENGTH_BOUNDS.1).contains(&name.len()),
            "case {case:?}: the row must sit inside the published length bounds, got {}",
            name.len()
        );

        let (status, body) = omni
            .over_the_wire(
                "CreateView",
                CREATE_VIEW_INPUT_SCHEMA,
                &create_input(&name, DEFINITION),
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: CreateView must admit {name:?}, got {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let created: CreateViewOutput = decoded(case, "CreateView", &body);
        assert_eq!(
            (created.name.as_str(), created.r#type.as_str()),
            (name.as_str(), VIEW_TYPE_USER),
            "case {case:?}: the created view must carry the name it was given, got {created:?}"
        );

        // The round trip: the reader carries the same member, so it resolves the same name.
        let (status, body) = omni
            .over_the_wire(
                "GetView",
                GET_VIEW_INPUT_SCHEMA,
                &GetViewInput { name: name.clone() },
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: GetView must resolve {name:?}, got {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let read: GetViewOutput = decoded(case, "GetView", &body);
        assert_eq!(
            (
                read.name.as_str(),
                read.definition.as_str(),
                read.arn.as_str()
            ),
            (
                created.name.as_str(),
                created.definition.as_str(),
                created.arn.as_str()
            ),
            "case {case:?}: GetView must answer with the persisted view, got {read:?}"
        );

        // So does the writer, and the patch it applies is observable afterwards.
        let patched_definition = format!("{DEFINITION} ORDER BY service");
        let (status, body) = omni
            .over_the_wire(
                "UpdateView",
                UPDATE_VIEW_INPUT_SCHEMA,
                &UpdateViewInput {
                    name: name.clone(),
                    definition: Some(patched_definition.clone()),
                    description: None,
                },
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: UpdateView must resolve {name:?}, got {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let patched: UpdateViewOutput = decoded(case, "UpdateView", &body);
        assert_eq!(
            patched.definition, patched_definition,
            "case {case:?}: the patch must reach the stored view, got {:?}",
            patched.definition
        );

        // And the deleter, after which the name resolves to nothing.
        let (status, body) = omni
            .over_the_wire(
                "DeleteView",
                DELETE_VIEW_INPUT_SCHEMA,
                &DeleteViewInput { name: name.clone() },
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: DeleteView must resolve {name:?}, got {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let (status, body) = omni
            .over_the_wire(
                "GetView",
                GET_VIEW_INPUT_SCHEMA,
                &GetViewInput { name: name.clone() },
            )
            .await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted view's name must resolve to nothing, got {status} \
             {code:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn a_name_outside_the_class_is_refused_with_the_expression_the_member_states() {
    let omni = Omni::new();
    let expected = pattern_message("name");
    for (row, name) in REFUSED_NAMES {
        assert!(
            (LENGTH_BOUNDS.0..=LENGTH_BOUNDS.1).contains(&name.len()),
            "case {row:?}: the row must break the pattern alone, but its length is {}",
            name.len()
        );
        // Every operation carrying `ViewName` carries it at the top-level `name` member, so
        // the clause names that member whichever operation refused.
        for operation in ["CreateView", "GetView", "UpdateView", "DeleteView"] {
            let case = format!("{row} at {operation}");
            let message = refusal_message(&omni, &case, operation, name).await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(name),
                "case {case:?}: the clause must state no value the caller sent, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_advertised_expression_is_the_rule_the_front_door_enforces() {
    let omni = Omni::new();

    // What a client reads: the expression quoted in the message of a real refusal.
    let message = refusal_message(&omni, "the advertised expression", "CreateView", "view.Sales")
        .await;
    let (_, advertised) = message
        .rsplit_once(PATTERN_REQUIREMENT)
        .expect("the message states a pattern requirement");
    assert_eq!(
        advertised, ADVERTISED_EXPRESSION,
        "the refusal must advertise the member's own expression, got {advertised:?}"
    );

    // Compiled as the regex it is, anchored the way the member anchors it, it has to decide
    // every name exactly as the front door decided it.
    let compiled = regex::Regex::new(&format!("^{advertised}$"))
        .expect("the advertised expression compiles as a regex");
    let rows = admitted_names().into_iter().chain(
        REFUSED_NAMES
            .iter()
            .map(|(case, name)| (*case, (*name).to_string())),
    );
    for (case, name) in rows {
        let (status, body) = omni
            .over_the_wire(
                "CreateView",
                CREATE_VIEW_INPUT_SCHEMA,
                &create_input(&name, DEFINITION),
            )
            .await;
        let admitted = status == 200;
        if !admitted {
            let (code, message) = error_envelope(case, &body);
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (
                    VALIDATION_STATUS,
                    VALIDATION_EXCEPTION,
                    pattern_message("name").as_str()
                ),
                "case {case:?}: a name the front door refuses must be refused for the pattern, \
                 got {status} {code:?}: {message:?}"
            );
        }
        assert_eq!(
            admitted,
            compiled.is_match(&name),
            "case {case:?}: the advertised expression must decide {name:?} as the front door \
             did (admitted: {admitted})"
        );
    }
}

#[tokio::test]
async fn the_published_length_bounds_decide_what_the_expression_leaves_open() {
    let omni = Omni::new();
    let (min, max) = LENGTH_BOUNDS;
    // One character short of the minimum, and one past the maximum: each violates the
    // published bound AND the expression, so both clauses are stated and counted.
    for (case, name, requirement) in [
        (
            "one character short of the published minimum",
            "view.".to_string(),
            format!("Member must have length greater than or equal to {min}"),
        ),
        (
            "one character past the published maximum",
            format!("view.a{}", "b".repeat(251)),
            format!("Member must have length less than or equal to {max}"),
        ),
    ] {
        let message = refusal_message(&omni, case, "CreateView", &name).await;
        assert!(
            message.starts_with("2 validation errors detected: "),
            "case {case:?}: the envelope must count both clauses, got {message:?}"
        );
        assert!(
            message.contains(&requirement),
            "case {case:?}: expected the clause {requirement:?}, got {message:?}"
        );
        assert!(
            message.contains(&format!("{PATTERN_REQUIREMENT}{ADVERTISED_EXPRESSION}")),
            "case {case:?}: expected the advertised expression, got {message:?}"
        );
    }
}
