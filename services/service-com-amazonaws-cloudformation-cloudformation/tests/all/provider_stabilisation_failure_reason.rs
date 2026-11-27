// SPDX-License-Identifier: CC0-1.0

//! What a resource whose PROVIDER could not bring it up records as its failure, end to end over
//! the wires CloudFormation and the services that own the declared types speak.
//!
//! RULE: a failure a type's RESOURCE PROVIDER answers for is reported on that resource's event
//! inside the one envelope CloudFormation quotes a provider's answer in — `Resource handler
//! returned message: "<the provider's message>" (RequestToken: <token>, HandlerErrorCode:
//! <class>)` — whether the provider failed because the owning service REFUSED the call it made
//! or because that service ACCEPTED the call and then settled the resource in a state it serves
//! no traffic from, which the provider reports from its own read of the resource. The class says
//! which kind of failure it was: an accepted resource the owning service then could not bring up
//! is `GeneralServiceException`, a request that service would not take is `InvalidRequest`.
//!
//! Why it matters: `HandlerErrorCode` is the machine-readable half of a failed deployment — the
//! field deployment tooling branches on to tell "the service failed, retry" from "the template
//! is wrong, fix it" — and it exists only inside that envelope. A reason worded outside it
//! carries no class at all, so every such parse fails and a service-side failure to stabilise a
//! resource reads like a bad declaration.
//!
//! The rows differ along what the rule generalises over: WHY the provider failed (a resource the
//! owning service settled unusable against a call it refused outright), the shape of the
//! provider's own sentence (fixed text, and text built from the declaration), WHICH value misses
//! being a network load balancer (a network load balancer nobody created, an application load
//! balancer, a target group, a value that is no ARN at all), and therefore the class the failure
//! falls in. A second test puts the refusal half on ANOTHER service's type — a DB instance whose
//! declared role association Amazon RDS refuses once it reports the instance available — where
//! the class is the one that refusal's own code falls in, and the provider's answer is quoted
//! once rather than wrapped in an envelope it already carried.
//!
//! The POSITIVE CONTROL deploys a link over a network load balancer THE SAME TEMPLATE creates:
//! the stack completes, the link is `AVAILABLE` over the ARN the stack published for the load
//! balancer, and that ARN resolves in Elastic Load Balancing itself until the stack is deleted —
//! so what the rows above pin is a provider reporting a failure rather than an estate nothing
//! can deploy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::{apigateway, elb_arn, is_api_gateway_id, member_text};
use crate::harness::{
    call, create_stack, envelope_parts, failed_resource_reason, is_a_request_id, member, refusal,
    router, settled_stack, xml_blocks, xml_text, xml_texts, REGION, SETTLE_DEADLINE,
    SETTLE_POLL_INTERVAL,
};
use crate::stack_lifecycle::{ec2, elbv2};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

/// The logical id every template below declares its VPC link under.
const LINK: &str = "Private";

/// How many characters a minted VPC link id carries: API Gateway assigns a record of the
/// account ten lowercase alphanumeric characters.
const VPC_LINK_ID_CHARACTERS: usize = 10;

/// The class a failure of the owning SERVICE falls in — it accepted the call and then could not
/// bring the resource up — and the class a request that service would not take falls in. Both
/// are values of `HandlerErrorCode`.
const SERVICE_FAILED: &str = "GeneralServiceException";
const INVALID_REQUEST: &str = "InvalidRequest";

/// The `statusMessage` API Gateway settles a link `FAILED` with when a declared target names a
/// network load balancer that does not exist. Fixed text: it names no part of the request.
const NLB_ARN_NOT_FOUND: &str = "NLB ARN not found";

/// The `statusMessage` that service settles a link with when a target names no network load
/// balancer at all; the target it names is the variable part.
fn not_a_network_load_balancer(target: &str) -> String {
    format!("Unable to provision the VPC link: {target} does not name a network load balancer")
}

/// How the `AWS::ApiGateway::VpcLink` provider words a link it could not stabilise: the
/// identifier API Gateway minted for the link, and the description that service settled it with.
fn failed_to_stabilise(vpc_link_id: &str, status_message: &str) -> String {
    format!("Failed to stabilize Vpc Link with id {vpc_link_id} Status Message {status_message}.")
}

