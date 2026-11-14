// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares `AWS::Route53Resolver::FirewallDomainList`,
//! `::FirewallRuleGroup` and `::FirewallRuleGroupAssociation` really creates them in
//! Route 53 Resolver, and the identifiers the stack hands back are the ones that service
//! answers to.
//!
//! The rule under test: **a stack's physical resource id is the identifier the service that
//! owns the resource assigned — for a type whose identifier that service MINTS, the one its
//! create answered with — and a property of the type that is no member of that create (a
//! domain list's `Domains`, a rule group's `FirewallRules`) is applied through the call the
//! owning service publishes for it, so the resource the stack reports is the resource the
//! template declared.** Two crates have to agree for that to hold (CloudFormation makes the
//! calls; Route 53 Resolver serves them and mints the ids), so the suite lives here rather
//! than in either crate.
//!
//! The rows differ along what the rule generalises over: whether the domain list declares
//! domains at all and how many, whether the rule group declares rules and how many, each
//! `Action` and `BlockResponse` a declared rule may carry, whether an integer property is
//! written as a number or as the quoted scalar a template may write it as, whether the
//! association declares mutation protection or takes its documented default, whether the
//! template is JSON or YAML, and which intrinsic each output reads a resource with (`Ref`,
//! `Fn::GetAtt`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the three identifiers themselves, which are
//! Route 53 Resolver's to mint: what is checked about those is that ONE value appears
//! everywhere the stack reports a resource's identity, that it has the form that service
//! mints, and that that service's own `GetFirewallDomainList` / `GetFirewallRuleGroup` /
//! `GetFirewallRuleGroupAssociation` resolve it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) and
/// for Route 53 Resolver's provisioning span (`PROVISIONING_DELAY`, 500 ms) to come due, so
/// the stack reports the template it deployed and the resources report their settled
/// statuses.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(900);

