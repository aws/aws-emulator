// SPDX-License-Identifier: CC0-1.0

//! The template's `Parameters` section is the input CONTRACT of every stack operation that
//! submits a body, and the SET of parameters is part of it.
//!
//! Two rules, each about the request rather than about the deployment, and each answered
//! with HTTP 400 `ValidationError` leaving the stack untouched:
//!
//! * every declared parameter needs a value in force — from the request, from the
//!   declaration's own `Default`, or (on an update) from the value the stack already
//!   carries — so a declaration stating no `Default` that nothing supplies a value for
//!   refuses the call, naming every such parameter at once;
//! * a `ParameterKey` the request states that the section declares no parameter for refuses
//!   the call, naming every such key.
//!
//! The tables span the class the rules generalise over — how many parameters are
//! unsatisfied or undeclared, whether a declaration has a `Default`, what its `Type` is,
//! and whether the section exists at all — rather than one recorded request, and the same
//! rules are exercised through `CreateStack`, `UpdateStack` and `CreateChangeSet`.

use crate::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_text,
};

/// The resource every row's template declares: a type no owning service has to be asked
/// about, so a case about the REQUEST does not depend on a deployment.
const RESOURCE: &str = r#""Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}"#;

/// A template whose `Parameters` section is `section` (empty text for a template that
/// declares none at all).
fn template(section: &str) -> String {
    if section.is_empty() {
        format!("{{{RESOURCE}}}")
    } else {
        format!(r#"{{"Parameters":{{{section}}},{RESOURCE}}}"#)
    }
}

/// The members a set of parameter values is passed as.
fn passing(values: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, (key, value)) in values.iter().enumerate() {
        let n = index + 1;
        members.push(member(&format!("Parameters.member.{n}.ParameterKey"), key));
        members.push(member(
            &format!("Parameters.member.{n}.ParameterValue"),
            value,
        ));
    }
    members
}

/// One row: the declarations a template carries, the values a request passes, and the
/// refusal that pair is answered with (`None` where the contract is satisfied).
struct SetCase {
    case: &'static str,
    section: &'static str,
    values: &'static [(&'static str, &'static str)],
    refused: Option<&'static str>,
}

/// The class the two rules cover. Where a row expects SEVERAL keys in one message, they are
/// declared — and stated — in name order, so the row does not depend on how the template
/// reader happens to order a section (which the build's `serde_json` features decide).
fn set_cases() -> Vec<SetCase> {
    vec![
        SetCase {
            case: "the one declaration with no Default left without a value",
            section: r#""Secret":{"Type":"String"}"#,
            values: &[],
            refused: Some("Parameters: [Secret] must have values"),
        },
        SetCase {
            case: "two declarations with no Default left without values",
            section: r#""Alpha":{"Type":"String"},"Beta":{"Type":"Number"}"#,
            values: &[],
            refused: Some("Parameters: [Alpha, Beta] must have values"),
        },
        SetCase {
            case: "one of two declarations left without a value",
            section: r#""Alpha":{"Type":"String"},"Beta":{"Type":"Number"}"#,
            values: &[("Alpha", "a")],
            refused: Some("Parameters: [Beta] must have values"),
        },
        SetCase {
            case: "every declaration with no Default given a value",
            section: r#""Alpha":{"Type":"String"},"Beta":{"Type":"Number"}"#,
            values: &[("Alpha", "a"), ("Beta", "7")],
            refused: None,
        },
        SetCase {
            case: "a declaration whose own Default answers for it",
            section: r#""Alpha":{"Type":"String","Default":"a"}"#,
            values: &[],
            refused: None,
        },
        SetCase {
            case: "a declaration whose Default is the empty string",
            section: r#""Alpha":{"Type":"String","Default":""}"#,
            values: &[],
            refused: None,
        },
        SetCase {
            case: "a list-typed declaration with no Default left without a value",
            section: r#""Names":{"Type":"CommaDelimitedList"}"#,
            values: &[],
            refused: Some("Parameters: [Names] must have values"),
        },
        SetCase {
            case: "a list-typed declaration given its several values",
            section: r#""Names":{"Type":"CommaDelimitedList"}"#,
            values: &[("Names", "a,b,c")],
            refused: None,
        },
        SetCase {
            case: "one key the section declares nothing for",
            section: r#""Alpha":{"Type":"String","Default":"a"}"#,
            values: &[("Bogus", "b")],
            refused: Some("Parameters: [Bogus] do not exist in the template"),
        },
        SetCase {
            case: "two keys the section declares nothing for",
            section: r#""Alpha":{"Type":"String","Default":"a"}"#,
            values: &[("Bogus", "b"), ("Other", "o")],
            refused: Some("Parameters: [Bogus, Other] do not exist in the template"),
        },
        SetCase {
            case: "an undeclared key beside every declared one",
            section: r#""Alpha":{"Type":"String"}"#,
            values: &[("Alpha", "a"), ("Bogus", "b")],
            refused: Some("Parameters: [Bogus] do not exist in the template"),
        },
        SetCase {
            case: "a key stated against a template that declares no parameters at all",
            section: "",
            values: &[("Bogus", "b")],
            refused: Some("Parameters: [Bogus] do not exist in the template"),
        },
        SetCase {
            case: "no values against a template that declares no parameters at all",
            section: "",
            values: &[],
            refused: None,
        },
        SetCase {
            // Both rules are broken at once: the declared parameter has no value and the
            // request names one the template does not declare. The unsatisfied declaration
            // is what the call is answered for — a key that declares nothing cannot supply
            // the value the declaration is missing.
            case: "an unsatisfied declaration beside an undeclared key",
            section: r#""Alpha":{"Type":"String"}"#,
            values: &[("Bogus", "b")],
            refused: Some("Parameters: [Alpha] must have values"),
        },
    ]
}

