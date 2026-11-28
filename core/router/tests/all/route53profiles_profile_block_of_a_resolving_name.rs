// SPDX-License-Identifier: CC0-1.0

//! What a Route 53 Profile's DNS Firewall push does to a public name that RESOLVES, asked from
//! inside the VPC the Profile is associated with.
//!
//! RULE: the Amazon-provided resolver of a VPC — at the link-local 169.254.169.253 and at the
//! base of the VPC's range plus two — answers a public name whose records a DNS standard fixes
//! with those records under `NOERROR`, and a DNS Firewall rule group reaching that VPC through a
//! Route 53 Profile replaces that answer with the matched `BLOCK` rule's `BlockResponse` — for a
//! `NXDOMAIN` rule, the name that does not exist, with an empty answer section — for the names
//! its domain list covers and for no other name, which keeps the answer it had before the
//! Profile carried anything.
//!
//! The DIFFERENCE between those two answers is the only observation a Profile's push has: no API
//! reports what a VPC's resolver answers, so a Profile whose resolver answers nothing usable for
//! a public name has no observable effect at all. Each name is therefore asked THREE times —
//! before the Profile carries the rule group, while it does, and after the Profile is
//! disassociated from the VPC — and the first phase is an assertion of its own: a name that did
//! not resolve before the rule existed would prove nothing about the block.
//!
//! The rows differ along what the rule generalises over: the FORM of the domain list entry that
//! covers the name (a plain domain matched whole, a wildcard entry), the name's position
//! relative to that entry (the entry's own domain, a label under it, a label under a plain entry
//! that it does not cover), whether any list covers the name at all, whether the name resolves
//! in the first place, and which of the resolver's two addresses the query was sent to.
//!
//! Four crates have to agree for the rule to hold — Amazon EC2 owns the VPC, its subnet and the
//! interface the function is attached through, Route 53 Resolver owns the rule group, its rules
//! and its domain lists, Route 53 Profiles owns the Profile and its two kinds of association,
//! and Lambda runs the handler whose datagram reaches the resolver — so the suite lives here.
//!
//! Every identifier below was minted by the service that owns it and read back out of that
//! service's own answer; the only values written by hand are the ones a request carries.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element};
// The in-VPC querier is the one the sibling suites of this resolver use, so one querier is
// maintained rather than four.
use crate::route53profiles_firewall_in_vpc_resolution::PROBE_HANDLER;

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version an ec2Query form body names.
const EC2_API_VERSION: &str = "2016-11-15";

/// The network the function sits in, and the two addresses its VPC's resolver answers at: the
/// link-local one, and the base of the range plus two.
const VPC_CIDR: &str = "10.74.0.0/16";
/// See [`VPC_CIDR`].
const SUBNET_CIDR: &str = "10.74.1.0/24";
/// See [`VPC_CIDR`].
const ZONE: &str = "us-west-2a";
/// See [`VPC_CIDR`].
const LINK_LOCAL_RESOLVER: &str = "169.254.169.253";
/// See [`VPC_CIDR`].
const RANGE_RESOLVER: &str = "10.74.0.2";
/// Both of those addresses, which every phase asks its rows of: a client in the VPC addresses
/// either one, and the same rules decide what each answers.
const RESOLVER_ADDRESSES: [&str; 2] = [LINK_LOCAL_RESOLVER, RANGE_RESOLVER];

