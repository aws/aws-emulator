// SPDX-License-Identifier: CC0-1.0

//! The AWS AppConfig estate a stack declares, end to end: every resource exists in AppConfig
//! itself, under the identifier the stack publishes.
//!
//! RULE: for a resource type whose owning service MINTS the resource's identifier, the stack
//! creates the resource through that service's own create and publishes the identifier THAT
//! service answered as the resource's physical id — so the value a client reads out of `Ref`,
//! out of the type's own identifier attribute, out of `DescribeStackResources` or out of a
//! stack output is one that service accepts, and every follow-on call made with it resolves.
//! The four `AWS::AppConfig::*` types a configuration estate declares are such resources:
//! `CreateApplication`, `CreateEnvironment`, `CreateConfigurationProfile` and
//! `CreateDeploymentStrategy` each take the resource's NAME and no identifier, and answer the
//! `Id` AppConfig minted — modelled `Id` with `@pattern ^[a-z0-9]{4,7}$`
//! (`smithy_query(query="operation",operation="CreateApplication")` on that service's model),
//! which no name CloudFormation decides could satisfy.
//!
//! The rows differ along what the rule generalises over: WHICH of the four types the resource
//! is (each one's id is read back through AppConfig's own wire), whether the template NAMES
//! the resource or leaves CloudFormation to name it, whether a child names its application
//! through `Ref` or through `Fn::GetAtt ApplicationId`, and both spellings a template may give
//! a numeric property (a JSON number and the text a parameter resolves to). The refusing side
//! is a declaration that states none of a property its create requires: nothing is created for
//! it and the stack does not report success.
//!
//! Everything goes over the wire each service speaks through the one router `harness.rs`
//! builds — the CloudFormation calls over its awsQuery wire, the AppConfig reads over that
//! service's own `restJson1` wire — so what is asserted is what a client that deploys the
//! template and then calls AppConfig observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, deploy, describe_stack, member, router, wait_for_the_transition, xml_text,
    REGION,
};
use crate::stack_resources::{resources_of, wire_resources};

/// How many characters a minted AppConfig id carries, and which they may be: the modelled `Id`
/// shape every one of these creates answers (`@pattern ^[a-z0-9]{4,7}$`).
const ID_MIN_CHARACTERS: usize = 4;
const ID_MAX_CHARACTERS: usize = 7;

/// Whether `value` is an identifier of the form AppConfig mints, which is the whole of that
/// modelled pattern.
fn is_appconfig_id(value: &str) -> bool {
    (ID_MIN_CHARACTERS..=ID_MAX_CHARACTERS).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// One AWS AppConfig call over the `restJson1` wire its model declares, on the method and URI
/// that operation's own `@http` binding names, signed for `appconfig` — that model's own
/// `sigv4Name` — in the Region the stack's resources live in.
pub(crate) async fn appconfig(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(ToString::to_string).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appconfig.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("appconfig.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/appconfig/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed AWS AppConfig request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The physical resource ids `DescribeStackResources` reports, keyed by logical id.
async fn physical_ids(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String)> {
    let (status, body) = resources_of(router, stack).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResources({stack}) answered {status} with {body}"
    );
    wire_resources(&body)
        .into_iter()
        .filter_map(|resource| Some((resource.logical_id?, resource.physical_id?)))
        .collect()
}

/// The physical resource id of one logical id, or a panic naming the case: a resource the
/// stack reports without one is exactly the defect this suite is about.
fn id_of(case: &str, reported: &[(String, String)], logical_id: &str) -> String {
    reported
        .iter()
        .find(|(logical, _)| logical == logical_id)
        .map(|(_, physical)| physical.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a physical resource id for {logical_id:?}, got {reported:?}")
        })
}

/// The stack's outputs, keyed by `OutputKey`.
async fn outputs(router: &MultiServiceRouter, case: &str, stack: &str) -> Vec<(String, String)> {
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks({stack}) answered {status} with {body}"
    );
    let keys = crate::stack_lifecycle::xml_texts(&body, "OutputKey");
    let values = crate::stack_lifecycle::xml_texts(&body, "OutputValue");
    assert_eq!(
        keys.len(),
        values.len(),
        "case {case:?}: expected one OutputValue per OutputKey {keys:?}, got {values:?} in \
         {body}"
    );
    keys.into_iter().zip(values).collect()
}

/// One output's value, or a panic naming the case.
fn output(case: &str, reported: &[(String, String)], key: &str) -> String {
    reported
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| panic!("case {case:?}: expected the output {key:?}, got {reported:?}"))
}

