// SPDX-License-Identifier: CC0-1.0

//! Which revisions of a declaration keep a resource whose identifier its OWNING SERVICE
//! minted, end to end over the awsQuery wire.
//!
//! RULE under test: **a resource whose physical id its owning service assigned is the SAME
//! resource across a deployment that restates only properties that service's own update call
//! can change — including the resource's NAME, where the service identifies the resource by
//! the id it minted rather than by that name — so the deployment updates it in place and
//! every value the stack resolves from its identity (`Ref`, the type's identifier attribute,
//! an output, an export) is unchanged; a deployment that restates a property the resource
//! cannot take a new value for, or the create-only VALUE inside such a property, asks for a
//! different resource, which CloudFormation replaces and whose published identity therefore
//! moves.**
//!
//! Why it matters: the published identifier is the resource's identity to every consumer —
//! it is cached, written into parameters, baked into ARNs and imported by other stacks. An
//! in-place rename that silently re-identifies the resource breaks every consumer that held
//! the old value, and re-creates in the owning service a resource nothing asked for.
//!
//! The rows differ along what the rule generalises over: WHICH updatable property the
//! revision restates (the description, the name, the IAM role, two of them at once, none at
//! all), and — as the other side of the rule — a revision that restates the create-only value
//! inside the type's conditionally-replacing property. Driven over
//! `AWS::Bedrock::KnowledgeBase`, whose `knowledgeBaseId` Amazon Bedrock Agent mints
//! (`^[0-9a-zA-Z]{10}$`) and whose `UpdateKnowledgeBase` takes that id together with a new
//! required `name`, a `description` and a `roleArn`, while the embedding model the knowledge
//! base's vectors were produced by is not a value it can be given anew.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::bedrock_knowledge_base_identity::{
    described_physical_id, get_knowledge_base, is_knowledge_base_id, knowledge_base_resource,
    model_arn, opensearch_storage, output_value, published_outputs, TITAN_G1, TITAN_V2,
};
use crate::elbv2_resource_provisioning::poll_to_terminal;
use crate::stack_lifecycle::{call, deploy, member, router, xml_text};

/// The logical id of the knowledge base every row deploys, and the export name its stack
/// publishes the minted id under.
const LOGICAL_ID: &str = "Escalation";

/// What the revision of a row restates, and what the rule owes the resource for it.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The `Name` the revision declares.
    name: &'static str,
    /// The `Description` the revision declares.
    description: &'static str,
    /// The IAM role the revision declares the knowledge base under.
    role: &'static str,
    /// The embedding model the revision declares — the create-only value inside the type's
    /// conditionally-replacing `KnowledgeBaseConfiguration`.
    model: &'static str,
    /// Whether the revision declares one unrelated resource MORE, so that a revision which
    /// restates the knowledge base unchanged is still a change to the stack.
    declares_another_resource: bool,
    /// Whether the revision asks for the SAME resource, so the published identity stays.
    keeps_its_identity: bool,
}

/// The declaration every row starts from.
const FIRST_NAME: &str = "minted-first-name";
const FIRST_DESCRIPTION: &str = "the first revision";
const FIRST_ROLE: &str = "minted-first-role";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the revision restates the knowledge base unchanged while the stack \
                   changes elsewhere",
            stack: "minted-unchanged",
            name: FIRST_NAME,
            description: FIRST_DESCRIPTION,
            role: FIRST_ROLE,
            model: TITAN_G1,
            declares_another_resource: true,
            keeps_its_identity: true,
        },
        Row {
            case: "the revision edits the description alone",
            stack: "minted-described",
            name: FIRST_NAME,
            description: "the second revision",
            role: FIRST_ROLE,
            model: TITAN_G1,
            declares_another_resource: false,
            keeps_its_identity: true,
        },
        Row {
            case: "the revision RENAMES the knowledge base",
            stack: "minted-renamed",
            name: "minted-second-name",
            description: FIRST_DESCRIPTION,
            role: FIRST_ROLE,
            model: TITAN_G1,
            declares_another_resource: false,
            keeps_its_identity: true,
        },
        Row {
            case: "the revision renames it AND edits the description",
            stack: "minted-renamed-described",
            name: "minted-renamed-described-name",
            description: "the renamed revision",
            role: FIRST_ROLE,
            model: TITAN_G1,
            declares_another_resource: false,
            keeps_its_identity: true,
        },
        Row {
            case: "the revision states another IAM role",
            stack: "minted-rerolled",
            name: FIRST_NAME,
            description: FIRST_DESCRIPTION,
            role: "minted-second-role",
            model: TITAN_G1,
            declares_another_resource: false,
            keeps_its_identity: true,
        },
        Row {
            case: "the revision states another EMBEDDING MODEL, which the knowledge base \
                   cannot be given anew",
            stack: "minted-remodelled",
            name: FIRST_NAME,
            description: FIRST_DESCRIPTION,
            role: FIRST_ROLE,
            model: TITAN_V2,
            declares_another_resource: false,
            keeps_its_identity: false,
        },
    ]
}