/// The public names the rows ask about: the second-level domains RFC 2606 §3 reserves for
/// documentation, and the one host label conventionally published beside each. Those are the
/// public names a resolver that cannot recurse can still answer, so they are the ones a block of
/// a RESOLVING name can be observed on.
const PLAIN_ENTRY_DOMAIN: &str = "example.com";
/// See [`PLAIN_ENTRY_DOMAIN`]: the host label published beside it, which a plain entry naming the
/// domain does not cover.
const HOST_UNDER_PLAIN_ENTRY: &str = "www.example.com";
/// See [`PLAIN_ENTRY_DOMAIN`]: the domain a WILDCARD entry names, which such an entry covers.
const WILDCARD_ENTRY_DOMAIN: &str = "example.org";
/// See [`PLAIN_ENTRY_DOMAIN`]: a label under that wildcard entry.
const HOST_UNDER_WILDCARD_ENTRY: &str = "www.example.org";
/// See [`PLAIN_ENTRY_DOMAIN`]: the twin control name, on no domain list of the group.
const CONTROL_DOMAIN: &str = "example.net";
/// See [`PLAIN_ENTRY_DOMAIN`]: the host label published beside the control name, also on no list.
const CONTROL_HOST: &str = "www.example.net";

/// A name under the blocked domain that no standard fixes any record for, so nothing in this
/// emulator can resolve it: the row that holds the firewall to answering out of the rules alone.
const UNRESOLVABLE_NAME: &str = "broker.example.com";

/// The address such a documentation name is answered with: the first address of `192.0.2.0/24`,
/// the block RFC 5737 §3 reserves for documentation (TEST-NET-1).
const DOCUMENTATION_ADDRESS: &str = "192.0.2.1";

/// The domain list entries of the two rules, in the two forms an entry may take: a plain domain,
/// matched whole, and the wildcard form, which "can optionally start with `*`".
const PLAIN_ENTRY: &str = PLAIN_ENTRY_DOMAIN;
/// See [`PLAIN_ENTRY`].
const WILDCARD_ENTRY: &str = "*.example.org";

/// The DNS record type a probe asks for, as RFC 1035 numbers it.
const TYPE_A: u64 = 1;

/// The response codes a reply carries, as RFC 1035 §4.1.1 numbers them: no error, a server
/// failure — what this resolver answers a name it would have to recurse for — and a name that
/// does not exist, which is what a `BLOCK` rule answering `NXDOMAIN` states.
const RCODE_NOERROR: u64 = 0;
/// See [`RCODE_NOERROR`].
const RCODE_SERVFAIL: u64 = 2;
/// See [`RCODE_NOERROR`].
const RCODE_NXDOMAIN: u64 = 3;

/// The stack, the execution role and the function that carry the probe.
const STACK: &str = "xsvc-r53p-resolving-probe";
/// See [`STACK`].
const EXEC_ROLE: &str = "xsvc-r53p-resolving-exec";
/// See [`STACK`].
const FUNCTION: &str = "xsvc-r53p-resolving-probe-fn";

/// The `StackStatus` the probe stack must reach, and the suffix of every status that is an
/// operation still in flight.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
/// See [`CREATE_COMPLETE`].
const IN_PROGRESS: &str = "_IN_PROGRESS";

/// The terminal status every resource this suite waits on reports once the work its create
/// started is done: a rule group of Route 53 Resolver, and both kinds of Profile association.
const COMPLETE: &str = "COMPLETE";

/// The `ResourceType` a Profile resource association that carries a DNS Firewall rule group
/// reports.
const FIREWALL_RULE_GROUP: &str = "FIREWALL_RULE_GROUP";

/// How long one invocation is given, as the function's own `Timeout`, and how much memory it is
/// configured with. Both generous: what is under test is what the resolver answers, never how
/// fast an interpreter gets through a handler. 3008 MiB is well inside `MemorySize`'s documented
/// `128..=10240` range.
const FUNCTION_TIMEOUT: u64 = 120;
/// See [`FUNCTION_TIMEOUT`].
const FUNCTION_MEMORY: u64 = 3_008;

/// How often to look while waiting for work a service's own clock owes — a stack completing, a
/// function reaching `Active`, an association settling — and how long to keep looking. Nothing
/// here sleeps for a fixed span: every wait is for the CONDITION, and the deadline is generous so
/// a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(25);
/// See [`LOOK`].
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// The emulator as it is ASSEMBLED, on one engine: every service this suite drives is the one the
/// router mounts, so the firewall the resolver consults is the one Route 53 Resolver registered
/// when it was mounted.
fn build_router() -> MultiServiceRouter {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the credential
    // scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form-encoded (`ec2Query` / `awsQuery`) call, as its status and its XML body.
async fn form_call(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One Amazon EC2 action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = form_call(router, "ec2", EC2_API_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {body}"
    );
    body
}

/// One CloudFormation action.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    form_call(router, "cloudformation", "2010-05-15", action, members).await
}

