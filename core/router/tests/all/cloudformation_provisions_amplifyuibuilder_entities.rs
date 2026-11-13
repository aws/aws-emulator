// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the Amplify UI Builder entities of an Amplify app
//! deploys: the app is created in AWS Amplify, the component, the form and the theme are
//! created in Amplify UI Builder against the app and environment the template names, the stack
//! publishes the ids those services minted, deleting the stack takes every one of them away —
//! and a declaration Amplify UI Builder REFUSES fails the resource with its own refusal and
//! rolls the stack back.
//!
//! The rule under test: **a resource type AWS publishes is one a template may declare, and a
//! declaration of it is deployed by calling the service that OWNS the type — so what the stack
//! publishes is what that service answered, and what that service refuses is a FAILED RESOURCE
//! rather than a template-format error.** Three crates have to agree for that to hold
//! (CloudFormation submits the template and makes the calls, AWS Amplify serves the app, and
//! Amplify UI Builder serves the three entities of its environment), so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The rows of the deploy table differ along what the rule generalises over: WHICH of the three
//! entity types the declaration carries, whether the app environment is read from the app
//! resource (`Fn::GetAtt`), from a stack parameter or written out, which optional members each
//! declaration carries (a component's children, events and variants, a form's call-to-action and
//! sectional elements, a theme's overrides), which published return value each output reads
//! (`Ref` and each of the three `Fn::GetAtt` values), whether the declaration carries the type's
//! own `Tags`, the template spelling (JSON with long-form intrinsics, YAML with the short-form
//! tags), and how many entities one stack declares.
//!
//! The refusal rows differ along the CLASS of declaration Amplify UI Builder refuses: a
//! sectional element whose `type` is outside the values that member documents, a form name
//! another form of the app environment already holds, and a component naming the reserved
//! property name in one of its maps. Each is the owning service's own refusal, reported on the
//! resource that failed — never the `Template format error: Unrecognized resource types` a type
//! CloudFormation does not know is refused with, which no row of either table may ever see.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stacks belong to.
const REGION: &str = "us-east-1";

/// How long one poll of a stack's status waits before asking again, and how many times: the
/// stack's own clock-owed span is half a second, so a create settles well inside this budget
/// and nothing is waited out past the first observation.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS: usize = 60;

/// The fixed text CloudFormation refuses a template declaring a type it does NOT know with,
/// which is what the defect this suite pins answered for three types AWS publishes.
const UNRECOGNIZED_TYPES: &str = "Unrecognized resource types";

/// The backend environment of the app the entities below are created in. Amplify UI Builder
/// takes it as an opaque `@httpLabel`, and `staging` is the environment Amplify Studio names.
const ENVIRONMENT: &str = "staging";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_amplify_amplify::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation action over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one operation of a `restJson1` service on the URI its own `@http` binding declares,
/// under its own signing name, with the body that operation takes.
async fn rest_json_body(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|body| Bytes::from(body.to_string())).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(signing_name))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let document: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, document)
}

/// [`rest_json_body`] for an operation whose input is entirely in its URI.
async fn rest_json(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
) -> (u16, Value) {
    rest_json_body(router, signing_name, method, uri, None).await
}

/// `CreateApp` for one name: the id AWS Amplify minted for the app, as a client's own call
/// gets it.
async fn create_app(router: &MultiServiceRouter, name: &str) -> String {
    let (status, document) = rest_json_body(
        router,
        "amplify",
        "POST",
        "/apps",
        Some(json!({"name": name})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApp {name:?}: expected HTTP 200, got {status} with {document}"
    );
    document
        .pointer("/app/appId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateApp {name:?}: no appId in {document}"))
        .to_string()
}

/// `GetApp` for one app id: the status and the app record AWS Amplify reports.
async fn get_app(router: &MultiServiceRouter, app_id: &str) -> (u16, Value) {
    let (status, document) = rest_json(router, "amplify", "GET", &format!("/apps/{app_id}")).await;
    let app = document.get("app").cloned().unwrap_or(Value::Null);
    (status, if status == 200 { app } else { document })
}

/// One Amplify UI Builder entity as that service reports it: the answer document IS the entity,
/// its read binding it as the `@httpPayload`.
async fn get_entity(
    router: &MultiServiceRouter,
    collection: &str,
    app_id: &str,
    environment: &str,
    id: &str,
) -> (u16, Value) {
    rest_json(
        router,
        "amplifyuibuilder",
        "GET",
        &format!("/app/{app_id}/environment/{environment}/{collection}/{id}"),
    )
    .await
}

