// SPDX-License-Identifier: CC0-1.0

//! `AWS::AppConfig::Application`, `AWS::AppConfig::Environment`,
//! `AWS::AppConfig::ConfigurationProfile` and `AWS::AppConfig::DeploymentStrategy`: the
//! AppConfig estate a stack declares, created in AWS AppConfig itself.
//!
//! This is the per-type half of provisioning for four resource types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::logs_resources`] for the reason that module records —
//! the catalogue file is far past the length a change to this repository may touch — and is
//! reached through the same seam ([`super::Owner`]).
//!
//! ## Why the resources are really created
//!
//! AppConfig identifies an application, an environment, a configuration profile and a
//! deployment strategy by an id IT mints, modelled `Id` with pattern `^[a-z0-9]{4,7}$`
//! (`smithy_query(query="operation",operation="CreateApplication")` on that service's model).
//! Every later call addresses the resource by that id: `CreateHostedConfigurationVersion` and
//! `StartDeployment` take an `ApplicationId`, a `ConfigurationProfileId`, an `EnvironmentId`
//! and a `DeploymentStrategyId`, and each answers `ResourceNotFoundException` for an
//! identifier that service holds nothing under. So a stack that reported an AppConfig
//! application created while nothing created it publishes, through `Ref` and its outputs, a
//! value no AppConfig call resolves — and every follow-on call a client makes with it is
//! refused even though the stack reported success. All four types are therefore provisioned
//! here rather than left declared-only, and the id each create answers with becomes the
//! resource's physical id ([`crate::state::template::identity_is_service_assigned`] names the
//! four), which is what `Ref` returns and what `Fn::GetAtt ApplicationId` /
//! `EnvironmentId` / `ConfigurationProfileId` / `Id` report.
//!
//! ## The properties
//!
//! Every property these four types publish is a member of the owning create under the SAME
//! name: AppConfig's model spells its members in upper camel exactly as the resource types
//! spell their properties (`CreateConfigurationProfileRequest$LocationUri` against
//! `AWS::AppConfig::ConfigurationProfile`'s `LocationUri`, `CreateDeploymentStrategyRequest$
//! GrowthFactor` against that type's `GrowthFactor`), so a declared property travels
//! verbatim. Three are not carried that way:
//!
//! * `Name` is the name CloudFormation decided for the resource — the one the declaration
//!   asked for, or the one the naming rules generated — so the create is made under it
//!   rather than under whatever the property document happens to hold;
//! * `ApplicationId` ADDRESSES the call rather than travelling in it: it is the
//!   `@httpLabel` of `CreateEnvironment` and `CreateConfigurationProfile`;
//! * `Tags` is the `[{Key, Value}]` list a template writes against the `TagMap` AppConfig's
//!   creates take, so it is rendered as that map ([`appconfig_tags`]).
//!
//! A declared property none of those calls carries is REFUSED rather than dropped
//! ([`refuse_unapplied_properties`]): a resource reported created with less than its
//! declaration asked for is the shallow success these arms exist to avoid.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{applicable_tags, call_rest_json, percent_encoded, Created, Readiness, Refusal, Removal};
use crate::state::provisioner::{attribute_text, tags_as_map, tags_of};
use crate::state::template::{
    APPCONFIG_APPLICATION, APPCONFIG_CONFIGURATION_PROFILE, APPCONFIG_DEPLOYMENT_STRATEGY,
    APPCONFIG_ENVIRONMENT,
};
use crate::state::StoredResource;

/// The SigV4 signing name of the service that owns these types, which is what routes the
/// in-process call to AWS AppConfig's own handlers (that model's `sigv4Name`/
/// `endpointPrefix`, `smithy_query(query="service_info")`). It speaks `restJson1`, so every
/// call below rides [`call_rest_json`] on the URI its own operation's `@http` binding
/// declares.
const APPCONFIG_SIGNING_NAME: &str = "appconfig";

/// The error code AppConfig answers for a resource it does not hold
/// (`ResourceNotFoundException`, the modelled refusal of its reads, writes and deletes),
/// which is what says a delete has nothing left to remove and a read has nothing to report.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a refusal of this module's own carries for a declaration it cannot act on: the
/// same one the catalogue's arms use for a property this emulator does not apply.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property, or
/// states one as something the member cannot carry.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// The code a create that answered 2xx without the identifier it assigns is reported under.
const UNIDENTIFIED_FAILURE: &str = "InternalFailure";