/// One Route 53 Resolver operation over its `awsJson1_1` wire, which must succeed.
async fn route53resolver(router: &MultiServiceRouter, operation: &str, request: &Value) -> Value {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("route53resolver.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Route53Resolver.{operation}"))
        .header("authorization", authorization("route53resolver"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "setup: Route53Resolver.{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// One Route 53 Profiles operation over its `restJson1` wire that must succeed.
async fn profiles_ok(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Value {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("route53profiles.{REGION}.amazonaws.com"))
        .header("authorization", authorization("route53profiles"));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(
            body.map(Value::to_string).unwrap_or_default().into_bytes(),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert!(
        (200..300).contains(&status),
        "setup: {method} {uri} must succeed, got {status} with {payload}"
    );
    payload
}

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The string member `member` of `document`.
fn string_of(document: &Value, member: &str) -> String {
    document
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: {document} must report {member}"))
        .to_string()
}

/// The network the function and the Profile both address.
struct Network {
    /// `Vpc$VpcId`, which is what the Profile is associated WITH.
    vpc: String,
    /// `Subnet$SubnetId` the function's interface is placed in.
    subnet: String,
    /// `SecurityGroup$GroupId` the function carries.
    group: String,
}

/// Create that network through Amazon EC2's own operations.
async fn ec2_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await,
        "vpcId",
    );
    let subnet = element(
        &ec2(
            router,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", SUBNET_CIDR),
                ("AvailabilityZone", ZONE),
            ],
        )
        .await,
        "subnetId",
    );
    let group = element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("VpcId", vpc.as_str()),
                ("GroupName", "xsvc-r53p-resolving-group"),
                ("GroupDescription", "the group the probe function carries"),
            ],
        )
        .await,
        "groupId",
    );
    Network { vpc, subnet, group }
}

/// One rule of the group this suite builds: what its domain list holds, and the members of the
/// rule itself.
struct DeclaredRule {
    /// What the rule is called, which is also what its domain list is called.
    name: &'static str,
    /// The entries of its domain list.
    domains: &'static [&'static str],
    /// `CreateFirewallRule$Priority`.
    priority: u64,
}

/// The rules the Profile's rule group carries: one per FORM a domain list entry may take, both
/// answering `NXDOMAIN` so what the row reads is the entry's reach and not the arm.
const DECLARED_RULES: &[DeclaredRule] = &[
    DeclaredRule {
        name: "xsvc-plain-entry",
        domains: &[PLAIN_ENTRY],
        priority: 10,
    },
    DeclaredRule {
        name: "xsvc-wildcard-entry",
        domains: &[WILDCARD_ENTRY],
        priority: 20,
    },
];

