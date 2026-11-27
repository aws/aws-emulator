// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares a Cognito-authenticated Amazon OpenSearch Service
//! domain inside its own VPC, beside the internal Application Load Balancer and the target
//! group such an estate is fronted by, deploys — and the domain the OWNING service holds
//! names the stack's own Cognito pools, role, VPC, subnet and security group.
//!
//! RULE under test: **every property a resource provisioner forwards to the owning service's
//! create travels under the TYPE that service's model declares for the member it is bound to,
//! carrying the value the template's intrinsics resolved to — so a property whose member is
//! modelled a string holding a JSON document is sent as that document's TEXT, and a property
//! whose member is modelled a structure is sent as that structure.** A request that spells a
//! member's value in another JSON class is no request that member accepts: the owning service
//! refuses it at deserialization, the resource then fails for a reason that is about the
//! provisioner rather than about the template, and the whole stack rolls back — publishing no
//! outputs at all, so nothing downstream can address the estate it was meant to create.
//!
//! `AWS::OpenSearchService::Domain` is the type that exercises both halves at once:
//! `CreateDomain$AccessPolicies` targets `PolicyDocument`, a string
//! (`smithy_query(query="operation", operation="CreateDomain")`), while a template declares an
//! access policy the way every policy document is declared — as the policy itself; and
//! `CognitoOptions`, `DomainEndpointOptions`, `VPCOptions`, `ClusterConfig` and `EBSOptions`
//! are structures on both sides, so each is forwarded as declared.
//!
//! The rows differ along what the rule generalises over rather than restating one template:
//! the FORM the document property is declared in (an inline object, the text of one, and a
//! declaration carrying none at all), whether Cognito authentication for Dashboards is
//! enabled or left off, whether the endpoint requires HTTPS, and which published TLS policy
//! the endpoint applies — three of the five values the model's `TLSSecurityPolicy` names.
//!
//! What each row then asserts is read back out of the service that OWNS it, never recomputed
//! here: Amazon OpenSearch Service's own `DescribeDomain` for the domain's Cognito, VPC and
//! endpoint configuration and the addresses it publishes, and Elastic Load Balancing's own
//! `DescribeLoadBalancers` for the load balancer the stack published. The host form a load
//! balancer publishes as its `DNSName` is that crate's own rule and is asserted in its own
//! suite; what this one holds is that the output over it resolves to the value Elastic Load
//! Balancing reports.
//!
//! Eight services on one engine, which is why the suite lives here: CloudFormation reaches
//! Amazon EC2, Elastic Load Balancing, Amazon Cognito's two services, IAM and Amazon
//! OpenSearch Service through the in-process callback to create what the template declares.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_monitored_alb_search_estate::{
    cloudformation, describe_domain, elbv2, output, resource_reasons, settle_stack, REGION,
};
use crate::query_wire::{opt_element, outputs_of};

/// The engine release every row deploys, which the cluster below is a pairing the owning
/// service's catalogue offers for — so the only thing a row can fail on is the configuration
/// under test.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The cluster every row's domain runs on.
fn cluster_config() -> Value {
    json!({"InstanceType": "t3.small.search", "InstanceCount": 1})
}

