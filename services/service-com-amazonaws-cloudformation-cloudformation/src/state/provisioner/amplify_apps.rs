// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arm of AWS Amplify's own control plane: the app an
//! `AWS::Amplify::App` declares. One file of [`super`]'s per-type arms, dispatched to by its
//! `Provisioner`, beside the arms of the Amplify UI Builder entities that live INSIDE such an
//! app ([`super::amplify_ui_builder`]).
//!
//! ## Why the app is really created
//!
//! An Amplify app is what every other Amplify resource hangs off: a branch, a domain
//! association and a backend environment are each created under the `appId` that service
//! minted, and so are the component, the form and the theme of an app environment — a
//! template declaring one of those states the app by `Fn::GetAtt <app>.AppId`. A stack that
//! reported the app created while nothing created it publishes an id that resolves to nothing,
//! so every resource that names it fails and `amplify list-apps` reports no app at all.
//!
//! ## The identity is that service's
//!
//! `CreateApp` takes the app's NAME and no identifier, and answers the app carrying the
//! `appId` it minted (`^d[a-z0-9]+$`, which no name CloudFormation decides satisfies) and the
//! `appArn` built around it. "Ref returns the ARN of the app", which is the type's primary
//! identifier, so the physical id is that ARN and the id is published as `AppId`
//! ([`super::super::template::identity_is_service_assigned`] names the type). The NAME stays
//! CloudFormation's — the `Name` the declaration states, or the generated one — and
//! `UpdateApp` takes a new one for the app that exists, so renaming an app CHANGES it rather
//! than replacing it ([`super::super::template`]'s `name_is_create_only`).
//!
//! ## What the declaration is
//!
//! Most of this type's properties are the member of the same name with a lower-cased initial
//! ([`camel_cased`]). Three are not, and each is converted here:
//!
//! * `IAMServiceRole` is that call's `iamServiceRoleArn`;
//! * `EnvironmentVariables` is a LIST of `{Name, Value}` pairs while the member is a MAP;
//! * `BasicAuthConfig` states the user and the password apart, while the member pair is
//!   `enableBasicAuth` beside the `basicAuthCredentials` that service takes "base64-encode[d]
//!   … in the format `user:password`".
//!
//! `Tags` is a LIST of `{Key, Value}` pairs for this type, converted to the `TagMap` that
//! call's own `tags` member takes, and carrying the tags CloudFormation gives the resource
//! beside the ones the declaration asks for.
//!
//! A property outside [`AMPLIFY_APP_MAPPED_PROPERTIES`] is REFUSED rather than dropped: a
//! resource reported as created with less than its declaration asked for is not the resource
//! the template described.

use super::*;

/// AWS Amplify's signing name, from its model (`sigv4Name: amplify`), which speaks
/// `restJson1` — so its calls ride on [`call_rest_json_named`], each on the URI its own
/// operation's `@http` binding declares, naming the operation so a refusal is reported the way
/// that service's own SDK raises it.
pub(super) const AMPLIFY_SIGNING_NAME: &str = "amplify";

/// The collection AWS Amplify binds its apps to, as that model's `@http` traits declare:
/// `POST /apps` creates one, and `GET`/`POST`/`DELETE /apps/{appId}` read, change and remove
/// it.
const AMPLIFY_APPS_URI: &str = "/apps";

/// The operations this module calls, as AWS Amplify's own model names them.
const CREATE_APP: &str = "CreateApp";
const GET_APP: &str = "GetApp";
const UPDATE_APP: &str = "UpdateApp";
const DELETE_APP: &str = "DeleteApp";

/// The error code AWS Amplify answers for an app it does not hold (`NotFoundException`, the
/// modelled 404 of its read, its update and its delete).
const AMPLIFY_NOT_FOUND: &str = "NotFoundException";

/// The member every one of those operations answers the app record in.
const APP_MEMBER: &str = "app";

/// The member the app's name reaches `CreateApp` and `UpdateApp` as.
const NAME_MEMBER: &str = "name";

/// The property the declaration states the app's name in.
const NAME_PROPERTY: &str = "Name";

