// SPDX-License-Identifier: CC0-1.0

//! The AWS Network Security Manager estate a stack declares, end to end: the four
//! `AWS::NetworkSecurityManager::*` types are types a template may DECLARE, and each declared
//! entity exists in that service itself, under the identity it minted; what that service — or
//! the type's own resource handler — refuses is a FAILED RESOURCE rather than a
//! template-format error; and deleting the stack takes the entity away.
//!
//! RULE: CloudFormation resolves every resource type whose owning service publishes it and
//! deploys each declared resource by CALLING that service's own control plane, reserving the
//! `Template format error: Unrecognized resource types: [<types>]` refusal for types no
//! service publishes. So the handle the stack publishes (`Ref`) resolves in the owning
//! service, the values the type publishes are the ones that service reported, and a refusal
//! of the declaration is quoted on the resource that failed.
//!
//! The rows of the deploy table differ along what the rule generalises over: WHICH of the four
//! entity types a template declares and how many of them, whether one entity references
//! another through `Fn::GetAtt` (a template grouping a rule, a policy enforcing that template,
//! and a policy enforcing a rule directly — the two forms an association may take), whether
//! the declaration carries a description and the type's own `Tags`, which published return
//! value an output reads (`Ref`, the ARN attribute, the id attribute), and the template
//! spelling (JSON with long-form intrinsics, YAML with the short-form tags).
//!
//! The tag set an entity ends up carrying is NOT asserted: that service publishes no read that
//! reports it — `GetRule`, `GetTemplate` and `GetPolicy` answer no `tags` member at all, and
//! its `ListTagsForResource` is still a scaffold stub — so a row that declares `Tags` pins
//! only that the set reaches the create, which is where its `tags` map travels.
//!
//! The refusal rows differ along the CLASS of association entry the policy's own handler
//! refuses: one naming BOTH ARNs and one naming NEITHER — each reported in the handler's own
//! words on the resource that failed, never as the template-format refusal, which no row of
//! any table here may ever see.
//!
//! Everything goes over the wire each service speaks through the one router `harness.rs`
//! builds — the CloudFormation calls over its awsQuery wire, the Network Security Manager
//! calls over that service's own `restJson1` wire — so what is asserted is what a client that
//! deploys the template and then calls that service observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_stack, failed_resource_reason, member, quoted_and_class, router,
    settled_stack_status, stack_event_reasons, REGION, SETTLE_DEADLINE, SETTLE_POLL_INTERVAL,
};
use crate::stack_lifecycle::{describe_stack, form_encode, output_value};

/// The four resource types under test.
const RULE: &str = "AWS::NetworkSecurityManager::Rule";
const TEMPLATE: &str = "AWS::NetworkSecurityManager::Template";
const POLICY: &str = "AWS::NetworkSecurityManager::Policy";
const SCOPE: &str = "AWS::NetworkSecurityManager::Scope";

/// The fixed text CloudFormation refuses a template declaring a type it does NOT know with,
/// which no case here may ever see.
const UNRECOGNIZED_TYPES: &str = "Unrecognized resource types";

/// The one firewall type the rule, template and policy shapes publish (`RuleFirewallType`,
/// `TemplateFirewallType` and `PolicyFirewallType` in that service's own model).
const WAF: &str = "WAF";

/// The two `RuleType` values that model publishes: "CONFIGURATION rules contain firewall
/// settings, and INSPECTION rules contain rule groups."
const CONFIGURATION: &str = "CONFIGURATION";
const INSPECTION: &str = "INSPECTION";

/// The `EntityStatus` a published entity carries: "When true, the resource is saved in
/// published (ACTIVE) state", and `isPublished` defaults to `true`.
const ACTIVE: &str = "ACTIVE";

/// The error code that service answers for an entity it does not hold, and the status that
/// error carries (the modelled 404 of each `Get…`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: u16 = 404;