/// And the storage it is attached to.
fn ebs_options() -> Value {
    json!({"EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10})
}

/// The network every row's estate is placed in: a VPC and two subnets of it in two
/// Availability Zones, since Elastic Load Balancing places an Application Load Balancer in at
/// least two — while the domain is placed in the FIRST of them alone, which is the placement
/// such an estate gives a domain reached only from inside the VPC.
const VPC_CIDR: &str = "10.73.0.0/16";
const SUBNET_A_CIDR: &str = "10.73.1.0/24";
const SUBNET_B_CIDR: &str = "10.73.2.0/24";

/// The key `DomainStatus$Endpoints` publishes a VPC-reached domain's IPv4 host under
/// (`smithy_query(query="doc",shape="DomainStatus",member="Endpoints")` — "IPv4 IP addresses —
/// `'vpc', …`"), which is also the entry the stack's endpoint output reads through the dotted
/// `Fn::GetAtt` path a map-valued attribute is addressed with.
const ENDPOINTS_KEY_VPC: &str = "vpc";

/// How long a row waits for a condition to hold, and how often it looks again. Generous on
/// purpose: what is waited for is the CONDITION, and the deadline is here only so a failure is
/// reported as one rather than hanging.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// Every output key a row declares — the identifiers and addresses a client of such an estate
/// addresses it through, which is what a rolled-back stack publishes none of.
const DECLARED_OUTPUTS: &[&str] = &[
    "AlbArn",
    "AlbDnsName",
    "AlbSgId",
    "CognitoAccessRoleArn",
    "IdentityPoolId",
    "OpenSearchDomainArn",
    "OpenSearchDomainEndpoint",
    "OpenSearchDomainName",
    "OpenSearchSgId",
    "PrivateSubnetAId",
    "TargetGroupArn",
    "UserPoolClientId",
    "UserPoolId",
    "VpcId",
];

/// The emulator with the eight services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them, so every resource the stack declares is created in
/// the service that owns it.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // The CLASSIC Elastic Load Balancing front door too: the two Elastic Load Balancing models
    // share one endpoint and one signing name, so a request for an ELBv2 resource reaches that
    // door and is claimed from there by the model that declares the action and the version.
    router.register_service(
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(engine),
    );
    router
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// How a row's template spells the access policy of its domain — the one property of the type
/// whose API member is modelled a string carrying a JSON document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// As the policy document itself, which is how a template declares a policy.
    Object,
    /// As the TEXT of one, which a template may also write.
    Text,
    /// Not declared at all.
    Absent,
}

/// One row: the names its estate is deployed under, the form its access policy is declared in
/// and the document it carries, whether its domain authenticates Dashboards through Amazon
/// Cognito, and the endpoint options it declares.
struct Row {
    case: &'static str,
    stack: &'static str,
    domain: &'static str,
    load_balancer: &'static str,
    target_group: &'static str,
    /// The prefix the row's user pool hosts its sign-in pages on: a pool that hosts none can
    /// authenticate nobody, so a domain enabling Cognito authentication needs one. No prefix
    /// states the word the service reserves, which is why none of them is named for the
    /// stack it belongs to.
    user_pool_domain: &'static str,
    policy: Policy,
    /// The access policy document, as text. The `Object` row declares the document this text
    /// denotes; the `Text` row declares this very text, spaced as a client writes it.
    document: &'static str,
    cognito: bool,
    enforce_https: bool,
    tls_policy: &'static str,
}

/// The document the first row declares as an object: one statement, with a nested principal.
const ONE_STATEMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"es:ESHttpGet","Resource":"*"}]}"#;

/// And the one the second declares as text: two statements, an array-valued action and both
/// effects, written with the whitespace a client's `json.dumps` leaves between its tokens.
const TWO_STATEMENTS: &str = r#"{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"AWS": "*"}, "Action": ["es:ESHttpGet", "es:ESHttpPost"], "Resource": "*"}, {"Effect": "Deny", "Principal": {"AWS": "*"}, "Action": "es:ESHttpDelete", "Resource": "*"}]}"#;