/// The properties this arm maps onto `CreateApp`'s members, and so the ones a declaration may
/// carry: every property of the type whose value this arm really applies.
///
/// Source: the type's own entry in the "AWS resource and property types reference" read beside
/// `CreateApp`'s own members (`smithy_query(query="operation", operation="CreateApp")`).
const AMPLIFY_APP_MAPPED_PROPERTIES: &[&str] = &[
    "AccessToken",
    "AutoBranchCreationConfig",
    "BasicAuthConfig",
    "BuildSpec",
    "CacheConfig",
    "ComputeRoleArn",
    "CustomHeaders",
    "CustomRules",
    "Description",
    "EnableBranchAutoDeletion",
    "EnvironmentVariables",
    "IAMServiceRole",
    "JobConfig",
    "Name",
    "OauthToken",
    "Platform",
    "Repository",
    "Tags",
];

/// The properties whose member name is NOT the property with a lower-cased initial, as
/// `(property, member)`: the one such rename this type carries
/// (`CreateApp$iamServiceRoleArn`).
const AMPLIFY_APP_RENAMED_PROPERTIES: &[(&str, &str)] = &[("IAMServiceRole", "iamServiceRoleArn")];

/// The properties stated as a LIST of `{Name, Value}` pairs that the owning API takes as a
/// MAP: the app's own environment variables, and the ones an auto-created branch inherits.
const ENVIRONMENT_VARIABLES_PROPERTY: &str = "EnvironmentVariables";
const ENVIRONMENT_VARIABLES_MEMBER: &str = "environmentVariables";

/// The property that states basic authorization apart from the credentials member it becomes,
/// with the two halves it states them in, and the members it maps onto.
const BASIC_AUTH_CONFIG_PROPERTY: &str = "BasicAuthConfig";
const BASIC_AUTH_ENABLE_PROPERTY: &str = "EnableBasicAuth";
const BASIC_AUTH_USERNAME_PROPERTY: &str = "Username";
const BASIC_AUTH_PASSWORD_PROPERTY: &str = "Password";
const ENABLE_BASIC_AUTH_MEMBER: &str = "enableBasicAuth";
const BASIC_AUTH_CREDENTIALS_MEMBER: &str = "basicAuthCredentials";

/// The property that carries the configuration of an auto-created branch, which states its own
/// environment variables and basic authorization the same way the app does.
const AUTO_BRANCH_CREATION_CONFIG_PROPERTY: &str = "AutoBranchCreationConfig";
const AUTO_BRANCH_CREATION_CONFIG_MEMBER: &str = "autoBranchCreationConfig";

/// The member the declared tag list reaches `CreateApp` as — its own `tags` map, which
/// `UpdateApp` does not take — and the property it is declared in.
const TAGS_MEMBER: &str = "tags";
const TAGS_PROPERTY: &str = "Tags";

/// The attributes an `AWS::Amplify::App` publishes, as `(attribute, member)`: the id AWS
/// Amplify minted, the name the app carries, its ARN and the default domain that service
/// serves its branches under — each read off that service's own report of the app rather than
/// recomputed here.
const AMPLIFY_APP_ATTRIBUTES: &[(&str, &str)] = &[
    ("AppId", "appId"),
    ("AppName", "name"),
    ("Arn", "appArn"),
    ("DefaultDomain", "defaultDomain"),
];

/// The attribute carrying the ARN that is this type's primary identifier.
const APP_ARN_ATTRIBUTE: &str = "Arn";

/// The properties the app's own read reports back under the same names, for drift detection,
/// as `(property, member)`: the SCALAR members `GetApp` answers that a declaration also
/// states.
///
/// The configuration documents and the lists are left out: what the read reports are that
/// service's own member names and shapes, and a property the read does not report is not
/// evidence of anything ([`super::drift`]).
const AMPLIFY_APP_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Name", "name"),
    ("Description", "description"),
    ("Repository", "repository"),
    ("Platform", "platform"),
    ("BuildSpec", "buildSpec"),
    ("CustomHeaders", "customHeaders"),
    ("EnableBranchAutoDeletion", "enableBranchAutoDeletion"),
    ("IAMServiceRole", "iamServiceRoleArn"),
    ("ComputeRoleArn", "computeRoleArn"),
];

