// SPDX-License-Identifier: CC0-1.0

//! What a template may state for an output's `Export.Name` and for a `Mappings` value, end to
//! end over the awsQuery wire.
//!
//! Two template-format rules, and the one consequence they share.
//!
//! * An export name is the handle OTHER stacks import a value by, so CloudFormation fixes it
//!   from the template, the parameter values in force and the stack's own pseudo parameters
//!   alone: "the Name field of Export must not depend on any resources, imported values, or
//!   Fn::GetAZs". A name computed from one of those is refused with HTTP 400
//!   `ValidationError`, naming the output that declared it; a name built from literals,
//!   parameters and pseudo parameters is accepted, and the stack publishes the name it
//!   RESOLVED TO.
//! * The `Mappings` section carries literals — "the keys and values in mappings must be
//!   literal strings", and "you can't use parameters, pseudo parameters, or intrinsic
//!   functions in the Mappings section" — so a value stated as an expression, or as a map of
//!   its own, is refused the same way, and a literal one is what an `Fn::FindInMap` resolves
//!   to.
//!
//! Both are decided from the DOCUMENT, so every operation that reads a body gives the same
//! verdict: the table below is driven through `CreateStack`, `UpdateStack`,
//! `CreateChangeSet` and `ValidateTemplate`.
//!
//! The shared consequence: a create refused for its template writes NOTHING, so the stack
//! name it asked for stays free and a create submitting the CORRECTED template under that
//! same name is accepted, deploys, and reports the export name and the mapped value its
//! template declares.
//!
//! The rows span what the rules generalise over: which function the name reads (`Ref` to a
//! resource, `Fn::GetAtt`, `Fn::ImportValue`, `Fn::GetAZs`, an `Fn::Sub` variable, a function
//! nesting one of them), which output declares it, which spelling the body comes in (JSON and
//! YAML), and — for a mapping value — whether the non-literal is an intrinsic, a nested map or
//! a list carrying one.

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, list_members, member, output_value, router,
    settled_stack, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// How the service refuses an output whose `Export.Name` depends on something an export name
/// may not: the OUTPUT is named, then the clause stating what such a name may not read.
fn malformed_export(key: &str) -> String {
    format!(
        "Template format error: Output {key} is malformed.  The Name field of Export must \
         not depend on any resources, imported values, or Fn::GetAZs."
    )
}

/// How the service refuses a `Mappings` value that is no literal. Fixed text: nothing of the
/// template reaches it.
const MAPPING_VALUE_NOT_LITERAL: &str =
    "Template format error: Every Mappings attribute must be a String or a List.";

/// The corrected template every refused row is re-submitted with, under the SAME stack name:
/// an export name built from the stack's own pseudo parameter, and an output value an
/// `Fn::FindInMap` reads out of a literal mapping.
const CORRECTED: &str = r#"{"Mappings":{"Tiers":{"prod":{"Size":"resolved-literal"}}},
     "Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
     "Outputs":{"O":{"Value":{"Fn::FindInMap":["Tiers","prod","Size"]},
       "Export":{"Name":{"Fn::Sub":"${AWS::StackName}-export"}}}}}"#;

/// The value the corrected template's mapping carries, which its output must report.
const CORRECTED_VALUE: &str = "resolved-literal";

/// One template every operation that reads a body must refuse, and the message the refusal
/// must carry character for character.
struct Refused {
    case: &'static str,
    template: &'static str,
    message: String,
}