/// Which of the four types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`]: the algorithm resolves a type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::AppConfig::Application`.
    Application,
    /// `AWS::AppConfig::Environment`.
    Environment,
    /// `AWS::AppConfig::ConfigurationProfile`.
    ConfigurationProfile,
    /// `AWS::AppConfig::DeploymentStrategy`.
    DeploymentStrategy,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        APPCONFIG_APPLICATION => Some(Kind::Application),
        APPCONFIG_ENVIRONMENT => Some(Kind::Environment),
        APPCONFIG_CONFIGURATION_PROFILE => Some(Kind::ConfigurationProfile),
        APPCONFIG_DEPLOYMENT_STRATEGY => Some(Kind::DeploymentStrategy),
        _ => None,
    }
}

/// The property naming the application an environment and a configuration profile belong to,
/// which addresses the call rather than travelling in its body.
const APPLICATION_ID: &str = "ApplicationId";

/// The property every one of these types is created under, which carries the name
/// CloudFormation decided rather than the declaration's own text.
const NAME: &str = "Name";

/// The property carrying a resource's tags, which reaches each create as a map.
const TAGS: &str = "Tags";

/// Every property `AWS::AppConfig::Application` publishes.
const APPLICATION_PROPERTIES: &[&str] = &["Description", NAME, TAGS];

/// Every property `AWS::AppConfig::Environment` publishes.
const ENVIRONMENT_PROPERTIES: &[&str] = &[APPLICATION_ID, "Description", "Monitors", NAME, TAGS];

/// Every property `AWS::AppConfig::ConfigurationProfile` publishes.
const CONFIGURATION_PROFILE_PROPERTIES: &[&str] = &[
    APPLICATION_ID,
    "Description",
    "KmsKeyIdentifier",
    "LocationUri",
    NAME,
    "RetrievalRoleArn",
    TAGS,
    "Type",
    "Validators",
];

/// Every property `AWS::AppConfig::DeploymentStrategy` publishes.
const DEPLOYMENT_STRATEGY_PROPERTIES: &[&str] = &[
    "DeploymentDurationInMinutes",
    "Description",
    "FinalBakeTimeInMinutes",
    "GrowthFactor",
    "GrowthType",
    NAME,
    "ReplicateTo",
    TAGS,
];

/// The members of a deployment strategy AppConfig models as a NUMBER rather than as text:
/// `DeploymentDurationInMinutes` and `FinalBakeTimeInMinutes` are integers and
/// `GrowthFactor` a float (that model's `MinutesBetween0And24Hours` and `GrowthFactor`), so a
/// declaration that writes one as the text a parameter reference resolves to is sent as the
/// number it spells.
const DEPLOYMENT_STRATEGY_NUMBERS: &[&str] = &[
    "DeploymentDurationInMinutes",
    "FinalBakeTimeInMinutes",
    "GrowthFactor",
];

/// Create the resource one of these types declares, in AWS AppConfig itself — so a client
/// that deployed the stack can pass the stack's own `ApplicationId` output straight to
/// `CreateHostedConfigurationVersion` and read every resource back with that service's own
/// reads.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::Application => create_application(ctx, region, resource, propagated).await,
        Kind::Environment => create_environment(ctx, region, resource, propagated).await,
        Kind::ConfigurationProfile => {
            create_configuration_profile(ctx, region, resource, propagated).await
        }
        Kind::DeploymentStrategy => {
            create_deployment_strategy(ctx, region, resource, propagated).await
        }
    }
}

/// Create the application an `AWS::AppConfig::Application` declares.
///
/// "Ref returns the application ID", which AppConfig assigns: `CreateApplication` takes no
/// identifier — only the name — and answers the `Id` it minted, so that id is this
/// resource's physical id.
async fn create_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, APPLICATION_PROPERTIES)?;
    let mut request = Map::new();
    named(&mut request, resource);
    carry(&mut request, &resource.properties, &["Description"]);
    tagged(&mut request, &resource.properties, propagated);
    let answered = call_appconfig(
        ctx,
        region,
        "POST",
        APPLICATIONS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    minted(&answered, "CreateApplication")
}