/// Create the rule group, its domain lists and its rules through Route 53 Resolver's own
/// operations, and answer the group's `Id` and `Arn` as that service minted them.
async fn plant_the_rule_group(router: &MultiServiceRouter) -> (String, String) {
    let answered = route53resolver(
        router,
        "CreateFirewallRuleGroup",
        &json!({ "CreatorRequestId": "xsvc-r53p-resolving-group", "Name": "xsvc-r53p-resolving" }),
    )
    .await;
    let group = answered.get("FirewallRuleGroup").cloned().unwrap_or_else(|| {
        panic!("setup: the create must answer a FirewallRuleGroup, got {answered}")
    });
    let group_id = string_of(&group, "Id");
    let group_arn = string_of(&group, "Arn");
    for rule in DECLARED_RULES {
        let list = route53resolver(
            router,
            "CreateFirewallDomainList",
            &json!({ "CreatorRequestId": rule.name, "Name": rule.name }),
        )
        .await;
        let list_id = string_of(
            list.get("FirewallDomainList").unwrap_or_else(|| {
                panic!("setup: the create must answer a FirewallDomainList, got {list}")
            }),
            "Id",
        );
        route53resolver(
            router,
            "UpdateFirewallDomains",
            &json!({
                "FirewallDomainListId": list_id,
                "Operation": "ADD",
                "Domains": rule.domains,
            }),
        )
        .await;
        route53resolver(
            router,
            "CreateFirewallRule",
            &json!({
                "CreatorRequestId": format!("{}-rule", rule.name),
                "FirewallRuleGroupId": group_id,
                "FirewallDomainListId": list_id,
                "Name": rule.name,
                "Priority": rule.priority,
                "Action": "BLOCK",
                // "the domain name that's in the query doesn't exist".
                "BlockResponse": "NXDOMAIN",
            }),
        )
        .await;
    }
    (group_id, group_arn)
}

/// Wait until every object the firewall is read off reports its terminal status: the rule group
/// with each of its rules, the Profile resource association that carries the group, and the
/// Profile association that carries the Profile to the VPC.
async fn wait_until_the_push_has_settled(
    router: &MultiServiceRouter,
    group_id: &str,
    resource_association_id: &str,
    association_id: &str,
) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let group = route53resolver(
            router,
            "GetFirewallRuleGroup",
            &json!({ "FirewallRuleGroupId": group_id }),
        )
        .await;
        let rules = route53resolver(
            router,
            "ListFirewallRules",
            &json!({ "FirewallRuleGroupId": group_id }),
        )
        .await;
        let resource = profiles_ok(
            router,
            "GET",
            &format!("/profileresourceassociation/{resource_association_id}"),
            None,
        )
        .await;
        let association = profiles_ok(
            router,
            "GET",
            &format!("/profileassociation/{association_id}"),
            None,
        )
        .await;
        let group_settled =
            group.pointer("/FirewallRuleGroup/Status").and_then(Value::as_str) == Some(COMPLETE);
        let rules_listed = rules
            .get("FirewallRules")
            .and_then(Value::as_array)
            .is_some_and(|listed| {
                listed.len() == DECLARED_RULES.len()
                    && listed.iter().all(|rule| {
                        // A rule states its lifecycle only where it has one: "For rules that do
                        // not require asynchronous provisioning, this field may be absent"
                        // (`FirewallRule$Status`), so a rule reporting none is settled.
                        rule.get("Status")
                            .and_then(Value::as_str)
                            .is_none_or(|status| status == COMPLETE)
                    })
            });
        let resource_settled = resource
            .pointer("/ProfileResourceAssociation/Status")
            .and_then(Value::as_str)
            == Some(COMPLETE);
        let association_settled =
            association.pointer("/ProfileAssociation/Status").and_then(Value::as_str)
                == Some(COMPLETE);
        if group_settled && rules_listed && resource_settled && association_settled {
            // The resource association reports the kind of resource it carries, as the one that
            // pushes a rule group to a VPC must.
            assert_eq!(
                resource
                    .pointer("/ProfileResourceAssociation/ResourceType")
                    .and_then(Value::as_str),
                Some(FIREWALL_RULE_GROUP),
                "setup: the resource association must report the kind of resource it carries, \
                 got {resource}"
            );
            return;
        }
        let last = json!({
            "group": group,
            "rules": rules,
            "resource": resource,
            "association": association,
        });
        assert!(
            Instant::now() < deadline,
            "setup: the Profile's push must settle, it still reports {last}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// The template the suite deploys: the execution role, and the probe function ATTACHED to the VPC
/// the Profile is associated with. The inline source is archived into the deployment package by
/// CloudFormation, which is how a function whose handler is written here comes to exist.
fn stack_template(network: &Network) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ExecRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": EXEC_ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "probe-grants",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "logs:CreateLogGroup",
                                    "logs:CreateLogStream",
                                    "logs:PutLogEvents",
                                    "ec2:CreateNetworkInterface",
                                    "ec2:DescribeNetworkInterfaces",
                                    "ec2:DeleteNetworkInterface",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Probe": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["ExecRole", "Arn"]},
                    "Timeout": FUNCTION_TIMEOUT,
                    "MemorySize": FUNCTION_MEMORY,
                    "VpcConfig": {
                        "SubnetIds": [network.subnet],
                        "SecurityGroupIds": [network.group],
                    },
                    "Code": {"ZipFile": PROBE_HANDLER},
                },
            },
        },
    })
    .to_string()
}