/// The table: every kind of dependency an export name may not have, and every kind of
/// non-literal a mapping value may not be.
fn refused() -> Vec<Refused> {
    vec![
        Refused {
            case: "an export name that is a Ref to a declared resource",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":"v","Export":{"Name":{"Ref":"Q"}}}}}"#,
            message: malformed_export("O"),
        },
        Refused {
            // The output's own key is what the refusal names, so a row declaring another key
            // must be reported for that key.
            case: "an export name that reads an attribute off a resource",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"Named":{"Value":"v",
                   "Export":{"Name":{"Fn::GetAtt":["Q","TopicName"]}}}}}"#,
            message: malformed_export("Named"),
        },
        Refused {
            case: "an export name that imports another stack's export",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"Imported":{"Value":"v",
                   "Export":{"Name":{"Fn::ImportValue":"somebody-elses-export"}}}}}"#,
            message: malformed_export("Imported"),
        },
        Refused {
            case: "an export name reading the Region's Availability Zones",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"Zoned":{"Value":"v",
                   "Export":{"Name":{"Fn::Select":["0",{"Fn::GetAZs":""}]}}}}}"#,
            message: malformed_export("Zoned"),
        },
        Refused {
            case: "an export name whose Fn::Sub reads a resource attribute",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"Subbed":{"Value":"v",
                   "Export":{"Name":{"Fn::Sub":"${Q.TopicName}-export"}}}}}"#,
            message: malformed_export("Subbed"),
        },
        Refused {
            case: "an export name whose Fn::Sub names a resource",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"SubbedRef":{"Value":"v",
                   "Export":{"Name":{"Fn::Sub":"${Q}-export"}}}}}"#,
            message: malformed_export("SubbedRef"),
        },
        Refused {
            // The dependency NESTED inside another function is the same dependency: what the
            // name is computed from is the whole expression.
            case: "an export name joining a literal with a Ref to a resource",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"Joined":{"Value":"v",
                   "Export":{"Name":{"Fn::Join":["-",[{"Ref":"Q"},"export"]]}}}}}"#,
            message: malformed_export("Joined"),
        },
        Refused {
            case: "the same dependency in the YAML spelling of a body",
            template: concat!(
                "Resources:\n",
                "  Q:\n",
                "    Type: AWS::SNS::Topic\n",
                "Outputs:\n",
                "  Yamled:\n",
                "    Value: v\n",
                "    Export:\n",
                "      Name: !Ref Q\n",
            ),
            message: malformed_export("Yamled"),
        },
        Refused {
            // The output declared FIRST is the one reported: that is where its author reads
            // it. The second output is malformed too, so the row pins the choice.
            case: "two malformed exports, the first-declared one reported",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"Zeta":{"Value":"v","Export":{"Name":{"Ref":"Q"}}},
                   "Alpha":{"Value":"v","Export":{"Name":{"Ref":"Q"}}}}}"#,
            message: malformed_export("Zeta"),
        },
        Refused {
            case: "a mapping value that is a Ref to a parameter",
            template: r#"{"Parameters":{"P":{"Type":"String","Default":"x"}},
                 "Mappings":{"M":{"prod":{"Size":{"Ref":"P"}}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}}}"#,
            message: MAPPING_VALUE_NOT_LITERAL.to_string(),
        },
        Refused {
            case: "a mapping value that is a pseudo parameter",
            template: r#"{"Mappings":{"M":{"prod":{"Size":{"Ref":"AWS::Region"}}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}}}"#,
            message: MAPPING_VALUE_NOT_LITERAL.to_string(),
        },
        Refused {
            case: "a mapping value that is another lookup",
            template: r#"{"Mappings":{"M":{"prod":{"Size":{"Fn::FindInMap":["M","prod","Other"]},
                   "Other":"literal"}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}}}"#,
            message: MAPPING_VALUE_NOT_LITERAL.to_string(),
        },
        Refused {
            case: "a mapping value that is an Fn::Sub",
            template: r#"{"Mappings":{"M":{"prod":{"Size":{"Fn::Sub":"${AWS::StackName}-size"}}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}}}"#,
            message: MAPPING_VALUE_NOT_LITERAL.to_string(),
        },
        Refused {
            case: "a mapping value that is a map of its own",
            template: r#"{"Mappings":{"M":{"prod":{"Size":{"deeper":"literal"}}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}}}"#,
            message: MAPPING_VALUE_NOT_LITERAL.to_string(),
        },
        Refused {
            case: "a mapping value that is a list carrying an intrinsic",
            template: r#"{"Parameters":{"P":{"Type":"String","Default":"x"}},
                 "Mappings":{"M":{"prod":{"Sizes":["small",{"Ref":"P"}]}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}}}"#,
            message: MAPPING_VALUE_NOT_LITERAL.to_string(),
        },
        Refused {
            case: "a non-literal mapping value in the YAML spelling of a body",
            template: concat!(
                "Parameters:\n",
                "  P:\n",
                "    Type: String\n",
                "    Default: x\n",
                "Mappings:\n",
                "  M:\n",
                "    prod:\n",
                "      Size: !Ref P\n",
                "Resources:\n",
                "  Q:\n",
                "    Type: AWS::SNS::Topic\n",
            ),
            message: MAPPING_VALUE_NOT_LITERAL.to_string(),
        },
    ]
}