#[tokio::test]
async fn a_creates_parameter_set_is_held_to_the_templates_parameters_section() {
    let router = router();
    for (index, row) in set_cases().into_iter().enumerate() {
        let case = row.case;
        let name = format!("pset-{index}");
        let (code, body) = create_stack(
            &router,
            &name,
            &template(row.section),
            &passing(row.values),
        )
        .await;
        match row.refused {
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the request must be refused, got {code} {body}"
                );
                let answered = (xml_text(&body, "Code"), xml_text(&body, "Message"));
                let expected = (
                    Some("ValidationError".to_string()),
                    Some(message.to_string()),
                );
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected {expected:?}, got {answered:?}"
                );
                // The refusal is of the REQUEST: no stack is left behind under that name.
                let (read, read_body) =
                    call(&router, "DescribeStacks", &[member("StackName", &name)]).await;
                assert_eq!(
                    read, 400,
                    "case {case:?}: a refused create leaves no stack, got {read} {read_body}"
                );
            }
            None => assert_eq!(
                code, 200,
                "case {case:?}: the contract is satisfied, got {code} {body}"
            ),
        }
    }
}

/// One row of the update table: the body the update submits, the values it passes, and the
/// refusal that pair is answered with (`None` where the contract is satisfied).
struct UpdateCase {
    case: &'static str,
    body: String,
    values: Vec<(&'static str, &'static str)>,
    refused: Option<&'static str>,
}