/// The refusal the policy's own resource handler words for an association entry that does not
/// name exactly one of the two ARNs it may reference, and the `HandlerErrorCode` class it is
/// reported under.
const AMBIGUOUS_ASSOCIATION: &str = "Each AssociatedTemplateAndRuleList entry must specify \
                                     exactly one of TemplateArn or RuleArn.";
const INVALID_REQUEST: &str = "InvalidRequest";

/// The key a stack of this suite carries its own tag under, which CloudFormation propagates to
/// every resource it creates.
const STACK_TAG_KEY: &str = "sim2real";

/// One AWS WAF rule configuration, as a rule's `Configuration` document: "For an AWS WAF
/// CONFIGURATION rule, provide a single web ACL setting, such as `DefaultAction`".
fn waf_configuration() -> Value {
    json!({"DefaultAction": {"Allow": {}}})
}

/// A policy's `PolicyConfiguration`, whose `WafConfig` members are both required of a WAF
/// policy (`ExistingCustomerWebACLResolution` and `ConflictResolution`).
fn policy_configuration() -> Value {
    json!({
        "RemediationEnabled": false,
        "ResourcesCleanUp": true,
        "WafConfig": {
            "ExistingCustomerWebACLResolution": "RETROFIT",
            "ConflictResolution": "MERGE_WHERE_APPLICABLE",
        }
    })
}

