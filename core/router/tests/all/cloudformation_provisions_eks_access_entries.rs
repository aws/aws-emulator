// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::EKS::AccessEntry` really creates that
//! access entry in Amazon EKS — and deleting the stack deletes it again.
//!
//! The rule under test: **a resource type CloudFormation provisions is created through the
//! owning service's own API, with every call that resource's declaration asks for, and a
//! resource that service refuses fails the resource and the stack rather than completing
//! it.** For an access entry those calls are `CreateAccessEntry` plus one
//! `AssociateAccessPolicy` per entry of the declared `AccessPolicies`, so the entry is
//! independently readable through `DescribeAccessEntry` and its policies through
//! `ListAssociatedAccessPolicies`. Three crates have to agree for that to hold
//! (CloudFormation makes the calls, Amazon EKS serves them, and EC2 owns the subnets the
//! host cluster is placed in), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: how many access policies the
//! declaration carries (none, one, two), the scope each is associated under (`cluster`,
//! and `namespace` with its namespaces), the entry's `Type` (left out, declared
//! `STANDARD`, and the `EC2_LINUX` node type that takes neither groups nor policies),
//! whether the declaration names Kubernetes groups, a username and tags, and — for the
//! refusal half — what Amazon EKS rejects (a policy ARN it does not publish, a cluster it
//! does not hold). Every expected value is computed from what the row declares.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the region the cluster, its access
/// entries and the stack belong to.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// Long enough for the clock-owed spans these services choose to come due: the stack's
/// (500 ms) and the cluster's.
const PAST_THE_SPANS: Duration = Duration::from_millis(1_200);

/// The prefix of every Amazon EKS access-policy ARN: an access policy is owned by the
/// service, so its ARN carries neither a Region nor an account id.
const ACCESS_POLICY_ARN_PREFIX: &str = "arn:aws:eks::aws:cluster-access-policy/";

/// The emulator with the three services this suite exercises, mounted on ONE engine
/// exactly as `core/router/src/lib.rs` mounts them. Request authentication is off, as in
/// every other suite here; the in-process call CloudFormation makes signs its request
/// either way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One answer, as this suite reads it: the status, the body as text and as JSON.
struct Answer {
    status: u16,
    text: String,
    body: Value,
}

/// Drive one request at `signing_name`'s wire.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: String,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, body }
}

/// Percent-encode one value: the test crate's single encoder (`query_wire::form_encode`),
/// which leaves only RFC 3986's unreserved characters literal. That is what an awsQuery
/// form component needs AND what an AWS SDK puts in the URI for a non-greedy `@httpLabel`
/// — a principal ARN carries both `:` and `/`, so both have to travel escaped.
fn encoded(value: &str) -> String {
    crate::query_wire::form_encode(value)
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&encoded(key));
        body.push('=');
        body.push_str(&encoded(value));
    }
    send(
        router,
        "cloudformation",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
    .await
}

/// Call one EC2 action over its ec2Query wire.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> Answer {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={}", encoded(value)));
    }
    send(
        router,
        "ec2",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
    .await
}

/// Call one Amazon EKS operation over its restJson1 wire.
async fn eks(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    send(router, "eks", method, uri, "application/json", payload).await
}

/// The text of the first `<name>…</name>` element of an XML body, with the serializer's
/// escaping undone: the test crate's own reader (`query_wire::elements`, which decodes the
/// five escaped entities), narrowed to the first occurrence — a stack's status reason
/// quotes the owning service's message, so the text has to come back as that service
/// wrote it.
fn element(xml: &str, name: &str) -> Option<String> {
    crate::query_wire::elements(xml, name).into_iter().next()
}

/// A role ARN of the account this emulator serves, which is what an access entry's
/// principal is.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The ARN of the Amazon EKS access policy `name` publishes
/// ([`ACCESS_POLICY_ARN_PREFIX`]).
fn policy_arn(name: &str) -> String {
    format!("{ACCESS_POLICY_ARN_PREFIX}{name}")
}

