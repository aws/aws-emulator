// SPDX-License-Identifier: CC0-1.0

//! What a change set says a change would do to the RESOURCE, end to end over the awsQuery
//! wire.
//!
//! **The rule every row holds the service to:** a `Modify`'s `Replacement` is read off each
//! changed property's `RequiresRecreation` and `Evaluation` — a property the resource cannot
//! take a new value for at all is `Always`, and with a determined new value that is
//! `Replacement` `True`; a property that carries such a value AMONG values the resource CAN
//! take anew is `Conditionally`, which is `Replacement` `Conditional`; and every other
//! property, including the NAME of a resource whose owning service renames it in place, is
//! `Never`, which is `Replacement` `False`.
//!
//! The rows span that classification rather than one recorded case: a property whose
//! create-only value is nested inside it, the name of a resource its owning service renames
//! in place, both of those together (the shape the recorded trace submits), a plainly
//! updatable property, and — the other side of the rule — the name of a resource its owning
//! service identifies BY that name, which no call renames.
//!
//! Every read goes through the real request path (form-encoded body in, XML envelope out).

use service_com_amazonaws_cloudformation_cloudformation::state::change_action;
use service_com_amazonaws_cloudformation_cloudformation::state::change_plan::{
    change_source, evaluation, policy_action, replacement, requires_recreation,
};
use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, create_stack, member, router_with_engine, wait_for_the_transition, xml_blocks, xml_text,
};

/// The embedding model the deployed revision's knowledge base reads.
const FIRST_MODEL: &str = "arn:aws:bedrock:us-east-1::foundation-model/amazon.titan-embed-text-v1";

/// Another embedding model, the one an edit swaps in.
const SECOND_MODEL: &str =
    "arn:aws:bedrock:us-east-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The revision every row starts from: an Amazon Bedrock knowledge base, whose owning API
/// takes a knowledge base's id together with a new `name` and its whole
/// `knowledgeBaseConfiguration` on `UpdateKnowledgeBase`, beside an Amazon SNS topic, whose
/// owning API has no call that renames a topic.
///
/// The knowledge base is really created in Amazon Bedrock Agent by the deployment
/// (`state::provisioner::bedrock_knowledge_bases`), so the declaration carries the vector
/// store that create requires of a self-managed `VECTOR` knowledge base — and its NAME
/// carries the row's stack, because a knowledge base's name is the ACCOUNT's to hold once
/// and every row of the table deploys into the same account.
fn base_template(stack: &str) -> String {
    format!(
        r#"{{"Resources":{{
             "Escalation":{{"Type":"AWS::Bedrock::KnowledgeBase","Properties":{{
                "Name":"{stack}-escalation-kb-g1",
                "Description":"the escalation knowledge base",
                "RoleArn":"arn:aws:iam::000000000000:role/recreation-kb",
                "KnowledgeBaseConfiguration":{{"Type":"VECTOR",
                   "VectorKnowledgeBaseConfiguration":{{"EmbeddingModelArn":"{FIRST_MODEL}"}}}},
                "StorageConfiguration":{{"Type":"OPENSEARCH_SERVERLESS",
                   "OpensearchServerlessConfiguration":{{
                      "CollectionArn":"arn:aws:aoss:us-east-1:000000000000:collection/recreation",
                      "VectorIndexName":"recreation-index",
                      "FieldMapping":{{
                         "VectorField":"bedrock-knowledge-base-default-vector",
                         "TextField":"AMAZON_BEDROCK_TEXT_CHUNK",
                         "MetadataField":"AMAZON_BEDROCK_METADATA"}}}}}}}}}},
             "Notify":{{"Type":"AWS::SNS::Topic","Properties":{{
                "TopicName":"recreation-notify","DisplayName":"the stack's own"}}}}}}}}"#
    )
}

/// `base_template` with each `(original, replacement)` spliced in, which is how a row states
/// "the same revision, with this edit".
fn edited(stack: &str, edits: &[(&str, &str)]) -> String {
    let mut template = base_template(stack);
    for (original, replacement) in edits {
        assert!(
            template.contains(original),
            "the base revision must carry {original:?} for a row to edit it"
        );
        template = template.replace(original, replacement);
    }
    template
}