/// One Network Security Manager call over the `restJson1` wire its model declares, on the
/// method and URI that operation's own `@http` binding names, signed for
/// `network-security-manager` — that model's own `sigv4Name` — in the Region the stack's
/// resources live in.
async fn nsm(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Option<String>, Value) {
    let payload = body.map(ToString::to_string).unwrap_or_default();
    let host = format!("network-security-manager.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/\
                 network-security-manager/aws4_request, SignedHeaders=host, \
                 Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed Network Security Manager request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, error_code, parsed)
}

/// That service's own read of ONE entity, by the ARN it minted — the `…Identifier` every call
/// on one takes, escaped as a single path segment the way an SDK sends a non-greedy label.
async fn get_entity(
    router: &MultiServiceRouter,
    collection: &str,
    arn: &str,
) -> (u16, Option<String>, Value) {
    nsm(
        router,
        "GET",
        &format!("{collection}/{}", form_encode(arn)),
        None,
    )
    .await
}

/// Every policy that service lists for the account.
async fn list_policies(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, _, answered) = nsm(router, "GET", "/policies", None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListPolicies, got {status} with {answered}"
    );
    answered
        .get("policies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Submit `template` as `stack_name`, carrying the stack's own tag, and answer the
/// `CreateStack` body: every type here is one a template may declare, so this never sees the
/// template-format refusal.
async fn submitted(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = create_stack(
        router,
        stack_name,
        template,
        &[
            member("Tags.member.1.Key", STACK_TAG_KEY),
            member("Tags.member.1.Value", stack_name),
        ],
    )
    .await;
    assert!(
        !body.contains(UNRECOGNIZED_TYPES),
        "case {case:?}: expected the Network Security Manager types to be recognised, got {body}"
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack — every type declared is one a \
         template may declare — got {status} with body {body}"
    );
    body
}

/// Deploy `template` as `stack_name` and answer the `DescribeStacks` body of the COMPLETED
/// stack, quoting the stack's own failure reasons when the create did not complete.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    submitted(router, case, stack_name, template).await;
    let reached = settled_stack_status(case, router, REGION, stack_name).await;
    assert_eq!(
        reached, "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {reached} with failure reasons {:?}",
        stack_event_reasons(router, REGION, stack_name).await
    );
    let (status, described) = describe_stack(router, stack_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {described}"
    );
    described
}

/// The value of one stack output, or a panic naming the case.
fn output(case: &str, described: &str, key: &str) -> String {
    output_value(described, key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {described}"))
}

/// Whether `arn` has the form that service's ARN members publish for `path`
/// (`^arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:(.+)$`, with the
/// resource path its own creates build): the namespace, the Region the stack's resources live
/// in and the `<path>/<id>` resource segment, whose id is inside the `^[a-z0-9]{1,64}$` that
/// service's id shapes declare.
fn is_an_entity_arn(arn: &str, path: &str) -> bool {
    let Some((prefix, id)) = arn.rsplit_once(&format!(":{path}/")) else {
        return false;
    };
    prefix.starts_with("arn:")
        && prefix.contains(&format!(":network-security-manager:{REGION}:"))
        && !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// What one row expects of one declared entity.
struct Declared {
    /// The collection that service addresses the entity under, and the resource path its ARN
    /// carries.
    collection: &'static str,
    path: &'static str,
    /// The output key whose value is what `Ref` returned — the entity's ARN.
    ref_output: &'static str,
    /// The output keys reading the two attributes the type publishes, where the row reads them.
    arn_output: Option<&'static str>,
    id_output: Option<&'static str>,
    /// The members that service must report the entity with, as `(member, value)`: the name
    /// the declaration asked for, the firewall type, the description where the row states one.
    reported: &'static [(&'static str, &'static str)],
}

/// One row: a template and the entities it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    declared: Vec<Declared>,
}

fn rows() -> Vec<Row> {
    vec![
        // One RULE alone, in JSON, with every return value the type publishes read back.
        Row {
            case: "one configuration rule, every return value read back",
            stack_name: "nsm-rule-round-trip",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Rule": {
                        "Type": RULE,
                        "Properties": {
                            "RuleName": "nsm-round-trip-rule",
                            "FirewallType": WAF,
                            "RuleType": CONFIGURATION,
                            "Configuration": waf_configuration(),
                            "Tags": [{"Key": "sim2real-nsm", "Value": "round-trip"}],
                        }
                    }
                },
                "Outputs": {
                    "RuleRef": {"Value": {"Ref": "Rule"}},
                    "RuleArn": {"Value": {"Fn::GetAtt": ["Rule", "RuleArn"]}},
                    "RuleId": {"Value": {"Fn::GetAtt": ["Rule", "RuleId"]}}
                }
            })
            .to_string(),
            declared: vec![Declared {
                collection: "/rules",
                path: "rule",
                ref_output: "RuleRef",
                arn_output: Some("RuleArn"),
                id_output: Some("RuleId"),
                reported: &[
                    ("ruleName", "nsm-round-trip-rule"),
                    ("firewallType", WAF),
                    ("ruleType", CONFIGURATION),
                    ("status", ACTIVE),
                ],
            }],
        },
        // An INSPECTION rule and the TEMPLATE that groups it, referencing the rule's ARN
        // through `Fn::GetAtt`, written in YAML with the short-form intrinsic tags and
        // carrying descriptions.
        Row {
            case: "a template grouping an inspection rule it references by GetAtt, YAML",
            stack_name: "nsm-template-groups-a-rule",
            template: format!(
                "\
Resources:
  Rule:
    Type: {RULE}
    Properties:
      RuleName: nsm-grouped-rule
      RuleDescription: the rule a template groups
      FirewallType: {WAF}
      RuleType: {INSPECTION}
      Configuration:
        Rules: []
  Template:
    Type: {TEMPLATE}
    Properties:
      TemplateName: nsm-grouping-template
      TemplateDescription: the template that groups it
      FirewallType: {WAF}
      AssociatedRuleList:
        - RuleArn: !GetAtt Rule.RuleArn
Outputs:
  RuleRef:
    Value: !Ref Rule
  TemplateRef:
    Value: !Ref Template
  TemplateArn:
    Value: !GetAtt Template.TemplateArn
"
            ),
            declared: vec![
                Declared {
                    collection: "/rules",
                    path: "rule",
                    ref_output: "RuleRef",
                    arn_output: None,
                    id_output: None,
                    reported: &[
                        ("ruleName", "nsm-grouped-rule"),
                        ("ruleDescription", "the rule a template groups"),
                        ("ruleType", INSPECTION),
                        ("status", ACTIVE),
                    ],
                },
                Declared {
                    collection: "/templates",
                    path: "template",
                    ref_output: "TemplateRef",
                    arn_output: Some("TemplateArn"),
                    id_output: None,
                    reported: &[
                        ("templateName", "nsm-grouping-template"),
                        ("templateDescription", "the template that groups it"),
                        ("firewallType", WAF),
                        ("status", ACTIVE),
                    ],
                },
            ],
        },
        // The whole chain: a rule, the template that groups it and the POLICY that enforces
        // that template, each referencing the one before it by `Fn::GetAtt`.
        Row {
            case: "a policy enforcing a template that groups a rule",
            stack_name: "nsm-policy-enforces-a-template",
            template: json!({
                "Resources": {
                    "Rule": {
                        "Type": RULE,
                        "Properties": {
                            "RuleName": "nsm-policy-chain-rule",
                            "FirewallType": WAF,
                            "RuleType": CONFIGURATION,
                            "Configuration": waf_configuration(),
                        }
                    },
                    "Template": {
                        "Type": TEMPLATE,
                        "Properties": {
                            "TemplateName": "nsm-policy-chain-template",
                            "FirewallType": WAF,
                            "AssociatedRuleList": [
                                {"RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]}}
                            ],
                        }
                    },
                    "Policy": {
                        "Type": POLICY,
                        "Properties": {
                            "PolicyName": "nsm-policy-chain-policy",
                            "FirewallType": WAF,
                            "Priority": 1000,
                            "AssociatedTemplateAndRuleList": [
                                {"TemplateArn": {"Fn::GetAtt": ["Template", "TemplateArn"]}}
                            ],
                            "PolicyConfiguration": policy_configuration(),
                        }
                    }
                },
                "Outputs": {
                    "RuleRef": {"Value": {"Ref": "Rule"}},
                    "TemplateRef": {"Value": {"Ref": "Template"}},
                    "PolicyRef": {"Value": {"Ref": "Policy"}},
                    "PolicyId": {"Value": {"Fn::GetAtt": ["Policy", "PolicyId"]}}
                }
            })
            .to_string(),
            declared: vec![
                Declared {
                    collection: "/rules",
                    path: "rule",
                    ref_output: "RuleRef",
                    arn_output: None,
                    id_output: None,
                    reported: &[("ruleName", "nsm-policy-chain-rule"), ("status", ACTIVE)],
                },
                Declared {
                    collection: "/templates",
                    path: "template",
                    ref_output: "TemplateRef",
                    arn_output: None,
                    id_output: None,
                    reported: &[
                        ("templateName", "nsm-policy-chain-template"),
                        ("status", ACTIVE),
                    ],
                },
                Declared {
                    collection: "/policies",
                    path: "policy",
                    ref_output: "PolicyRef",
                    arn_output: None,
                    id_output: Some("PolicyId"),
                    reported: &[
                        ("policyName", "nsm-policy-chain-policy"),
                        ("firewallType", WAF),
                        ("status", ACTIVE),
                    ],
                },
            ],
        },
        // A POLICY that enforces a RULE directly — the other form an association entry takes —
        // with a description and the type's own tags.
        Row {
            case: "a policy enforcing a rule directly, with a description and tags",
            stack_name: "nsm-policy-enforces-a-rule",
            template: json!({
                "Resources": {
                    "Rule": {
                        "Type": RULE,
                        "Properties": {
                            "RuleName": "nsm-direct-rule",
                            "FirewallType": WAF,
                            "RuleType": CONFIGURATION,
                            "Configuration": waf_configuration(),
                        }
                    },
                    "Policy": {
                        "Type": POLICY,
                        "Properties": {
                            "PolicyName": "nsm-direct-policy",
                            "PolicyDescription": "the policy that enforces one rule",
                            "FirewallType": WAF,
                            "Priority": 1,
                            "AssociatedTemplateAndRuleList": [
                                {"RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]}}
                            ],
                            "PolicyConfiguration": policy_configuration(),
                            "Tags": [{"Key": "sim2real-nsm", "Value": "direct"}],
                        }
                    }
                },
                "Outputs": {
                    "RuleRef": {"Value": {"Ref": "Rule"}},
                    "PolicyRef": {"Value": {"Ref": "Policy"}},
                    "PolicyArn": {"Value": {"Fn::GetAtt": ["Policy", "PolicyArn"]}}
                }
            })
            .to_string(),
            declared: vec![
                Declared {
                    collection: "/rules",
                    path: "rule",
                    ref_output: "RuleRef",
                    arn_output: None,
                    id_output: None,
                    reported: &[("ruleName", "nsm-direct-rule"), ("status", ACTIVE)],
                },
                Declared {
                    collection: "/policies",
                    path: "policy",
                    ref_output: "PolicyRef",
                    arn_output: Some("PolicyArn"),
                    id_output: None,
                    reported: &[
                        ("policyName", "nsm-direct-policy"),
                        ("policyDescription", "the policy that enforces one rule"),
                        ("status", ACTIVE),
                    ],
                },
            ],
        },
    ]
}

