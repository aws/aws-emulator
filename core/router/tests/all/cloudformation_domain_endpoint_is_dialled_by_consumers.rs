// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a deployed search domain's stack output hands its consumers, dialled.
//!
//! RULE under test: **an endpoint attribute of a deployed resource is the address the OWNING
//! service publishes for that resource, read off that service's own descriptor rather than
//! composed by the deployment — so `Fn::GetAtt <domain>.DomainEndpoint` on an
//! `AWS::OpenSearchService::Domain` is exactly what Amazon OpenSearch Service reports as
//! `DomainStatus$Endpoint` ("the domain-specific endpoint used to submit index, search, and
//! data upload requests to the domain") for the same domain, before and after every revision
//! the deployment applies to it in place; and because it is that value, a consumer handed the
//! output addresses the domain itself.** The consequence the reported symptom is about: a
//! deployment that MINTS an endpoint hostname of its own reports `CREATE_COMPLETE` carrying a
//! plausible address that addresses nothing, so every consumer of the stack — an exported
//! output, a `!GetAtt` into another resource's configuration, an operator copying the output
//! — is silently handed an address the service never published.
//!
//! The rows differ along what the rule generalises over rather than restating one template:
//!
//! * which ADDRESS FAMILIES the domain is provisioned on (IPv4 only, and `dualstack`, which
//!   is provisioned a version 2 endpoint beside its IPv4 one);
//! * the engine release and the cluster shape behind the endpoint (a one-node and a two-node
//!   cluster, on two releases of the owning service's own catalogue);
//! * which operation produced the outputs a consumer then read — the create, and a revision
//!   applied to the domain in place either through `UpdateStack` or through an executed
//!   change set.
//!
//! Every row is asserted TWICE, once per completed deployment, and no expected address is a
//! literal: each is read out of Amazon OpenSearch Service through `DescribeDomain` on its own
//! `restJson1` wire, so a row cannot drift from the service that holds the domain. Beside the
//! equality, each observation DIALS what the stack handed out — the attribute's own value, and
//! the URL a template composed from it with `Fn::Sub`, which is how a consumer receives it —
//! and the cluster that answers has to be the domain the stack created, rather than merely
//! something that answers.
//!
//! Two services on one engine: CloudFormation reaches Amazon OpenSearch Service through the
//! in-process callback to create and to revise the domain, and that service serves the
//! domain's data plane, so neither crate's own test target can observe both ends of it. The
//! awsQuery caller, the domain read, the stack poller, the change-set poller and the
//! credential scope are the test crate's own
//! ([`crate::cloudformation_provisions_opensearch_domains`],
//! [`crate::cloudformation_domain_access_policy_revision`]) rather than a second copy.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_domain_access_policy_revision::settled_change_set;
use crate::cloudformation_provisions_opensearch_domains::{
    authorization, build_router, cloudformation, describe_domain, resource_reasons, settle_stack,
    xml_text,
};
use crate::query_wire::outputs_of;

/// The logical id every row declares its domain under.
const DOMAIN_LOGICAL_ID: &str = "Domain";

/// The output a row reads the endpoint attribute through, and the output whose value a
/// template COMPOSES out of that attribute with `Fn::Sub` — the two forms a consumer of the
/// stack is handed the address in.
const ENDPOINT_OUTPUT: &str = "DomainEndpoint";
const COMPOSED_URL_OUTPUT: &str = "SearchUrl";

/// `IPAddressType`'s `dualstack` value, which is what gives a domain a version 2 endpoint
/// beside its IPv4 one.
const DUALSTACK: &str = "dualstack";

/// The hour a row's domain starts its automated snapshot at when it is first deployed, and
/// the hour its revision moves that to.
///
/// `SnapshotOptions` is a member of `UpdateDomainConfig`
/// (`smithy_query(query="operation", operation="UpdateDomainConfig")`), so a revision stating
/// it differently is applied to the domain the stack already has rather than replacing it —
/// which is the deployment this suite is about, since a replaced domain would publish a new
/// address legitimately. Both hours are inside `AutomatedSnapshotStartHour`'s own 0..24 range.
const DEPLOYED_SNAPSHOT_HOUR: u64 = 3;
const REVISED_SNAPSHOT_HOUR: u64 = 17;

/// How a row's revision is deployed: the two ways a client applies a changed template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deploy {
    UpdateStack,
    ExecuteChangeSet,
}

/// One row: the domain it deploys, the cluster behind its endpoint, which address families it
/// is provisioned on, and how its revision reaches it.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// Inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and 3..28 characters.
    domain: &'static str,
    /// An engine release of the owning service's own catalogue, with an instance type that
    /// release offers.
    engine_version: &'static str,
    instance_type: &'static str,
    instance_count: u64,
    /// `None` for a domain reached over IPv4 only.
    ip_address_type: Option<&'static str>,
    deploy: Deploy,
}