/// Create the VPC and the two subnets a cluster is placed in, through EC2 itself, and
/// answer the subnet ids: Amazon EKS resolves every subnet a cluster names against EC2,
/// so they have to exist before any cluster can be created.
async fn given_subnets(router: &MultiServiceRouter) -> Vec<String> {
    let created = ec2(router, "CreateVpc", &[("CidrBlock", "10.7.0.0/16")]).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateVpc must succeed, got {} with body {}",
        created.status, created.text
    );
    let vpc_id = element(&created.text, "vpcId")
        .unwrap_or_else(|| panic!("setup: no <vpcId> in {}", created.text));
    let mut subnets = Vec::new();
    for cidr_block in ["10.7.1.0/24", "10.7.2.0/24"] {
        let answer = ec2(
            router,
            "CreateSubnet",
            &[("VpcId", &vpc_id), ("CidrBlock", cidr_block)],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "setup: CreateSubnet {cidr_block} must succeed, got {} with body {}",
            answer.status, answer.text
        );
        subnets.push(
            element(&answer.text, "subnetId")
                .unwrap_or_else(|| panic!("setup: no <subnetId> in {}", answer.text)),
        );
    }
    subnets
}

/// Create the cluster an access entry hangs off, through Amazon EKS itself.
async fn given_cluster(router: &MultiServiceRouter, name: &str, subnets: &[String]) {
    let answer = eks(
        router,
        "POST",
        "/clusters",
        Some(json!({
            "name": name,
            "roleArn": role_arn("eksClusterRole"),
            "resourcesVpcConfig": { "subnetIds": subnets },
            "accessConfig": { "authenticationMode": "API_AND_CONFIG_MAP" },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateCluster {name:?} must succeed, got {} with body {}",
        answer.status, answer.text
    );
}

/// Create a stack from `template`, failing the case with the response body when the create
/// itself is refused.
async fn create_stack(router: &MultiServiceRouter, case: &str, stack_name: &str, template: &str) {
    let answer = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {} with body {}",
        answer.status, answer.text
    );
}

/// The `DescribeStacks` body for one stack.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let answer = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {} with body {}",
        answer.status, answer.text
    );
    answer.text
}

/// The template a row deploys: one `AWS::EKS::AccessEntry` whose properties the row
/// declares, with the entry's ARN as the stack's one output so `Fn::GetAtt` is exercised
/// too.
fn template(properties: Value) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        LOGICAL_ID.to_string(),
        json!({ "Type": "AWS::EKS::AccessEntry", "Properties": properties }),
    );
    json!({
        "Resources": Value::Object(resources),
        "Outputs": {
            "EntryArn": { "Value": { "Fn::GetAtt": [LOGICAL_ID, "AccessEntryArn"] } }
        }
    })
    .to_string()
}

/// The logical id every row declares its access entry under.
const LOGICAL_ID: &str = "Entry";

/// One access policy a row expects on the provisioned entry: the policy's name, the scope
/// type it is associated under, and the namespaces that scope names.
struct ExpectedPolicy {
    policy: &'static str,
    scope_type: &'static str,
    namespaces: &'static [&'static str],
}

/// One row of the table: the properties the template declares for the entry, and what
/// Amazon EKS must then hold for it.
struct Row {
    case: &'static str,
    /// The declared properties, built from the cluster name and the principal ARN the row
    /// runs against.
    properties: fn(&str, &str) -> Value,
    /// The `type` the entry must carry — declared by the row, or the documented default
    /// for a declaration that names none.
    expected_type: &'static str,
    /// The Kubernetes groups the entry must carry.
    expected_groups: &'static [&'static str],
    /// The username the entry must carry, where the row declares one.
    expected_username: Option<&'static str>,
    /// The policies `ListAssociatedAccessPolicies` must answer with.
    expected_policies: &'static [ExpectedPolicy],
    /// The tags the row's own declaration asks for, which the entry must carry beside the
    /// ones CloudFormation propagates ([`PROPAGATED_TAG_KEYS`]).
    expected_declared_tags: &'static [(&'static str, &'static str)],
}

/// The tag keys CloudFormation gives every resource it creates, naming the stack and the
/// resource's place in it; `aws:cloudformation:stack-name` and `…:logical-id` are the two
/// whose values this suite can compute, so those are the ones it asserts the values of.
const PROPAGATED_TAG_KEYS: &[&str] = &[
    "aws:cloudformation:logical-id",
    "aws:cloudformation:stack-id",
    "aws:cloudformation:stack-name",
];