#[tokio::test]
async fn an_update_is_held_to_the_same_parameter_set_contract_as_a_create() {
    let router = router();
    let original = template(r#""Alpha":{"Type":"String"}"#);
    let replacement = r#""Alpha":{"Type":"String"},"Beta":{"Type":"String"}"#;
    let (code, body) = create_stack(
        &router,
        "pset-update",
        &original,
        &passing(&[("Alpha", "a")]),
    )
    .await;
    assert_eq!(
        code, 200,
        "the stack an update is applied to must exist first, got {code} {body}"
    );
    wait_for_the_transition().await;

    let rows: Vec<UpdateCase> = vec![
        UpdateCase {
            case: "a new template declaring a parameter the update states no value for",
            body: template(replacement),
            values: vec![("Alpha", "a")],
            refused: Some("Parameters: [Beta] must have values"),
        },
        UpdateCase {
            case: "an update naming a key the submitted template declares nothing for",
            body: original.clone(),
            values: vec![("Alpha", "a"), ("Bogus", "b")],
            refused: Some("Parameters: [Bogus] do not exist in the template"),
        },
        UpdateCase {
            case: "an update stating a value for every declaration of the new template",
            body: template(replacement),
            values: vec![("Alpha", "a2"), ("Beta", "b2")],
            refused: None,
        },
    ];
    for row in rows {
        let UpdateCase {
            case,
            body: body_submitted,
            values,
            refused,
        } = row;
        let mut members = vec![
            member("StackName", "pset-update"),
            member("TemplateBody", &body_submitted),
        ];
        members.extend(passing(&values));
        let (code, body) = call(&router, "UpdateStack", &members).await;
        match refused {
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the update must be refused, got {code} {body}"
                );
                let answered = (xml_text(&body, "Code"), xml_text(&body, "Message"));
                let expected = (
                    Some("ValidationError".to_string()),
                    Some(message.to_string()),
                );
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected {expected:?}, got {answered:?}"
                );
                // A refused update leaves the stack with the template and the parameter it
                // already had.
                let (_, described) =
                    call(&router, "DescribeStacks", &[member("StackName", "pset-update")]).await;
                assert_eq!(
                    xml_text(&described, "ParameterValue").as_deref(),
                    Some("a"),
                    "case {case:?}: the stack keeps the value it had, got {described}"
                );
            }
            None => {
                assert_eq!(
                    code, 200,
                    "case {case:?}: the contract is satisfied, got {code} {body}"
                );
                wait_for_the_transition().await;
            }
        }
    }
}

/// One row of the change-set table: the `Parameters` section the plan's body declares, the
/// values it passes, and the refusal that pair is answered with.
struct PlanCase {
    case: &'static str,
    section: &'static str,
    values: Vec<(&'static str, &'static str)>,
    message: &'static str,
}

#[tokio::test]
async fn a_change_set_is_held_to_the_same_parameter_set_contract() {
    let router = router();
    let (code, body) = create_stack(
        &router,
        "pset-changeset",
        &template(r#""Alpha":{"Type":"String"}"#),
        &passing(&[("Alpha", "a")]),
    )
    .await;
    assert_eq!(
        code, 200,
        "the stack the change set plans against must exist first, got {code} {body}"
    );
    wait_for_the_transition().await;

    let rows: Vec<PlanCase> = vec![
        PlanCase {
            case: "a planned template declaring a parameter the request states no value for",
            section: r#""Alpha":{"Type":"String"},"Beta":{"Type":"String"}"#,
            values: vec![("Alpha", "a")],
            message: "Parameters: [Beta] must have values",
        },
        PlanCase {
            case: "a plan naming a key the planned template declares nothing for",
            section: r#""Alpha":{"Type":"String"}"#,
            values: vec![("Alpha", "a"), ("Bogus", "b")],
            message: "Parameters: [Bogus] do not exist in the template",
        },
    ];
    for (index, row) in rows.into_iter().enumerate() {
        let PlanCase {
            case,
            section,
            values,
            message,
        } = row;
        let mut members = vec![
            member("StackName", "pset-changeset"),
            member("ChangeSetName", &format!("plan-{index}")),
            member("TemplateBody", &template(section)),
        ];
        members.extend(passing(&values));
        let (code, body) = call(&router, "CreateChangeSet", &members).await;
        assert_eq!(
            code, 400,
            "case {case:?}: the plan must be refused, got {code} {body}"
        );
        let answered = (xml_text(&body, "Code"), xml_text(&body, "Message"));
        let expected = (
            Some("ValidationError".to_string()),
            Some(message.to_string()),
        );
        assert_eq!(
            answered, expected,
            "case {case:?}: expected {expected:?}, got {answered:?}"
        );
        // Nothing was written: the stack reports no change set under that name.
        let (_, listed) = call(
            &router,
            "ListChangeSets",
            &[member("StackName", "pset-changeset")],
        )
        .await;
        assert!(
            !listed.contains(&format!("plan-{index}")),
            "case {case:?}: a refused plan is not recorded, got {listed}"
        );
    }
}