/// Every entity id Amplify UI Builder lists for one app environment's collection.
async fn list_entities(
    router: &MultiServiceRouter,
    collection: &str,
    app_id: &str,
    environment: &str,
) -> Vec<String> {
    let (_, document) = rest_json(
        router,
        "amplifyuibuilder",
        "GET",
        &format!("/app/{app_id}/environment/{environment}/{collection}"),
    )
    .await;
    document
        .get("entities")
        .and_then(Value::as_array)
        .map(|entities| {
            entities
                .iter()
                .filter_map(|entity| entity.get("id").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it first reports `wanted`, and answer the last status it reported with the
/// `DescribeStacks` answer that status was read from: the operation's completion is owed from
/// the instant the resources settle, which no fixed sleep can name.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = stack_status(router, stack).await;
    for _ in 0..POLLS {
        if last.0 == wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, stack).await;
    }
    last
}

/// One submit of a template: the action that submits it, the stack it is submitted for, the
/// parameters and stack-level tags it carries, and the `StackStatus` it must reach.
struct Submit<'a> {
    action: &'a str,
    stack_name: &'a str,
    template: &'a str,
    parameters: &'a [(&'a str, &'a str)],
    tags: &'a [(&'a str, &'a str)],
    wanted: &'a str,
}

/// Submit a template, assert every declared type was RECOGNISED, and wait for the stack to
/// reach the status the submit wants; answers the `DescribeStacks` body it was last read from.
///
/// The parameters and the tags travel as the positional members an awsQuery list is written as.
async fn deploy(router: &MultiServiceRouter, case: &str, submit: Submit<'_>) -> String {
    let Submit {
        action,
        stack_name,
        template,
        parameters,
        tags,
        wanted,
    } = submit;
    let mut members = vec![("StackName", stack_name), ("TemplateBody", template)];
    let mut positional: Vec<(String, String)> = parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                (
                    format!("Parameters.member.{position}.ParameterKey"),
                    (*key).to_string(),
                ),
                (
                    format!("Parameters.member.{position}.ParameterValue"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect();
    positional.extend(tags.iter().enumerate().flat_map(|(index, (key, value))| {
        let position = index + 1;
        vec![
            (
                format!("Tags.member.{position}.Key"),
                (*key).to_string(),
            ),
            (
                format!("Tags.member.{position}.Value"),
                (*value).to_string(),
            ),
        ]
    }));
    for (key, value) in &positional {
        members.push((key.as_str(), value.as_str()));
    }
    let (status, body) = cloudformation(router, action, &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action} — every AWS::AmplifyUIBuilder type is \
         one a template may declare — got {status} with body {body}"
    );
    assert!(
        !body.contains(UNRECOGNIZED_TYPES),
        "case {case:?}: expected every declared type to be recognised, got {body}"
    );
    let (reached, described) = wait_for_stack_status(router, stack_name, wanted).await;
    assert_eq!(
        reached, wanted,
        "case {case:?}: expected the stack to reach {wanted}, got {described}"
    );
    described
}

/// What one row expects of one declared Amplify UI Builder entity.
struct Declared {
    /// The collection the entity's own URIs name, which says which type it is.
    collection: &'static str,
    /// The output key whose value is the id that service minted, which is what `Ref` returns.
    id_output: &'static str,
    /// Each output key paired with the value it must carry, for the two further values the type
    /// publishes (`AppId` and `EnvironmentName`).
    scope_outputs: &'static [(&'static str, &'static str)],
    /// The entity's name, as the declaration states it: every one of the three types marks
    /// `Name` required, so a row always states one.
    name: &'static str,
    /// The members the entity must report, as the declaration stated them.
    members: Value,
    /// The tags the entity must carry, declared and propagated together.
    tags: &'static [(&'static str, &'static str)],
}

/// One row: a template, the parameters the submit passes, the stack-level tags it is deployed
/// with, the app environment its entities live in, and the entities it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(&'static str, &'static str)>,
    stack_tags: &'static [(&'static str, &'static str)],
    environment: &'static str,
    declared: Vec<Declared>,
}

/// The app every row declares beside its entities, and the outputs that read it back.
fn app_declaration(name: &str) -> Value {
    json!({
        "Type": "AWS::Amplify::App",
        "Properties": {"Name": name, "Description": "the app the entities belong to"}
    })
}

fn rows() -> Vec<Row> {
    vec![
        // The issue's own stack: the three entity types at once, in one app environment read
        // off the app resource, every published return value read back, in JSON with long-form
        // intrinsics and the types' own `Tags`.
        Row {
            case: "a component, a form and a theme in one stack, every return value read, JSON",
            stack_name: "amplifyuibuilder-round-trip",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "App": app_declaration("uibuilder-round-trip"),
                    "Theme": {
                        "Type": "AWS::AmplifyUIBuilder::Theme",
                        "Properties": {
                            "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                            "EnvironmentName": ENVIRONMENT,
                            "Name": "RoundTripTheme",
                            "Values": [
                                {"Key": "colors", "Value": {"Children": [
                                    {"Key": "brand", "Value": {"Value": "#0b5"}}
                                ]}}
                            ],
                            "Overrides": [{"Key": "spacing", "Value": {"Value": "16px"}}],
                            "Tags": {"owner": "studio"}
                        }
                    },
                    "Form": {
                        "Type": "AWS::AmplifyUIBuilder::Form",
                        "Properties": {
                            "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                            "EnvironmentName": ENVIRONMENT,
                            "Name": "RoundTripForm",
                            "DataType": {"DataSourceType": "DataStore", "DataTypeName": "Todo"},
                            "FormActionType": "create",
                            "SchemaVersion": "1.0",
                            "Fields": {
                                "title": {
                                    "Label": "Title",
                                    "InputType": {"Type": "TextField", "Required": true}
                                }
                            },
                            "SectionalElements": {
                                "Heading": {"Type": "Heading", "Level": 1, "Text": "Todo"}
                            },
                            "Style": {"HorizontalGap": {"Value": "16px"}},
                            "Cta": {"Position": "bottom", "Submit": {"Excluded": false}}
                        }
                    },
                    "Component": {
                        "Type": "AWS::AmplifyUIBuilder::Component",
                        "Properties": {
                            "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                            "EnvironmentName": ENVIRONMENT,
                            "Name": "RoundTripComponent",
                            "ComponentType": "Button",
                            "SchemaVersion": "1.0",
                            "Properties": {
                                "label": {"Value": "Save"},
                                "model": {
                                    "BindingProperties": {"Property": "item", "Field": "title"}
                                }
                            },
                            "Overrides": {"Button": {"fontWeight": "bold"}},
                            "BindingProperties": {
                                "item": {"Type": "Data", "DefaultValue": "none"}
                            },
                            "Variants": [{
                                "VariantValues": {"breakpoint": "large"},
                                "Overrides": {"Button": {"fontSize": "20px"}}
                            }],
                            "Children": [{
                                "ComponentType": "Text",
                                "Name": "Caption",
                                "Properties": {"label": {"Value": "Saves the item"}}
                            }],
                            "Events": {
                                "onClick": {
                                    "Action": "Amplify.Navigation",
                                    "Parameters": {"Anchor": {"Value": "#top"}}
                                }
                            },
                            "Tags": {"owner": "studio"}
                        }
                    }
                },
                "Outputs": {
                    "AppIdAtt": {"Value": {"Fn::GetAtt": ["App", "AppId"]}},
                    "ThemeId": {"Value": {"Fn::GetAtt": ["Theme", "Id"]}},
                    "ThemeRef": {"Value": {"Ref": "Theme"}},
                    "ThemeAppId": {"Value": {"Fn::GetAtt": ["Theme", "AppId"]}},
                    "ThemeEnvironment": {
                        "Value": {"Fn::GetAtt": ["Theme", "EnvironmentName"]}
                    },
                    "FormId": {"Value": {"Fn::GetAtt": ["Form", "Id"]}},
                    "ComponentId": {"Value": {"Fn::GetAtt": ["Component", "Id"]}}
                }
            })
            .to_string(),
            parameters: Vec::new(),
            stack_tags: &[("sim2real", "amplifyuibuilder-round-trip")],
            environment: ENVIRONMENT,
            declared: vec![
                Declared {
                    collection: "themes",
                    id_output: "ThemeId",
                    scope_outputs: &[("ThemeEnvironment", ENVIRONMENT)],
                    name: "RoundTripTheme",
                    members: json!({
                        "name": "RoundTripTheme",
                        "values": [
                            {"key": "colors", "value": {"children": [
                                {"key": "brand", "value": {"value": "#0b5"}}
                            ]}}
                        ],
                        "overrides": [{"key": "spacing", "value": {"value": "16px"}}]
                    }),
                    tags: &[
                        ("owner", "studio"),
                        ("sim2real", "amplifyuibuilder-round-trip"),
                    ],
                },
                Declared {
                    collection: "forms",
                    id_output: "FormId",
                    scope_outputs: &[],
                    name: "RoundTripForm",
                    members: json!({
                        "name": "RoundTripForm",
                        "formActionType": "create",
                        "schemaVersion": "1.0",
                        "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Todo"},
                        "fields": {
                            "title": {
                                "label": "Title",
                                "inputType": {"type": "TextField", "required": true}
                            }
                        },
                        "sectionalElements": {
                            "Heading": {"type": "Heading", "level": 1, "text": "Todo"}
                        },
                        "style": {"horizontalGap": {"value": "16px"}},
                        "cta": {"position": "bottom", "submit": {"excluded": false}}
                    }),
                    tags: &[("sim2real", "amplifyuibuilder-round-trip")],
                },
                Declared {
                    collection: "components",
                    id_output: "ComponentId",
                    scope_outputs: &[],
                    name: "RoundTripComponent",
                    members: json!({
                        "name": "RoundTripComponent",
                        "componentType": "Button",
                        "schemaVersion": "1.0",
                        "properties": {
                            "label": {"value": "Save"},
                            "model": {
                                "bindingProperties": {"property": "item", "field": "title"}
                            }
                        },
                        "overrides": {"Button": {"fontWeight": "bold"}},
                        "bindingProperties": {"item": {"type": "Data", "defaultValue": "none"}},
                        "variants": [{
                            "variantValues": {"breakpoint": "large"},
                            "overrides": {"Button": {"fontSize": "20px"}}
                        }],
                        "children": [{
                            "componentType": "Text",
                            "name": "Caption",
                            "properties": {"label": {"value": "Saves the item"}}
                        }],
                        "events": {
                            "onClick": {
                                "action": "Amplify.Navigation",
                                "parameters": {"anchor": {"value": "#top"}}
                            }
                        }
                    }),
                    tags: &[
                        ("owner", "studio"),
                        ("sim2real", "amplifyuibuilder-round-trip"),
                    ],
                },
            ],
        },
        // The same type in YAML with the short-form tags, the app environment read from a
        // stack PARAMETER rather than written out, and TWO entities of one type in one stack —
        // one carrying the overrides a theme publishes beside its values and one carrying none.
        Row {
            case: "two themes, one with overrides, the environment from a parameter, YAML",
            stack_name: "amplifyuibuilder-yaml",
            template: "\
Parameters:
  Environment:
    Type: String
Resources:
  App:
    Type: AWS::Amplify::App
    Properties:
      Name: uibuilder-yaml
  First:
    Type: AWS::AmplifyUIBuilder::Theme
    Properties:
      AppId: !GetAtt App.AppId
      EnvironmentName: !Ref Environment
      Name: FirstTheme
      Values:
        - Key: tokens
          Value:
            Value: small
      Overrides:
        - Key: spacing
          Value:
            Value: 8px
  Second:
    Type: AWS::AmplifyUIBuilder::Theme
    Properties:
      AppId: !GetAtt App.AppId
      EnvironmentName: !Ref Environment
      Name: SecondTheme
      Values:
        - Key: tokens
          Value:
            Value: large
Outputs:
  AppIdAtt:
    Value: !GetAtt App.AppId
  FirstId:
    Value: !GetAtt First.Id
  FirstAppId:
    Value: !GetAtt First.AppId
  FirstEnvironment:
    Value: !GetAtt First.EnvironmentName
  SecondId:
    Value: !GetAtt Second.Id
"
            .to_string(),
            parameters: vec![("Environment", "dev")],
            stack_tags: &[],
            environment: "dev",
            declared: vec![
                Declared {
                    collection: "themes",
                    id_output: "FirstId",
                    scope_outputs: &[("FirstAppId", ""), ("FirstEnvironment", "dev")],
                    name: "FirstTheme",
                    members: json!({
                        "name": "FirstTheme",
                        "values": [{"key": "tokens", "value": {"value": "small"}}],
                        "overrides": [{"key": "spacing", "value": {"value": "8px"}}]
                    }),
                    tags: &[],
                },
                Declared {
                    collection: "themes",
                    id_output: "SecondId",
                    scope_outputs: &[],
                    name: "SecondTheme",
                    members: json!({
                        "name": "SecondTheme",
                        "values": [{"key": "tokens", "value": {"value": "large"}}]
                    }),
                    tags: &[],
                },
            ],
        },
        // A Studio-shaped theme: SEVERAL top-level values, each nesting its own children three
        // levels deep, beside the overrides of one more — the form a design-token document
        // really takes, which the entity has to report back entry for entry.
        Row {
            case: "a theme whose values nest three levels under several top-level keys",
            stack_name: "amplifyuibuilder-tokens",
            template: json!({
                "Resources": {
                    "App": app_declaration("uibuilder-tokens"),
                    "Tokens": {
                        "Type": "AWS::AmplifyUIBuilder::Theme",
                        "Properties": {
                            "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                            "EnvironmentName": ENVIRONMENT,
                            "Name": "TokensTheme",
                            "Values": [
                                {"Key": "colors", "Value": {"Children": [
                                    {"Key": "brand", "Value": {"Children": [
                                        {"Key": "10", "Value": {"Value": "#e0f5f7"}},
                                        {"Key": "80", "Value": {"Value": "#047d95"}}
                                    ]}},
                                    {"Key": "space", "Value": {"Value": "8px"}}
                                ]}},
                                {"Key": "font", "Value": {"Children": [
                                    {"Key": "primary", "Value": {"Value": "Amazon Ember"}}
                                ]}},
                                {"Key": "borderRadius", "Value": {"Value": "4px"}}
                            ],
                            "Overrides": [
                                {"Key": "dark", "Value": {"Children": [
                                    {"Key": "primaryColor", "Value": {"Value": "#ffffff"}}
                                ]}}
                            ]
                        }
                    }
                },
                "Outputs": {
                    "AppIdAtt": {"Value": {"Fn::GetAtt": ["App", "AppId"]}},
                    "TokensId": {"Value": {"Fn::GetAtt": ["Tokens", "Id"]}}
                }
            })
            .to_string(),
            parameters: Vec::new(),
            stack_tags: &[],
            environment: ENVIRONMENT,
            declared: vec![Declared {
                collection: "themes",
                id_output: "TokensId",
                scope_outputs: &[],
                name: "TokensTheme",
                members: json!({
                    "name": "TokensTheme",
                    "values": [
                        {"key": "colors", "value": {"children": [
                            {"key": "brand", "value": {"children": [
                                {"key": "10", "value": {"value": "#e0f5f7"}},
                                {"key": "80", "value": {"value": "#047d95"}}
                            ]}},
                            {"key": "space", "value": {"value": "8px"}}
                        ]}},
                        {"key": "font", "value": {"children": [
                            {"key": "primary", "value": {"value": "Amazon Ember"}}
                        ]}},
                        {"key": "borderRadius", "value": {"value": "4px"}}
                    ],
                    "overrides": [
                        {"key": "dark", "value": {"children": [
                            {"key": "primaryColor", "value": {"value": "#ffffff"}}
                        ]}}
                    ]
                }),
                tags: &[],
            }],
        },
    ]
}