/// Deploy that stack and wait for `CREATE_COMPLETE`.
async fn deploy(router: &MultiServiceRouter, network: &Network) {
    let template = stack_template(network);
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({STACK:?}) must be accepted, got {status} with {answered}"
    );
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", STACK)]).await;
        assert_eq!(
            status, 200,
            "setup: DescribeStacks({STACK:?}) must succeed, got {status} with {body}"
        );
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if reported == CREATE_COMPLETE {
            return;
        }
        if !reported.is_empty() && !reported.ends_with(IN_PROGRESS) {
            let (_, events) =
                cloudformation(router, "DescribeStackEvents", &[("StackName", STACK)]).await;
            panic!(
                "setup: the stack must reach {CREATE_COMPLETE}, it reports {reported} with \
                 resource failures {:?}",
                elements(&events, "ResourceStatusReason")
            );
        }
        assert!(
            Instant::now() < deadline,
            "setup: the stack must finish, it still reports {reported:?}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// Wait until Lambda reports the probe function `Active`, which is when it can be invoked.
async fn wait_until_active(router: &MultiServiceRouter) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (_, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            &[],
        )
        .await;
        if configuration["State"].as_str() == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: Lambda must report {FUNCTION:?} Active, it still reports {configuration}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// What one query must be answered with: the response code, and the records of the answer section
/// as `(type, value)` pairs.
#[derive(Clone, Debug)]
struct Answer {
    /// The response code the client reads, as RFC 1035 §4.1.1 numbers one.
    rcode: u64,
    /// One `(type, value)` pair per record of the answer section, in the order they came.
    records: Vec<(u64, String)>,
}

/// The answer a public name a standard reserves for documentation carries: that standard's
/// address, under no error.
fn from_the_standard() -> Answer {
    Answer {
        rcode: RCODE_NOERROR,
        records: vec![(TYPE_A, DOCUMENTATION_ADDRESS.to_string())],
    }
}

/// A `BLOCK` rule answering `NXDOMAIN`: the name does not exist, and the answer section is empty.
fn blocked_nxdomain() -> Answer {
    Answer {
        rcode: RCODE_NXDOMAIN,
        records: Vec::new(),
    }
}

/// A name no zone of the VPC holds and no standard fixes: this resolver could not have resolved
/// it, so it answers nothing for it.
fn unresolvable() -> Answer {
    Answer {
        rcode: RCODE_SERVFAIL,
        records: Vec::new(),
    }
}

/// One row of the table: a query, what it is answered with while NO rule group reaches the VPC —
/// which is also what it must be answered with again once the Profile is disassociated from it —
/// and what it is answered with while the Profile carries the group.
struct Row {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The name the query asks about.
    name: &'static str,
    /// The answer with no rule group in force.
    unfiltered: Answer,
    /// The answer while the Profile's rule group filters the VPC.
    filtered: Answer,
}

/// The rows: one per form the rule generalises over. Every query asks for an `A` record, which is
/// the type a client resolving a name asks first.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the domain a plain entry of a BLOCK rule's list names",
            name: PLAIN_ENTRY_DOMAIN,
            unfiltered: from_the_standard(),
            filtered: blocked_nxdomain(),
        },
        Row {
            // An entry is matched WHOLE, so the name under it keeps resolving.
            case: "the host label published beside that domain, which a plain entry does not cover",
            name: HOST_UNDER_PLAIN_ENTRY,
            unfiltered: from_the_standard(),
            filtered: from_the_standard(),
        },
        Row {
            case: "the domain a wildcard entry names, which such an entry covers",
            name: WILDCARD_ENTRY_DOMAIN,
            unfiltered: from_the_standard(),
            filtered: blocked_nxdomain(),
        },
        Row {
            case: "a label under that wildcard entry",
            name: HOST_UNDER_WILDCARD_ENTRY,
            unfiltered: from_the_standard(),
            filtered: blocked_nxdomain(),
        },
        Row {
            case: "the twin control domain, on no domain list of the group",
            name: CONTROL_DOMAIN,
            unfiltered: from_the_standard(),
            filtered: from_the_standard(),
        },
        Row {
            case: "the host label published beside the control domain, also on no list",
            name: CONTROL_HOST,
            unfiltered: from_the_standard(),
            filtered: from_the_standard(),
        },
        Row {
            // The firewall answers out of its rules alone: a name the entry does not cover is
            // left exactly as unresolvable as it was.
            case: "a name under the blocked domain that no standard fixes a record for",
            name: UNRESOLVABLE_NAME,
            unfiltered: unresolvable(),
            filtered: unresolvable(),
        },
    ]
}