/// Create the environment an `AWS::AppConfig::Environment` declares, under the application
/// its `ApplicationId` names — so a deployment into the environment the template declared
/// really has one to address.
///
/// `Fn::GetAtt EnvironmentId` is the id AppConfig assigned, which `CreateEnvironment` answers
/// under `Id`: the create takes the application in its URI and no identifier for the
/// environment itself.
async fn create_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, ENVIRONMENT_PROPERTIES)?;
    let mut request = Map::new();
    named(&mut request, resource);
    carry(&mut request, &resource.properties, &["Description", "Monitors"]);
    tagged(&mut request, &resource.properties, propagated);
    let uri = format!(
        "{}/environments",
        application_uri(&required(resource, &resource.properties, APPLICATION_ID)?)
    );
    let answered =
        call_appconfig(ctx, region, "POST", &uri, Some(&Value::Object(request))).await?;
    minted(&answered, "CreateEnvironment")
}

/// Create the configuration profile an `AWS::AppConfig::ConfigurationProfile` declares, under
/// the application its `ApplicationId` names — so a hosted configuration version of the
/// profile the template declared really has one to address.
///
/// `Fn::GetAtt ConfigurationProfileId` is the id AppConfig assigned, answered under `Id`, and
/// `Fn::GetAtt KmsKeyArn` is the ARN that service reports for the key the profile encrypts
/// with, which only the create answers.
async fn create_configuration_profile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        resource,
        CONFIGURATION_PROFILE_PROPERTIES,
    )?;
    let mut request = Map::new();
    named(&mut request, resource);
    request.insert(
        "LocationUri".to_string(),
        Value::String(required(resource, &resource.properties, "LocationUri")?),
    );
    carry(
        &mut request,
        &resource.properties,
        &[
            "Description",
            "KmsKeyIdentifier",
            "RetrievalRoleArn",
            "Type",
            "Validators",
        ],
    );
    tagged(&mut request, &resource.properties, propagated);
    let uri = format!(
        "{}/configurationprofiles",
        application_uri(&required(resource, &resource.properties, APPLICATION_ID)?)
    );
    let answered =
        call_appconfig(ctx, region, "POST", &uri, Some(&Value::Object(request))).await?;
    let mut created = minted(&answered, "CreateConfigurationProfile")?;
    if let Some(key_arn) = answered.get("KmsKeyArn").and_then(Value::as_str) {
        created
            .attributes
            .insert("KmsKeyArn".to_string(), key_arn.to_string());
    }
    Ok(created)
}

/// Create the deployment strategy an `AWS::AppConfig::DeploymentStrategy` declares.
///
/// "Ref returns the deployment strategy ID": `CreateDeploymentStrategy` takes no identifier
/// and answers the `Id` AppConfig minted, which is also the `DeploymentStrategyId` every
/// `StartDeployment` takes.
async fn create_deployment_strategy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        resource,
        DEPLOYMENT_STRATEGY_PROPERTIES,
    )?;
    let mut request = Map::new();
    named(&mut request, resource);
    // Both are required members of the create, so a declaration stating neither describes no
    // strategy that could be made: it is refused here, naming the property, rather than sent
    // as a request the owning service would reject.
    for property in ["DeploymentDurationInMinutes", "GrowthFactor"] {
        if resource
            .properties
            .get(property)
            .is_none_or(Value::is_null)
        {
            return Err(missing(resource, property));
        }
    }
    carry(
        &mut request,
        &resource.properties,
        &["Description", "GrowthType", "ReplicateTo"],
    );
    carry_numbers(
        &mut request,
        &resource.properties,
        resource,
        DEPLOYMENT_STRATEGY_NUMBERS,
    )?;
    tagged(&mut request, &resource.properties, propagated);
    let answered = call_appconfig(
        ctx,
        region,
        "POST",
        DEPLOYMENT_STRATEGIES_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    minted(&answered, "CreateDeploymentStrategy")
}

/// Whether the owning service reports the created resource ready.
///
/// Nothing is asked, and nothing waits: each of these resources EXISTS from the moment its
/// create answers — an application can be given environments and profiles, a profile can take
/// a hosted configuration version, a strategy can be deployed with — and a brand-new
/// environment is answered in the only state AppConfig reports one ready for deployment in.
/// None of the four creates reports a non-terminal state for the resource to settle out of.
pub(super) fn ready() -> Readiness {
    Readiness::Ready
}