/// Why the provider of one row's link failed.
enum Failed {
    /// The owning service accepted the create and then settled the link in a status it carries
    /// no traffic in, with this `statusMessage` — so what the provider reports is its own read
    /// of a resource that will not come up.
    Settled(String),
    /// The owning service REFUSED the create, with a message carrying this fragment: there is no
    /// link at all, so nothing was read about one.
    Refused(String),
}

/// One row: a VPC link declaration whose provider cannot bring the link up, and the class its
/// failure falls in.
struct Row {
    case: &'static str,
    stack: &'static str,
    link_name: &'static str,
    target: String,
    failed: Failed,
    class: &'static str,
}

fn rows() -> Vec<Row> {
    let unresolvable = network_load_balancer_arn("stabilise-absent");
    let application = elb_arn("loadbalancer/app/stabilise-alb/0123456789abcdef");
    let target_group = elb_arn("targetgroup/stabilise-targets/0123456789abcdef");
    let not_an_arn = "the-orders-load-balancer".to_string();
    vec![
        Row {
            // The recorded shape: a well-formed network load balancer ARN of this account that
            // names a load balancer nobody created.
            case: "a network load balancer ARN nobody created",
            stack: "stabilise-absent",
            link_name: "stabilise-absent-link",
            target: unresolvable,
            failed: Failed::Settled(NLB_ARN_NOT_FOUND.to_string()),
            class: SERVICE_FAILED,
        },
        Row {
            case: "an application load balancer",
            stack: "stabilise-alb",
            link_name: "stabilise-alb-link",
            target: application.clone(),
            failed: Failed::Settled(not_a_network_load_balancer(&application)),
            class: SERVICE_FAILED,
        },
        Row {
            case: "a target group, which is no load balancer at all",
            stack: "stabilise-targets",
            link_name: "stabilise-targets-link",
            target: target_group.clone(),
            failed: Failed::Settled(not_a_network_load_balancer(&target_group)),
            class: SERVICE_FAILED,
        },
        Row {
            // The other half of the rule: the owning service refused the call the provider made
            // rather than settling a link, so the SAME envelope carries the class a refused
            // request falls in.
            case: "a value that is no ARN at all, which the create is refused for",
            stack: "stabilise-garbage",
            link_name: "stabilise-garbage-link",
            target: not_an_arn.clone(),
            failed: Failed::Refused(not_an_arn),
            class: INVALID_REQUEST,
        },
    ]
}

/// The ARN of a NETWORK load balancer of this account, in the form that service writes one
/// (`loadbalancer/net/<name>/<assigned id>`) — well formed, and held by nobody.
fn network_load_balancer_arn(name: &str) -> String {
    elb_arn(&format!("loadbalancer/net/{name}/0123456789abcdef"))
}

/// One `AWS::ApiGateway::VpcLink` declaration over the targets a case states.
fn vpc_link(name: &str, targets: Value) -> Value {
    json!({
        "Type": "AWS::ApiGateway::VpcLink",
        "Properties": { "Name": name, "TargetArns": targets },
    })
}

/// Create a stack and poll it to the status it settles in, answering that read's body.
async fn deploy(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) -> String {
    let (code, body) = create_stack(router, stack, template, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack({stack}) must answer 200, got {code} {body}"
    );
    settled_stack(case, router, REGION, stack).await
}

/// The `PhysicalResourceId` the stack recorded on the event `logical_id` reached
/// `resource_status` in — the identifier the owning service assigned the resource the event is
/// about.
async fn event_physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
    resource_status: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .find(|entry| {
            xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id)
                && xml_text(entry, "ResourceStatus").as_deref() == Some(resource_status)
        })
        .and_then(|entry| xml_text(entry, "PhysicalResourceId"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an event for {logical_id} in {resource_status}")
        })
}

