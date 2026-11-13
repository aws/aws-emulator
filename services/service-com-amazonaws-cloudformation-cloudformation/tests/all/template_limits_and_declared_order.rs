// SPDX-License-Identifier: CC0-1.0

//! The TEMPLATE LIMITS a request is held to and the ORDER a summary reports a template in,
//! end to end over the awsQuery wire.
//!
//! Five rules, each asserted over a table whose rows differ along what the rule generalises
//! over:
//!
//! * **A request that does not state exactly ONE source for its template is refused for what
//!   it really states**: the template given both by value and by reference, the stack's own
//!   template reused alongside a submitted one, and no source at all are three distinct
//!   refusals. The rows differ along the operation whose input carries the three members and
//!   along which combination the request states.
//! * **What a summary reports about a template's resources**: `ResourceTypes` carries the
//!   type of EACH resource the body declares, ascending by type name, while each
//!   `ResourceIdentifierSummary` carries the logical ids declared with its type in the order
//!   the body declares them — so one response reads the two members back in two orders. The
//!   rows differ along the spelling of the body (JSON and YAML), whether the declared order
//!   agrees with the alphabetical one, and whether a type is declared once or several times.
//! * **The size of a template an operation reads is the quota of the operation that takes
//!   it**: a body submitted for DEPLOYMENT is held to the 51,200-byte inline quota, while an
//!   operation that only REPORTS on a template reads one up to the size of the biggest
//!   template CloudFormation reads at all. The rows differ along the size (at each bound and
//!   one past it) and along the operation.
//! * **A `Mappings` lookup is diagnosed where it misses**: a lookup naming a map the section
//!   does not declare is refused for that MAP, one whose keys the map does not carry for the
//!   lookup it made — and neither is evaluated by `ValidateTemplate`, whose verdict is on the
//!   document. The rows differ along which part of the lookup misses and along the operation.
//! * **Each member of the `Mappings` section is a map of maps**: a member that is not one is
//!   refused naming that member, by every operation that reads the body. The rows differ
//!   along how the member is misspelled (a scalar, a list, one level only) and along the
//!   operation, with a well-formed section as the control.
//!
//! Beside them, two refusals a change set is held to before anything is written: a name
//! re-created under a different `ClientToken` names that attribute, and `OnStackFailure` of
//! `DELETE` is only valid for a change set that CREATES its stack.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    TEMPLATE_BODY_MAX_BYTES, TEMPLATE_MAX_BYTES,
};

use crate::stack_lifecycle::{
    call, create_stack, list_members, member, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};

/// A template of one topic, for the rows that only need a body a stack can be made of.
fn one_topic() -> String {
    json!({"Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}}).to_string()
}

/// The `<Message>` of a refusal, or the whole body when it is no refusal.
fn message(body: &str) -> String {
    xml_text(body, "Message").unwrap_or_else(|| body.to_string())
}

/// `GetTemplateSummary` over the body a request carries.
async fn summarise(router: &MultiServiceRouter, template: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("TemplateBody", template)],
    )
    .await
}

/// `ValidateTemplate` over the body a request carries.
async fn validate(router: &MultiServiceRouter, template: &str) -> (u16, String) {
    call(
        router,
        "ValidateTemplate",
        &[member("TemplateBody", template)],
    )
    .await
}

// ---------------------------------------------------------------------------
// One source for the template
// ---------------------------------------------------------------------------

/// A `TemplateURL` for the rows that state a second source: the refusal is answered before
/// anything is fetched, so what the URL addresses never matters to them.
const TEMPLATE_URL: &str = "https://s3.us-east-1.amazonaws.com/templates/other.json";

/// The three refusals, spelled out here rather than read from the crate: the sentence IS the
/// requirement, so a reworded one must fail this suite.
const BOTH_TEMPLATE_SOURCES: &str = "You cannot specify both Template URL and Template Body.";
const PREVIOUS_AND_SUBMITTED: &str =
    "You cannot specify both usePreviousTemplate and Template Body/Template URL.";