/// Create the app an `AWS::Amplify::App` declares, in AWS Amplify — so a client that deployed
/// the stack finds it in `amplify list-apps`, reads it back with `GetApp` for the `AppId` the
/// stack publishes, and can create the branches, the domains and the Amplify UI Builder
/// entities that hang off that id.
///
/// The declaration IS the request once its members are converted
/// ([`declared_app_members`]), under the NAME CloudFormation decided, with the declared tags
/// beside the ones CloudFormation gives the resource ([`tags_of`]).
///
/// The identity is that service's: the `appArn` it answered becomes this resource's physical
/// id — the type's primary identifier — and the four values the type publishes are read off the
/// app it answered.
pub(super) async fn create_amplify_app(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, AMPLIFY_APP_MAPPED_PROPERTIES)?;
    let mut request = declared_app_members(&resource.properties)?;
    request.insert(
        NAME_MEMBER.to_string(),
        Value::String(app_name(&resource.properties, resource)),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert(TAGS_MEMBER.to_string(), Value::Object(tags_as_map(&tags)));
    }
    let answered = call_rest_json_named(
        ctx,
        AMPLIFY_SIGNING_NAME,
        region,
        CREATE_APP,
        "POST",
        AMPLIFY_APPS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let attributes = app_attributes(&answered)?;
    Ok(Created {
        // "Ref returns the ARN of the app", which is the identifier AWS Amplify built around
        // the id it minted — neither of them a value a naming rule could have computed.
        physical_resource_id: attributes.get(APP_ARN_ATTRIBUTE).cloned(),
        attributes,
    })
}

/// Apply a changed declaration to the app the stack created, through `UpdateApp` — the one
/// call that changes an app that exists.
///
/// It takes the app's id together with the declared configuration, the app's NAME included, so
/// a re-declared name renames the app a stack has rather than replacing it. The app keeps the
/// id and the ARN that service minted, so the handle the stack publishes is unchanged.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it: a changed `Tags` list, which `UpdateApp` does not
/// carry — an app's tags are that service's own tagging calls.
pub(super) async fn update_amplify_app(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if declared.get(TAGS_PROPERTY) != resource.properties.get(TAGS_PROPERTY) {
        return Ok(None);
    }
    // The same refusal the create is held to, over the document THIS deployment declares: a
    // property no arm applies must not be reported as an applied change any more than as a
    // created resource.
    refuse_unmapped_properties(
        &StoredResource {
            properties: declared.clone(),
            ..resource.clone()
        },
        AMPLIFY_APP_MAPPED_PROPERTIES,
    )?;
    let mut request = declared_app_members(declared)?;
    request.insert(
        NAME_MEMBER.to_string(),
        Value::String(app_name(declared, resource)),
    );
    let answered = call_rest_json_named(
        ctx,
        AMPLIFY_SIGNING_NAME,
        region,
        UPDATE_APP,
        "POST",
        &app_uri(&app_id_of(resource)),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::attributes(app_attributes(&answered)?)))
}

