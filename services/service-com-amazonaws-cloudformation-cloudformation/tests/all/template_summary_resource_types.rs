// SPDX-License-Identifier: CC0-1.0

//! What `GetTemplateSummary`'s `ResourceTypes` reports about a template's resources, end to
//! end over the awsQuery wire.
//!
//! THE RULE: "a list of all the template resource types that are defined in the template" is
//! the type of EACH resource the body declares — so a type several resources share is
//! reported once per resource and nothing is collapsed — reported ASCENDING BY TYPE NAME,
//! whatever order the body declares its resources in. The per-TYPE reading of the same
//! section is `ResourceIdentifierSummaries`, which carries one entry per type with the logical
//! ids declared with it IN DECLARED ORDER, so the two members are two readings of one section
//! and this suite asserts both of one response.
//!
//! The rows differ along what the rule generalises over: how many resources share a type,
//! whether the declared order agrees with the ascending one, how many types the body
//! declares, and the spelling of the body (JSON and YAML).

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{call, list_members, member, router, xml_blocks, xml_text, xml_texts};

/// `GetTemplateSummary` over a body the request carries.
async fn summarise(router: &MultiServiceRouter, template: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("TemplateBody", template)],
    )
    .await
}

/// The `ResourceTypes` the response states, in wire order.
fn reported_types(xml: &str) -> Vec<String> {
    xml_blocks(xml, "ResourceTypes")
        .first()
        .map(|block| xml_texts(block, "member"))
        .unwrap_or_default()
}

/// Each `ResourceIdentifierSummary` as `(ResourceType, LogicalResourceIds)`, in wire order.
fn reported_summaries(xml: &str) -> Vec<(String, Vec<String>)> {
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

/// One row: a body, the types its summary must report (ascending by type name), and the
/// identifier summaries beside them (one per type, in the order the body declares them).
struct Row {
    case: &'static str,
    template: String,
    types: Vec<&'static str>,
    summaries: Vec<(&'static str, Vec<&'static str>)>,
}

fn rows() -> Vec<Row> {
    // The bodies are written out as TEXT rather than built with `json!`, which sorts an
    // object's members as it serialises them: what these rows turn on is the order the body
    // itself states, so each row states it.
    vec![
        Row {
            case: "two types declared against the ascending order",
            template: r#"{"Resources":{
                "Store":{"Type":"AWS::S3::Bucket"},
                "Access":{"Type":"AWS::IAM::Role"}}}"#
                .to_string(),
            types: vec!["AWS::IAM::Role", "AWS::S3::Bucket"],
            summaries: vec![
                ("AWS::S3::Bucket", vec!["Store"]),
                ("AWS::IAM::Role", vec!["Access"]),
            ],
        },
        Row {
            case: "one type declared by three resources",
            template: r#"{"Resources":{
                "Third":{"Type":"AWS::SNS::Topic"},
                "First":{"Type":"AWS::SNS::Topic"},
                "Second":{"Type":"AWS::SNS::Topic"}}}"#
                .to_string(),
            types: vec!["AWS::SNS::Topic", "AWS::SNS::Topic", "AWS::SNS::Topic"],
            summaries: vec![("AWS::SNS::Topic", vec!["Third", "First", "Second"])],
        },
        Row {
            case: "a body whose declared order already is the ascending one",
            template: r#"{"Resources":{
                "Access":{"Type":"AWS::IAM::Role"},
                "Store":{"Type":"AWS::S3::Bucket"}}}"#
                .to_string(),
            types: vec!["AWS::IAM::Role", "AWS::S3::Bucket"],
            summaries: vec![
                ("AWS::IAM::Role", vec!["Access"]),
                ("AWS::S3::Bucket", vec!["Store"]),
            ],
        },
        Row {
            case: "YAML, four resources of three types, two of them sharing one",
            template: "Resources:\n  \
                 Zebra:\n    Type: AWS::SQS::Queue\n  \
                 Alpha:\n    Type: AWS::SNS::Topic\n  \
                 Mid:\n    Type: AWS::SQS::Queue\n  \
                 Store:\n    Type: AWS::S3::Bucket\n"
                .to_string(),
            types: vec![
                "AWS::S3::Bucket",
                "AWS::SNS::Topic",
                "AWS::SQS::Queue",
                "AWS::SQS::Queue",
            ],
            summaries: vec![
                ("AWS::SQS::Queue", vec!["Zebra", "Mid"]),
                ("AWS::SNS::Topic", vec!["Alpha"]),
                ("AWS::S3::Bucket", vec!["Store"]),
            ],
        },
        Row {
            case: "a single resource, where the set and the declaration agree trivially",
            template: json!({"Resources": {"Only": {"Type": "AWS::SNS::Topic"}}}).to_string(),
            types: vec!["AWS::SNS::Topic"],
            summaries: vec![("AWS::SNS::Topic", vec!["Only"])],
        },
    ]
}

#[tokio::test]
async fn a_summary_reports_the_type_of_every_declared_resource_by_type_name() {
    let router = router();
    for row in rows() {
        let case = row.case;
        let (status, body) = summarise(&router, &row.template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the body summarised, got {status} {body}"
        );

        let reported = reported_types(&body);
        assert_eq!(
            reported, row.types,
            "case {case:?}: expected one entry per declared resource, ascending by type name, \
             {:?}, got {reported:?}",
            row.types
        );

        // The same response's per-TYPE reading: the identifier summaries carry each type
        // once, which is what shows the repetition above is this member's own rather than
        // the reader's.
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
        let summaries = reported_summaries(&body);
        assert_eq!(
            summaries, expected,
            "case {case:?}: expected the identifier summaries {expected:?}, got {summaries:?}"
        );
    }
}