const ROWS: &[Row] = &[
    Row {
        case: "a cognito-authenticated vpc domain whose access policy is declared as an object",
        stack: "sim-os-cognito-object",
        domain: "sim-os-cognito-object",
        load_balancer: "sim-os-cognito-object-alb",
        target_group: "sim-os-cognito-obj-tg",
        user_pool_domain: "sim-os-search-object",
        policy: Policy::Object,
        document: ONE_STATEMENT,
        cognito: true,
        enforce_https: true,
        tls_policy: "Policy-Min-TLS-1-2-2019-07",
    },
    Row {
        case: "the same estate whose access policy is declared as the text of a larger document",
        stack: "sim-os-cognito-text",
        domain: "sim-os-cognito-text",
        load_balancer: "sim-os-cognito-text-alb",
        target_group: "sim-os-cognito-txt-tg",
        user_pool_domain: "sim-os-search-text",
        policy: Policy::Text,
        document: TWO_STATEMENTS,
        cognito: true,
        enforce_https: true,
        tls_policy: "Policy-Min-TLS-1-0-2019-07",
    },
    Row {
        case: "a vpc domain that authenticates no dashboards user and declares no access policy",
        stack: "sim-os-no-cognito",
        domain: "sim-os-no-cognito",
        load_balancer: "sim-os-no-cognito-alb",
        target_group: "sim-os-no-cognito-tg",
        user_pool_domain: "sim-os-search-plain",
        policy: Policy::Absent,
        document: "",
        cognito: false,
        enforce_https: false,
        tls_policy: "Policy-Min-TLS-1-2-PFS-2023-10",
    },
];

/// The `CognitoOptions` container the row declares: the three resources of the stack the
/// feature is served by for a row that enables it, and the disabled container for one that
/// does not — whose members name nothing, because nothing would use them.
fn cognito_options(row: &Row) -> Value {
    if !row.cognito {
        return json!({"Enabled": false});
    }
    json!({
        "Enabled": true,
        "UserPoolId": {"Ref": "UserPool"},
        "IdentityPoolId": {"Ref": "IdentityPool"},
        "RoleArn": {"Fn::GetAtt": ["CognitoAccessRole", "Arn"]},
    })
}

/// The `AccessPolicies` property as the row's template writes it, or `None` for the row that
/// declares none.
fn declared_policy(row: &Row) -> Option<Value> {
    match row.policy {
        Policy::Object => {
            Some(serde_json::from_str(row.document).expect("the row's document is JSON"))
        }
        Policy::Text => Some(Value::String(row.document.to_string())),
        Policy::Absent => None,
    }
}