const ROWS: &[Row] = &[
    Row {
        case: "one cluster-scoped policy, no type and no username declared",
        properties: |cluster, principal| {
            json!({
                "ClusterName": cluster,
                "PrincipalArn": principal,
                "AccessPolicies": [{
                    "PolicyArn": policy_arn("AmazonEKSViewPolicy"),
                    "AccessScope": { "Type": "cluster" }
                }]
            })
        },
        // "If you don't specify a value, Amazon EKS sets the value to STANDARD"
        // (`CreateAccessEntryRequest$type`).
        expected_type: "STANDARD",
        expected_groups: &[],
        expected_username: None,
        expected_policies: &[ExpectedPolicy {
            policy: "AmazonEKSViewPolicy",
            scope_type: "cluster",
            namespaces: &[],
        }],
        expected_declared_tags: &[],
    },
    Row {
        case: "two policies, one namespace-scoped, with groups, a username, a type and tags",
        properties: |cluster, principal| {
            json!({
                "ClusterName": cluster,
                "PrincipalArn": principal,
                "Type": "STANDARD",
                "Username": "deploy-bot",
                "KubernetesGroups": ["platform", "readers"],
                "Tags": [{ "Key": "team", "Value": "platform" }],
                "AccessPolicies": [
                    {
                        "PolicyArn": policy_arn("AmazonEKSAdminPolicy"),
                        "AccessScope": {
                            "Type": "namespace",
                            "Namespaces": ["kube-system", "team-a"]
                        }
                    },
                    {
                        "PolicyArn": policy_arn("AmazonEKSViewPolicy"),
                        "AccessScope": { "Type": "cluster" }
                    }
                ]
            })
        },
        expected_type: "STANDARD",
        expected_groups: &["platform", "readers"],
        expected_username: Some("deploy-bot"),
        expected_policies: &[
            ExpectedPolicy {
                policy: "AmazonEKSAdminPolicy",
                scope_type: "namespace",
                namespaces: &["kube-system", "team-a"],
            },
            ExpectedPolicy {
                policy: "AmazonEKSViewPolicy",
                scope_type: "cluster",
                namespaces: &[],
            },
        ],
        expected_declared_tags: &[("team", "platform")],
    },
    Row {
        case: "no access policies at all, groups only",
        properties: |cluster, principal| {
            json!({
                "ClusterName": cluster,
                "PrincipalArn": principal,
                "KubernetesGroups": ["viewers"]
            })
        },
        expected_type: "STANDARD",
        expected_groups: &["viewers"],
        expected_username: None,
        expected_policies: &[],
        expected_declared_tags: &[],
    },
    Row {
        case: "a node entry type, which takes neither groups nor policies",
        properties: |cluster, principal| {
            json!({
                "ClusterName": cluster,
                "PrincipalArn": principal,
                "Type": "EC2_LINUX"
            })
        },
        expected_type: "EC2_LINUX",
        // Amazon EKS derives this type's groups itself — the node-authorization group —
        // even though the declaration names none, which is why a node type refuses a
        // `KubernetesGroups` of the caller's at all.
        expected_groups: &["system:nodes"],
        expected_username: None,
        expected_policies: &[],
        expected_declared_tags: &[],
    },
];

/// One refusal row: a declaration the owning service rejects, and the `HandlerErrorCode`
/// class its refusal must be reported under on the failed resource's own event.
struct RefusalRow {
    case: &'static str,
    /// The declared properties, built from the cluster name and the principal ARN.
    properties: fn(&str, &str) -> Value,
    expected_handler_code: &'static str,
    /// The cluster the entry must NOT exist on afterwards — the row's own cluster for a
    /// declaration Amazon EKS refused part-way, so a half-created entry would be visible.
    reads_back_on_host_cluster: bool,
}

const REFUSAL_ROWS: &[RefusalRow] = &[
    RefusalRow {
        case: "an access policy ARN Amazon EKS does not publish",
        properties: |cluster, principal| {
            json!({
                "ClusterName": cluster,
                "PrincipalArn": principal,
                "AccessPolicies": [{
                    "PolicyArn": policy_arn("AmazonEKSNoSuchPolicy"),
                    "AccessScope": { "Type": "cluster" }
                }]
            })
        },
        expected_handler_code: "InvalidRequest",
        reads_back_on_host_cluster: true,
    },
    RefusalRow {
        case: "a namespace-scoped policy naming no namespaces",
        properties: |cluster, principal| {
            json!({
                "ClusterName": cluster,
                "PrincipalArn": principal,
                "AccessPolicies": [{
                    "PolicyArn": policy_arn("AmazonEKSEditPolicy"),
                    "AccessScope": { "Type": "namespace" }
                }]
            })
        },
        expected_handler_code: "InvalidRequest",
        reads_back_on_host_cluster: true,
    },
    RefusalRow {
        case: "a cluster Amazon EKS does not hold",
        properties: |_cluster, principal| {
            json!({
                "ClusterName": "no-such-cluster",
                "PrincipalArn": principal,
                "AccessPolicies": [{
                    "PolicyArn": policy_arn("AmazonEKSViewPolicy"),
                    "AccessScope": { "Type": "cluster" }
                }]
            })
        },
        expected_handler_code: "NotFound",
        reads_back_on_host_cluster: false,
    },
];