/// The rows differ along what the rule generalises over: the address families the domain is
/// provisioned on, the release and the cluster shape behind its endpoint, and which operation
/// completed the deployment whose outputs a consumer read.
const ROWS: &[Row] = &[
    Row {
        case: "a one-node IPv4 domain revised with UpdateStack",
        stack: "sim-epdial-ipv4",
        domain: "sim-epdial-ipv4",
        engine_version: "OpenSearch_2.11",
        instance_type: "t3.small.search",
        instance_count: 1,
        ip_address_type: None,
        deploy: Deploy::UpdateStack,
    },
    Row {
        case: "a dualstack domain revised through an executed change set",
        stack: "sim-epdial-dual",
        domain: "sim-epdial-dual",
        engine_version: "OpenSearch_2.11",
        instance_type: "t3.small.search",
        instance_count: 1,
        ip_address_type: Some(DUALSTACK),
        deploy: Deploy::ExecuteChangeSet,
    },
    Row {
        case: "a two-node domain on another engine release, revised through a change set",
        stack: "sim-epdial-or1",
        domain: "sim-epdial-or1",
        engine_version: "OpenSearch_2.19",
        instance_type: "or1.medium.search",
        instance_count: 2,
        ip_address_type: None,
        deploy: Deploy::ExecuteChangeSet,
    },
];

/// The body a row deploys: the domain, the endpoint attribute as an output, and a URL the
/// template composes out of that attribute — the form a consumer is handed the address in
/// when a template feeds it to another resource.
fn template(row: &Row, snapshot_hour: u64) -> String {
    let mut properties = json!({
        "DomainName": row.domain,
        "EngineVersion": row.engine_version,
        "ClusterConfig": {
            "InstanceType": row.instance_type,
            "InstanceCount": row.instance_count,
        },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
        "SnapshotOptions": { "AutomatedSnapshotStartHour": snapshot_hour },
    });
    if let Some(kind) = row.ip_address_type {
        properties
            .as_object_mut()
            .expect("the properties are an object")
            .insert("IPAddressType".to_string(), json!(kind));
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a search domain and the address its consumers are handed",
        "Resources": {
            DOMAIN_LOGICAL_ID: {
                "Type": "AWS::OpenSearchService::Domain",
                "Properties": properties,
            },
        },
        "Outputs": {
            ENDPOINT_OUTPUT: {
                "Value": { "Fn::GetAtt": [DOMAIN_LOGICAL_ID, ENDPOINT_OUTPUT] },
            },
            COMPOSED_URL_OUTPUT: {
                "Value": {
                    "Fn::Sub": format!("https://${{{DOMAIN_LOGICAL_ID}.{ENDPOINT_OUTPUT}}}/"),
                },
            },
        },
    })
    .to_string()
}