#[tokio::test]
async fn a_template_stating_one_of_them_is_refused_and_no_stack_is_created() {
    let router = router();

    for (index, row) in refused().into_iter().enumerate() {
        let stack = format!("refused-{index}");
        let (code, body) = create_stack(&router, &stack, row.template, &[]).await;
        assert_eq!(
            code, 400,
            "case {:?}: CreateStack must refuse the template, got {code} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected the ValidationError refusal, got {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(row.message.as_str()),
            "case {:?}: expected the refusal to read {:?}, got {body}",
            row.case,
            row.message
        );
        // The refusal wrote nothing: no stack of that name exists to be described.
        let (code, described) = describe_stack(&router, &stack).await;
        assert_eq!(
            code, 400,
            "case {:?}: a refused create must leave no stack, got {code} {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_create_refused_for_its_template_leaves_the_stack_name_free() {
    let router = router();
    let rows = refused();
    let names: Vec<String> = (0..rows.len()).map(|index| format!("reused-{index}")).collect();

    for (row, stack) in rows.iter().zip(&names) {
        let (code, body) = create_stack(&router, stack, row.template, &[]).await;
        assert_eq!(
            code, 400,
            "case {:?}: the first create must be refused, got {code} {body}",
            row.case
        );
        // The SAME name, with the template corrected: nothing holds the name, so this create
        // is the one AWS answers with a stack id.
        let (code, body) = create_stack(&router, stack, CORRECTED, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: the corrected create must be accepted, got {code} {body}",
            row.case
        );
        let stack_id = xml_text(&body, "StackId").unwrap_or_default();
        let prefix = format!(":stack/{stack}/");
        assert!(
            stack_id.starts_with("arn:aws:cloudformation:") && stack_id.contains(&prefix),
            "case {:?}: expected a stack id naming {stack}, got {stack_id:?}",
            row.case
        );
    }

    for (row, stack) in rows.iter().zip(&names) {
        let described = settled_stack(&router, row.case, stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the corrected stack must complete, got {described}",
            row.case
        );
        // Read back through the stack itself: the export name is the one the CORRECTED
        // template declared, resolved against the stack's own name.
        assert_eq!(
            export_name(&described, "O"),
            Some(format!("{stack}-export")),
            "case {:?}: expected the declared export name, got {described}",
            row.case
        );
        assert_eq!(
            output_value(&described, "O").as_deref(),
            Some(CORRECTED_VALUE),
            "case {:?}: expected the mapped literal as the output value, got {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn update_stack_create_change_set_and_validate_template_refuse_the_same_bodies() {
    let router = router();
    // The stack an update is submitted against, deployed from a template both rules admit.
    let (code, created) = create_stack(&router, "submittable", CORRECTED, &[]).await;
    assert_eq!(
        code, 200,
        "the stack under test must be created, got {code} {created}"
    );
    let described = settled_stack(&router, "the stack under test", "submittable").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the stack under test must complete, got {described}"
    );

    for row in refused() {
        for (operation, members) in [
            (
                "UpdateStack",
                vec![
                    member("StackName", "submittable"),
                    member("TemplateBody", row.template),
                ],
            ),
            (
                "CreateChangeSet",
                vec![
                    member("StackName", "planned"),
                    member("ChangeSetName", "cs"),
                    member("ChangeSetType", "CREATE"),
                    member("TemplateBody", row.template),
                ],
            ),
            (
                "ValidateTemplate",
                vec![member("TemplateBody", row.template)],
            ),
        ] {
            let (code, body) = call(&router, operation, &members).await;
            assert_eq!(
                code, 400,
                "case {:?}: {operation} must refuse the template, got {code} {body}",
                row.case
            );
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(row.message.as_str()),
                "case {:?}: expected {operation} to read {:?}, got {body}",
                row.case,
                row.message
            );
        }
    }
}

/// One template both rules ADMIT, and what the stack must then report for its one output.
struct Accepted {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    parameters: Vec<(String, String)>,
    /// The `ExportName` the output must carry, with `{stack}` standing for the stack's name.
    export_name: &'static str,
    /// The `OutputValue` it must carry.
    value: &'static str,
}

/// The other half of both rules: the names an export name MAY be computed from, and the
/// literals a mapping value may be.
fn accepted() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "a literal export name",
            stack_name: "literal-export",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":"v","Export":{"Name":"a-literal-export"}}}}"#,
            parameters: vec![],
            export_name: "a-literal-export",
            value: "v",
        },
        Accepted {
            case: "an export name built from the stack's own pseudo parameter",
            stack_name: "pseudo-export",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":"v",
                   "Export":{"Name":{"Fn::Sub":"${AWS::StackName}-export"}}}}}"#,
            parameters: vec![],
            export_name: "{stack}-export",
            value: "v",
        },
        Accepted {
            case: "an export name joined from a parameter value",
            stack_name: "parameter-export",
            template: r#"{"Parameters":{"Env":{"Type":"String","Default":"dev"}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":{"Ref":"Env"},
                   "Export":{"Name":{"Fn::Join":["-",[{"Ref":"Env"},"export"]]}}}}}"#,
            parameters: vec![
                member("Parameters.member.1.ParameterKey", "Env"),
                member("Parameters.member.1.ParameterValue", "prod"),
            ],
            export_name: "prod-export",
            value: "prod",
        },
        Accepted {
            case: "an export name whose Fn::Sub variable its own map supplies",
            stack_name: "supplied-export",
            template: r#"{"Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":"v",
                   "Export":{"Name":{"Fn::Sub":["${Local}-export",{"Local":"supplied"}]}}}}}"#,
            parameters: vec![],
            export_name: "supplied-export",
            value: "v",
        },
        Accepted {
            case: "a mapping whose value is a literal string",
            stack_name: "string-mapping",
            template: r#"{"Mappings":{"M":{"prod":{"Size":"large"}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":{"Fn::FindInMap":["M","prod","Size"]},
                   "Export":{"Name":"string-mapped-export"}}}}"#,
            parameters: vec![],
            export_name: "string-mapped-export",
            value: "large",
        },
        Accepted {
            case: "a mapping whose value is a literal number",
            stack_name: "number-mapping",
            template: r#"{"Mappings":{"M":{"prod":{"Count":4}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":{"Fn::FindInMap":["M","prod","Count"]},
                   "Export":{"Name":"number-mapped-export"}}}}"#,
            parameters: vec![],
            export_name: "number-mapped-export",
            value: "4",
        },
        Accepted {
            case: "a mapping whose value is a list of literals",
            stack_name: "list-mapping",
            template: r#"{"Mappings":{"M":{"prod":{"Sizes":["small","medium","large"]}}},
                 "Resources":{"Q":{"Type":"AWS::SNS::Topic"}},
                 "Outputs":{"O":{"Value":{"Fn::Select":["1",{"Fn::FindInMap":["M","prod","Sizes"]}]},
                   "Export":{"Name":"list-mapped-export"}}}}"#,
            parameters: vec![],
            export_name: "list-mapped-export",
            value: "medium",
        },
    ]
}