/// Whether the owning service has finished removing a deleted resource.
///
/// Nothing is asked: each of `DeleteApplication`, `DeleteEnvironment`,
/// `DeleteConfigurationProfile` and `DeleteDeploymentStrategy` takes the resource away as it
/// answers, and AppConfig reports no state a removed one is still being deleted in.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// Apply a CHANGED declaration to the resource the stack already has, through the owning
/// service's own update call.
///
/// AppConfig publishes one per type, each addressing the resource by the id it minted and
/// taking the members that CAN change on the resource that exists — so the resource keeps its
/// identity and the declaration is applied to it. A member the declaration no longer states is
/// carried as the value the API takes to mean none ([`cleared_member`]), which is what takes a
/// description, a monitor list or a validator list away.
///
/// `Ok(None)` is a change no update call can make ([`create_only`]): such a deployment
/// reports the change as not applied rather than as a success that dropped the caller's
/// change. Those properties are the ones CloudFormation REPLACES a resource over
/// ([`crate::state::template::replacement_properties`] names them, and the name a resource is
/// created under is in that position for every type here), so a deployment that restates one
/// replaces the resource rather than reaching this arm at all.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unapplied_properties(declared, resource, properties_of(kind))?;
    for property in create_only(kind) {
        if declared.get(*property) != resource.properties.get(*property) {
            return Ok(None);
        }
    }
    // The update of each of these types is bound to the SAME URI its own read is, so the
    // resource it changes is the one a drift read reports on ([`addressed`]).
    let uri = addressed(kind, resource, false)?;
    let (changeable, numbers) = match kind {
        Kind::Application => (&["Description"][..], &[][..]),
        Kind::Environment => (&["Description", "Monitors"][..], &[][..]),
        Kind::ConfigurationProfile => (
            &[
                "Description",
                "KmsKeyIdentifier",
                "RetrievalRoleArn",
                "Validators",
            ][..],
            &[][..],
        ),
        Kind::DeploymentStrategy => (
            &["Description", "GrowthType"][..],
            DEPLOYMENT_STRATEGY_NUMBERS,
        ),
    };
    let mut request = Map::new();
    for property in changeable {
        match declared.get(*property) {
            Some(value) if !value.is_null() => {
                request.insert((*property).to_string(), value.clone());
            }
            // Stated before and no longer: the member carries the value the API takes to mean
            // none, so what the resource holds matches the declaration in force.
            _ if resource
                .properties
                .get(*property)
                .is_some_and(|held| !held.is_null()) =>
            {
                if let Some(empty) = cleared_member(property) {
                    request.insert((*property).to_string(), empty);
                }
            }
            _ => {}
        }
    }
    carry_numbers(&mut request, declared, resource, numbers)?;
    if request.is_empty() {
        // Nothing this call carries is stated differently, so the resource is already as the
        // declaration asks: it keeps its identity and no call is made about it.
        return Ok(Some(Created::attributes(BTreeMap::new())));
    }
    let answered =
        call_appconfig(ctx, region, "PATCH", &uri, Some(&Value::Object(request))).await?;
    let mut attributes = BTreeMap::new();
    if let Some(key_arn) = answered.get("KmsKeyArn").and_then(Value::as_str) {
        attributes.insert("KmsKeyArn".to_string(), key_arn.to_string());
    }
    Ok(Some(Created::attributes(attributes)))
}