/// Delete the app a stack created, by the id AWS Amplify knows it by. An app that service no
/// longer holds (`NotFoundException`, judged on the CODE and never on a message that carries
/// the request's own values) is already in the state the delete asks for.
pub(super) async fn delete_amplify_app(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    match call_rest_json_named(
        ctx,
        AMPLIFY_SIGNING_NAME,
        region,
        DELETE_APP,
        "DELETE",
        &app_uri(&app_id_of(resource)),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == AMPLIFY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The app's configuration as AWS Amplify reports it, for drift detection: `GetApp`'s own
/// record, read by the id the stack published ([`AMPLIFY_APP_REPORTED_PROPERTIES`]). One that
/// service no longer holds was deleted.
pub(super) async fn describe_amplify_app(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    Ok(read_amplify_app(ctx, region, &app_id_of(resource))
        .await?
        .map(|app| reported_as_properties(&app, AMPLIFY_APP_REPORTED_PROPERTIES)))
}

/// The app record AWS Amplify holds under `app_id`, or `None` when it holds none
/// (`NotFoundException`, judged on the CODE and never on a message).
async fn read_amplify_app(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    app_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json_named(
        ctx,
        AMPLIFY_SIGNING_NAME,
        region,
        GET_APP,
        "GET",
        &app_uri(app_id),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get(APP_MEMBER).cloned()),
        Err(refusal) if refusal.code == AMPLIFY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI one app is addressed at: the collection and the `appId` `@httpLabel`, escaped as a
/// single path segment ([`percent_encoded`]) so a value carrying URI structure addresses one
/// app rather than another path.
fn app_uri(app_id: &str) -> String {
    format!("{AMPLIFY_APPS_URI}/{}", percent_encoded(app_id))
}

/// The `appId` every call on the app takes, which is the `AppId` the create reported for it.
///
/// This resource's physical id is the app's ARN — the type's primary identifier — so the id
/// is read off the attribute that carries it, and a record that carries none falls back to
/// the id inside that ARN, which is its last path segment.
fn app_id_of(resource: &StoredResource) -> String {
    if let Some(app_id) = resource.attributes.get("AppId") {
        return app_id.clone();
    }
    resource
        .physical_resource_id
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The NAME the app is created under: the `Name` the declaration states, or the generated
/// `<stack>-<logical id>-<suffix>` name CloudFormation decided for a declaration that states
/// none — which is this resource's `AppName` once the create has answered.
fn app_name(declared: &Value, resource: &StoredResource) -> String {
    if let Some(name) = declared
        .get(NAME_PROPERTY)
        .and_then(scalar_text)
        .filter(|name| !name.is_empty())
    {
        return name;
    }
    resource
        .attributes
        .get("AppName")
        .cloned()
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// One declared property document as the request members AWS Amplify takes: every member name
/// re-cased ([`camel_cased`]), the three properties whose shape or name differs converted, and
/// the name and the tags left out — the caller adds the first and the create adds the second as
/// the map that call takes — so one reader serves the create and the update.
fn declared_app_members(declared: &Value) -> Result<Map<String, Value>, Refusal> {
    let declared = match declared.as_object() {
        Some(properties) => properties.clone(),
        // A declaration that is not an object states no member of the request.
        None => return Ok(Map::new()),
    };
    let mut request = Map::new();
    for (property, value) in &declared {
        if property == NAME_PROPERTY || property == TAGS_PROPERTY {
            continue;
        }
        match property.as_str() {
            ENVIRONMENT_VARIABLES_PROPERTY => {
                request.insert(
                    ENVIRONMENT_VARIABLES_MEMBER.to_string(),
                    Value::Object(name_value_map(value)),
                );
            }
            BASIC_AUTH_CONFIG_PROPERTY => {
                for (member, carried) in basic_auth_members(value) {
                    request.insert(member, carried);
                }
            }
            AUTO_BRANCH_CREATION_CONFIG_PROPERTY => {
                request.insert(
                    AUTO_BRANCH_CREATION_CONFIG_MEMBER.to_string(),
                    auto_branch_creation_members(value)?,
                );
            }
            _ => {
                let member = AMPLIFY_APP_RENAMED_PROPERTIES
                    .iter()
                    .find(|(renamed, _)| renamed == property)
                    .map(|(_, member)| (*member).to_string())
                    .unwrap_or_else(|| lower_initial(property));
                request.insert(member, camel_cased(value, 0)?);
            }
        }
    }
    Ok(request)
}

/// One `{Name, Value}` pair list as the MAP the owning API takes it as: a pair stating neither
/// as a scalar names no variable and contributes nothing.
fn name_value_map(declared: &Value) -> Map<String, Value> {
    let Some(pairs) = declared.as_array() else {
        return Map::new();
    };
    pairs
        .iter()
        .filter_map(|pair| {
            let name = pair.get("Name").and_then(scalar_text)?;
            let value = pair.get("Value").and_then(scalar_text)?;
            Some((name, Value::String(value)))
        })
        .collect()
}

/// One declared `BasicAuthConfig` as the member PAIR the owning API takes: the flag it states,
/// and the credentials that service takes "base64-encode[d] … in the format `user:password`"
/// where the declaration states both halves.
///
/// The codec is `aws_smithy_types::base64` — the standard-alphabet one the wire serializers of
/// this workspace already use, so nothing here hand-rolls an encoding.
fn basic_auth_members(declared: &Value) -> Vec<(String, Value)> {
    let mut members = Vec::new();
    if let Some(enabled) = declared.get(BASIC_AUTH_ENABLE_PROPERTY) {
        members.push((ENABLE_BASIC_AUTH_MEMBER.to_string(), enabled.clone()));
    }
    let credentials = declared
        .get(BASIC_AUTH_USERNAME_PROPERTY)
        .and_then(scalar_text)
        .zip(
            declared
                .get(BASIC_AUTH_PASSWORD_PROPERTY)
                .and_then(scalar_text),
        );
    if let Some((user, password)) = credentials {
        members.push((
            BASIC_AUTH_CREDENTIALS_MEMBER.to_string(),
            Value::String(::aws_smithy_types::base64::encode(format!(
                "{user}:{password}"
            ))),
        ));
    }
    members
}

/// One declared `AutoBranchCreationConfig` as that member's own structure: its members
/// re-cased, with the two it states the way the app itself does converted the same way
/// ([`name_value_map`], [`basic_auth_members`]).
fn auto_branch_creation_members(declared: &Value) -> Result<Value, Refusal> {
    let Some(properties) = declared.as_object() else {
        return Ok(declared.clone());
    };
    let mut converted = Map::new();
    for (property, value) in properties {
        match property.as_str() {
            ENVIRONMENT_VARIABLES_PROPERTY => {
                converted.insert(
                    ENVIRONMENT_VARIABLES_MEMBER.to_string(),
                    Value::Object(name_value_map(value)),
                );
            }
            BASIC_AUTH_CONFIG_PROPERTY => {
                for (member, carried) in basic_auth_members(value) {
                    converted.insert(member, carried);
                }
            }
            _ => {
                converted.insert(lower_initial(property), camel_cased(value, 0)?);
            }
        }
    }
    Ok(Value::Object(converted))
}

/// The attributes one answered app reports ([`AMPLIFY_APP_ATTRIBUTES`]).
///
/// An answer carrying no ARN is reported as a refusal rather than recorded as a created
/// resource: the ARN is this type's primary identifier, so there would be nothing for `Ref` to
/// resolve to and nothing for the stack's own deletion to act on.
fn app_attributes(answered: &Value) -> Result<BTreeMap<String, String>, Refusal> {
    let Some(app) = answered.get(APP_MEMBER) else {
        return Err(no_app_answered());
    };
    let attributes = reported_attributes(app, AMPLIFY_APP_ATTRIBUTES);
    if !attributes.contains_key(APP_ARN_ATTRIBUTE) {
        return Err(no_app_answered());
    }
    Ok(attributes)
}

/// The refusal an answer carrying no identified app is reported as.
fn no_app_answered() -> Refusal {
    Refusal {
        code: "InternalFailure".to_string(),
        message: "AWS Amplify answered a created app without an ARN".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a declared property is the API member of the same name with a lower-cased
    /// initial, except for the three this type states differently — the service role's member
    /// name, the environment variables' shape, and the basic-authorization pair.
    #[test]
    fn a_declaration_is_the_requests_members() {
        let declared = ::serde_json::json!({
            "Name": "a-name-the-caller-supplies",
            "Description": "an app",
            "Platform": "WEB_COMPUTE",
            "IAMServiceRole": "arn:aws:iam::000000000000:role/amplify",
            "EnvironmentVariables": [
                {"Name": "STAGE", "Value": "prod"},
                {"Name": "REGION", "Value": "us-east-1"}
            ],
            "BasicAuthConfig": {
                "EnableBasicAuth": true,
                "Username": "studio",
                "Password": "secret"
            },
            "CustomRules": [{"Source": "/<*>", "Target": "/index.html", "Status": "404-200"}],
            "Tags": [{"Key": "owner", "Value": "studio"}]
        });
        let request = declared_app_members(&declared).expect("the declaration is mapped");

        assert_eq!(
            request.get("iamServiceRoleArn").and_then(Value::as_str),
            Some("arn:aws:iam::000000000000:role/amplify"),
            "case \"the renamed service role\": expected the member the call takes, got \
             {request:?}"
        );
        assert_eq!(
            request.get(ENVIRONMENT_VARIABLES_MEMBER),
            Some(&::serde_json::json!({"STAGE": "prod", "REGION": "us-east-1"})),
            "case \"the environment variables\": expected the pairs as a map, got {request:?}"
        );
        assert_eq!(
            request.get(ENABLE_BASIC_AUTH_MEMBER),
            Some(&Value::Bool(true)),
            "case \"basic authorization\": expected the flag the call takes, got {request:?}"
        );
        assert_eq!(
            request
                .get(BASIC_AUTH_CREDENTIALS_MEMBER)
                .and_then(Value::as_str),
            // `studio:secret` base64-encoded, which is the form that member documents.
            Some("c3R1ZGlvOnNlY3JldA=="),
            "case \"basic authorization\": expected base64 user:password, got {request:?}"
        );
        assert_eq!(
            request.get("customRules"),
            Some(&::serde_json::json!([
                {"source": "/<*>", "target": "/index.html", "status": "404-200"}
            ])),
            "case \"a list of structures\": expected every member re-cased, got {request:?}"
        );
        for member in [NAME_MEMBER, TAGS_MEMBER] {
            assert!(
                !request.contains_key(member),
                "case \"what the caller adds itself\": expected no {member}, got {request:?}"
            );
        }
    }
}