/// `DescribeAccessEntry` for one principal on one cluster.
async fn describe_access_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
) -> Answer {
    eks(
        router,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}",
            encoded(cluster),
            encoded(principal)
        ),
        None,
    )
    .await
}

/// `ListAssociatedAccessPolicies` for one principal on one cluster.
async fn list_associated_access_policies(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
) -> Answer {
    eks(
        router,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encoded(cluster),
            encoded(principal)
        ),
        None,
    )
    .await
}

/// Every row: the stack completes, the access entry it declared exists in Amazon EKS with
/// the configuration the template gave it, the declared policies are associated to it, the
/// stack's `Fn::GetAtt` reports the ARN Amazon EKS minted, and deleting the stack deletes
/// the entry.
#[tokio::test]
async fn a_stack_provisions_the_eks_access_entry_its_template_declares() {
    let router = build_router();
    let subnets = given_subnets(&router).await;
    let cluster = "access-entry-host";
    given_cluster(&router, cluster, &subnets).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("access-entry-{index}");
        let principal = role_arn(&format!("principal-{index}"));
        let declared = (row.properties)(cluster, &principal);
        create_stack(&router, case, &stack_name, &template(declared)).await;
        tokio::time::sleep(PAST_THE_SPANS).await;

        let body = describe_stack(&router, case, &stack_name).await;
        assert_eq!(
            element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );

        // The entry is readable through the EKS API itself, independently of the stack.
        let read = describe_access_entry(&router, cluster, &principal).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the declared access entry to be readable, got {} with \
             body {}",
            read.status, read.text
        );
        let entry = &read.body["accessEntry"];
        assert_eq!(
            entry["principalArn"],
            json!(principal),
            "case {case:?}: expected the entry's principalArn to be {principal:?}, got {}",
            read.text
        );
        assert_eq!(
            entry["clusterName"],
            json!(cluster),
            "case {case:?}: expected the entry to be on cluster {cluster:?}, got {}",
            read.text
        );
        assert_eq!(
            entry["type"],
            json!(row.expected_type),
            "case {case:?}: expected type {:?}, got {}",
            row.expected_type,
            read.text
        );
        assert_eq!(
            entry["kubernetesGroups"],
            json!(row.expected_groups),
            "case {case:?}: expected kubernetesGroups {:?}, got {}",
            row.expected_groups,
            read.text
        );
        if let Some(username) = row.expected_username {
            assert_eq!(
                entry["username"],
                json!(username),
                "case {case:?}: expected username {username:?}, got {}",
                read.text
            );
        }

        // The entry carries the tags its declaration asks for AND the ones CloudFormation
        // gives every resource it creates, which are what a tag read on the handle the
        // stack publishes must answer with.
        for (key, value) in row.expected_declared_tags {
            assert_eq!(
                entry["tags"][*key],
                json!(value),
                "case {case:?}: expected the declared tag {key:?}={value:?} on the entry, \
                 got {}",
                read.text
            );
        }
        for key in PROPAGATED_TAG_KEYS {
            assert!(
                entry["tags"].get(*key).is_some(),
                "case {case:?}: expected the propagated tag {key:?} on the entry, got {}",
                read.text
            );
        }
        assert_eq!(
            entry["tags"]["aws:cloudformation:logical-id"],
            json!(LOGICAL_ID),
            "case {case:?}: expected the propagated logical-id tag to name the resource, \
             got {}",
            read.text
        );
        assert_eq!(
            entry["tags"]["aws:cloudformation:stack-name"],
            json!(stack_name),
            "case {case:?}: expected the propagated stack-name tag to name the stack, got {}",
            read.text
        );

        // The stack's `Fn::GetAtt` reports the ARN Amazon EKS minted for the entry, not a
        // handle CloudFormation invented.
        let reported_arn = element(&body, "OutputValue").unwrap_or_default();
        assert_eq!(
            json!(reported_arn),
            entry["accessEntryArn"],
            "case {case:?}: expected the stack to report the entry's own ARN, got \
             {reported_arn:?} against {}",
            read.text
        );

        // Every declared access policy is associated to the entry, under the scope the
        // declaration gave it, and nothing else is.
        let listed = list_associated_access_policies(&router, cluster, &principal).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected the associated policies to be listable, got {} with \
             body {}",
            listed.status, listed.text
        );
        let answered = listed.body["associatedAccessPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let answered_arns: BTreeSet<String> = answered
            .iter()
            .map(|policy| policy["policyArn"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected_arns: BTreeSet<String> = row
            .expected_policies
            .iter()
            .map(|expected| policy_arn(expected.policy))
            .collect();
        assert_eq!(
            answered_arns, expected_arns,
            "case {case:?}: expected exactly the declared policies associated, got {}",
            listed.text
        );
        for expected in row.expected_policies {
            let arn = policy_arn(expected.policy);
            let found = answered
                .iter()
                .find(|policy| policy["policyArn"] == json!(arn))
                .unwrap_or_else(|| panic!("case {case:?}: no {arn} in {}", listed.text));
            assert_eq!(
                found["accessScope"]["type"],
                json!(expected.scope_type),
                "case {case:?}: expected {arn} scoped {:?}, got {}",
                expected.scope_type,
                listed.text
            );
            let namespaces = found["accessScope"]["namespaces"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let namespaces: Vec<&str> = namespaces
                .iter()
                .map(|value| value.as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                namespaces, expected.namespaces,
                "case {case:?}: expected {arn} scoped to {:?}, got {}",
                expected.namespaces, listed.text
            );
        }

        // Deleting the stack deletes the entry: a read of it is then the 404 Amazon EKS
        // answers for an entry it does not hold.
        let deleted = cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {} with body {}",
            deleted.status, deleted.text
        );
        tokio::time::sleep(PAST_THE_SPANS).await;
        let settled = cloudformation(&router, "DescribeStacks", &[("StackName", &stack_name)]).await;
        let settled_status = element(&settled.text, "StackStatus").unwrap_or_default();
        assert!(
            // A stack whose deletion went through is reported DELETE_COMPLETE while it is
            // still resolvable, and refused `ValidationError` once its NAME resolves to
            // nothing ("Deleted stacks: you must specify the unique stack ID") — the two
            // answers a client's deletion waiter accepts, and nothing else.
            (settled.status == 200 && settled_status == "DELETE_COMPLETE")
                || (settled.status != 200 && settled.text.contains("ValidationError")),
            "case {case:?}: expected the stack's deletion to complete, got {} with body {}",
            settled.status, settled.text
        );
        let gone = describe_access_entry(&router, cluster, &principal).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: expected the stack's deletion to delete the entry, got {} with \
             body {}",
            gone.status, gone.text
        );
    }
}