/// One row of the table: a whole estate template, and how it names what it names.
struct Estate {
    case: &'static str,
    stack: &'static str,
    /// The template body, which declares all four types plus the outputs a client reads the
    /// ids out of.
    template: String,
}

/// The estates the rule is asserted over. Each declares all four types; the rows differ in how
/// the resources are NAMED (the template's own names, or CloudFormation's generated ones), how
/// a child names its application (`Ref` against `Fn::GetAtt ApplicationId`) and how the numeric
/// properties of a deployment strategy are spelled (JSON numbers against the text a parameter
/// resolves to).
fn estates() -> Vec<Estate> {
    vec![
        Estate {
            case: "names the template states, children naming the application by Ref, numbers \
                   as JSON numbers",
            stack: "appconfig-estate-named",
            template: json!({
                "Resources": {
                    "Application": {
                        "Type": "AWS::AppConfig::Application",
                        "Properties": {
                            "Name": "an-estate-application",
                            "Description": "the estate's application",
                            "Tags": [{"Key": "Stage", "Value": "beta"}]
                        }
                    },
                    "Environment": {
                        "Type": "AWS::AppConfig::Environment",
                        "Properties": {
                            "ApplicationId": {"Ref": "Application"},
                            "Name": "an-estate-environment"
                        }
                    },
                    "Profile": {
                        "Type": "AWS::AppConfig::ConfigurationProfile",
                        "Properties": {
                            "ApplicationId": {"Ref": "Application"},
                            "Name": "an-estate-profile",
                            "LocationUri": "hosted",
                            "Type": "AWS.Freeform"
                        }
                    },
                    "Strategy": {
                        "Type": "AWS::AppConfig::DeploymentStrategy",
                        "Properties": {
                            "Name": "an-estate-strategy",
                            "DeploymentDurationInMinutes": 0,
                            "FinalBakeTimeInMinutes": 0,
                            "GrowthFactor": 100,
                            "GrowthType": "LINEAR",
                            "ReplicateTo": "NONE"
                        }
                    }
                },
                "Outputs": {
                    "ApplicationId": {"Value": {"Ref": "Application"}},
                    "EnvironmentId": {"Value": {"Fn::GetAtt": ["Environment", "EnvironmentId"]}},
                    "ConfigurationProfileId": {
                        "Value": {"Fn::GetAtt": ["Profile", "ConfigurationProfileId"]}
                    },
                    "DeploymentStrategyId": {"Value": {"Ref": "Strategy"}}
                }
            })
            .to_string(),
        },
        Estate {
            case: "names CloudFormation generates, children naming the application by \
                   Fn::GetAtt ApplicationId, numbers as the text a parameter resolves to",
            stack: "appconfig-estate-generated",
            template: json!({
                "Parameters": {
                    "Duration": {"Type": "String", "Default": "20"},
                    "Growth": {"Type": "String", "Default": "25.5"}
                },
                "Resources": {
                    "Application": { "Type": "AWS::AppConfig::Application" },
                    "Environment": {
                        "Type": "AWS::AppConfig::Environment",
                        "Properties": {
                            "ApplicationId": {"Fn::GetAtt": ["Application", "ApplicationId"]}
                        }
                    },
                    "Profile": {
                        "Type": "AWS::AppConfig::ConfigurationProfile",
                        "Properties": {
                            "ApplicationId": {"Fn::GetAtt": ["Application", "ApplicationId"]},
                            "LocationUri": "hosted"
                        }
                    },
                    "Strategy": {
                        "Type": "AWS::AppConfig::DeploymentStrategy",
                        "Properties": {
                            "DeploymentDurationInMinutes": {"Ref": "Duration"},
                            "GrowthFactor": {"Ref": "Growth"},
                            "GrowthType": "EXPONENTIAL"
                        }
                    }
                },
                "Outputs": {
                    "ApplicationId": {"Value": {"Ref": "Application"}},
                    "EnvironmentId": {"Value": {"Fn::GetAtt": ["Environment", "EnvironmentId"]}},
                    "ConfigurationProfileId": {
                        "Value": {"Fn::GetAtt": ["Profile", "ConfigurationProfileId"]}
                    },
                    "DeploymentStrategyId": {"Value": {"Ref": "Strategy"}}
                }
            })
            .to_string(),
        },
    ]
}