/// A link its provider cannot bring up fails the resource in the envelope CloudFormation quotes
/// a provider's answer in, under the class that kind of failure falls in — and the stack rolls
/// back.
#[tokio::test]
async fn a_link_its_provider_cannot_bring_up_fails_in_the_handler_envelope() {
    let router = router();
    for row in rows() {
        let case = row.case;
        let template = json!({
            "Resources": { LINK: vpc_link(row.link_name, json!([row.target])) },
        })
        .to_string();
        let described = deploy(&router, case, row.stack, &template).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::ROLLBACK_COMPLETE),
            "case {case:?}: expected the stack to roll back, got {described}"
        );

        let reason = failed_resource_reason(
            case,
            &router,
            REGION,
            row.stack,
            LINK,
            resource_status::CREATE_FAILED,
        )
        .await;
        let (quoted, token, class) = envelope_parts(case, &reason);
        assert_eq!(
            class, row.class,
            "case {case:?}: expected the failure to be classified {:?}, got {reason:?}",
            row.class
        );
        assert!(
            is_a_request_id(&token),
            "case {case:?}: expected the token of the resource request, got {token:?} in \
             {reason:?}"
        );
        match &row.failed {
            Failed::Settled(status_message) => {
                // The link the provider read is the resource the event is about, so the id its
                // sentence names is the one the stack recorded for that resource.
                let vpc_link_id = event_physical_id(
                    &router,
                    case,
                    row.stack,
                    LINK,
                    resource_status::CREATE_FAILED,
                )
                .await;
                assert!(
                    is_api_gateway_id(&vpc_link_id, VPC_LINK_ID_CHARACTERS),
                    "case {case:?}: expected the stack to have recorded the identifier API \
                     Gateway minted, got {vpc_link_id:?}"
                );
                assert_eq!(
                    quoted,
                    failed_to_stabilise(&vpc_link_id, status_message),
                    "case {case:?}: expected the provider's own account of the link it could \
                     not stabilise, got {quoted:?}"
                );
            }
            Failed::Refused(fragment) => {
                assert!(
                    quoted.contains(fragment.as_str()),
                    "case {case:?}: expected the refusal to name {fragment:?}, got {quoted:?}"
                );
            }
        }
        assert!(
            !quoted.contains(ENVELOPE_OPENS),
            "case {case:?}: expected the provider's answer to be quoted ONCE, got {quoted:?}"
        );
    }
}

/// The head of the envelope, for the assertion that a provider's answer is quoted once: a
/// reason that already carried an envelope and was wrapped again is a shape no client can read.
const ENVELOPE_OPENS: &str = "Resource handler returned message: \"";

/// A DB instance whose declared `AssociatedRoles` names a value that is no IAM role ARN: Amazon
/// RDS accepts the create and then REFUSES the association its declaration asks for, which the
/// provider makes once that service reports the instance available.
///
/// `AssociatedRoles` is carried by `AddRoleToDBInstance` rather than by the create, "to add a
/// role to a DB instance, the status of the DB instance must be available".
const REFUSED_ROLE_TEMPLATE: &str = r#"{"Resources":{
    "Database":{"Type":"AWS::RDS::DBInstance","Properties":{
      "DBInstanceIdentifier":"stabilise-roles-db",
      "Engine":"mysql","DBInstanceClass":"db.t3.micro",
      "AssociatedRoles":[{"RoleArn":"not-an-iam-role-arn","FeatureName":"s3Import"}]}}}}"#;