/// One revision's template: the knowledge base as the row declares it, publishing the minted
/// id through the outputs the identity suite beside this one writes, beside `others`
/// unrelated resources no owning service has to provision.
fn template(
    stack: &str,
    name: &str,
    description: &str,
    role: &str,
    model: &str,
    others: usize,
) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        LOGICAL_ID.to_string(),
        knowledge_base_resource(
            Some(name),
            Some(description),
            role,
            model,
            opensearch_storage(stack, "minted-index"),
        ),
    );
    for index in 0..others {
        resources.insert(
            format!("Handle{index}"),
            json!({ "Type": "AWS::CloudFormation::WaitConditionHandle" }),
        );
    }
    json!({
        "Resources": Value::Object(resources),
        "Outputs": published_outputs(&[LOGICAL_ID])
    })
    .to_string()
}

/// The `(name, description, roleArn, embeddingModelArn)` Amazon Bedrock Agent reports for one
/// knowledge base — the configuration in force, read from the service that owns it.
async fn reported_configuration(
    router: &MultiServiceRouter,
    case: &str,
    knowledge_base_id: &str,
) -> (String, String, String, String) {
    let (status, knowledge_base) = get_knowledge_base(router, knowledge_base_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetKnowledgeBase({knowledge_base_id}) to answer 200, got \
         {status} with {knowledge_base}"
    );
    let text = |member_name: &str| {
        knowledge_base
            .get(member_name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let model = knowledge_base
        .pointer(
            "/knowledgeBaseConfiguration/vectorKnowledgeBaseConfiguration/embeddingModelArn",
        )
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (text("name"), text("description"), text("roleArn"), model)
}

/// Every row: deploy the first revision, deploy the row's revision over it, and hold the
/// published identity to what the rule owes that revision.
#[tokio::test]
async fn a_revision_of_only_updatable_properties_keeps_the_minted_identity() {
    for row in rows() {
        let router = router();
        deploy(
            &router,
            row.stack,
            &template(
                row.stack,
                FIRST_NAME,
                FIRST_DESCRIPTION,
                FIRST_ROLE,
                TITAN_G1,
                0,
            ),
        )
        .await;
        let created = described_physical_id(&router, row.case, row.stack, LOGICAL_ID).await;
        assert!(
            is_knowledge_base_id(&created),
            "case {:?}: expected the create to publish a minted ^[0-9a-zA-Z]{{10}}$ \
             identifier, got {created:?}",
            row.case
        );

        let (status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member(
                    "TemplateBody",
                    &template(
                        row.stack,
                        row.name,
                        row.description,
                        row.role,
                        row.model,
                        usize::from(row.declares_another_resource),
                    ),
                ),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateStack({}) must be accepted, got {status} {body}",
            row.case, row.stack
        );
        let described = poll_to_terminal(&router, row.case, row.stack).await;
        let settled = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            settled, "UPDATE_COMPLETE",
            "case {:?}: expected the revision to be deployed, got {settled:?} with \
             {described}",
            row.case
        );

        // What the stack publishes for the resource after the revision, through the reader a
        // client addresses it by and through the outputs the template exports.
        let published = described_physical_id(&router, row.case, row.stack, LOGICAL_ID).await;
        assert!(
            is_knowledge_base_id(&published),
            "case {:?}: expected a minted ^[0-9a-zA-Z]{{10}}$ identifier, got {published:?}",
            row.case
        );
        for output in [format!("{LOGICAL_ID}Ref"), format!("{LOGICAL_ID}Id")] {
            let reported = output_value(&router, row.case, row.stack, &output).await;
            assert_eq!(
                reported.as_deref(),
                Some(published.as_str()),
                "case {:?}: expected the output {output:?} to report the published identity \
                 {published:?}, got {reported:?}",
                row.case
            );
        }
        assert_eq!(
            published == created,
            row.keeps_its_identity,
            "case {:?}: expected the published identity to {} across the revision, got \
             {created:?} then {published:?}",
            row.case,
            if row.keeps_its_identity {
                "stay"
            } else {
                "move"
            }
        );

        // ...and the owning service reports the revision's own configuration under whichever
        // identity is in force, which is what says the declaration really reached it.
        let (name, description, role, model) =
            reported_configuration(&router, row.case, &published).await;
        let expected = (
            row.name.to_string(),
            row.description.to_string(),
            row.role.to_string(),
            model_arn(row.model),
        );
        let reported = (
            name,
            description,
            role.rsplit('/').next().unwrap_or_default().to_string(),
            model,
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected the knowledge base to report the revision's own \
             configuration {expected:?}, got {reported:?}",
            row.case
        );
    }
}