/// The tags one reported entity or app carries, as the map that service answers them in.
fn reported_tags(reported: &Value) -> BTreeMap<String, String> {
    reported
        .get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_declared_ui_entity_is_created_in_amplify_ui_builder() {
    for row in rows() {
        let router = build_router();
        let case = row.case;
        let described = deploy(
            &router,
            case,
            Submit {
                action: "CreateStack",
                stack_name: row.stack_name,
                template: &row.template,
                parameters: &row.parameters,
                tags: row.stack_tags,
                wanted: "CREATE_COMPLETE",
            },
        )
        .await;

        let outputs = outputs_of(&described);
        let app_id = outputs
            .get("AppIdAtt")
            .unwrap_or_else(|| panic!("case {case:?}: no AppIdAtt output in {described}"))
            .clone();
        // The app the entities belong to is one AWS Amplify itself holds, under the id it
        // minted — which is the form that service's own `AppId` member fixes.
        let (status, app) = get_app(&router, &app_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AWS Amplify to hold the app the stack published, got \
             {status} with {app}"
        );
        assert_eq!(
            app.get("appId").and_then(Value::as_str),
            Some(app_id.as_str()),
            "case {case:?}: expected the app to report the id the stack published, got {app}"
        );

        let mut seen: Vec<String> = Vec::new();
        for declared in &row.declared {
            let case = format!("{case} [{}]", declared.id_output);
            let id = outputs
                .get(declared.id_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {described}", declared.id_output)
                })
                .clone();
            assert!(
                !seen.contains(&id),
                "case {case:?}: expected each entity to carry its own id, got {id:?} twice"
            );
            seen.push(id.clone());

            let environment = row.environment;

            // Amplify UI Builder itself resolves the id the stack published, in the app
            // environment the declaration named, and what it holds is what the template
            // declared.
            let (status, reported) = get_entity(
                &router,
                declared.collection,
                &app_id,
                environment,
                &id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Amplify UI Builder to resolve the id the stack \
                 published, got {status} with {reported}"
            );
            assert_eq!(
                reported.get("appId").and_then(Value::as_str),
                Some(app_id.as_str()),
                "case {case:?}: expected the entity to belong to the app the stack created, \
                 got {reported}"
            );
            assert_eq!(
                reported.get("environmentName").and_then(Value::as_str),
                Some(environment),
                "case {case:?}: expected the declared environment, got {reported}"
            );
            assert_eq!(
                reported.get("name").and_then(Value::as_str),
                Some(declared.name),
                "case {case:?}: expected the declared name, got {reported}"
            );
            for (member, wanted) in declared
                .members
                .as_object()
                .expect("the row's members are an object")
            {
                let got = reported.get(member);
                assert_eq!(
                    got,
                    Some(wanted),
                    "case {case:?}: expected the declared {member} {wanted}, got {got:?} in \
                     {reported}"
                );
            }
            let tags = reported_tags(&reported);
            for (key, value) in declared.tags {
                assert_eq!(
                    tags.get(*key).map(String::as_str),
                    Some(*value),
                    "case {case:?}: expected the tag {key}={value} on the entity, got {tags:?}"
                );
            }
            // CloudFormation records the stack it created the resource for on the resource
            // itself, through the owning service's own tagging.
            if !declared.tags.is_empty() {
                assert_eq!(
                    tags.get("aws:cloudformation:stack-name").map(String::as_str),
                    Some(row.stack_name),
                    "case {case:?}: expected the stack's own provenance tag, got {tags:?}"
                );
            }
            for (output, wanted) in declared.scope_outputs {
                let published = outputs
                    .get(*output)
                    .unwrap_or_else(|| panic!("case {case:?}: no {output} output in {described}"));
                let wanted = if wanted.is_empty() { &app_id } else { *wanted };
                assert_eq!(
                    published, wanted,
                    "case {case:?}: expected the published {output} to be {wanted:?}, got \
                     {published:?}"
                );
            }
            // `Ref` returns the type's PRIMARY IDENTIFIER, which is the triple its own resource
            // shape is identified by: the app, the environment and the id that service minted,
            // joined with `|`. `Fn::GetAtt Id` reads the last part on its own.
            if declared.id_output == "ThemeId" {
                let reference = outputs
                    .get("ThemeRef")
                    .unwrap_or_else(|| panic!("case {case:?}: no ThemeRef output in {described}"));
                assert_eq!(
                    reference,
                    &format!("{app_id}|{environment}|{id}"),
                    "case {case:?}: expected Ref to return the primary identifier, got \
                     {reference:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_entities_it_created() {
    let router = build_router();
    let case = "a component, a form and a theme deleted with their stack";
    let stack_name = "amplifyuibuilder-deleted";
    let template = json!({
        "Resources": {
            "App": app_declaration("uibuilder-deleted"),
            "Theme": {
                "Type": "AWS::AmplifyUIBuilder::Theme",
                "Properties": {
                    "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                    "EnvironmentName": ENVIRONMENT,
                    "Name": "DeletedTheme",
                    "Values": [{"Key": "tokens", "Value": {"Value": "small"}}]
                }
            },
            "Form": {
                "Type": "AWS::AmplifyUIBuilder::Form",
                "Properties": {
                    "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                    "EnvironmentName": ENVIRONMENT,
                    "Name": "DeletedForm",
                    "DataType": {"DataSourceType": "Custom", "DataTypeName": "Todo"},
                    "FormActionType": "update",
                    "SchemaVersion": "1.0",
                    "Fields": {},
                    "SectionalElements": {},
                    "Style": {}
                }
            },
            "Component": {
                "Type": "AWS::AmplifyUIBuilder::Component",
                "Properties": {
                    "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                    "EnvironmentName": ENVIRONMENT,
                    "Name": "DeletedComponent",
                    "ComponentType": "Text",
                    "Properties": {},
                    "Overrides": {},
                    "BindingProperties": {},
                    "Variants": []
                }
            }
        },
        "Outputs": {
            "AppIdAtt": {"Value": {"Fn::GetAtt": ["App", "AppId"]}},
            "ThemeId": {"Value": {"Fn::GetAtt": ["Theme", "Id"]}},
            "FormId": {"Value": {"Fn::GetAtt": ["Form", "Id"]}},
            "ComponentId": {"Value": {"Fn::GetAtt": ["Component", "Id"]}}
        }
    })
    .to_string();

    let described = deploy(
        &router,
        case,
        Submit {
            action: "CreateStack",
            stack_name,
            template: &template,
            parameters: &[],
            tags: &[],
            wanted: "CREATE_COMPLETE",
        },
    )
    .await;
    let outputs = outputs_of(&described);
    let app_id = outputs
        .get("AppIdAtt")
        .unwrap_or_else(|| panic!("case {case:?}: no AppIdAtt output in {described}"))
        .clone();
    let entities: Vec<(&str, String)> = [
        ("themes", "ThemeId"),
        ("forms", "FormId"),
        ("components", "ComponentId"),
    ]
    .iter()
    .map(|(collection, key)| {
        (
            *collection,
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {described}"))
                .clone(),
        )
    })
    .collect();
    for (collection, id) in &entities {
        let listed = list_entities(&router, collection, &app_id, ENVIRONMENT).await;
        assert!(
            listed.contains(id),
            "case {case:?}: expected Amplify UI Builder to list {id} under {collection}, got \
             {listed:?}"
        );
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );

    // The stack's deletion takes every resource it created with it, in both services.
    for _ in 0..POLLS {
        let (status, _) = get_app(&router, &app_id).await;
        if status == 404 {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    for (collection, id) in &entities {
        let (status, answer) = get_entity(&router, collection, &app_id, ENVIRONMENT, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted {collection} entity {id} to be gone, got \
             {status} with {answer}"
        );
    }
    let (status, answer) = get_app(&router, &app_id).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the app to be gone from AWS Amplify, got {status} with \
         {answer}"
    );
}

#[tokio::test]
async fn an_update_applies_the_changed_declaration_to_the_entity_it_created() {
    let router = build_router();
    let case = "a renamed theme and re-declared values";
    let stack_name = "amplifyuibuilder-updated";
    // The app is created through AWS Amplify's own API, exactly as a client creates one, and
    // the stack states its id as a PARAMETER: what the update is about is the entity, not the
    // app it lives in.
    let app_id = create_app(&router, "uibuilder-updated").await;
    let declaration = |theme_name: &str, label: &str| {
        json!({
            "Parameters": {"AppId": {"Type": "String"}},
            "Resources": {
                "Theme": {
                    "Type": "AWS::AmplifyUIBuilder::Theme",
                    "Properties": {
                        "AppId": {"Ref": "AppId"},
                        "EnvironmentName": ENVIRONMENT,
                        "Name": theme_name,
                        "Values": [{"Key": "tokens", "Value": {"Value": label}}]
                    }
                }
            },
            "Outputs": {
                "ThemeId": {"Value": {"Fn::GetAtt": ["Theme", "Id"]}}
            }
        })
        .to_string()
    };
    let parameters: Vec<(&str, &str)> = vec![("AppId", app_id.as_str())];

    let described = deploy(
        &router,
        case,
        Submit {
            action: "CreateStack",
            stack_name,
            template: &declaration("BeforeTheme", "small"),
            parameters: &parameters,
            tags: &[],
            wanted: "CREATE_COMPLETE",
        },
    )
    .await;
    let outputs = outputs_of(&described);
    let id = outputs
        .get("ThemeId")
        .unwrap_or_else(|| panic!("case {case:?}: no ThemeId output in {described}"))
        .clone();

    let described = deploy(
        &router,
        case,
        Submit {
            action: "UpdateStack",
            stack_name,
            template: &declaration("AfterTheme", "large"),
            parameters: &parameters,
            tags: &[],
            wanted: "UPDATE_COMPLETE",
        },
    )
    .await;
    let updated = outputs_of(&described);
    assert_eq!(
        updated.get("ThemeId"),
        Some(&id),
        "case {case:?}: expected the theme to keep the id that service minted, got {described}"
    );

    // The change reached the theme Amplify UI Builder holds, not just the stack's record: the
    // entity is identified by its id, and `UpdateTheme` takes a new name for it, so a renamed
    // theme is the SAME theme.
    let (status, reported) = get_entity(&router, "themes", &app_id, ENVIRONMENT, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the theme to still be there, got {status} with {reported}"
    );
    assert_eq!(
        reported.get("name").and_then(Value::as_str),
        Some("AfterTheme"),
        "case {case:?}: expected the re-declared name, got {reported}"
    );
    assert_eq!(
        reported.get("values"),
        Some(&json!([{"key": "tokens", "value": {"value": "large"}}])),
        "case {case:?}: expected the re-declared values, got {reported}"
    );
    let listed = list_entities(&router, "themes", &app_id, ENVIRONMENT).await;
    assert_eq!(
        listed,
        vec![id.clone()],
        "case {case:?}: expected the one theme to be changed rather than a second one \
         created, got {listed:?}"
    );
}

/// One refusal row: a declaration Amplify UI Builder refuses, and the FIXED text of the
/// sentence it words that class of refusal with, which the failed resource's reason has to
/// carry.
struct Refused {
    case: &'static str,
    stack_name: &'static str,
    resources: fn() -> Value,
    sentence: &'static str,
}

const REFUSED: &[Refused] = &[
    // `SectionalElement$type`'s own documented values are `Heading`, `Text` and `Divider`, so a
    // form declaring anything else is not one that service will create.
    Refused {
        case: "a sectional element type outside the values the member documents",
        stack_name: "amplifyuibuilder-bad-element",
        resources: bad_sectional_element,
        sentence: "Invalid value Banner for formToCreate.sectionalElements",
    },
    // A component property map naming the reserved `tags` key is refused: "You can't specify
    // tags as a valid property for properties".
    Refused {
        case: "a component property map naming the reserved property name",
        stack_name: "amplifyuibuilder-reserved-name",
        resources: reserved_property_name,
        sentence: "cannot carry the reserved property name",
    },
];

/// The app every refusal row declares, under the environment its entity is created in.
fn refusal_app() -> Value {
    app_declaration("uibuilder-refused")
}

fn bad_sectional_element() -> Value {
    json!({
        "App": refusal_app(),
        "Form": {
            "Type": "AWS::AmplifyUIBuilder::Form",
            "Properties": {
                "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                "EnvironmentName": ENVIRONMENT,
                "Name": "RefusedForm",
                "DataType": {"DataSourceType": "Custom", "DataTypeName": "Todo"},
                "FormActionType": "create",
                "SchemaVersion": "1.0",
                "Fields": {},
                "SectionalElements": {"Banner": {"Type": "Banner"}},
                "Style": {}
            }
        }
    })
}

fn reserved_property_name() -> Value {
    json!({
        "App": refusal_app(),
        "Component": {
            "Type": "AWS::AmplifyUIBuilder::Component",
            "Properties": {
                "AppId": {"Fn::GetAtt": ["App", "AppId"]},
                "EnvironmentName": ENVIRONMENT,
                "Name": "RefusedComponent",
                "ComponentType": "Button",
                "Properties": {"tags": {"Value": "no"}},
                "Overrides": {},
                "BindingProperties": {},
                "Variants": []
            }
        }
    })
}

#[tokio::test]
async fn a_declaration_amplify_ui_builder_refuses_rolls_the_stack_back() {
    for row in REFUSED {
        let router = build_router();
        let case = row.case;
        let template = json!({
            "Resources": (row.resources)(),
            "Outputs": {"AppIdAtt": {"Value": {"Fn::GetAtt": ["App", "AppId"]}}}
        })
        .to_string();

        let body = deploy(
            &router,
            case,
            Submit {
                action: "CreateStack",
                stack_name: row.stack_name,
                template: &template,
                parameters: &[],
                tags: &[],
                wanted: "ROLLBACK_COMPLETE",
            },
        )
        .await;

        // The failure is the OWNING service's own refusal, on the resource that failed, and
        // never a refusal of the template: no event reports an unrecognized type at all.
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", row.stack_name)]).await;
        assert!(
            !events.contains(UNRECOGNIZED_TYPES),
            "case {case:?}: expected no template-format error for a type AWS publishes, got \
             {events}"
        );
        let reasons = elements(&events, "ResourceStatusReason");
        assert!(
            reasons.iter().any(|reason| reason.contains(row.sentence)),
            "case {case:?}: expected a failure reason carrying {:?}, got {reasons:?}",
            row.sentence
        );
        assert!(
            outputs_of(&body).is_empty(),
            "case {case:?}: expected no output for a stack that rolled back, got {body}"
        );
    }
}