#[tokio::test]
async fn a_template_both_rules_admit_publishes_the_name_and_the_value_it_declares() {
    let router = router();
    let cases = accepted();

    for case in &cases {
        let (code, body) =
            create_stack(&router, case.stack_name, case.template, &case.parameters).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must accept the template, got {code} {body}",
            case.case
        );
    }

    for case in &cases {
        let described = settled_stack(&router, case.case, case.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must complete, got {described}",
            case.case
        );
        let expected = case.export_name.replace("{stack}", case.stack_name);
        assert_eq!(
            export_name(&described, "O"),
            Some(expected.clone()),
            "case {:?}: expected the export name {expected:?}, got {described}",
            case.case
        );
        assert_eq!(
            output_value(&described, "O").as_deref(),
            Some(case.value),
            "case {:?}: expected the output value {:?}, got {described}",
            case.case,
            case.value
        );
    }
}

/// The `ExportName` a described stack reports for the output under `key`, read INSIDE that
/// output's own entry so two outputs' members cannot be paired — the reader
/// [`output_value`] is for a value.
fn export_name(described: &str, key: &str) -> Option<String> {
    list_members(described, "Outputs")
        .into_iter()
        .find(|entry| xml_text(entry, "OutputKey").as_deref() == Some(key))
        .and_then(|entry| xml_text(&entry, "ExportName"))
}