/// The other half of the rule, on another service's type: a call the owning service REFUSES
/// while the provider is bringing the resource up is reported exactly as a refused create is —
/// the envelope once, carrying the class THAT refusal's own code falls in rather than the class
/// of a service that failed to bring an accepted resource up.
#[tokio::test]
async fn a_call_refused_while_bringing_a_resource_up_carries_that_refusal_s_class() {
    let router = router();
    let case = "a DB instance whose declared role association Amazon RDS refuses";
    let stack = "stabilise-roles";
    let described = deploy(&router, case, stack, REFUSED_ROLE_TEMPLATE).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "case {case:?}: expected the stack to roll back, got {described}"
    );
    let reason = failed_resource_reason(
        case,
        &router,
        REGION,
        stack,
        "Database",
        resource_status::CREATE_FAILED,
    )
    .await;
    let (quoted, token, class) = envelope_parts(case, &reason);
    assert_eq!(
        class, INVALID_REQUEST,
        "case {case:?}: expected the class of the refusal Amazon RDS answered, got {reason:?}"
    );
    assert!(
        is_a_request_id(&token),
        "case {case:?}: expected the token of the resource request, got {token:?} in {reason:?}"
    );
    assert!(
        quoted.contains("not-an-iam-role-arn"),
        "case {case:?}: expected that service's own sentence about the value it refused, got \
         {quoted:?}"
    );
    assert!(
        !quoted.contains(ENVELOPE_OPENS),
        "case {case:?}: expected the refusal to be quoted ONCE, got {quoted:?}"
    );
}

/// One row of the control: a link over a network load balancer the same template creates.
struct Deployed {
    case: &'static str,
    stack: &'static str,
    link_name: &'static str,
    /// The name the template gives the load balancer, or `None` to leave it to CloudFormation.
    load_balancer_name: Option<&'static str>,
}

const DEPLOYED: &[Deployed] = &[
    Deployed {
        // The recorded shape: a VPC, a subnet of it, an internal network load balancer in that
        // subnet, and a link whose target is `Ref` of the load balancer.
        case: "a link over a load balancer the template names",
        stack: "stabilise-deployed-named",
        link_name: "stabilise-deployed-named-link",
        load_balancer_name: Some("stabilise-deployed-nlb"),
    },
    Deployed {
        case: "a link over a load balancer whose name CloudFormation decides",
        stack: "stabilise-deployed-generated",
        link_name: "stabilise-deployed-generated-link",
        load_balancer_name: None,
    },
];

/// The template one control row deploys: the network the load balancer is placed in, the load
/// balancer, and the link over the ARN the stack publishes for it.
fn deployed_template(row: &Deployed) -> String {
    let mut load_balancer = json!({
        "Type": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "Properties": {
            "Type": "network",
            "Scheme": "internal",
            "Subnets": [{ "Ref": "Subnet" }],
        },
    });
    if let Some(name) = row.load_balancer_name {
        load_balancer["Properties"]["Name"] = json!(name);
    }
    json!({
        "Resources": {
            "Vpc": {
                "Type": "AWS::EC2::VPC",
                "Properties": { "CidrBlock": "10.93.0.0/16" },
            },
            "Subnet": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": "10.93.1.0/24",
                    "AvailabilityZone": format!("{REGION}a"),
                },
            },
            "Nlb": load_balancer,
            LINK: vpc_link(row.link_name, json!([{ "Ref": "Nlb" }])),
        },
        "Outputs": {
            "VpcId": { "Value": { "Ref": "Vpc" } },
            "NlbArn": { "Value": { "Ref": "Nlb" } },
            "VpcLinkId": { "Value": { "Ref": LINK } },
        },
    })
    .to_string()
}

/// The value the settled stack reports for one declared output.
fn output(case: &str, described: &str, key: &str) -> String {
    xml_blocks(described, "member")
        .iter()
        .find(|entry| xml_text(entry, "OutputKey").as_deref() == Some(key))
        .and_then(|entry| xml_text(entry, "OutputValue"))
        .unwrap_or_else(|| panic!("case {case:?}: expected the output {key:?} in {described}"))
}

/// `DescribeLoadBalancers` over one ARN, as a client drives the estate the stack deployed.
async fn described_load_balancer(
    router: &MultiServiceRouter,
    load_balancer_arn: &str,
) -> (u16, String) {
    elbv2(
        router,
        "DescribeLoadBalancers",
        &[member("LoadBalancerArns.member.1", load_balancer_arn)],
    )
    .await
}