/// The body a row deploys: the network, the two groups, the Cognito pools and the role that
/// configures them, the internal load balancer and its target group, and the domain that
/// `DependsOn` the sign-in pages its Cognito configuration needs.
fn template(row: &Row) -> String {
    let mut domain = json!({
        "DomainName": row.domain,
        "EngineVersion": ENGINE_VERSION,
        "ClusterConfig": cluster_config(),
        "EBSOptions": ebs_options(),
        "VPCOptions": {
            "SubnetIds": [{"Ref": "PrivateSubnetA"}],
            "SecurityGroupIds": [{"Ref": "OpenSearchSg"}],
        },
        "DomainEndpointOptions": {
            "EnforceHTTPS": row.enforce_https,
            "TLSSecurityPolicy": row.tls_policy,
        },
        "CognitoOptions": cognito_options(row),
    });
    if let Some(declared) = declared_policy(row) {
        domain
            .as_object_mut()
            .expect("the domain's properties are an object")
            .insert("AccessPolicies".to_string(), declared);
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a cognito-authenticated search domain inside its own VPC",
        "Resources": {
            "Vpc": {"Type": "AWS::EC2::VPC", "Properties": {"CidrBlock": VPC_CIDR}},
            "PrivateSubnetA": {"Type": "AWS::EC2::Subnet", "Properties": {
                "VpcId": {"Ref": "Vpc"},
                "CidrBlock": SUBNET_A_CIDR,
                "AvailabilityZone": format!("{REGION}a"),
            }},
            "PrivateSubnetB": {"Type": "AWS::EC2::Subnet", "Properties": {
                "VpcId": {"Ref": "Vpc"},
                "CidrBlock": SUBNET_B_CIDR,
                "AvailabilityZone": format!("{REGION}b"),
            }},
            "OpenSearchSg": {"Type": "AWS::EC2::SecurityGroup", "Properties": {
                "GroupDescription": "the search domain group",
                "VpcId": {"Ref": "Vpc"},
            }},
            "AlbSg": {"Type": "AWS::EC2::SecurityGroup", "Properties": {
                "GroupDescription": "the load balancer group",
                "VpcId": {"Ref": "Vpc"},
            }},
            "UserPool": {"Type": "AWS::Cognito::UserPool", "Properties": {
                "UserPoolName": format!("{}-users", row.stack),
            }},
            "UserPoolClient": {"Type": "AWS::Cognito::UserPoolClient", "Properties": {
                "UserPoolId": {"Ref": "UserPool"},
                "ClientName": format!("{}-dashboards", row.stack),
            }},
            "UserPoolDomain": {"Type": "AWS::Cognito::UserPoolDomain", "Properties": {
                "Domain": row.user_pool_domain,
                "UserPoolId": {"Ref": "UserPool"},
            }},
            "IdentityPool": {"Type": "AWS::Cognito::IdentityPool", "Properties": {
                "IdentityPoolName": format!("{}-identities", row.stack),
                "AllowUnauthenticatedIdentities": false,
            }},
            "CognitoAccessRole": {"Type": "AWS::IAM::Role", "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "es.amazonaws.com"},
                        "Action": "sts:AssumeRole",
                    }],
                },
            }},
            "Alb": {"Type": "AWS::ElasticLoadBalancingV2::LoadBalancer", "Properties": {
                "Name": row.load_balancer,
                "Type": "application",
                // INTERNAL: the estate attaches no internet gateway to the VPC it declares,
                // and the load balancer it fronts a VPC-reached domain with carries only
                // private addresses.
                "Scheme": "internal",
                "SecurityGroups": [{"Ref": "AlbSg"}],
                "Subnets": [{"Ref": "PrivateSubnetA"}, {"Ref": "PrivateSubnetB"}],
            }},
            "TargetGroup": {"Type": "AWS::ElasticLoadBalancingV2::TargetGroup", "Properties": {
                "Name": row.target_group,
                "Port": 443,
                "Protocol": "HTTPS",
                "TargetType": "ip",
                "VpcId": {"Ref": "Vpc"},
            }},
            "Domain": {
                "Type": "AWS::OpenSearchService::Domain",
                // The sign-in pages an enabled Cognito configuration is served through are a
                // resource of the stack too, and the owning service refuses a configuration
                // naming a pool that hosts none — so the domain is declared to follow it.
                "DependsOn": ["UserPoolDomain", "IdentityPool", "CognitoAccessRole"],
                "Properties": domain,
            },
        },
        "Outputs": {
            "VpcId": {"Value": {"Ref": "Vpc"}},
            "PrivateSubnetAId": {"Value": {"Ref": "PrivateSubnetA"}},
            "OpenSearchSgId": {"Value": {"Ref": "OpenSearchSg"}},
            "AlbSgId": {"Value": {"Ref": "AlbSg"}},
            "UserPoolId": {"Value": {"Ref": "UserPool"}},
            "UserPoolClientId": {"Value": {"Ref": "UserPoolClient"}},
            "IdentityPoolId": {"Value": {"Ref": "IdentityPool"}},
            "CognitoAccessRoleArn": {"Value": {"Fn::GetAtt": ["CognitoAccessRole", "Arn"]}},
            "AlbArn": {"Value": {"Ref": "Alb"}},
            "AlbDnsName": {"Value": {"Fn::GetAtt": ["Alb", "DNSName"]}},
            "TargetGroupArn": {"Value": {"Ref": "TargetGroup"}},
            "OpenSearchDomainName": {"Value": {"Ref": "Domain"}},
            "OpenSearchDomainArn": {"Value": {"Fn::GetAtt": ["Domain", "Arn"]}},
            "OpenSearchDomainEndpoint": {
                "Value": {"Fn::GetAtt": ["Domain", format!("DomainEndpoints.{ENDPOINTS_KEY_VPC}")]},
            },
        },
    })
    .to_string()
}

