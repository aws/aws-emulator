// SPDX-License-Identifier: CC0-1.0

//! The refusal both write paths of this service answer the one resource-state condition
//! with: the account already holds a report under the requested name.
//!
//! The rule under test: a refusal that reports a resource-state condition carries its
//! modelled error code and HTTP status alone — the modelled exceptions declare a single
//! OPTIONAL `Message`, an error shape's own documentation is the model's description of
//! the error CLASS rather than text the service puts on the wire, so no `Message` member
//! is serialized at all and nothing from the request (least of all a report name) is
//! echoed back into it. `ModifyReportDefinition` declares no duplicate-name error —
//! `ValidationException` is its only client-input error
//! (`smithy_query(operation="ModifyReportDefinition")`) — so it reports the condition
//! under that code, and `PutReportDefinition` under `DuplicateReportNameException`
//! (`smithy_query(operation="PutReportDefinition")`); the envelope is the same on both.
//!
//! The rows below span what the rule generalises over: the operation that refuses, how
//! many reports the account holds (two, three, and the modelled report limit), which held
//! report the collision is with (the first, the middle, the last), and the spelling and
//! length of the names involved.
//!
//! The counterpart is pinned too, so the collision check cannot be widened into a refusal
//! of every rename: a definition naming a report the account does NOT hold under another
//! report is applied, including a name that differs from a held one only in case — report
//! names are case sensitive ("The name must be unique, is case sensitive, and can't
//! include spaces", `smithy_query(shape="ReportDefinition", member="ReportName")`).

use serde_json::{json, Value};

use service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice::state::MAX_REPORTS;

use super::report_definition_lifecycle::{error_code, expected_read_back, minimal_definition, Cur};

/// The longest name the model allows: `ReportName` is length 0..256
/// (`smithy_query(shape="ReportName")`).
const REPORT_NAME_MAX: usize = 256;

/// The `Message` member of an error body, under either spelling: the model names the
/// member `Message`, and the JSON protocols render an error message member lowercase.
/// Either one present means the service sent a message, so both are looked up.
fn error_message(body: &Value) -> Option<&Value> {
    ["Message", "message"]
        .into_iter()
        .find_map(|key| body.get(key))
}

/// A name using every punctuation character the `ReportName` pattern allows
/// (`^[0-9A-Za-z!\-_.*'()]+$`, per `smithy_query(shape="ReportName")`).
fn punctuated_name() -> String {
    "r3port!-_.*'()".to_string()
}

/// One colliding rename: the reports the account holds, the report the update names, and
/// the name the supplied definition carries.
struct Collision {
    case: &'static str,
    held: Vec<String>,
    report_name: String,
    new_name: String,
}

/// One rename the account has no report under: the reports it holds, the report the
/// update names, the name the definition carries, and the names it must hold afterwards.
struct Applied {
    case: &'static str,
    held: Vec<String>,
    report_name: String,
    new_name: String,
    after: Vec<String>,
}

fn collision_table() -> Vec<Collision> {
    let long_name = "n".repeat(REPORT_NAME_MAX);
    let punctuated = punctuated_name();
    let capped: Vec<String> = (0..MAX_REPORTS)
        .map(|index| format!("capped-{index}"))
        .collect();

    vec![
        Collision {
            case: "two reports, the first renamed onto the second",
            held: vec!["report-a".to_string(), "report-b".to_string()],
            report_name: "report-a".to_string(),
            new_name: "report-b".to_string(),
        },
        Collision {
            case: "two reports, the second renamed onto the first",
            held: vec!["report-a".to_string(), "report-b".to_string()],
            report_name: "report-b".to_string(),
            new_name: "report-a".to_string(),
        },
        Collision {
            case: "three reports, the middle renamed onto the last",
            held: vec![
                "report-a".to_string(),
                "report-b".to_string(),
                "report-c".to_string(),
            ],
            report_name: "report-b".to_string(),
            new_name: "report-c".to_string(),
        },
        Collision {
            case: "the account at the modelled report limit, the last renamed onto the first",
            held: capped.clone(),
            report_name: capped[MAX_REPORTS - 1].clone(),
            new_name: capped[0].clone(),
        },
        Collision {
            case: "the collision target is a name of the maximum modelled length",
            held: vec!["short-report".to_string(), long_name.clone()],
            report_name: "short-report".to_string(),
            new_name: long_name,
        },
        Collision {
            case: "the collision target is a name using the pattern's punctuation",
            held: vec![punctuated.clone(), "plain-report".to_string()],
            report_name: "plain-report".to_string(),
            new_name: punctuated,
        },
    ]
}

/// Create every report in `held`, failing the case if any creation is refused.
async fn hold(cur: &Cur, case: &str, held: &[String]) {
    for name in held {
        let (status, body) = cur.put(&minimal_definition(name), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: holding {name:?} got {status} {body:?}"
        );
    }
}