/// Invoke the probe once, asking every row's query of `resolver`, and answer the replies.
async fn probed(
    router: &MultiServiceRouter,
    phase: &str,
    resolver: &str,
    rows: &[Row],
) -> Vec<Value> {
    let probes: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "name": row.name, "type": TYPE_A }))
        .collect();
    let event = json!({ "resolver": resolver, "probes": probes });
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "phase {phase:?} at {resolver}: the invocation must be served, got {status} with {payload}"
    );
    assert_eq!(
        headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok()),
        None,
        "phase {phase:?} at {resolver}: the in-VPC querier must run to completion, got {payload}"
    );
    let results = payload
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "phase {phase:?} at {resolver}: the handler must report one result per probe, got \
                 {payload}"
            )
        });
    assert_eq!(
        results.len(),
        rows.len(),
        "phase {phase:?} at {resolver}: expected one result per probe, got {results:?}"
    );
    results
}

/// Hold every row's reply to what the phase expects.
fn assert_answers(
    phase: &str,
    resolver: &str,
    rows: &[Row],
    results: &[Value],
    expected: impl Fn(&Row) -> Answer,
) {
    for (row, result) in rows.iter().zip(results) {
        let Answer { rcode, records } = expected(row);
        let case = row.case;
        let name = row.name;
        assert_eq!(
            result.get("rcode").and_then(Value::as_u64),
            Some(rcode),
            "phase {phase:?} at {resolver}, case {case:?}: expected response code {rcode} for \
             {name}, got {result}"
        );
        let reported: Vec<(u64, String)> = result
            .get("records")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|record| {
                (
                    record.get("type").and_then(Value::as_u64).unwrap_or_default(),
                    record
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, records,
            "phase {phase:?} at {resolver}, case {case:?}: expected the records {records:?} for \
             {name}, got {result}"
        );
        assert_eq!(
            result.get("ancount").and_then(Value::as_u64),
            Some(records.len() as u64),
            "phase {phase:?} at {resolver}, case {case:?}: expected the answer count to agree \
             with the records for {name}, got {result}"
        );
        // The reply came back from the address the query was sent to, as a datagram's answer does.
        assert_eq!(
            result
                .get("peer")
                .and_then(Value::as_array)
                .and_then(|peer| peer.first())
                .and_then(Value::as_str),
            Some(resolver),
            "phase {phase:?} at {resolver}, case {case:?}: expected the reply to come from \
             {resolver} for {name}, got {result}"
        );
    }
}

/// The phases the rows are asked in, in order.
const BEFORE: &str = "before the Profile carries the rule group";
/// See [`BEFORE`].
const WHILE: &str = "while the Profile carries the rule group";
/// See [`BEFORE`].
const AFTER: &str = "after the Profile is disassociated from the VPC";

#[tokio::test]
async fn a_profiles_block_rule_answers_a_resolving_public_name_as_absent() {
    let router = build_router();
    let network = ec2_network(&router).await;
    let (group_id, group_arn) = plant_the_rule_group(&router).await;
    let profile = profiles_ok(
        &router,
        "POST",
        "/profile",
        Some(&json!({
            "ClientToken": "xsvc-r53p-resolving-profile",
            "Name": "xsvc-r53p-resolving-profile",
        })),
    )
    .await;
    let profile_id = string_of(
        profile
            .get("Profile")
            .unwrap_or_else(|| panic!("setup: CreateProfile must answer a Profile, got {profile}")),
        "Id",
    );
    deploy(&router, &network).await;
    wait_until_active(&router).await;
    let rows = rows();

    // The BASELINE, and an assertion in its own right: every public name a standard fixes
    // resolves from inside the fresh VPC before any firewall object exists, so what the block
    // does to one is observable at all. Asked at BOTH addresses the VPC's resolver answers at,
    // as every phase is, so the difference the push makes is pinned at each of them.
    for resolver in RESOLVER_ADDRESSES {
        let before = probed(&router, BEFORE, resolver, &rows).await;
        assert_answers(BEFORE, resolver, &rows, &before, |row| row.unfiltered.clone());
    }

    // The rule group reaches the Profile, and the Profile the VPC.
    let resource_association = profiles_ok(
        &router,
        "POST",
        "/profileresourceassociation",
        Some(&json!({
            "ProfileId": profile_id,
            "Name": "xsvc-r53p-resolving-resource",
            "ResourceArn": group_arn,
            // "If you are adding a DNS Firewall rule group, include also a priority", which must
            // be "between 100 and 9900".
            "ResourceProperties": r#"{"priority": 110}"#,
        })),
    )
    .await;
    let resource_association_id = string_of(
        resource_association
            .get("ProfileResourceAssociation")
            .unwrap_or_else(|| {
                panic!(
                    "setup: the associate must answer a ProfileResourceAssociation, got \
                     {resource_association}"
                )
            }),
        "Id",
    );
    let vpc_association = profiles_ok(
        &router,
        "POST",
        "/profileassociation",
        Some(&json!({
            "ProfileId": profile_id,
            "Name": "xsvc-r53p-resolving-vpc",
            "ResourceId": network.vpc,
        })),
    )
    .await;
    assert_eq!(
        vpc_association
            .pointer("/ProfileAssociation/ResourceId")
            .and_then(Value::as_str),
        Some(network.vpc.as_str()),
        "setup: the association must name the VPC it was made for, got {vpc_association}"
    );
    let association_id = string_of(
        vpc_association
            .get("ProfileAssociation")
            .unwrap_or_else(|| {
                panic!(
                    "setup: the associate must answer a ProfileAssociation, got {vpc_association}"
                )
            }),
        "Id",
    );
    wait_until_the_push_has_settled(&router, &group_id, &resource_association_id, &association_id)
        .await;

    // Both addresses the VPC's resolver answers at are filtered by the same rules.
    for resolver in RESOLVER_ADDRESSES {
        let filtered = probed(&router, WHILE, resolver, &rows).await;
        assert_answers(WHILE, resolver, &rows, &filtered, |row| {
            row.filtered.clone()
        });
    }

    // The push is the ASSOCIATION's: dissociating the Profile from the VPC gives every name the
    // answer it had before, which is what makes the block observable at all.
    profiles_ok(
        &router,
        "DELETE",
        &format!(
            "/profileassociation/Profileid/{profile_id}/resourceid/{}",
            network.vpc
        ),
        None,
    )
    .await;
    for resolver in RESOLVER_ADDRESSES {
        let after = probed(&router, AFTER, resolver, &rows).await;
        assert_answers(AFTER, resolver, &rows, &after, |row| row.unfiltered.clone());
    }
}