/// The `DomainStatus` the owning service reports for `name` once it reports the domain created
/// and no configuration change in flight on it — polled for the CONDITION, never for a span.
async fn settled_domain(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let status = describe_domain(router, case, name).await;
        if status.get("Created") == Some(&Value::Bool(true))
            && status.get("Processing") == Some(&Value::Bool(false))
        {
            return status;
        }
        last = status;
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!(
        "case {case:?}: the domain never reported itself created with no change in flight, \
         last seen {last}"
    );
}

/// One member of a reported record as the list of texts it carries.
fn reported_list(record: &Value, member: &str) -> Vec<String> {
    record
        .get(member)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Deploy every row, and hold the estate it created to the rule.
#[tokio::test]
async fn a_cognito_authenticated_vpc_domain_deploys_and_reads_back_from_its_owning_service() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let body = template(row);
        let (status, created) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", body.as_str()),
                // The estate declares a role of its own, which a deployment creates only for
                // a request that acknowledges it ("Requires capabilities : [CAPABILITY_IAM]").
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {created}"
        );

        let described = settle_stack(&router, case, row.stack).await;
        let stack_status = opt_element(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {case:?}: expected the estate to deploy, got {stack_status:?} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );

        // A deployed stack publishes every output it declares as a COMPLETE pair: that is what
        // a client reads the estate's identifiers and addresses out of.
        let outputs = outputs_of(&described);
        let reported: BTreeSet<&str> = outputs.keys().map(String::as_str).collect();
        let declared: BTreeSet<&str> = DECLARED_OUTPUTS.iter().copied().collect();
        assert_eq!(
            reported, declared,
            "case {case:?}: every Output of a CREATE_COMPLETE stack must be reported with its \
             OutputValue; got {outputs:?} in {described}"
        );
        for key in DECLARED_OUTPUTS {
            let value = output(&outputs, case, key);
            assert!(
                !value.is_empty(),
                "case {case:?}: output {key:?} carries an empty value ({outputs:?})"
            );
        }

        assert_alb(&router, case, &outputs).await;
        assert_domain(&router, case, row, &outputs).await;
    }
}

/// Elastic Load Balancing owns the load balancer the stack fronted the domain with, so what
/// the stack published for it is that service's own report of it.
async fn assert_alb(router: &MultiServiceRouter, case: &str, outputs: &BTreeMap<String, String>) {
    let alb_arn = output(outputs, case, "AlbArn");
    let described = elbv2(
        router,
        case,
        "DescribeLoadBalancers",
        &[("LoadBalancerArns.member.1", alb_arn)],
    )
    .await;
    assert_eq!(
        opt_element(&described, "Scheme").as_deref(),
        Some("internal"),
        "case {case:?}: expected the declared scheme to reach Elastic Load Balancing, got \
         {described}"
    );
    assert_eq!(
        opt_element(&described, "VpcId").as_deref(),
        Some(output(outputs, case, "VpcId")),
        "case {case:?}: expected the load balancer to be placed in the stack's own VPC, got \
         {described}"
    );
    // The address the output carries is the one that service publishes for it: the FORM of
    // that address is the owning crate's own rule, asserted in its own suite.
    assert_eq!(
        opt_element(&described, "DNSName").as_deref(),
        Some(output(outputs, case, "AlbDnsName")),
        "case {case:?}: expected the AlbDnsName output to be Elastic Load Balancing's own \
         LoadBalancer$DNSName, got {described}"
    );
}