/// Every report name the account holds, in the order the service enumerates them.
async fn held_names(cur: &Cur) -> Vec<String> {
    cur.describe_all()
        .await
        .iter()
        .filter_map(|definition| definition["ReportName"].as_str())
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_rename_onto_a_held_name_is_refused_with_its_code_and_status_alone() {
    for row in collision_table() {
        let Collision {
            case,
            held,
            report_name,
            new_name,
        } = row;
        let cur = Cur::new();
        hold(&cur, case, &held).await;

        let (status, response) = cur
            .call(
                "ModifyReportDefinition",
                &json!({
                    "ReportName": report_name,
                    "ReportDefinition": minimal_definition(&new_name),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            error_code(&response)
        );
        assert!(
            error_message(&response).is_none(),
            "case {case:?}: expected no Message member, got {:?}",
            error_message(&response)
        );
        let rendered = response.to_string();
        for name in held.iter().chain([&report_name, &new_name]) {
            assert!(
                !rendered.contains(name.as_str()),
                "case {case:?}: the error body must not echo the report name {name:?}, got \
                 {rendered}"
            );
        }

        // The refused update left every report exactly as it was: same names, same order,
        // same definitions.
        let listed = cur.read_back_all(case).await;
        let expected: Vec<Value> = held
            .iter()
            .map(|name| expected_read_back(&minimal_definition(name)))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: a refused rename must change nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_create_path_refuses_the_same_condition_with_the_same_envelope() {
    // The sibling path answering the identical condition: the account already holds a
    // report under the requested name. Rows differ in the spelling and length of the name
    // and in how many reports the account holds beside it.
    let long_name = "n".repeat(REPORT_NAME_MAX);
    let cases: Vec<(&str, Vec<String>, String)> = vec![
        (
            "the only report the account holds",
            vec!["plain-report".to_string()],
            "plain-report".to_string(),
        ),
        (
            "one of several, a name using the pattern's punctuation",
            vec![
                "plain-report".to_string(),
                punctuated_name(),
                "third-report".to_string(),
            ],
            punctuated_name(),
        ),
        (
            "a name of the maximum modelled length",
            vec![long_name.clone(), "plain-report".to_string()],
            long_name,
        ),
    ];

    for (case, held, duplicate) in cases {
        let cur = Cur::new();
        hold(&cur, case, &held).await;

        let (status, response) = cur.put(&minimal_definition(&duplicate), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "DuplicateReportNameException",
            "case {case:?}: expected DuplicateReportNameException, got {:?}",
            error_code(&response)
        );
        assert!(
            error_message(&response).is_none(),
            "case {case:?}: expected no Message member, got {:?}",
            error_message(&response)
        );
        let rendered = response.to_string();
        assert!(
            !rendered.contains(duplicate.as_str()),
            "case {case:?}: the error body must not echo the report name {duplicate:?}, got \
             {rendered}"
        );
    }
}

#[tokio::test]
async fn a_rename_the_account_holds_no_other_report_under_is_applied() {
    // The other side of the same rule: only a name ANOTHER report holds collides. Rows
    // span the definition keeping the report's own name, a name differing from a held one
    // only in case, a name a deleted report used to hold, and an unheld name at the
    // pattern's and the length bound's edges.
    let long_name = "n".repeat(REPORT_NAME_MAX);
    let cases = vec![
        Applied {
            case: "the definition keeps the report's own name",
            held: vec!["report-a".to_string(), "report-b".to_string()],
            report_name: "report-a".to_string(),
            new_name: "report-a".to_string(),
            after: vec!["report-a".to_string(), "report-b".to_string()],
        },
        Applied {
            case: "a name differing from a held one only in case",
            held: vec!["report-a".to_string(), "report-b".to_string()],
            report_name: "report-a".to_string(),
            new_name: "REPORT-B".to_string(),
            after: vec!["REPORT-B".to_string(), "report-b".to_string()],
        },
        Applied {
            case: "a name using the pattern's punctuation that nothing holds",
            held: vec!["report-a".to_string(), "report-b".to_string()],
            report_name: "report-b".to_string(),
            new_name: punctuated_name(),
            after: vec!["report-a".to_string(), punctuated_name()],
        },
        Applied {
            case: "an unheld name of the maximum modelled length",
            held: vec!["report-a".to_string()],
            report_name: "report-a".to_string(),
            new_name: long_name.clone(),
            after: vec![long_name],
        },
    ];

    for row in cases {
        let Applied {
            case,
            held,
            report_name,
            new_name,
            after,
        } = row;
        let cur = Cur::new();
        hold(&cur, case, &held).await;

        let (status, body) = cur
            .call(
                "ModifyReportDefinition",
                &json!({
                    "ReportName": report_name,
                    "ReportDefinition": minimal_definition(&new_name),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rename to be applied, got {status} {body:?}"
        );

        let actual = held_names(&cur).await;
        assert_eq!(
            actual, after,
            "case {case:?}: expected the account to hold {after:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_rename_onto_a_name_a_deleted_report_held_is_applied() {
    // The collision is with what the account HOLDS, not with what it ever held: deleting
    // the colliding report frees its name for the rename that was refused a moment ago.
    let cur = Cur::new();
    hold(
        &cur,
        "freed by a delete",
        &["report-a".to_string(), "report-b".to_string()],
    )
    .await;

    let rename = json!({
        "ReportName": "report-a",
        "ReportDefinition": minimal_definition("report-b"),
    });
    let (status, response) = cur.call("ModifyReportDefinition", &rename).await;
    assert_eq!(
        status, 400,
        "case \"while report-b is held\": expected HTTP 400, got {status} {response:?}"
    );

    let (status, body) = cur
        .call(
            "DeleteReportDefinition",
            &json!({ "ReportName": "report-b" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"delete report-b\": got {status} {body:?}"
    );

    let (status, body) = cur.call("ModifyReportDefinition", &rename).await;
    assert_eq!(
        status, 200,
        "case \"once report-b is deleted\": expected the rename to be applied, got {status} \
         {body:?}"
    );
    let actual = held_names(&cur).await;
    assert_eq!(
        actual,
        vec!["report-b".to_string()],
        "case \"once report-b is deleted\": expected the account to hold the renamed report \
         alone, got {actual:?}"
    );
}