const NO_TEMPLATE_SOURCE: &str =
    "You must specify exactly one of TemplateBody, TemplateURL or UsePreviousTemplate";

/// One row of the template-source table: what the request states beside the template members
/// it needs, and the sentence it is refused with.
struct SourceRow {
    case: &'static str,
    /// The template members the request states.
    sources: &'static [(&'static str, &'static str)],
    expected: &'static str,
}

#[tokio::test]
async fn a_request_that_states_no_one_template_source_is_refused_for_what_it_states() {
    let router = router();
    let stack = "one-source-stack";
    let (status, body) = create_stack(&router, stack, &one_topic(), &[]).await;
    assert_eq!(
        status, 200,
        "expected the stack the rows update to be created, got {status} {body}"
    );
    wait_for_the_transition().await;

    let revised = json!({"Resources": {"Topic": {"Type": "AWS::SNS::Topic"},
                                       "Queue": {"Type": "AWS::SQS::Queue"}}})
    .to_string();
    let rows: Vec<SourceRow> = vec![
        SourceRow {
            case: "a body and a URL",
            sources: &[("TemplateBody", ""), ("TemplateURL", TEMPLATE_URL)],
            expected: BOTH_TEMPLATE_SOURCES,
        },
        SourceRow {
            case: "a body and the previous template",
            sources: &[("TemplateBody", ""), ("UsePreviousTemplate", "true")],
            expected: PREVIOUS_AND_SUBMITTED,
        },
        SourceRow {
            case: "a URL and the previous template",
            sources: &[
                ("TemplateURL", TEMPLATE_URL),
                ("UsePreviousTemplate", "true"),
            ],
            expected: PREVIOUS_AND_SUBMITTED,
        },
        SourceRow {
            case: "all three at once, which states the template twice over",
            sources: &[
                ("TemplateBody", ""),
                ("TemplateURL", TEMPLATE_URL),
                ("UsePreviousTemplate", "true"),
            ],
            expected: BOTH_TEMPLATE_SOURCES,
        },
        SourceRow {
            case: "no source at all",
            sources: &[],
            expected: NO_TEMPLATE_SOURCE,
        },
        SourceRow {
            case: "UsePreviousTemplate false, which states no source either",
            sources: &[("UsePreviousTemplate", "false")],
            expected: NO_TEMPLATE_SOURCE,
        },
    ];

    for row in &rows {
        // The operations whose input carries all three ways of stating one template.
        for action in ["UpdateStack", "CreateChangeSet"] {
            let mut members = vec![member("StackName", stack)];
            if action == "CreateChangeSet" {
                members.push(member("ChangeSetName", "one-source-plan"));
            }
            for (key, value) in row.sources {
                let stated = if *key == "TemplateBody" { &revised } else { *value };
                members.push(member(key, stated));
            }
            let case = row.case;
            let (status, body) = call(&router, action, &members).await;
            assert_eq!(
                status, 400,
                "case {action:?} {case:?}: expected the request refused, got {status} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {action:?} {case:?}: expected a ValidationError, got {body}"
            );
            let reported = message(&body);
            assert_eq!(
                reported, row.expected,
                "case {action:?} {case:?}: expected {:?}, got {reported:?}",
                row.expected
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The order a summary reports a template's resources in
// ---------------------------------------------------------------------------

/// One row of the declared-order table: a body, and what a summary of it must report per
/// resource.
struct OrderRow {
    case: &'static str,
    template: String,
    /// `ResourceTypes`, the type of each resource the body declares, ascending by type name.
    types: Vec<&'static str>,
    /// Each `ResourceIdentifierSummary` as `(ResourceType, LogicalResourceIds)`, in the order
    /// the body declares the resources.
    summaries: Vec<(&'static str, Vec<&'static str>)>,
}

fn order_rows() -> Vec<OrderRow> {
    // The bodies are written out as TEXT rather than built with `json!`, which sorts an
    // object's members as it serialises it: what these rows are about is the order the body
    // itself states, so each row states it.
    vec![
        OrderRow {
            case: "JSON declaring three types against the alphabetical order",
            template: r#"{"Resources":{
                "Zebra":{"Type":"AWS::SNS::Topic"},
                "Alpha":{"Type":"AWS::SQS::Queue"},
                "Mid":{"Type":"AWS::S3::Bucket"}}}"#
                .to_string(),
            types: vec!["AWS::S3::Bucket", "AWS::SNS::Topic", "AWS::SQS::Queue"],
            summaries: vec![
                ("AWS::SNS::Topic", vec!["Zebra"]),
                ("AWS::SQS::Queue", vec!["Alpha"]),
                ("AWS::S3::Bucket", vec!["Mid"]),
            ],
        },
        OrderRow {
            case: "JSON declaring one type three times, out of alphabetical order",
            template: r#"{"Resources":{
                "Third":{"Type":"AWS::SNS::Topic"},
                "First":{"Type":"AWS::SNS::Topic"},
                "Second":{"Type":"AWS::SNS::Topic"}}}"#
                .to_string(),
            types: vec!["AWS::SNS::Topic", "AWS::SNS::Topic", "AWS::SNS::Topic"],
            summaries: vec![("AWS::SNS::Topic", vec!["Third", "First", "Second"])],
        },
        OrderRow {
            case: "YAML, two resources of one type and one of another",
            template: "Resources:\n  \
                 Zebra:\n    Type: AWS::SQS::Queue\n  \
                 Alpha:\n    Type: AWS::SNS::Topic\n  \
                 Mid:\n    Type: AWS::SQS::Queue\n"
                .to_string(),
            types: vec!["AWS::SNS::Topic", "AWS::SQS::Queue", "AWS::SQS::Queue"],
            summaries: vec![
                ("AWS::SQS::Queue", vec!["Zebra", "Mid"]),
                ("AWS::SNS::Topic", vec!["Alpha"]),
            ],
        },
        OrderRow {
            case: "a body whose declared order IS the alphabetical one",
            template: r#"{"Resources":{
                "Alpha":{"Type":"AWS::SNS::Topic"},
                "Beta":{"Type":"AWS::SQS::Queue"}}}"#
                .to_string(),
            types: vec!["AWS::SNS::Topic", "AWS::SQS::Queue"],
            summaries: vec![
                ("AWS::SNS::Topic", vec!["Alpha"]),
                ("AWS::SQS::Queue", vec!["Beta"]),
            ],
        },
    ]
}