/// Delete the resource a stack created.
///
/// Each delete addresses the resource by the id the create answered with, as that type's own
/// read does — an environment and a profile by their application and themselves, an
/// application and a strategy by themselves. A resource the owning service no longer holds is
/// already in the state the delete asks for, judged on the CODE and never on a message —
/// which is also what makes deleting an environment or a profile whose APPLICATION has
/// already gone a completed deletion rather than a failure.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    // `DeleteDeploymentStrategy` binds a URI of its own, so the delete is addressed as that
    // operation binds it rather than as the read is.
    let uri = addressed(kind, resource, true)?;
    match call_appconfig(ctx, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The resource's configuration as the owning service reports it, for drift detection: that
/// resource's own read, under this type's property names ([`reported_properties`]).
///
/// `None` is a resource the owning service no longer holds, which is a deleted one.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = addressed(kind, resource, false)?;
    let answered = match call_appconfig(ctx, region, "GET", &uri, None).await {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let mut held = Map::new();
    for property in reported_properties(kind) {
        if let Some(value) = answered.get(*property) {
            held.insert((*property).to_string(), value.clone());
        }
    }
    Ok(Some(Value::Object(held)))
}

/// The collection every application of the account lives in, which `CreateApplication` is
/// bound to.
const APPLICATIONS_URI: &str = "/applications";

/// The collection every deployment strategy of the account lives in, which
/// `CreateDeploymentStrategy` is bound to.
const DEPLOYMENT_STRATEGIES_URI: &str = "/deploymentstrategies";

/// The URI that addresses ONE application: the `@httpLabel` every call on it binds.
fn application_uri(application_id: &str) -> String {
    format!("{APPLICATIONS_URI}/{}", percent_encoded(application_id))
}

/// The URI that addresses ONE deployment strategy for a READ or an UPDATE
/// (`GetDeploymentStrategy`, `UpdateDeploymentStrategy`: `/deploymentstrategies/{Id}`).
fn deployment_strategy_uri(strategy_id: &str) -> String {
    format!(
        "{DEPLOYMENT_STRATEGIES_URI}/{}",
        percent_encoded(strategy_id)
    )
}

/// The URI `DeleteDeploymentStrategy` is bound to, which is NOT the one its read and its
/// update are: that operation's `@http` URI is `/deployementstrategies/{DeploymentStrategyId}`
/// in AppConfig's own model, so a delete written on the collection above addresses nothing.
const DELETE_DEPLOYMENT_STRATEGY_URI: &str = "/deployementstrategies";

/// The URI that addresses ONE resource of `kind`, for the calls that read, change or delete
/// it — the one place each type's addressing is written, so a delete and a drift read cannot
/// come to address different resources.
///
/// `for_delete` is the one difference between them: `DeleteDeploymentStrategy` binds a URI of
/// its own ([`DELETE_DEPLOYMENT_STRATEGY_URI`]).
fn addressed(kind: Kind, resource: &StoredResource, for_delete: bool) -> Result<String, Refusal> {
    let id = &resource.physical_resource_id;
    let uri = match kind {
        Kind::Application => application_uri(id),
        Kind::Environment => format!(
            "{}/environments/{}",
            application_uri(&required(resource, &resource.properties, APPLICATION_ID)?),
            percent_encoded(id)
        ),
        Kind::ConfigurationProfile => format!(
            "{}/configurationprofiles/{}",
            application_uri(&required(resource, &resource.properties, APPLICATION_ID)?),
            percent_encoded(id)
        ),
        Kind::DeploymentStrategy if for_delete => {
            format!(
                "{DELETE_DEPLOYMENT_STRATEGY_URI}/{}",
                percent_encoded(id)
            )
        }
        Kind::DeploymentStrategy => deployment_strategy_uri(id),
    };
    Ok(uri)
}

/// Every property the type of `kind` publishes, for the one check that refuses a declared
/// property no call this module makes carries.
fn properties_of(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Application => APPLICATION_PROPERTIES,
        Kind::Environment => ENVIRONMENT_PROPERTIES,
        Kind::ConfigurationProfile => CONFIGURATION_PROFILE_PROPERTIES,
        Kind::DeploymentStrategy => DEPLOYMENT_STRATEGY_PROPERTIES,
    }
}

/// The properties of `kind` no update call carries, so a declaration restating one asks for a
/// resource this arm cannot produce.
///
/// Each is a member of the type's create and of none of its update: AppConfig identifies an
/// environment and a profile by the application they belong to, fixes where a profile reads
/// its configuration from (`CreateConfigurationProfile`'s `LocationUri` and `Type`, neither a
/// member of `UpdateConfigurationProfile`) and fixes where a strategy replicates to
/// (`ReplicateTo`, no member of `UpdateDeploymentStrategy`). The NAME a resource is created
/// under is in that position for every type here, as it is for every type whose name
/// CloudFormation treats as create-only
/// ([`crate::state::template::replacing_properties`] is the one reading of that).
fn create_only(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Application => &[NAME],
        Kind::Environment => &[APPLICATION_ID, NAME],
        Kind::ConfigurationProfile => &[APPLICATION_ID, "LocationUri", NAME, "Type"],
        Kind::DeploymentStrategy => &[NAME, "ReplicateTo"],
    }
}