/// Every declared entity exists in the service that OWNS it, under the identity that service
/// minted, carrying what the declaration asked for and the tags the stack gives it.
#[tokio::test]
async fn declared_entities_are_created_in_the_service_that_owns_them() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let described = deployed(&router, case, row.stack_name, &row.template).await;
        for declared in &row.declared {
            let arn = output(case, &described, declared.ref_output);
            assert!(
                is_an_entity_arn(&arn, declared.path),
                "case {case:?}: expected {} to be an ARN that service minted for a {}, got \
                 {arn:?}",
                declared.ref_output,
                declared.path
            );
            if let Some(key) = declared.arn_output {
                let published = output(case, &described, key);
                assert_eq!(
                    published, arn,
                    "case {case:?}: expected the {key} attribute to be the ARN Ref returns, got \
                     {published:?}"
                );
            }

            let (status, _, entity) = get_entity(&router, declared.collection, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected that service to report the entity the stack created \
                 under {arn:?}, got {status} with {entity}"
            );
            for (member, expected) in declared.reported {
                assert_eq!(
                    entity.get(*member).and_then(Value::as_str),
                    Some(*expected),
                    "case {case:?}: expected {member} {expected:?} on the entity at {arn:?}, got \
                     {entity}"
                );
            }
            if let Some(key) = declared.id_output {
                let published = output(case, &described, key);
                assert!(
                    arn.ends_with(&format!("/{published}")),
                    "case {case:?}: expected the {key} attribute to be the id inside {arn:?}, \
                     got {published:?}"
                );
            }
        }    }
}