/// One request to the address the stack handed out, dialled exactly as a consumer dials it:
/// the consumer has a URL, so the authority of that URL is the host the request carries and
/// whatever follows it is the request path.
async fn dialled(router: &MultiServiceRouter, url: &str) -> (u16, Value) {
    let authority = url
        .split_once("://")
        .map_or(url, |(_scheme, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let request = http::Request::builder()
        .method("GET")
        .uri(url)
        .header("host", authority)
        .header("authorization", authorization("es"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One member of a reported record as the text it reads as, or `None` where the record
/// carries no such member.
fn reported_text(reported: &Value, member: &str) -> Option<String> {
    reported
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Hold one completed deployment of `row` to the rule, and answer the address its outputs
/// handed out so a later observation can be compared against it.
async fn the_outputs_hand_out_the_published_address(
    router: &MultiServiceRouter,
    row: &Row,
    observation: &str,
    described: &str,
    expected_snapshot_hour: u64,
) -> String {
    let case = format!("{} ({observation})", row.case);
    let outputs = outputs_of(described);

    // What the OWNING service publishes for the domain, which the expected address is read
    // from rather than spelled here.
    let (status, reported) = describe_domain(router, row.domain).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeDomain({}), got {status} with {reported}",
        row.domain
    );
    let published = reported_text(&reported, "Endpoint").unwrap_or_default();
    assert!(
        !published.is_empty(),
        "case {case:?}: the row's premise is that the service publishes a public endpoint for \
         the domain, got {reported}"
    );
    // The row's other premise: the deployment this observation is of really reached the
    // domain, so the equality below is asserted about a domain the revision touched rather
    // than about one nothing happened to.
    let snapshot_hour = reported
        .get("SnapshotOptions")
        .and_then(|options| options.get("AutomatedSnapshotStartHour"))
        .and_then(Value::as_u64);
    assert_eq!(
        snapshot_hour,
        Some(expected_snapshot_hour),
        "case {case:?}: expected the deployed configuration to be in force on the domain, got \
         {reported}"
    );

    // The rule: the endpoint attribute IS that published address, in both the form the
    // attribute is read in and the form a template composed out of it.
    assert_eq!(
        outputs.get(ENDPOINT_OUTPUT).map(String::as_str),
        Some(published.as_str()),
        "case {case:?}: expected {ENDPOINT_OUTPUT} to be the address the owning service \
         publishes for the domain ({published:?}), got {outputs:?}"
    );
    let composed = outputs.get(COMPOSED_URL_OUTPUT).cloned().unwrap_or_default();
    assert_eq!(
        composed,
        format!("https://{published}/"),
        "case {case:?}: expected {COMPOSED_URL_OUTPUT} to compose the published address, got \
         {outputs:?}"
    );

    // And because it is that address, a consumer that dials what the stack handed out reaches
    // the domain the stack created: the cluster banner a `GET /` answers names it.
    for url in [format!("https://{published}"), composed] {
        let (status, document) = dialled(router, &url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the address the stack handed out ({url}) to answer a \
             signed request, got {status} with {document}"
        );
        assert_eq!(
            document.get("name").and_then(Value::as_str),
            Some(row.domain),
            "case {case:?}: expected {url} to answer for the domain the stack created, got \
             {document}"
        );
        let cluster = document
            .get("cluster_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            cluster.ends_with(&format!(":{}", row.domain)),
            "case {case:?}: expected the answering cluster to be the domain's own, got \
             {cluster:?} in {document}"
        );
    }

    published
}

/// Apply `row`'s revision the way the row deploys it, and leave the stack settling.
async fn revise(router: &MultiServiceRouter, row: &Row, revision: &str) {
    let case = row.case;
    match row.deploy {
        Deploy::UpdateStack => {
            let (status, body) = cloudformation(
                router,
                "UpdateStack",
                &[("StackName", row.stack), ("TemplateBody", revision)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from UpdateStack, got {status} with {body}"
            );
        }
        Deploy::ExecuteChangeSet => {
            let change_set = format!("{}-revision", row.stack);
            let (status, body) = cloudformation(
                router,
                "CreateChangeSet",
                &[
                    ("StackName", row.stack),
                    ("ChangeSetName", &change_set),
                    ("TemplateBody", revision),
                    ("ChangeSetType", "UPDATE"),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from CreateChangeSet, got {status} with {body}"
            );
            // The preview is of a change the owning service makes ON the domain: a replaced
            // domain would publish a new address legitimately, so the row's premise is that
            // this revision is applied in place.
            let planned = settled_change_set(router, case, row.stack, &change_set).await;
            assert_eq!(
                (
                    xml_text(&planned, "LogicalResourceId"),
                    xml_text(&planned, "Action"),
                    xml_text(&planned, "Replacement"),
                ),
                (
                    DOMAIN_LOGICAL_ID.to_string(),
                    "Modify".to_string(),
                    "False".to_string(),
                ),
                "case {case:?}: expected one in-place modification of the domain, got {planned}"
            );
            let (status, body) = cloudformation(
                router,
                "ExecuteChangeSet",
                &[("StackName", row.stack), ("ChangeSetName", &change_set)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from ExecuteChangeSet, got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_stacks_endpoint_output_hands_consumers_the_address_the_service_published() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(row, DEPLOYED_SNAPSHOT_HOUR)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );
        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: expected the declared domain to deploy, got {described} with \
             reasons {:?}",
            resource_reasons(&router, row.stack).await
        );
        let after_create = the_outputs_hand_out_the_published_address(
            &router,
            row,
            "the completed create",
            &described,
            DEPLOYED_SNAPSHOT_HOUR,
        )
        .await;

        revise(&router, row, &template(row, REVISED_SNAPSHOT_HOUR)).await;
        let updated = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&updated, "StackStatus"),
            "UPDATE_COMPLETE",
            "case {case:?}: expected the revision to deploy, got {updated} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );
        let after_update = the_outputs_hand_out_the_published_address(
            &router,
            row,
            "the completed in-place revision",
            &updated,
            REVISED_SNAPSHOT_HOUR,
        )
        .await;

        // A revision the owning service applied TO the domain leaves the domain addressed
        // where it was, so the address a consumer of the first deployment already holds is
        // still the one the stack hands out.
        assert_eq!(
            after_update, after_create,
            "case {case:?}: an in-place revision must not move the address the stack publishes \
             for the domain"
        );
    }
}