/// The members of a resource's own read that are COMPARED against its declaration, each named
/// identically on both sides. A member that is part of the resource's identity — the ids the
/// read answers — is left out: it is not something a declaration states.
fn reported_properties(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Application => &[NAME, "Description"],
        Kind::Environment => &[NAME, "Description", "Monitors"],
        Kind::ConfigurationProfile => &[
            NAME,
            "Description",
            "LocationUri",
            "RetrievalRoleArn",
            "Type",
            "KmsKeyIdentifier",
            "Validators",
        ],
        Kind::DeploymentStrategy => &[
            NAME,
            "Description",
            "DeploymentDurationInMinutes",
            "FinalBakeTimeInMinutes",
            "GrowthFactor",
            "GrowthType",
            "ReplicateTo",
        ],
    }
}

/// What a member carries for a declaration that no longer states its property: the value the
/// owning API takes to mean NONE.
///
/// A description and a KMS key identifier are cleared with the empty string — `Description` is
/// `@length(min: 0, …)` and `UpdateConfigurationProfile$KmsKeyIdentifier` targets
/// `KmsKeyIdentifierOrEmpty`, which that minimum is there for — and a monitor or validator
/// list with the empty list. A member with no such value is left out rather than sent, since
/// omitting it is what leaves the resource as it is.
fn cleared_member(property: &str) -> Option<Value> {
    match property {
        "Description" | "KmsKeyIdentifier" => Some(Value::String(String::new())),
        "Monitors" | "Validators" => Some(Value::Array(Vec::new())),
        _ => None,
    }
}

/// Put the name CloudFormation decided for the resource on the request, which is the name the
/// create is made under.
fn named(request: &mut Map<String, Value>, resource: &StoredResource) {
    request.insert(
        NAME.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
}

/// Copy the declared properties `carried` onto the request under the member names the owning
/// create spells them with — the SAME names, since AppConfig's model spells every one of them
/// exactly as the resource type spells its property. A property the declaration does not
/// state is left out rather than sent empty, so that service's own validation judges the
/// request it would judge for a client's own call.
fn carry(request: &mut Map<String, Value>, properties: &Value, carried: &[&str]) {
    for property in carried {
        match properties.get(*property) {
            Some(Value::Null) | None => {}
            Some(value) => {
                request.insert((*property).to_string(), value.clone());
            }
        }
    }
}

/// [`carry`] for the members AppConfig models as a NUMBER: a declaration that writes one as
/// the text a parameter reference resolves to is sent as the number it spells, and a value
/// that is no number at all is refused here — naming the property — rather than sent as a
/// request that member cannot carry.
fn carry_numbers(
    request: &mut Map<String, Value>,
    properties: &Value,
    resource: &StoredResource,
    members: &[&str],
) -> Result<(), Refusal> {
    for property in members {
        match properties.get(*property) {
            Some(Value::Null) | None => {}
            Some(value) => {
                let number = declared_number(value).ok_or_else(|| Refusal {
                    code: MISSING_PROPERTY.to_string(),
                    message: format!(
                        "the {property} property of {} is a number, and {value} is not one",
                        resource.resource_type
                    ),
                })?;
                request.insert((*property).to_string(), number);
            }
        }
    }
    Ok(())
}

/// A declared number in either spelling a template gives one: JSON's own, or the text a
/// template spells one as. An integral value is carried as an integer, so a member modelled as
/// one is not handed a fractional rendering of it.
fn declared_number(value: &Value) -> Option<Value> {
    match value {
        Value::Number(_) => Some(value.clone()),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(whole) = text.parse::<i64>() {
                return Some(Value::Number(whole.into()));
            }
            text.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
        }
        _ => None,
    }
}

/// Put the tags CloudFormation gives the resource on the request, as the `TagMap` each of
/// these creates takes.
///
/// The resource carries the tags its declaration asks for plus the ones CloudFormation gives
/// it, less the keys in AWS's reserved namespace: AppConfig refuses such a key on every call
/// that takes a caller's tag set, and the three tags CloudFormation records of its own accord
/// have no public call to set them through — so they are left off rather than sent to be
/// refused, since a deployment must not fail over tags CloudFormation adds
/// ([`applicable_tags`], the one place that rule is written).
fn tagged(request: &mut Map<String, Value>, properties: &Value, propagated: &[(String, String)]) {
    let tags = appconfig_tags(properties, propagated);
    if !tags.is_empty() {
        request.insert(TAGS.to_string(), Value::Object(tags));
    }
}