/// Amazon OpenSearch Service owns the domain, so every property the template declared for it
/// is read back from that service — each under the type its member is modelled with.
async fn assert_domain(
    router: &MultiServiceRouter,
    case: &str,
    row: &Row,
    outputs: &BTreeMap<String, String>,
) {
    let name = output(outputs, case, "OpenSearchDomainName");
    let status = settled_domain(router, case, name).await;

    assert_eq!(
        status.get("Deleted"),
        Some(&Value::Bool(false)),
        "case {case:?}: a deployed domain is not deleted, got {status}"
    );
    assert_eq!(
        status.get("ARN").and_then(Value::as_str),
        Some(output(outputs, case, "OpenSearchDomainArn")),
        "case {case:?}: expected the stack's ARN output to be the owning service's own \
         DomainStatus$ARN, got {status}"
    );

    // The endpoint configuration the template declared, with the member a create that named
    // it not reporting what the service sets for a new domain.
    assert_eq!(
        status.get("DomainEndpointOptions"),
        Some(&json!({
            "EnforceHTTPS": row.enforce_https,
            "TLSSecurityPolicy": row.tls_policy,
            "CustomEndpointEnabled": false,
        })),
        "case {case:?}: expected the declared endpoint options to reach the domain, got {status}"
    );

    // The Cognito configuration names the stack's OWN pools and role, as the outputs over
    // them report them.
    let expected_cognito = if row.cognito {
        json!({
            "Enabled": true,
            "UserPoolId": output(outputs, case, "UserPoolId"),
            "IdentityPoolId": output(outputs, case, "IdentityPoolId"),
            "RoleArn": output(outputs, case, "CognitoAccessRoleArn"),
        })
    } else {
        json!({"Enabled": false})
    };
    assert_eq!(
        status.get("CognitoOptions"),
        Some(&expected_cognito),
        "case {case:?}: expected the domain to authenticate Dashboards through the stack's own \
         Cognito resources, got {status}"
    );

    // The placement names the stack's own network, as EC2 minted it.
    let vpc_options = status
        .get("VPCOptions")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no VPCOptions in {status}"));
    assert_eq!(
        vpc_options.get("VPCId").and_then(Value::as_str),
        Some(output(outputs, case, "VpcId")),
        "case {case:?}: expected the domain to report the stack's VPC, got {vpc_options}"
    );
    assert_eq!(
        reported_list(&vpc_options, "SubnetIds"),
        vec![output(outputs, case, "PrivateSubnetAId").to_string()],
        "case {case:?}: expected the domain to report the subnet it was placed in, got \
         {vpc_options}"
    );
    assert_eq!(
        reported_list(&vpc_options, "SecurityGroupIds"),
        vec![output(outputs, case, "OpenSearchSgId").to_string()],
        "case {case:?}: expected the domain to report the group it was placed behind, got \
         {vpc_options}"
    );

    // A domain reached inside a VPC has no public host at all: it publishes the one host it
    // IS reached on, in the map, which is what the stack's endpoint output resolved to.
    let endpoints = status
        .get("Endpoints")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no Endpoints map in {status}"));
    let keys: Vec<&str> = endpoints.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![ENDPOINTS_KEY_VPC],
        "case {case:?}: expected the endpoint map of a VPC-reached domain to carry only the \
         IPv4 host, got {status}"
    );
    assert_eq!(
        endpoints.get(ENDPOINTS_KEY_VPC).and_then(Value::as_str),
        Some(output(outputs, case, "OpenSearchDomainEndpoint")),
        "case {case:?}: expected the stack's endpoint output to be the host the owning service \
         publishes, got {status}"
    );
    assert!(
        status.get("Endpoint").is_none(),
        "case {case:?}: a domain reached inside a VPC publishes no public endpoint, got {status}"
    );

    // And the policy document: carried as the TEXT its member is modelled with, denoting the
    // document the template declared however the template spelled it.
    let carried = status.get("AccessPolicies");
    match declared_policy(row) {
        None => assert!(
            carried.is_none_or(|held| held.as_str().is_some_and(str::is_empty)),
            "case {case:?}: a domain declaring no access policy holds none, got {carried:?}"
        ),
        Some(_) => {
            let text = carried
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no AccessPolicies text in {status}"));
            let held: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: the held policy is not JSON ({error}): {text:?}")
            });
            let expected: Value =
                serde_json::from_str(row.document).expect("the row's document is JSON");
            assert_eq!(
                held, expected,
                "case {case:?}: expected the declared document to reach the domain as its own \
                 text, got {text:?}"
            );
        }
    }
}