/// The ASSOCIATIONS an entity was created with are the ARNs the template referenced: a
/// template reports the rule it groups, and a policy the template or the rule it enforces,
/// each under the member that service reports an association in.
#[tokio::test]
async fn an_entitys_associations_are_the_arns_the_template_referenced() {
    let router = router();
    let case = "a policy enforcing a template that groups a rule";
    let stack_name = "nsm-associations";
    let template = json!({
        "Resources": {
            "Rule": {
                "Type": RULE,
                "Properties": {
                    "RuleName": "nsm-associated-rule",
                    "FirewallType": WAF,
                    "RuleType": CONFIGURATION,
                    "Configuration": waf_configuration(),
                }
            },
            "Template": {
                "Type": TEMPLATE,
                "Properties": {
                    "TemplateName": "nsm-associated-template",
                    "FirewallType": WAF,
                    "AssociatedRuleList": [{"RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]}}],
                }
            },
            "Policy": {
                "Type": POLICY,
                "Properties": {
                    "PolicyName": "nsm-associated-policy",
                    "FirewallType": WAF,
                    "Priority": 500,
                    "AssociatedTemplateAndRuleList": [
                        {"TemplateArn": {"Fn::GetAtt": ["Template", "TemplateArn"]}}
                    ],
                    "PolicyConfiguration": policy_configuration(),
                }
            }
        },
        "Outputs": {
            "RuleRef": {"Value": {"Ref": "Rule"}},
            "TemplateRef": {"Value": {"Ref": "Template"}},
            "PolicyRef": {"Value": {"Ref": "Policy"}}
        }
    })
    .to_string();

    let described = deployed(&router, case, stack_name, &template).await;
    let rule_arn = output(case, &described, "RuleRef");
    let template_arn = output(case, &described, "TemplateRef");
    let policy_arn = output(case, &described, "PolicyRef");

    let (_, _, grouped) = get_entity(&router, "/templates", &template_arn).await;
    assert_eq!(
        grouped
            .pointer("/associatedRuleList/0/ruleArn")
            .and_then(Value::as_str),
        Some(rule_arn.as_str()),
        "case {case:?}: expected the template to group the rule the declaration referenced, got \
         {grouped}"
    );
    let (_, _, enforced) = get_entity(&router, "/policies", &policy_arn).await;
    assert_eq!(
        enforced
            .pointer("/associatedTemplateAndRuleList/0/templateArn")
            .and_then(Value::as_str),
        Some(template_arn.as_str()),
        "case {case:?}: expected the policy to enforce the template the declaration referenced, \
         got {enforced}"
    );
    // The policy's own configuration reaches that service under its own member names, which
    // the type's property names are the upper-initialled spelling of.
    assert_eq!(
        enforced
            .pointer("/policyConfiguration/wafConfig/existingCustomerWebACLResolution")
            .and_then(Value::as_str),
        Some("RETROFIT"),
        "case {case:?}: expected the declared policy configuration to reach that service, got \
         {enforced}"
    );
}

/// A template declaring ALL FOUR types — the whole estate, the scope included — is ADMITTED: a
/// `StackId` is minted and the template-format refusal is never the answer for a type whose
/// owning service publishes it.
#[tokio::test]
async fn a_template_declaring_every_entity_type_is_admitted() {
    let router = router();
    let case = "the whole estate, the scope included";
    let stack_name = "nsm-every-type";
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Rule": {
                "Type": RULE,
                "Properties": {
                    "RuleName": "nsm-estate-rule",
                    "FirewallType": WAF,
                    "RuleType": CONFIGURATION,
                    "Configuration": waf_configuration(),
                }
            },
            "Template": {
                "Type": TEMPLATE,
                "Properties": {
                    "TemplateName": "nsm-estate-template",
                    "FirewallType": WAF,
                    "AssociatedRuleList": [{"RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]}}],
                }
            },
            "Policy": {
                "Type": POLICY,
                "Properties": {
                    "PolicyName": "nsm-estate-policy",
                    "FirewallType": WAF,
                    "Priority": 1000,
                    "AssociatedTemplateAndRuleList": [
                        {"TemplateArn": {"Fn::GetAtt": ["Template", "TemplateArn"]}}
                    ],
                    "PolicyConfiguration": policy_configuration(),
                }
            },
            "Scope": {
                "Type": SCOPE,
                "Properties": {
                    "ScopeName": "nsm-estate-scope",
                    "ScopeConfiguration": {
                        "ResourceScopes": {
                            "AWS::EC2::EIP": {"IncludeAll": true}
                        }
                    },
                }
            }
        }
    })
    .to_string();

    let body = submitted(&router, case, stack_name, &template).await;
    assert!(
        body.contains("<StackId>"),
        "case {case:?}: expected CreateStack to mint a StackId, got {body}"
    );
}