/// Every resource of the estate exists in AppConfig under the id the stack publishes, that id
/// is of the form the owning service mints, and the stack's outputs and per-resource reports
/// carry the same value.
#[tokio::test]
async fn an_appconfig_estate_exists_under_the_ids_the_stack_publishes() {
    for estate in estates() {
        let router = router();
        let Estate {
            case,
            stack,
            template,
        } = &estate;
        deploy(&router, stack, template).await;

        let reported = physical_ids(&router, case, stack).await;
        let published = outputs(&router, case, stack).await;

        // The four ids, read from the stack's own outputs, which is what a client passes to
        // AppConfig.
        let application_id = output(case, &published, "ApplicationId");
        let environment_id = output(case, &published, "EnvironmentId");
        let profile_id = output(case, &published, "ConfigurationProfileId");
        let strategy_id = output(case, &published, "DeploymentStrategyId");

        for (label, id) in [
            ("ApplicationId", &application_id),
            ("EnvironmentId", &environment_id),
            ("ConfigurationProfileId", &profile_id),
            ("DeploymentStrategyId", &strategy_id),
        ] {
            assert!(
                is_appconfig_id(id),
                "case {case:?}: expected the output {label:?} to be an id of the form AppConfig \
                 mints (4-7 lowercase alphanumerics), got {id:?}"
            );
        }

        // `Ref` and `Fn::GetAtt <Type>Id` report the SAME value the per-resource read does: one
        // identity per resource, not two.
        for (logical_id, label, id) in [
            ("Application", "ApplicationId", &application_id),
            ("Environment", "EnvironmentId", &environment_id),
            ("Profile", "ConfigurationProfileId", &profile_id),
            ("Strategy", "DeploymentStrategyId", &strategy_id),
        ] {
            let physical = id_of(case, &reported, logical_id);
            assert_eq!(
                &physical, id,
                "case {case:?}: expected the output {label:?} and {logical_id:?}'s physical \
                 resource id to be the one identity, got {id:?} and {physical:?}"
            );
        }

        // Each resource is really there, read back through AppConfig's own wire by the id the
        // stack published.
        let (status, document) =
            appconfig(&router, "GET", &format!("/applications/{application_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetApplication({application_id}) to answer 200, got \
             {status} with {document}"
        );
        assert_eq!(
            document.get("Id").and_then(Value::as_str),
            Some(application_id.as_str()),
            "case {case:?}: expected GetApplication to report the published id, got {document}"
        );

        let (status, document) = appconfig(
            &router,
            "GET",
            &format!("/applications/{application_id}/environments/{environment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetEnvironment({application_id}, {environment_id}) to \
             answer 200, got {status} with {document}"
        );
        assert_eq!(
            document.get("ApplicationId").and_then(Value::as_str),
            Some(application_id.as_str()),
            "case {case:?}: expected the environment to belong to the published application, \
             got {document}"
        );

        let (status, document) = appconfig(
            &router,
            "GET",
            &format!("/applications/{application_id}/configurationprofiles/{profile_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetConfigurationProfile({application_id}, {profile_id}) \
             to answer 200, got {status} with {document}"
        );
        assert_eq!(
            document.get("LocationUri").and_then(Value::as_str),
            Some("hosted"),
            "case {case:?}: expected the profile to carry the declared LocationUri, got \
             {document}"
        );

        let (status, document) = appconfig(
            &router,
            "GET",
            &format!("/deploymentstrategies/{strategy_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDeploymentStrategy({strategy_id}) to answer 200, got \
             {status} with {document}"
        );
        assert_eq!(
            document.get("Id").and_then(Value::as_str),
            Some(strategy_id.as_str()),
            "case {case:?}: expected GetDeploymentStrategy to report the published id, got \
             {document}"
        );

        // The follow-on control-plane call a client makes with the ids the stack published:
        // a hosted configuration version of the profile the stack created, which the reported
        // defect answered `ResourceNotFoundException` for because nothing existed under them.
        let (status, document) = appconfig(
            &router,
            "POST",
            &format!(
                "/applications/{application_id}/configurationprofiles/{profile_id}/\
                 hostedconfigurationversions"
            ),
            Some(&json!({
                "Content": "eyJ1c2VOZXdCYWNrZW5kIjogdHJ1ZX0=",
                "ContentType": "application/json"
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateHostedConfigurationVersion on the published ids to \
             answer 201, got {status} with {document}"
        );
    }
}

/// The numeric properties a deployment strategy declares reach AppConfig as the numbers they
/// spell, in whichever of the two spellings a template gives one: a JSON number, or the text a
/// parameter reference resolves to.
#[tokio::test]
async fn a_strategy_carries_the_numbers_its_declaration_states() {
    let router = router();
    // Row 1 states them as JSON numbers, row 2 as the text of the stack's own parameters.
    let expected: &[(&str, &str, f64, f64)] = &[
        (
            "names the template states, children naming the application by Ref, numbers as \
             JSON numbers",
            "appconfig-estate-named",
            0.0,
            100.0,
        ),
        (
            "names CloudFormation generates, children naming the application by Fn::GetAtt \
             ApplicationId, numbers as the text a parameter resolves to",
            "appconfig-estate-generated",
            20.0,
            25.5,
        ),
    ];
    for (estate, (case, stack, duration, growth)) in estates().into_iter().zip(expected) {
        deploy(&router, stack, &estate.template).await;
        let published = outputs(&router, case, stack).await;
        let strategy_id = output(case, &published, "DeploymentStrategyId");
        let (status, document) = appconfig(
            &router,
            "GET",
            &format!("/deploymentstrategies/{strategy_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDeploymentStrategy({strategy_id}) to answer 200, got \
             {status} with {document}"
        );
        let actual_duration = document
            .get("DeploymentDurationInMinutes")
            .and_then(Value::as_f64);
        assert_eq!(
            actual_duration,
            Some(*duration),
            "case {case:?}: expected the strategy's DeploymentDurationInMinutes to be \
             {duration}, got {actual_duration:?} in {document}"
        );
        let actual_growth = document.get("GrowthFactor").and_then(Value::as_f64);
        assert_eq!(
            actual_growth,
            Some(*growth),
            "case {case:?}: expected the strategy's GrowthFactor to be {growth}, got \
             {actual_growth:?} in {document}"
        );
    }
}

/// Deleting the stack takes the estate away in AppConfig: the ids the stack published no
/// longer resolve, so the stack's deletion really removed what its creation really made.
#[tokio::test]
async fn deleting_the_stack_removes_the_estate() {
    let router = router();
    let estate = estates().remove(0);
    let case = "the estate's own deletion";
    let stack = "appconfig-estate-deleted";
    deploy(&router, stack, &estate.template).await;
    let published = outputs(&router, case, stack).await;
    let application_id = output(case, &published, "ApplicationId");
    let strategy_id = output(case, &published, "DeploymentStrategyId");

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack answered {status} with {body}"
    );
    wait_for_the_transition().await;

    for (label, uri) in [
        ("the application", format!("/applications/{application_id}")),
        (
            "the deployment strategy",
            format!("/deploymentstrategies/{strategy_id}"),
        ),
    ] {
        let (status, document) = appconfig(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {label} to be gone from AppConfig, got {status} with \
             {document}"
        );
    }
}

/// One declaration that states none of a property its create requires, and the resource it
/// declares. A create AppConfig could not make is reported as the resource's failure, so the
/// stack does not reach `CREATE_COMPLETE` with nothing behind it.
struct Incomplete {
    case: &'static str,
    stack: &'static str,
    resources: Value,
}

/// The refusing rows: one per required property of the four creates that a declaration can
/// leave out.
fn incomplete_estates() -> Vec<Incomplete> {
    vec![
        Incomplete {
            case: "an environment naming no application",
            stack: "appconfig-environment-without-application",
            resources: json!({
                "Environment": {
                    "Type": "AWS::AppConfig::Environment",
                    "Properties": { "Name": "an-orphan" }
                }
            }),
        },
        Incomplete {
            case: "a configuration profile stating no LocationUri",
            stack: "appconfig-profile-without-location",
            resources: json!({
                "Application": {
                    "Type": "AWS::AppConfig::Application",
                    "Properties": { "Name": "a-located-application" }
                },
                "Profile": {
                    "Type": "AWS::AppConfig::ConfigurationProfile",
                    "Properties": {
                        "ApplicationId": {"Ref": "Application"},
                        "Name": "a-profile"
                    }
                }
            }),
        },
        Incomplete {
            case: "a deployment strategy stating no GrowthFactor",
            stack: "appconfig-strategy-without-growth",
            resources: json!({
                "Strategy": {
                    "Type": "AWS::AppConfig::DeploymentStrategy",
                    "Properties": {
                        "Name": "a-strategy",
                        "DeploymentDurationInMinutes": 0
                    }
                }
            }),
        },
    ]
}

/// A declaration that states none of a property its create requires fails the resource: the
/// stack does not report `CREATE_COMPLETE`, so nothing publishes an identifier for a resource
/// nobody created.
#[tokio::test]
async fn an_incomplete_declaration_fails_the_resource() {
    let router = router();
    for incomplete in incomplete_estates() {
        let Incomplete {
            case,
            stack,
            resources,
        } = &incomplete;
        let template = json!({ "Resources": resources }).to_string();
        let (status, body) = create_stack(&router, stack, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to accept the template, got {status} with \
             {body}"
        );
        wait_for_the_transition().await;
        let (status, body) = describe_stack(&router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStacks to answer, got {status} with {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        assert_ne!(
            reported, "CREATE_COMPLETE",
            "case {case:?}: expected the stack NOT to report the create complete, got {body}"
        );
    }
}