/// How long a poll waits before it looks at the stack again.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long a poll keeps looking before it gives up: generous, so how fast the host runs
/// never decides the outcome.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// Look at the stack again until its own `StackStatus` is one it has REACHED — every status
/// CloudFormation reports while an operation is still running ends in `_IN_PROGRESS` — and
/// answer that status, so a case reads the OUTCOME of the operation and never a span of the
/// clock.
async fn settled_stack(
    router: &MultiServiceRouter,
    region: &str,
    stack_name: &str,
    case: &str,
) -> String {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (_, described) = cloudformation(
            router,
            region,
            "DescribeStacks",
            &[("StackName", stack_name)],
        )
        .await;
        let status = opt_element(&described, "StackStatus").unwrap_or_default();
        if !status.is_empty() && !status.ends_with("_IN_PROGRESS") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack operation must finish, it still reports {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The prefixes Route 53 Resolver mints its DNS Firewall identifiers under: a domain list is
/// `rslvr-fdl-…`, a rule group `rslvr-frg-…` and a rule group association
/// `rslvr-frgassoc-…`.
const DOMAIN_LIST_ID_PREFIX: &str = "rslvr-fdl-";
const RULE_GROUP_ID_PREFIX: &str = "rslvr-frg-";
const ASSOCIATION_ID_PREFIX: &str = "rslvr-frgassoc-";

/// The VPCs the rows name literally, in the form Amazon EC2 ids take.
///
/// A DNS Firewall rule group is associated WITH a VPC of the caller's account, and
/// Route 53 Resolver resolves that `VpcId` against the account's own Amazon EC2 store, so
/// the account holds these VPCs before any stack naming them is deployed
/// ([`register_literal_vpcs`]).
const LITERAL_VPC_IDS: [&str; 2] = ["vpc-0a1b2c3d4e5f60718", "vpc-00112233445566778"];

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The network the rows name is the caller's own: it exists before any stack is
    // deployed, and no stack here declares it.
    register_literal_vpcs(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_route53resolver_route53resolver::EmulatedService::new(engine),
    );
    router
}

/// Put the VPCs the rows name into the account's own Amazon EC2 store, as the modelled
/// shape that service reports.
fn register_literal_vpcs(engine: &engine::InMemoryEngine) {
    use service_com_amazonaws_ec2_amazonec2 as ec2;

    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's Amazon EC2 store");
    let mut store = ec2_state.lock();
    for (index, vpc_id) in LITERAL_VPC_IDS.iter().enumerate() {
        store.vpcs.insert(
            (*vpc_id).to_string(),
            ec2::ops::types::Vpc {
                vpc_id: Some((*vpc_id).to_string()),
                cidr_block: Some(format!("10.{index}.0.0/16")),
                state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        );
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Route 53 Resolver operation over its awsJson1_1 wire, signed for `region`.
async fn route53resolver(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("route53resolver.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Route53Resolver.{operation}"))
        .header("authorization", authorization("route53resolver", region))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// A `DescribeStackResources` answer as the logical-id -> physical-id map a client reads it
/// as.
fn physical_ids_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "LogicalResourceId")
        .into_iter()
        .zip(elements(xml, "PhysicalResourceId"))
        .collect()
}

/// One rule the stack's rule group must hold: its priority, action and block response.
type ExpectedRule = (i64, &'static str, Option<&'static str>);

/// One row of the table: a template, and what the rule says the DNS Firewall resources it
/// declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is also the Region the resources are created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The declared domain list's name, and the domains it must hold in the order declared.
    expected_list_name: &'static str,
    expected_domains: &'static [&'static str],
    /// The declared rule group's name, and the rules it must hold.
    expected_group_name: &'static str,
    expected_rules: &'static [ExpectedRule],
    /// The VPC, priority, name and mutation protection the association must report, where
    /// the row declares one.
    expected_vpc_id: Option<&'static str>,
    expected_priority: Option<i64>,
    expected_association_name: Option<&'static str>,
    expected_mutation_protection: Option<&'static str>,
    /// The output keys whose value must be the domain list's id, the rule group's id and the
    /// association's id, each written with a different intrinsic over the same resource.
    list_id_outputs: &'static [&'static str],
    group_id_outputs: &'static [&'static str],
    association_id_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a domain list, a blocking rule group and an association, JSON, numbers",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "DNS Firewall: a domain list, a rule group and its association",
                "Resources": {
                    "DomainList": {
                        "Type": "AWS::Route53Resolver::FirewallDomainList",
                        "Properties": {
                            "Name": "blocked-domains",
                            "Domains": ["blocked.example.test.", "*.ads.example.test."],
                            "Tags": [{ "Key": "Owner", "Value": "networking" }]
                        }
                    },
                    "RuleGroup": {
                        "Type": "AWS::Route53Resolver::FirewallRuleGroup",
                        "Properties": {
                            "Name": "blocking-group",
                            "FirewallRules": [
                                {
                                    "FirewallDomainListId": { "Ref": "DomainList" },
                                    "Priority": 100,
                                    "Action": "BLOCK",
                                    "BlockResponse": "NXDOMAIN"
                                },
                                {
                                    "FirewallDomainListId": { "Ref": "DomainList" },
                                    "Priority": 200,
                                    "Action": "ALERT"
                                }
                            ],
                            "Tags": [{ "Key": "Owner", "Value": "networking" }]
                        }
                    },
                    "Association": {
                        "Type": "AWS::Route53Resolver::FirewallRuleGroupAssociation",
                        "Properties": {
                            "Name": "filtering-for-the-app-vpc",
                            "FirewallRuleGroupId": { "Ref": "RuleGroup" },
                            "VpcId": "vpc-0a1b2c3d4e5f60718",
                            "Priority": 101,
                            "MutationProtection": "DISABLED"
                        }
                    }
                },
                "Outputs": {
                    "DomainListId": { "Value": { "Ref": "DomainList" } },
                    "DomainListIdAttribute": { "Value": { "Fn::GetAtt": ["DomainList", "Id"] } },
                    "RuleGroupId": { "Value": { "Ref": "RuleGroup" } },
                    "RuleGroupIdAttribute": { "Value": { "Fn::GetAtt": ["RuleGroup", "Id"] } },
                    "AssociationId": { "Value": { "Ref": "Association" } },
                    "AssociationIdAttribute": { "Value": { "Fn::GetAtt": ["Association", "Id"] } }
                }
            })
            .to_string()
        },
        expected_list_name: "blocked-domains",
        expected_domains: &["blocked.example.test.", "*.ads.example.test."],
        expected_group_name: "blocking-group",
        expected_rules: &[(100, "BLOCK", Some("NXDOMAIN")), (200, "ALERT", None)],
        expected_vpc_id: Some("vpc-0a1b2c3d4e5f60718"),
        expected_priority: Some(101),
        expected_association_name: Some("filtering-for-the-app-vpc"),
        expected_mutation_protection: Some("DISABLED"),
        list_id_outputs: &["DomainListId", "DomainListIdAttribute"],
        group_id_outputs: &["RuleGroupId", "RuleGroupIdAttribute"],
        association_id_outputs: &["AssociationId", "AssociationIdAttribute"],
    },
    Row {
        case: "one domain, an overriding rule and quoted numbers, YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  DomainList:",
                "    Type: AWS::Route53Resolver::FirewallDomainList",
                "    Properties:",
                "      Name: one-domain",
                "      Domains:",
                "        - single.example.test.",
                "  RuleGroup:",
                "    Type: AWS::Route53Resolver::FirewallRuleGroup",
                "    Properties:",
                "      Name: overriding-group",
                "      FirewallRules:",
                "        - FirewallDomainListId: !Ref DomainList",
                "          Priority: \"300\"",
                "          Action: BLOCK",
                "          BlockResponse: OVERRIDE",
                "          BlockOverrideDomain: safe.example.test.",
                "          BlockOverrideDnsType: CNAME",
                "          BlockOverrideTtl: \"60\"",
                "  Association:",
                "    Type: AWS::Route53Resolver::FirewallRuleGroupAssociation",
                "    Properties:",
                "      Name: yaml-association",
                "      FirewallRuleGroupId: !Ref RuleGroup",
                "      VpcId: vpc-00112233445566778",
                "      Priority: \"9900\"",
                "Outputs:",
                "  DomainListId:",
                "    Value: !Ref DomainList",
                "  RuleGroupId:",
                "    Value: !Ref RuleGroup",
                "  AssociationId:",
                "    Value: !Ref Association",
                "",
            ]
            .join("\n")
        },
        expected_list_name: "one-domain",
        expected_domains: &["single.example.test."],
        expected_group_name: "overriding-group",
        expected_rules: &[(300, "BLOCK", Some("OVERRIDE"))],
        expected_vpc_id: Some("vpc-00112233445566778"),
        // The highest priority an association's doc publishes, written as a quoted scalar.
        expected_priority: Some(9900),
        expected_association_name: Some("yaml-association"),
        // The association declares none, so it reports the documented default.
        expected_mutation_protection: Some("DISABLED"),
        list_id_outputs: &["DomainListId"],
        group_id_outputs: &["RuleGroupId"],
        association_id_outputs: &["AssociationId"],
    },
    Row {
        case: "an empty domain list and a rule group with no rules, no association",
        region: "ap-southeast-2",
        template: || {
            json!({
                "Resources": {
                    "DomainList": {
                        "Type": "AWS::Route53Resolver::FirewallDomainList",
                        "Properties": { "Name": "empty-list" }
                    },
                    "RuleGroup": {
                        "Type": "AWS::Route53Resolver::FirewallRuleGroup",
                        "Properties": { "Name": "empty-group" }
                    }
                },
                "Outputs": {
                    "DomainListId": { "Value": { "Ref": "DomainList" } },
                    "RuleGroupId": { "Value": { "Fn::GetAtt": ["RuleGroup", "Id"] } }
                }
            })
            .to_string()
        },
        expected_list_name: "empty-list",
        expected_domains: &[],
        expected_group_name: "empty-group",
        expected_rules: &[],
        expected_vpc_id: None,
        expected_priority: None,
        expected_association_name: None,
        expected_mutation_protection: None,
        list_id_outputs: &["DomainListId"],
        group_id_outputs: &["RuleGroupId"],
        association_id_outputs: &[],
    },
];