/// One `ResourceChangeDetail` as the response carries it, flattened for comparison.
#[derive(Debug, PartialEq, Eq)]
struct Detail {
    attribute: Option<String>,
    name: Option<String>,
    requires_recreation: Option<String>,
    evaluation: Option<String>,
    change_source: Option<String>,
}

/// The expected form of one detail, as a row spells it.
fn detail(name: &str, requires_recreation: &str) -> Detail {
    Detail {
        attribute: Some("Properties".to_string()),
        name: Some(name.to_string()),
        requires_recreation: Some(requires_recreation.to_string()),
        // Every edit below states the property's new value outright, so CloudFormation "can
        // determine that the target value will change, and its value".
        evaluation: Some(evaluation::STATIC.to_string()),
        change_source: Some(change_source::DIRECT_MODIFICATION.to_string()),
    }
}

/// One `ResourceChange` as the response carries it.
#[derive(Debug)]
struct ReportedChange {
    action: Option<String>,
    logical_resource_id: Option<String>,
    replacement: Option<String>,
    policy_action: Option<String>,
    details: Vec<Detail>,
}

/// Every `ResourceChange` a `DescribeChangeSet` response carries, in response order.
fn reported_changes(xml: &str) -> Vec<ReportedChange> {
    xml_blocks(xml, "ResourceChange")
        .iter()
        .map(|change| ReportedChange {
            action: xml_text(change, "Action"),
            logical_resource_id: xml_text(change, "LogicalResourceId"),
            replacement: xml_text(change, "Replacement"),
            policy_action: xml_text(change, "PolicyAction"),
            details: xml_blocks(change, "Details")
                .first()
                .map(|details| {
                    xml_blocks(details, "member")
                        .iter()
                        .map(|entry| Detail {
                            attribute: xml_text(entry, "Attribute"),
                            name: xml_text(entry, "Name"),
                            requires_recreation: xml_text(entry, "RequiresRecreation"),
                            evaluation: xml_text(entry, "Evaluation"),
                            change_source: xml_text(entry, "ChangeSource"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// One row: an edit to the deployed revision, and what the plan must say about the resource
/// it changes.
struct RecreationCase {
    case: &'static str,
    stack: &'static str,
    edits: Vec<(&'static str, &'static str)>,
    /// The one resource the edit changes.
    logical_resource_id: &'static str,
    replacement: &'static str,
    /// The action on the EXISTING physical resource, which only a change that recreates it
    /// takes.
    policy_action: Option<&'static str>,
    details: Vec<Detail>,
}

fn recreation_cases() -> Vec<RecreationCase> {
    vec![
        // The recorded trace's own property: the embedding model lives INSIDE the knowledge
        // base's configuration, and the knowledge base's vectors were produced by it, so
        // the configuration carries a value the resource cannot take anew among values it
        // can.
        RecreationCase {
            case: "the embedding model nested in a knowledge base's configuration",
            stack: "recreation-embedding-model",
            edits: vec![(FIRST_MODEL, SECOND_MODEL)],
            logical_resource_id: "Escalation",
            replacement: replacement::CONDITIONAL,
            policy_action: Some(policy_action::REPLACE_AND_DELETE),
            details: vec![detail(
                "KnowledgeBaseConfiguration",
                requires_recreation::CONDITIONALLY,
            )],
        },
        RecreationCase {
            case: "a knowledge base's name, which its owning service renames in place",
            stack: "recreation-kb-name",
            edits: vec![("escalation-kb-g1", "escalation-kb-g2")],
            logical_resource_id: "Escalation",
            replacement: replacement::FALSE,
            policy_action: None,
            details: vec![detail("Name", requires_recreation::NEVER)],
        },
        // The shape the recorded change set submits: the knowledge base renamed AND reading
        // another embedding model. The rename recreates nothing, so the configuration
        // decides, and the plan cannot promise replacement up front.
        RecreationCase {
            case: "a knowledge base renamed and given another embedding model",
            stack: "recreation-kb-name-and-model",
            edits: vec![
                ("escalation-kb-g1", "escalation-kb-g2"),
                (FIRST_MODEL, SECOND_MODEL),
            ],
            logical_resource_id: "Escalation",
            replacement: replacement::CONDITIONAL,
            policy_action: Some(policy_action::REPLACE_AND_DELETE),
            details: vec![
                detail(
                    "KnowledgeBaseConfiguration",
                    requires_recreation::CONDITIONALLY,
                ),
                detail("Name", requires_recreation::NEVER),
            ],
        },
        RecreationCase {
            case: "a knowledge base's description, which carries no create-only value",
            stack: "recreation-kb-description",
            edits: vec![(
                "the escalation knowledge base",
                "the escalation knowledge base, rewritten",
            )],
            logical_resource_id: "Escalation",
            replacement: replacement::FALSE,
            policy_action: None,
            details: vec![detail("Description", requires_recreation::NEVER)],
        },
        // The other side of the rule: Amazon SNS has no call that renames a topic, so the
        // topic a renamed declaration asks for is a different topic.
        RecreationCase {
            case: "the name of a resource its owning service identifies by that name",
            stack: "recreation-topic-name",
            edits: vec![("recreation-notify", "recreation-notified")],
            logical_resource_id: "Notify",
            replacement: replacement::TRUE,
            policy_action: Some(policy_action::REPLACE_AND_DELETE),
            details: vec![detail("TopicName", requires_recreation::ALWAYS)],
        },
        RecreationCase {
            case: "an updatable property of a resource identified by its name",
            stack: "recreation-topic-display",
            edits: vec![("the stack's own", "renamed in the template")],
            logical_resource_id: "Notify",
            replacement: replacement::FALSE,
            policy_action: None,
            details: vec![detail("DisplayName", requires_recreation::NEVER)],
        },
    ]
}

#[tokio::test]
async fn a_change_set_reports_what_each_changed_property_does_to_the_resource() {
    let (router, engine) = router_with_engine();
    for case in recreation_cases() {
        let name = case.case;
        let (code, body) = create_stack(&router, case.stack, &base_template(case.stack), &[]).await;
        assert_eq!(
            code, 200,
            "case {name:?}: the stack the change set is for must be created, got {code} {body}"
        );
        // The knowledge base its owning service creates comes up `CREATING`, and the stack's
        // own create is held open until that service reports it `ACTIVE`, so the deployment
        // is given past that instant before the change set is asked for.
        wait_for_the_transition().await;
        settle_the_store(&router, &engine).await;

        let (code, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", case.stack),
                member("ChangeSetName", "recreation"),
                member("TemplateBody", &edited(case.stack, &case.edits)),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {name:?}: the change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;

        let (code, body) = call(
            &router,
            "DescribeChangeSet",
            &[
                member("ChangeSetName", "recreation"),
                member("StackName", case.stack),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {name:?}: the change set must be describable, got {code} {body}"
        );

        let reported = reported_changes(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {name:?}: expected the one edit to plan one change, got {reported:?}"
        );
        let change = &reported[0];
        assert_eq!(
            change.logical_resource_id.as_deref(),
            Some(case.logical_resource_id),
            "case {name:?}: expected the change of {:?}, got {change:?}",
            case.logical_resource_id
        );
        assert_eq!(
            change.action.as_deref(),
            Some(change_action::MODIFY),
            "case {name:?}: expected a Modify, got {change:?}"
        );
        assert_eq!(
            change.replacement.as_deref(),
            Some(case.replacement),
            "case {name:?}: expected Replacement {:?}, got {change:?}",
            case.replacement
        );
        assert_eq!(
            change.policy_action.as_deref(),
            case.policy_action,
            "case {name:?}: expected PolicyAction {:?}, got {change:?}",
            case.policy_action
        );
        assert_eq!(
            change.details, case.details,
            "case {name:?}: expected Details {:?}, got {:?}",
            case.details, change.details
        );
    }
}