/// Those of the tags CloudFormation gives one of these resources that AppConfig's own creates
/// will TAKE, as the map they take them in.
fn appconfig_tags(properties: &Value, propagated: &[(String, String)]) -> Map<String, Value> {
    tags_as_map(&tags_of(properties, &applicable_tags(propagated)))
}

/// What a create answered: the identifier AppConfig minted for the resource, as the physical
/// id that replaces the name CloudFormation computed.
///
/// A create that answered 2xx WITHOUT an `Id` identifies no resource, so the resource is
/// reported as failed rather than recorded under an id nothing answers to.
fn minted(answered: &Value, operation: &str) -> Result<Created, Refusal> {
    let id = answered
        .get("Id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("AppConfig answered {operation} without an Id"),
        })?;
    Ok(Created {
        physical_resource_id: Some(id.to_string()),
        attributes: BTreeMap::new(),
    })
}

/// One property a call cannot be addressed without, as the text it carries, or the refusal for
/// a declaration that states none.
fn required(
    resource: &StoredResource,
    properties: &Value,
    property: &str,
) -> Result<String, Refusal> {
    match properties
        .get(property)
        .filter(|value| !value.is_null())
        .map(attribute_text)
    {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(missing(resource, property)),
    }
}

/// The refusal for a declaration that states none of a property its type requires.
fn missing(resource: &StoredResource, property: &str) -> Refusal {
    Refusal {
        code: MISSING_PROPERTY.to_string(),
        message: format!("{} requires the {property} property", resource.resource_type),
    }
}