/// Every ordering of one `Resources` section is summarised with the SAME `ResourceTypes`
/// list — ascending by type name — while the identifier summaries of that same response
/// follow the order each body states. The invariant is asserted over every permutation of a
/// four-resource section, which is the dimension the rule generalises over: the ORDER the
/// body declares its resources in.
#[tokio::test]
async fn the_types_reported_do_not_depend_on_the_order_the_body_declares_resources_in() {
    // Four declarations of three types, two of them sharing one type, each needing no
    // properties. Identifiers are each type's primary identifier, which the per-TYPE member
    // carries beside the logical ids.
    let declarations: [(&str, &str); 4] = [
        ("Store", "AWS::S3::Bucket"),
        ("Access", "AWS::IAM::Role"),
        ("Notify", "AWS::SNS::Topic"),
        ("Archive", "AWS::S3::Bucket"),
    ];
    // The one list every ordering reports: one entry per declared resource, ascending.
    let expected_types = vec![
        "AWS::IAM::Role".to_string(),
        "AWS::S3::Bucket".to_string(),
        "AWS::S3::Bucket".to_string(),
        "AWS::SNS::Topic".to_string(),
    ];
    let router = router();

    for order in permutations(declarations.len()) {
        let stated: Vec<(&str, &str)> = order.iter().map(|at| declarations[*at]).collect();
        let case = stated
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<&str>>()
            .join(",");
        // Written out as text: `json!` would state the members in its own order, and what
        // this case turns on is the order the BODY states them in.
        let body = stated
            .iter()
            .map(|(id, resource_type)| format!(r#""{id}":{{"Type":"{resource_type}"}}"#))
            .collect::<Vec<String>>()
            .join(",");
        let template = format!(r#"{{"Resources":{{{body}}}}}"#);

        let (status, response) = summarise(&router, &template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the body summarised, got {status} {response}"
        );

        let reported = reported_types(&response);
        assert_eq!(
            reported, expected_types,
            "case {case:?}: expected {expected_types:?} whatever order the body states, got \
             {reported:?}"
        );

        // The per-TYPE member of the SAME response follows this body's own order: one entry
        // per type, at the position of the first resource declared with it, carrying the
        // logical ids in declared order.
        let mut expected_summaries: Vec<(String, Vec<String>)> = Vec::new();
        for (id, resource_type) in &stated {
            match expected_summaries
                .iter_mut()
                .find(|(reported_type, _)| reported_type == resource_type)
            {
                Some((_, ids)) => ids.push((*id).to_string()),
                None => {
                    expected_summaries.push(((*resource_type).to_string(), vec![(*id).to_string()]))
                }
            }
        }
        let summaries = reported_summaries(&response);
        assert_eq!(
            summaries, expected_summaries,
            "case {case:?}: expected the identifier summaries {expected_summaries:?}, got \
             {summaries:?}"
        );
    }
}

/// Every ordering of `count` items, as index lists.
fn permutations(count: usize) -> Vec<Vec<usize>> {
    if count == 0 {
        return vec![Vec::new()];
    }
    let mut orders = Vec::new();
    for at in 0..count {
        for mut rest in permutations(count - 1) {
            for index in &mut rest {
                if *index >= at {
                    *index += 1;
                }
            }
            let mut order = vec![at];
            order.extend(rest);
            orders.push(order);
        }
    }
    orders
}

/// A body declaring MANY resources of one type is summarised with as many entries as it
/// declares resources: the list is as long as the `Resources` section, whatever that length
/// is, while the identifier summary of that one type stays a single entry carrying every
/// logical id. The rows differ along the COUNT, which is the dimension a client reads the
/// list's length for.
#[tokio::test]
async fn a_summary_reports_as_many_entries_as_the_body_declares_resources() {
    // `AWS::CloudFormation::WaitConditionHandle` is a type whose declaration needs no
    // properties, so a body of any length can be written out of it.
    const TYPE: &str = "AWS::CloudFormation::WaitConditionHandle";
    let router = router();

    for count in [1usize, 2, 7, 40] {
        let case = format!("{count} resources of one type");
        let declarations: Vec<String> = (0..count)
            .map(|index| format!(r#""Handle{index}":{{"Type":"{TYPE}"}}"#))
            .collect();
        let template = format!(r#"{{"Resources":{{{}}}}}"#, declarations.join(","));

        let (status, body) = summarise(&router, &template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the body summarised, got {status} {body}"
        );

        let reported = reported_types(&body);
        assert_eq!(
            reported,
            vec![TYPE.to_string(); count],
            "case {case:?}: expected {count} entries of {TYPE:?}, got {reported:?}"
        );

        let expected_ids: Vec<String> = (0..count).map(|index| format!("Handle{index}")).collect();
        let summaries = reported_summaries(&body);
        assert_eq!(
            summaries,
            vec![(TYPE.to_string(), expected_ids.clone())],
            "case {case:?}: expected one identifier summary carrying every logical id, got \
             {summaries:?}"
        );
    }
}