/// Every row: the ids the stack reports for its DNS Firewall resources are the ones
/// Route 53 Resolver minted, each resource holds what the template declared, and deleting
/// the stack removes all of them there.
#[tokio::test]
async fn a_stack_provisions_the_dns_firewall_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("firewall-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (_, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        let physical_ids = physical_ids_of(&resources);
        let outputs = outputs_of(&described);

        let list_id = minted_id(
            case,
            &physical_ids,
            "DomainList",
            DOMAIN_LIST_ID_PREFIX,
            &resources,
        );
        let group_id = minted_id(
            case,
            &physical_ids,
            "RuleGroup",
            RULE_GROUP_ID_PREFIX,
            &resources,
        );
        for (member, keys, expected) in [
            ("the domain list", row.list_id_outputs, &list_id),
            ("the rule group", row.group_id_outputs, &group_id),
        ] {
            for key in keys {
                assert_eq!(
                    outputs.get(*key),
                    Some(expected),
                    "case {case:?}: expected output {key:?} to be {member}'s id {expected:?}, \
                     got {:?}",
                    outputs.get(*key)
                );
            }
        }

        // The domain list Route 53 Resolver holds under that id is the one the template
        // declared, with the domains its `Domains` property asked for.
        let (status, answer) = route53resolver(
            &router,
            region,
            "GetFirewallDomainList",
            &json!({ "FirewallDomainListId": list_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFirewallDomainList on the id the stack \
             reported, got {status} with body {answer}"
        );
        let domain_list = answer
            .get("FirewallDomainList")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            domain_list.get("Name").and_then(Value::as_str),
            Some(row.expected_list_name),
            "case {case:?}: expected the declared Name, got {domain_list}"
        );
        assert_eq!(
            domain_list.get("DomainCount").and_then(Value::as_i64),
            Some(row.expected_domains.len() as i64),
            "case {case:?}: expected the declared domains to be counted, got {domain_list}"
        );
        assert_eq!(
            domain_list.get("Status").and_then(Value::as_str),
            Some("COMPLETE"),
            "case {case:?}: expected the domain list to have settled, got {domain_list}"
        );
        let (status, listed) = route53resolver(
            &router,
            region,
            "ListFirewallDomains",
            &json!({ "FirewallDomainListId": list_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListFirewallDomains, got {status} with body \
             {listed}"
        );
        let domains: Vec<String> = listed
            .get("Domains")
            .and_then(Value::as_array)
            .map(|domains| {
                domains
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let expected_domains: Vec<String> = row
            .expected_domains
            .iter()
            .map(|domain| (*domain).to_string())
            .collect();
        assert_eq!(
            domains, expected_domains,
            "case {case:?}: expected the declared domains {expected_domains:?}, got {listed}"
        );

        // The rule group holds the rules the template declared, each matching against the
        // domain list the stack created — which is what proves the declaration's reference
        // reached Route 53 Resolver as the id it minted.
        let (status, answer) = route53resolver(
            &router,
            region,
            "GetFirewallRuleGroup",
            &json!({ "FirewallRuleGroupId": group_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFirewallRuleGroup on the id the stack \
             reported, got {status} with body {answer}"
        );
        let rule_group = answer
            .get("FirewallRuleGroup")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            rule_group.get("Name").and_then(Value::as_str),
            Some(row.expected_group_name),
            "case {case:?}: expected the declared Name, got {rule_group}"
        );
        assert_eq!(
            rule_group.get("RuleCount").and_then(Value::as_i64),
            Some(row.expected_rules.len() as i64),
            "case {case:?}: expected the declared rules to be counted, got {rule_group}"
        );

        let (status, listed) = route53resolver(
            &router,
            region,
            "ListFirewallRules",
            &json!({ "FirewallRuleGroupId": group_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListFirewallRules, got {status} with body \
             {listed}"
        );
        let rules = listed
            .get("FirewallRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            rules.len(),
            row.expected_rules.len(),
            "case {case:?}: expected the declared rules to be the ones held, got {listed}"
        );
        for (priority, action, block_response) in row.expected_rules {
            let found = rules
                .iter()
                .find(|rule| rule.get("Priority").and_then(Value::as_i64) == Some(*priority))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no rule at priority {priority} in {listed}")
                });
            assert_eq!(
                found.get("Action").and_then(Value::as_str),
                Some(*action),
                "case {case:?}: expected the declared Action at priority {priority}, got {found}"
            );
            assert_eq!(
                found.get("BlockResponse").and_then(Value::as_str),
                *block_response,
                "case {case:?}: expected the declared BlockResponse at priority {priority}, got \
                 {found}"
            );
            assert_eq!(
                found.get("FirewallDomainListId").and_then(Value::as_str),
                Some(list_id.as_str()),
                "case {case:?}: expected the rule to match the domain list the stack created \
                 ({list_id:?}), got {found}"
            );
        }

        // The association, where the row declares one: its id is Route 53 Resolver's, it
        // names the group the stack created and it converges to COMPLETE.
        let association_id = row.expected_vpc_id.map(|_| {
            minted_id(
                case,
                &physical_ids,
                "Association",
                ASSOCIATION_ID_PREFIX,
                &resources,
            )
        });
        for key in row.association_id_outputs {
            assert_eq!(
                outputs.get(*key),
                association_id.as_ref(),
                "case {case:?}: expected output {key:?} to be the association id \
                 {association_id:?}, got {:?}",
                outputs.get(*key)
            );
        }
        if let Some(association_id) = &association_id {
            let (status, answer) = route53resolver(
                &router,
                region,
                "GetFirewallRuleGroupAssociation",
                &json!({ "FirewallRuleGroupAssociationId": association_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetFirewallRuleGroupAssociation on the id \
                 the stack reported, got {status} with body {answer}"
            );
            let association = answer
                .get("FirewallRuleGroupAssociation")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                association
                    .get("FirewallRuleGroupId")
                    .and_then(Value::as_str),
                Some(group_id.as_str()),
                "case {case:?}: expected the association to name the group {group_id:?}, got \
                 {association}"
            );
            assert_eq!(
                association.get("VpcId").and_then(Value::as_str),
                row.expected_vpc_id,
                "case {case:?}: expected the declared VpcId, got {association}"
            );
            assert_eq!(
                association.get("Priority").and_then(Value::as_i64),
                row.expected_priority,
                "case {case:?}: expected the declared Priority, got {association}"
            );
            assert_eq!(
                association.get("Name").and_then(Value::as_str),
                row.expected_association_name,
                "case {case:?}: expected the declared Name, got {association}"
            );
            assert_eq!(
                association.get("MutationProtection").and_then(Value::as_str),
                row.expected_mutation_protection,
                "case {case:?}: expected the declared MutationProtection, got {association}"
            );
            assert_eq!(
                association.get("Status").and_then(Value::as_str),
                Some("COMPLETE"),
                "case {case:?}: expected the association to have reached COMPLETE, got \
                 {association}"
            );
        }

        // Deleting the stack removes every DNS Firewall resource it created — the
        // association first, since the group it joins depends on it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
        // The stack's own delete has to have gone through: a resource the owning service
        // refused to delete would leave the stack in DELETE_FAILED with that refusal, and
        // the reads below would then be asserting the refusal rather than the removal.
        let (_, deleted_stack) = cloudformation(
            &router,
            region,
            "DescribeStackEvents",
            &[("StackName", &stack_name)],
        )
        .await;
        assert!(
            !elements(&deleted_stack, "ResourceStatus")
                .iter()
                .any(|status| status == "DELETE_FAILED"),
            "case {case:?}: expected every resource of the stack to be deleted, got the events \
             {:?} with the reasons {:?}",
            elements(&deleted_stack, "ResourceStatus"),
            elements(&deleted_stack, "ResourceStatusReason")
        );
        for (operation, member, id) in [
            Some((
                "GetFirewallDomainList",
                "FirewallDomainListId",
                list_id.clone(),
            )),
            Some((
                "GetFirewallRuleGroup",
                "FirewallRuleGroupId",
                group_id.clone(),
            )),
            association_id.clone().map(|id| {
                (
                    "GetFirewallRuleGroupAssociation",
                    "FirewallRuleGroupAssociationId",
                    id,
                )
            }),
        ]
        .into_iter()
        .flatten()
        {
            let (status, gone) =
                route53resolver(&router, region, operation, &json!({ member: id })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from {operation} once the stack deleted the \
                 resource, got {status} with body {gone}"
            );
        }
    }
}

/// The physical resource id the stack reports for one declared resource, checked to be an
/// identifier Route 53 Resolver MINTED rather than one CloudFormation's naming rules
/// computed: the prefix that service uses for the resource, followed by lower-case
/// hexadecimal.
fn minted_id(
    case: &str,
    physical_ids: &BTreeMap<String, String>,
    logical_id: &str,
    prefix: &str,
    reported: &str,
) -> String {
    let id = physical_ids.get(logical_id).cloned().unwrap_or_else(|| {
        panic!("case {case:?}: no physical resource id for {logical_id:?} in {reported}")
    });
    let Some(suffix) = id.strip_prefix(prefix) else {
        panic!(
            "case {case:?}: expected {logical_id:?} to report an id prefixed {prefix:?}, got {id:?}"
        )
    };
    assert!(
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: expected {logical_id:?} to report {prefix:?} and lower-case hexadecimal, \
         got {id:?}"
    );
    id
}

/// A rule group whose declared rule Route 53 Resolver refuses is NOT reported as created:
/// the stack carries that service's own refusal, and the rollback leaves no rule group
/// behind.
///
/// The other half of the rule — nothing is passed off as created when the owning service
/// refused to create it — on the property that is no member of the create: a blocking rule
/// that states no `BlockResponse` is a rule DNS Firewall will not make.
#[tokio::test]
async fn a_rule_group_whose_rule_is_refused_fails_the_stack() {
    let router = build_router();
    let case = "a BLOCK rule with no BlockResponse";
    let region = "us-west-2";
    let template = json!({
        "Resources": {
            "DomainList": {
                "Type": "AWS::Route53Resolver::FirewallDomainList",
                "Properties": { "Name": "refused-list", "Domains": ["refused.example.test."] }
            },
            "RuleGroup": {
                "Type": "AWS::Route53Resolver::FirewallRuleGroup",
                "Properties": {
                    "Name": "refused-group",
                    "FirewallRules": [{
                        "FirewallDomainListId": { "Ref": "DomainList" },
                        "Priority": 100,
                        "Action": "BLOCK"
                    }]
                }
            }
        },
        "Outputs": { "RuleGroupId": { "Value": { "Ref": "RuleGroup" } } }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[
            ("StackName", "firewall-refused"),
            ("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    // The create's own outcome is what this case is about, so the stack is looked at until
    // it has one.
    let stack_status = settled_stack(&router, region, "firewall-refused", case).await;
    assert_eq!(
        stack_status, "ROLLBACK_COMPLETE",
        "case {case:?}: expected the failed create to be rolled back, got {stack_status:?}"
    );
    let (_, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", "firewall-refused")],
    )
    .await;
    assert!(
        elements(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported.contains("RSLVR-02016") && reported.contains("BLOCK RESPONSE")),
        "case {case:?}: expected the failing resource's event to carry Route 53 Resolver's own \
         refusal — its coded sentence, naming the block option the rule requires as that service \
         spells it — got {events}"
    );

    // The rollback removed the rule group the failed create had already made, and the
    // domain list the stack created before it, so nothing of the stack is left behind.
    for (operation, member) in [
        ("ListFirewallRuleGroups", "FirewallRuleGroups"),
        ("ListFirewallDomainLists", "FirewallDomainLists"),
    ] {
        let (status, listed) =
            route53resolver(&router, region, operation, &json!({ "MaxResults": 10 })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with body {listed}"
        );
        let held: Vec<&Value> = listed
            .get(member)
            .and_then(Value::as_array)
            .map(|held| {
                held.iter()
                    // The lists DNS Firewall publishes itself are not this stack's, so they
                    // are not what this assertion is about.
                    .filter(|entry| entry.get("ManagedOwnerName").is_none())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            held.is_empty(),
            "case {case:?}: expected the rolled-back stack to leave no {member} behind, got \
             {listed}"
        );
    }
}