/// The control: a link over a load balancer the stack itself creates comes up, and the ARN the
/// stack published for that load balancer resolves in Elastic Load Balancing — until the stack
/// is deleted, which is the only point that service answers for it with its own not-found.
#[tokio::test]
async fn a_link_over_a_load_balancer_the_stack_creates_comes_up_and_resolves() {
    let router = router();
    // One Availability Zone of this Region has to be known to Amazon EC2 for the subnet the
    // load balancer is placed in: the zones are that service's own, read here rather than
    // assumed.
    let (code, zones) = ec2(&router, "DescribeAvailabilityZones", &[]).await;
    assert_eq!(
        code, 200,
        "setup: DescribeAvailabilityZones must answer 200, got {code} {zones}"
    );
    assert!(
        xml_texts(&zones, "zoneName").iter().any(|zone| zone == &format!("{REGION}a")),
        "setup: expected Amazon EC2 to report the zone the templates place their subnet in, \
         got {zones}"
    );

    for row in DEPLOYED {
        let case = row.case;
        let described = deploy(&router, case, row.stack, &deployed_template(row)).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: expected the stack to complete, got {described}"
        );
        let vpc_id = output(case, &described, "VpcId");
        let load_balancer_arn = output(case, &described, "NlbArn");
        let vpc_link_id = output(case, &described, "VpcLinkId");

        // The load balancer the stack says it created is one Elastic Load Balancing holds, by
        // the very ARN the stack published for it, and it is in the state that service reports
        // a load balancer ready in.
        let (code, reported) = described_load_balancer(&router, &load_balancer_arn).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DescribeLoadBalancers({load_balancer_arn}) to answer 200, \
             got {code} {reported}"
        );
        for (member_name, expected) in [
            ("LoadBalancerArn", load_balancer_arn.as_str()),
            ("Type", "network"),
            ("Scheme", "internal"),
            ("VpcId", vpc_id.as_str()),
            ("Code", "active"),
        ] {
            assert_eq!(
                xml_text(&reported, member_name).as_deref(),
                Some(expected),
                "case {case:?}: expected the load balancer's {member_name} to be {expected:?}, \
                 got {reported}"
            );
        }
        if let Some(name) = row.load_balancer_name {
            assert_eq!(
                xml_text(&reported, "LoadBalancerName").as_deref(),
                Some(name),
                "case {case:?}: expected the declared name, got {reported}"
            );
        }

        // …and the link over it came up, over that same ARN.
        let (code, link) = apigateway(&router, "GET", &format!("/vpclinks/{vpc_link_id}")).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected GetVpcLink({vpc_link_id}) to answer 200, got {code} {link}"
        );
        assert_eq!(
            member_text(&link, "status").as_deref(),
            Some("AVAILABLE"),
            "case {case:?}: expected the link to have come up, got {link}"
        );
        assert_eq!(
            link.get("targetArns"),
            Some(&json!([load_balancer_arn])),
            "case {case:?}: expected the link to target the load balancer the stack published, \
             got {link}"
        );
        // Nothing failed: a resource that came up records no failure at all.
        let (code, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: DescribeStackEvents must answer 200, got {code} {events}"
        );
        assert!(
            !events.contains(resource_status::CREATE_FAILED),
            "case {case:?}: expected no resource of the stack to have failed, got {events}"
        );

        // Only the DELETE takes the load balancer away: the not-found that was the whole report
        // is the right answer here and nowhere earlier.
        let (code, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DeleteStack({}) must answer 200, got {code} {body}",
            row.stack
        );
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        let answered = loop {
            // Every read of the stack is what carries its deletion on.
            let _ = call(&router, "ListStacks", &[]).await;
            let answered = described_load_balancer(&router, &load_balancer_arn).await;
            if answered.0 != 200 || tokio::time::Instant::now() >= deadline {
                break answered;
            }
            tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        };
        let (code, body) = answered;
        assert_eq!(
            code, 400,
            "case {case:?}: expected the deleted stack's load balancer to be gone from Elastic \
             Load Balancing, got {code} {body}"
        );
        let (refused, message) = refusal(&body);
        assert_eq!(
            (refused.as_str(), message.as_str()),
            (
                "LoadBalancerNotFound",
                format!("Load balancers '[{load_balancer_arn}]' not found").as_str()
            ),
            "case {case:?}: expected that service's own not-found, got {body}"
        );
    }
}