/// Deleting the stack deletes the entity it created, so the owning service's own read of it
/// answers in that service's words that it no longer holds one.
#[tokio::test]
async fn deleting_the_stack_deletes_the_policy_it_created() {
    let router = router();
    let case = "a policy deleted with its stack";
    let stack_name = "nsm-policy-deleted";
    let template = json!({
        "Resources": {
            // The rule the policy associates: the model says of
            // `associatedTemplateAndRuleList` "For AWS WAF policies, specify 1 to 100 templates
            // or rules", so a WAF policy declares at least one.
            "Rule": {
                "Type": RULE,
                "Properties": {
                    "RuleName": "nsm-deleted-policy-rule",
                    "FirewallType": WAF,
                    "RuleType": CONFIGURATION,
                    "Configuration": waf_configuration(),
                }
            },
            "Policy": {
                "Type": POLICY,
                "Properties": {
                    "PolicyName": "nsm-deleted-policy",
                    "FirewallType": WAF,
                    "Priority": 10,
                    "AssociatedTemplateAndRuleList": [
                        {"RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]}}
                    ],
                    "PolicyConfiguration": policy_configuration(),
                }
            }
        },
        "Outputs": {"PolicyRef": {"Value": {"Ref": "Policy"}}}
    })
    .to_string();

    let described = deployed(&router, case, stack_name, &template).await;
    let policy_arn = output(case, &described, "PolicyRef");
    let (status, _, policy) = get_entity(&router, "/policies", &policy_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the policy to exist before the delete, got {status} with \
         {policy}"
    );

    let (status, deleted) = call(&router, "DeleteStack", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    settle_the_deletion(&router, case, "/policies", &policy_arn).await;

    let (status, error_code, answered) = get_entity(&router, "/policies", &policy_arn).await;
    assert_eq!(
        (status, error_code.as_deref()),
        (NOT_FOUND_STATUS, Some(RESOURCE_NOT_FOUND)),
        "case {case:?}: expected the policy to be gone with the stack, got {status} with \
         {answered}"
    );
    assert!(
        list_policies(&router).await.is_empty(),
        "case {case:?}: expected that service to list no policy once the stack is gone"
    );
}

/// Wait until the entity at `arn` is gone from the owning service.
///
/// Waits for the CONDITION rather than for a span, under the harness's own generous deadline,
/// so a loaded host cannot decide the outcome.
async fn settle_the_deletion(
    router: &MultiServiceRouter,
    case: &str,
    collection: &str,
    arn: &str,
) {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, _, answered) = get_entity(router, collection, arn).await;
        if status != 200 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the entity at {arn:?} was still held after the deletion deadline: \
             {answered}"
        );
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
}