/// Every refusal row: a declaration the owning service rejects fails the resource and the
/// stack — which rolls back, summarising the failed resource on the stack and reporting
/// that service's own refusal on the resource's event — and leaves no access entry behind
/// for it.
#[tokio::test]
async fn a_declaration_eks_refuses_fails_the_stack_and_leaves_no_entry() {
    let router = build_router();
    let subnets = given_subnets(&router).await;
    let cluster = "refused-entry-host";
    given_cluster(&router, cluster, &subnets).await;

    for (index, row) in REFUSAL_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("refused-entry-{index}");
        let principal = role_arn(&format!("refused-principal-{index}"));
        let declared = (row.properties)(cluster, &principal);
        create_stack(&router, case, &stack_name, &template(declared)).await;
        tokio::time::sleep(PAST_THE_SPANS).await;

        let body = describe_stack(&router, case, &stack_name).await;
        assert_eq!(
            element(&body, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than \
             completed, got {body}"
        );
        // The STACK's reason summarises which resource failed; Amazon EKS's own refusal,
        // and the handler class it falls in, are reported on that resource's event.
        let reason = element(&body, "StackStatusReason").unwrap_or_default();
        assert_eq!(
            reason,
            resource_failure_summary(failed_action::CREATE, [LOGICAL_ID]),
            "case {case:?}: expected the stack reason to summarise what failed, got \
             {reason:?}"
        );
        let events = cloudformation(
            &router,
            "DescribeStackEvents",
            &[("StackName", &stack_name)],
        )
        .await
        .text;
        assert!(
            crate::query_wire::elements(&events, "ResourceStatusReason")
                .iter()
                .any(|reported| reported
                    .contains(&format!("HandlerErrorCode: {}", row.expected_handler_code))),
            "case {case:?}: expected the failing resource's event to report {:?}, got \
             {events}",
            row.expected_handler_code
        );

        if row.reads_back_on_host_cluster {
            let gone = describe_access_entry(&router, cluster, &principal).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: a refused declaration must leave no access entry behind, \
                 got {} with body {}",
                gone.status, gone.text
            );
        }
    }
}