/// The refusal for a declared property no call this module makes carries, naming which
/// property it was: refusing is what keeps a resource from being reported as created — or as
/// updated — with less than its declaration asked for. `properties` is the document the
/// deployment DECLARES, which on an update is not the one the record carries.
fn refuse_unapplied_properties(
    properties: &Value,
    resource: &StoredResource,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if published.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

/// Call one AWS AppConfig operation on the `restJson1` wire its model declares, on the URI
/// that operation's own `@http` binding states.
async fn call_appconfig(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&Value>,
) -> Result<Value, Refusal> {
    call_rest_json(ctx, APPCONFIG_SIGNING_NAME, region, method, uri, body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `StoredResource` of one of these types, as a deployment hands it to the arms above.
    fn declared(resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Subject".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: "a-name".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// The four type names this module owns are exactly the ones it claims, and nothing else:
    /// a near miss of one is another type's, which this module must not answer for.
    #[test]
    fn the_owned_types_are_the_four_appconfig_types() {
        let cases: &[(&str, Option<Kind>)] = &[
            (APPCONFIG_APPLICATION, Some(Kind::Application)),
            (APPCONFIG_ENVIRONMENT, Some(Kind::Environment)),
            (
                APPCONFIG_CONFIGURATION_PROFILE,
                Some(Kind::ConfigurationProfile),
            ),
            (
                APPCONFIG_DEPLOYMENT_STRATEGY,
                Some(Kind::DeploymentStrategy),
            ),
            ("AWS::AppConfig::HostedConfigurationVersion", None),
            ("AWS::AppConfig::Deployment", None),
            ("AWS::AppConfig::Extension", None),
            ("AWS::SQS::Queue", None),
            ("", None),
        ];
        for (resource_type, expected) in cases {
            let actual = kind_of(resource_type);
            assert_eq!(
                actual, *expected,
                "case {resource_type:?}: expected kind_of {expected:?}, got {actual:?}"
            );
        }
    }

    /// Each type's delete and drift read address the resource exactly as its own operation's
    /// `@http` binding does — including `DeleteDeploymentStrategy`, whose URI segment is NOT
    /// the one its read binds.
    #[test]
    fn a_resource_is_addressed_as_its_own_operation_binds_it() {
        let cases: &[(&str, Kind, Value, bool, &str)] = &[
            (
                "an application",
                Kind::Application,
                serde_json::json!({}),
                false,
                "/applications/a-name",
            ),
            (
                "an environment",
                Kind::Environment,
                serde_json::json!({ "ApplicationId": "abc1234" }),
                false,
                "/applications/abc1234/environments/a-name",
            ),
            (
                "a configuration profile",
                Kind::ConfigurationProfile,
                serde_json::json!({ "ApplicationId": "abc1234" }),
                false,
                "/applications/abc1234/configurationprofiles/a-name",
            ),
            (
                "a deployment strategy read",
                Kind::DeploymentStrategy,
                serde_json::json!({}),
                false,
                "/deploymentstrategies/a-name",
            ),
            (
                "a deployment strategy delete",
                Kind::DeploymentStrategy,
                serde_json::json!({}),
                true,
                "/deployementstrategies/a-name",
            ),
        ];
        for (case, kind, properties, for_delete, expected) in cases {
            let resource = declared(APPCONFIG_APPLICATION, properties.clone());
            let actual = addressed(*kind, &resource, *for_delete)
                .unwrap_or_else(|refusal| panic!("case {case:?}: expected a URI, got {refusal}"));
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected the URI {expected:?}, got {actual:?}"
            );
        }
    }

    /// A child whose declaration names no application describes no resource that could be
    /// created, so it is refused here naming that property rather than sent as a request the
    /// owning service would reject.
    #[test]
    fn a_child_without_an_application_is_refused() {
        for (case, kind, resource_type) in [
            ("an environment", Kind::Environment, APPCONFIG_ENVIRONMENT),
            (
                "a configuration profile",
                Kind::ConfigurationProfile,
                APPCONFIG_CONFIGURATION_PROFILE,
            ),
        ] {
            let resource = declared(resource_type, serde_json::json!({ "Name": "a-name" }));
            let refusal = addressed(kind, &resource, false)
                .expect_err(&format!("case {case:?}: expected a refusal"));
            assert!(
                refusal.message.contains(APPLICATION_ID)
                    && refusal.message.contains(resource_type),
                "case {case:?}: expected the refusal to name the property and the type, got \
                 {:?}",
                refusal.message
            );
        }
    }

    /// A number a template writes as text is the same number as one written as JSON's own, an
    /// integral value stays an integer, and a value that is no number at all is none.
    #[test]
    fn a_number_is_read_in_either_spelling() {
        let cases: &[(&str, Value, Option<Value>)] = &[
            (
                "a JSON integer",
                serde_json::json!(100),
                Some(serde_json::json!(100)),
            ),
            (
                "the text of one",
                serde_json::json!("100"),
                Some(serde_json::json!(100)),
            ),
            (
                "text with spaces",
                serde_json::json!(" 0 "),
                Some(serde_json::json!(0)),
            ),
            (
                "a fractional growth factor",
                serde_json::json!("12.5"),
                Some(serde_json::json!(12.5)),
            ),
            ("a word", serde_json::json!("linear"), None),
            ("a boolean", serde_json::json!(true), None),
        ];
        for (case, value, expected) in cases {
            let actual = declared_number(value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected declared_number {expected:?}, got {actual:?}"
            );
        }
    }

    /// A property no call this module makes carries is refused, naming it, so nothing is
    /// reported created with less than the declaration asked for — and every property each
    /// type publishes is accepted.
    #[test]
    fn an_unapplied_property_is_refused() {
        let resource = declared(APPCONFIG_CONFIGURATION_PROFILE, Value::Null);
        let published = serde_json::json!({
            "ApplicationId": "abc1234",
            "Name": "a-profile",
            "Description": "the profile",
            "LocationUri": "hosted",
            "RetrievalRoleArn": "arn:aws:iam::000000000000:role/a-role",
            "Type": "AWS.Freeform",
            "KmsKeyIdentifier": "a-key",
            "Validators": [],
            "Tags": []
        });
        assert!(
            refuse_unapplied_properties(&published, &resource, CONFIGURATION_PROFILE_PROPERTIES)
                .is_ok(),
            "case \"every published property\": expected it to be applied, got a refusal"
        );
        let unknown = serde_json::json!({ "Name": "a-profile", "DeletionProtectionCheck": "APPLY" });
        let refusal =
            refuse_unapplied_properties(&unknown, &resource, CONFIGURATION_PROFILE_PROPERTIES)
                .expect_err("case \"a property no call carries\": expected a refusal");
        assert!(
            refusal.message.contains("DeletionProtectionCheck"),
            "case \"a property no call carries\": expected the refusal to name it, got {:?}",
            refusal.message
        );
    }
}