/// Each `ResourceIdentifierSummary` the response carries, as `(ResourceType, ids)`.
fn wire_summaries(xml: &str) -> Vec<(String, Vec<String>)> {
    list_members(xml, "ResourceIdentifierSummaries")
        .iter()
        .map(|entry| {
            let ids = xml_blocks(entry, "LogicalResourceIds")
                .first()
                .map(|block| xml_texts(block, "member"))
                .unwrap_or_default();
            (xml_text(entry, "ResourceType").unwrap_or_default(), ids)
        })
        .collect()
}

#[tokio::test]
async fn a_summary_reports_every_declared_resources_type_and_its_resources_as_declared() {
    let router = router();
    for row in order_rows() {
        let case = row.case;
        let (status, body) = summarise(&router, &row.template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the body summarised, got {status} {body}"
        );
        let reported: Vec<String> = xml_blocks(&body, "ResourceTypes")
            .first()
            .map(|block| xml_texts(block, "member"))
            .unwrap_or_default();
        assert_eq!(
            reported, row.types,
            "case {case:?}: expected {:?}, got {reported:?}",
            row.types
        );
        let expected: Vec<(String, Vec<String>)> = row
            .summaries
            .iter()
            .map(|(resource_type, ids)| {
                (
                    (*resource_type).to_string(),
                    ids.iter().map(|id| (*id).to_string()).collect(),
                )
            })
            .collect();
        let summaries = wire_summaries(&body);
        assert_eq!(
            summaries, expected,
            "case {case:?}: expected {expected:?}, got {summaries:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// How big a template each operation reads
// ---------------------------------------------------------------------------

/// A JSON template of exactly `bytes` bytes declaring one topic: the padding sits in
/// `Metadata`, a section whose content the template format bounds by nothing.
fn template_of_size(bytes: usize) -> String {
    let wrapper = json!({
        "Metadata": {"Pad": ""},
        "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}
    })
    .to_string();
    let padding = "p".repeat(bytes - wrapper.len());
    json!({
        "Metadata": {"Pad": padding},
        "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}
    })
    .to_string()
}

#[tokio::test]
async fn the_size_of_a_template_an_operation_reads_is_that_operations_own_quota() {
    let router = router();
    // (case, the body's size, whether the operations that SUBMIT it accept it, whether the
    // operations that only REPORT on it accept it)
    let rows: Vec<(&str, usize, bool, bool)> = vec![
        ("a small body", 512, true, true),
        (
            "at the inline submission quota",
            TEMPLATE_BODY_MAX_BYTES,
            true,
            true,
        ),
        (
            "one byte past the inline submission quota",
            TEMPLATE_BODY_MAX_BYTES + 1,
            false,
            true,
        ),
        (
            "at the size of the biggest template CloudFormation reads",
            TEMPLATE_MAX_BYTES,
            false,
            true,
        ),
        (
            "one byte past that size",
            TEMPLATE_MAX_BYTES + 1,
            false,
            false,
        ),
    ];
    for (case, size, submittable, summarisable) in rows {
        let body_text = template_of_size(size);
        assert_eq!(
            body_text.len(),
            size,
            "case {case:?}: the row's body must be exactly {size} bytes, got {}",
            body_text.len()
        );

        let (status, body) = validate(&router, &body_text).await;
        let expected = if submittable { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected ValidateTemplate to answer {expected}, got {status} \
             {}",
            message(&body)
        );

        let (status, body) = summarise(&router, &body_text).await;
        let expected = if summarisable { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected GetTemplateSummary to answer {expected}, got {status} \
             {}",
            message(&body)
        );
        if !summarisable {
            // The refusal names the member and the ceiling the value failed.
            let reported = message(&body);
            assert!(
                reported.contains("at 'templateBody' failed to satisfy constraint")
                    && reported.contains(&TEMPLATE_MAX_BYTES.to_string()),
                "case {case:?}: expected the refusal to name the member and the ceiling, got \
                 {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Where a Mappings lookup misses
// ---------------------------------------------------------------------------

/// One row of the lookup table: a template whose `Fn::FindInMap` misses, and the sentence the
/// operations that SUBMIT the body are refused with.
struct LookupRow {
    case: &'static str,
    template: String,
    expected: String,
}

fn lookup_rows() -> Vec<LookupRow> {
    let declared = json!({"Sizes": {"small": {"Type": "t3.micro"}}});
    vec![
        LookupRow {
            case: "a map the Mappings section does not declare",
            template: json!({
                "Mappings": declared,
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
                "Outputs": {"V": {"Value": {"Fn::FindInMap": ["Other", "small", "Type"]}}}
            })
            .to_string(),
            expected: "Template error: Mapping named 'Other' is not present in the 'Mappings' \
                       section of template."
                .to_string(),
        },
        LookupRow {
            case: "a template declaring no Mappings section at all",
            template: json!({
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
                "Outputs": {"V": {"Value": {"Fn::FindInMap": ["Sizes", "small", "Type"]}}}
            })
            .to_string(),
            expected: "Template error: Mapping named 'Sizes' is not present in the 'Mappings' \
                       section of template."
                .to_string(),
        },
        LookupRow {
            case: "a top-level key the declared map does not carry",
            template: json!({
                "Mappings": declared,
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
                "Outputs": {"V": {"Value": {"Fn::FindInMap": ["Sizes", "large", "Type"]}}}
            })
            .to_string(),
            expected: "Template error: Unable to get mapping for Sizes::large::Type".to_string(),
        },
        LookupRow {
            case: "a second-level key the declared entry does not carry",
            template: json!({
                "Mappings": declared,
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
                "Outputs": {"V": {"Value": {"Fn::FindInMap": ["Sizes", "small", "Cores"]}}}
            })
            .to_string(),
            expected: "Template error: Unable to get mapping for Sizes::small::Cores"
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn a_mapping_lookup_that_misses_is_diagnosed_where_it_misses() {
    let router = router();
    for (index, row) in lookup_rows().into_iter().enumerate() {
        let case = row.case;
        // The operations that SUBMIT the body resolve the lookup, so each refuses it with the
        // sentence that diagnoses where it missed.
        let (status, body) =
            create_stack(&router, &format!("lookup-{index}"), &row.template, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateStack to refuse the template, got {status} {body}"
        );
        let reported = message(&body);
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected {:?}, got {reported:?}",
            row.expected
        );

        // `ValidateTemplate` does not EVALUATE the template's intrinsics: its verdict is on
        // the document, so a lookup this body makes is left to the operation that deploys it.
        let (status, validated) = validate(&router, &row.template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ValidateTemplate to report on the template, got {status} \
             {}",
            message(&validated)
        );
    }
}

// ---------------------------------------------------------------------------
// How a Mappings member is spelled
// ---------------------------------------------------------------------------

/// One row of the Mappings-spelling table: a `Mappings` section, and the member its refusal
/// names (`None` for a section every operation accepts).
struct MappingsRow {
    case: &'static str,
    mappings: serde_json::Value,
    refused_member: Option<&'static str>,
}

fn mappings_rows() -> Vec<MappingsRow> {
    vec![
        MappingsRow {
            case: "a member stated as a scalar",
            mappings: json!({"Sizes": "t3.micro"}),
            refused_member: Some("Sizes"),
        },
        MappingsRow {
            case: "a member stated as a list",
            mappings: json!({"Sizes": ["small", "large"]}),
            refused_member: Some("Sizes"),
        },
        MappingsRow {
            case: "a member of one level only, whose top-level key holds no mapping",
            mappings: json!({"Sizes": {"small": "t3.micro"}}),
            refused_member: Some("Sizes"),
        },
        MappingsRow {
            case: "the second of two members, the first being well formed",
            mappings: json!({"Sizes": {"small": {"Type": "t3.micro"}}, "Tiers": 7}),
            refused_member: Some("Tiers"),
        },
        MappingsRow {
            case: "a well-formed section of two maps",
            mappings: json!({"Sizes": {"small": {"Type": "t3.micro"}},
                             "Tiers": {"prod": {"Tier": "large"}}}),
            refused_member: None,
        },
    ]
}

#[tokio::test]
async fn every_mappings_member_must_be_a_map() {
    let router = router();
    for (index, row) in mappings_rows().into_iter().enumerate() {
        let case = row.case;
        let template = json!({
            "Mappings": row.mappings,
            "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}
        })
        .to_string();
        let expected = row
            .refused_member
            .map(|name| format!("Template format error: Every Mappings member {name} must be a map"));

        // Every operation that READS the body gives the same verdict: the one that validates
        // it, the one that only summarises it, and the one that submits it for deployment.
        for action in ["ValidateTemplate", "GetTemplateSummary"] {
            let (status, body) =
                call(&router, action, &[member("TemplateBody", &template)]).await;
            match &expected {
                Some(expected) => {
                    assert_eq!(
                        status, 400,
                        "case {action:?} {case:?}: expected the body refused, got {status} \
                         {body}"
                    );
                    let reported = message(&body);
                    assert_eq!(
                        reported, *expected,
                        "case {action:?} {case:?}: expected {expected:?}, got {reported:?}"
                    );
                }
                None => assert_eq!(
                    status, 200,
                    "case {action:?} {case:?}: expected the body read, got {status} {body}"
                ),
            }
        }
        let (status, body) =
            create_stack(&router, &format!("mappings-{index}"), &template, &[]).await;
        match &expected {
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case \"CreateStack\" {case:?}: expected the body refused, got {status} \
                     {body}"
                );
                let reported = message(&body);
                assert_eq!(
                    reported, *expected,
                    "case \"CreateStack\" {case:?}: expected {expected:?}, got {reported:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case \"CreateStack\" {case:?}: expected the create accepted, got \
                     {status} {body}"
                );
                wait_for_the_transition().await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// What a change set is refused for before anything is written
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_change_set_name_re_created_under_another_client_token_names_that_attribute() {
    let router = router();
    let stack = "client-token-stack";
    let (status, body) = create_stack(&router, stack, &one_topic(), &[]).await;
    assert_eq!(
        status, 200,
        "expected the stack the change sets plan for, got {status} {body}"
    );
    wait_for_the_transition().await;

    let change_set = "token-plan";
    let revised = json!({"Resources": {"Topic": {"Type": "AWS::SNS::Topic"},
                                       "Queue": {"Type": "AWS::SQS::Queue"}}})
    .to_string();
    let plan = |token: &str| {
        vec![
            member("StackName", stack),
            member("ChangeSetName", change_set),
            member("TemplateBody", &revised),
            member("ClientToken", token),
        ]
    };
    let (status, first) = call(&router, "CreateChangeSet", &plan("token-one")).await;
    assert_eq!(
        status, 200,
        "expected the first change set created, got {status} {first}"
    );

    // The same request again IS that request: the token says so, and the change set that is
    // there is what it is answered with.
    let (status, replayed) = call(&router, "CreateChangeSet", &plan("token-one")).await;
    assert_eq!(
        status, 200,
        "case \"the same token again\": expected the change set that is there, got {status} \
         {replayed}"
    );
    assert_eq!(
        xml_text(&replayed, "Id"),
        xml_text(&first, "Id"),
        "case \"the same token again\": expected the same change set id, got {replayed}"
    );

    // Another token under the same name is another request, so the name cannot be created
    // under it — and the refusal names the attribute that differs.
    let (status, body) = call(&router, "CreateChangeSet", &plan("token-two")).await;
    assert_eq!(
        status, 400,
        "case \"another token\": expected the request refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("AlreadyExistsException"),
        "case \"another token\": expected the modelled AlreadyExistsException, got {body}"
    );
    let expected = format!(
        "ChangeSet {change_set} cannot be created due to a mismatch with existing attribute \
         ClientToken"
    );
    let reported = message(&body);
    assert_eq!(
        reported, expected,
        "case \"another token\": expected {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn on_stack_failure_delete_is_only_valid_for_a_change_set_that_creates_its_stack() {
    let router = router();
    let stack = "on-failure-stack";
    let (status, body) = create_stack(&router, stack, &one_topic(), &[]).await;
    assert_eq!(
        status, 200,
        "expected the stack the rows plan against, got {status} {body}"
    );
    wait_for_the_transition().await;

    let expected = "OnStackFailure parameter can only be specified to DELETE when ChangeSetType \
                    is CREATE.";
    // (case, the ChangeSetType the request states, the stack it names, whether DELETE is
    // valid for it)
    let rows: Vec<(&str, Option<&str>, &str, bool)> = vec![
        ("the default type, which is UPDATE", None, stack, false),
        ("UPDATE, stated outright", Some("UPDATE"), stack, false),
        ("CREATE, which creates the stack", Some("CREATE"), "on-failure-created", true),
    ];
    for (case, change_set_type, named_stack, valid) in rows {
        let mut members = vec![
            member("StackName", named_stack),
            member("ChangeSetName", "on-failure-plan"),
            member("TemplateBody", &one_topic()),
            member("OnStackFailure", "DELETE"),
        ];
        if let Some(change_set_type) = change_set_type {
            members.push(member("ChangeSetType", change_set_type));
        }
        let (status, body) = call(&router, "CreateChangeSet", &members).await;
        if valid {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the change set created, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
        let reported = message(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}