/// A changed declaration reaches the entity the stack already has, through that entity's own
/// `Update…`, rather than being dropped or replaced: the entity keeps the identity that service
/// minted and reports what the new declaration states.
#[tokio::test]
async fn a_changed_declaration_is_applied_to_the_policy_the_stack_has() {
    let router = router();
    let case = "an update that re-describes the policy and raises its priority";
    let stack_name = "nsm-policy-updated";
    let template_of = |description: &str, priority: i64| {
        json!({
            "Resources": {
                // The rule the policy associates, which both declarations state alike: a WAF
                // policy specifies "1 to 100 templates or rules", so neither the create nor
                // the update may leave the list empty.
                "Rule": {
                    "Type": RULE,
                    "Properties": {
                        "RuleName": "nsm-updated-policy-rule",
                        "FirewallType": WAF,
                        "RuleType": CONFIGURATION,
                        "Configuration": waf_configuration(),
                    }
                },
                "Policy": {
                    "Type": POLICY,
                    "Properties": {
                        "PolicyName": "nsm-updated-policy",
                        "PolicyDescription": description,
                        "FirewallType": WAF,
                        "Priority": priority,
                        "AssociatedTemplateAndRuleList": [
                            {"RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]}}
                        ],
                        "PolicyConfiguration": policy_configuration(),
                    }
                }
            },
            "Outputs": {"PolicyRef": {"Value": {"Ref": "Policy"}}}
        })
        .to_string()
    };

    let described = deployed(&router, case, stack_name, &template_of("before", 1)).await;
    let published = output(case, &described, "PolicyRef");

    let (status, updated) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", &template_of("after", 2000)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {updated}"
    );
    let reached = settled_stack_status(case, &router, REGION, stack_name).await;
    assert_eq!(
        reached, "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {reached} with failure reasons {:?}",
        stack_event_reasons(&router, REGION, stack_name).await
    );
    let (_, described) = describe_stack(&router, stack_name).await;
    assert_eq!(
        output_value(&described, "PolicyRef"),
        Some(published.clone()),
        "case {case:?}: expected the policy to keep the identity that service minted, got \
         {described}"
    );

    // The change reached the policy THAT service holds, not just the stack's record — and it is
    // the one policy rather than a second one.
    let (status, policy) = {
        let (status, _, policy) = get_entity(&router, "/policies", &published).await;
        (status, policy)
    };
    assert_eq!(
        (
            status,
            policy.get("policyDescription").and_then(Value::as_str),
            policy.get("priority").and_then(Value::as_i64),
        ),
        (200, Some("after"), Some(2000)),
        "case {case:?}: expected the re-declared description and priority, got {status} with \
         {policy}"
    );
    assert_eq!(
        list_policies(&router).await.len(),
        1,
        "case {case:?}: expected the one policy to be changed rather than a second one created"
    );
}

/// An association entry that does not name exactly ONE of the two ARNs it may reference is the
/// policy handler's own refusal: the resource FAILS with that sentence quoted on its event, the
/// stack rolls back, and nothing is left in the owning service.
#[tokio::test]
async fn an_ambiguous_policy_association_fails_the_resource_with_the_handlers_own_refusal() {
    // The two classes of entry no `TemplateOrRuleReference` can be written for: one naming
    // BOTH of the ARNs it may reference, and one naming NEITHER.
    let rows: &[(&str, &str, Value)] = &[
        (
            "an entry naming both a template and a rule",
            "nsm-policy-ambiguous-both",
            json!([{
                "TemplateArn": {"Fn::GetAtt": ["Template", "TemplateArn"]},
                "RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]},
            }]),
        ),
        (
            "an entry naming neither",
            "nsm-policy-ambiguous-neither",
            json!([{}]),
        ),
    ];

    for (case, stack_name, associations) in rows {
        let router = router();
        let template = json!({
            "Resources": {
                "Rule": {
                    "Type": RULE,
                    "Properties": {
                        "RuleName": "nsm-ambiguous-rule",
                        "FirewallType": WAF,
                        "RuleType": CONFIGURATION,
                        "Configuration": waf_configuration(),
                    }
                },
                "Template": {
                    "Type": TEMPLATE,
                    "Properties": {
                        "TemplateName": "nsm-ambiguous-template",
                        "FirewallType": WAF,
                        "AssociatedRuleList": [{"RuleArn": {"Fn::GetAtt": ["Rule", "RuleArn"]}}],
                    }
                },
                "Policy": {
                    "Type": POLICY,
                    "Properties": {
                        "PolicyName": "nsm-ambiguous-policy",
                        "FirewallType": WAF,
                        "Priority": 1000,
                        "AssociatedTemplateAndRuleList": associations,
                        "PolicyConfiguration": policy_configuration(),
                    }
                }
            }
        })
        .to_string();

        submitted(&router, case, stack_name, &template).await;
        let reason = failed_resource_reason(
            case,
            &router,
            REGION,
            stack_name,
            "Policy",
            "CREATE_FAILED",
        )
        .await;
        let (quoted, class) = quoted_and_class(case, &reason);
        assert_eq!(
            (quoted.as_str(), class.as_str()),
            (AMBIGUOUS_ASSOCIATION, INVALID_REQUEST),
            "case {case:?}: expected the policy handler's own refusal on the failed resource, \
             got {reason:?}"
        );
        assert!(
            !reason.contains(UNRECOGNIZED_TYPES),
            "case {case:?}: a declaration this emulator deploys must never be refused as an \
             unrecognised type, got {reason:?}"
        );
        // Nothing the rolled-back stack created is left behind in the owning service.
        assert!(
            list_policies(&router).await.is_empty(),
            "case {case:?}: expected no policy to be left once the resource failed"
        );
    }
}
