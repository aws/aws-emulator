// SPDX-License-Identifier: CC0-1.0

//! Extension action-point dispatch: the seam that makes an extension association do
//! something rather than sit in state as metadata.
//!
//! An extension declares ACTIONS, each bound to one of the model's `ActionPoint` values,
//! and an extension association binds one version of one extension to one application,
//! environment or configuration profile. The action point is a point in AppConfig's OWN
//! workflow: when a call (or the deployment the call started) reaches it, every action
//! bound there by an extension associated to the resource the workflow is running on is
//! invoked at the `Uri` the action names, and what the target answers is applied.
//!
//! That gives three rules, which every caller of this module implements:
//!
//! * the actions bound at an action point are those of the associations bound to the
//!   application, the environment or the configuration profile the workflow is running on
//!   ([`association_binds`]) — the same binding `AppliedExtensions` reports;
//! * the target is handed a JSON event naming the action point in PascalCase (`Type`),
//!   the resources it is running on, the configuration content base64-encoded, the
//!   `ContentVersion` as a STRING, the association's parameter values and a
//!   system-generated `InvocationId` ([`ActionEvent`]);
//! * an action point the service consults SYNCHRONOUSLY before performing the call
//!   ([`consulted_before_the_call`]) gates it: a failed invocation refuses the calling
//!   operation ([`Failure::refuses_the_call`]) and persists nothing, and an answer
//!   carrying `Content` replaces the configuration bytes the call was about to store. An
//!   action point of a deployment's own lifecycle does not gate anything, but an
//!   `AT_DEPLOYMENT_TICK` answer carrying the [`DIRECTIVE_ROLL_BACK`] directive rolls the
//!   deployment back.
//!
//! The `Uri` "can be an Amazon Resource Name (ARN) for one of the following: an Lambda
//! function, an Amazon Simple Queue Service queue, an Amazon Simple Notification Service
//! topic, or the Amazon EventBridge default event bus"
//! (`smithy_query(query="doc",shape="Action",member="Uri")`). All four are invoked here,
//! each through its own service's API over the in-process callback, and only a function
//! can ANSWER: a queue, a topic and a bus accept the event and return nothing, so an
//! action on one of them can neither veto a call nor transform what it stores.
//!
//! A target is reached through [`TargetKind`], which admits ONLY an ARN naming one of
//! those four resources, and the invocation is made with a client bound to that service's
//! endpoint over `OperationContext::callback` — the transport back into this same
//! emulator. So the destination of an action's invocation is always a service this
//! emulator itself serves: a `Uri` carrying a URL, a host name or an ARN of any other
//! resource reaches nothing, and no socket leaves the machine. How many invocations one
//! call can make is bounded by the association store's own cap
//! (`crate::state::MAX_EXTENSION_ASSOCIATIONS`), each association binding at most one
//! action per point (the model's `ActionList`, `length 1..1`), and the work one report of
//! a deployment can drive by [`MAX_DRIVE_ROUNDS`].

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use ::base64::Engine as _;
use ::serde_json::{Map, Value};

use crate::ops::types::ActionInvocation;

use super::{ComAmazonawsAppconfigAmazonappconfigState as ServiceStore, Deployment, TaggedResource};

/// The action points of the model's `ActionPoint` enum (`ACTION_POINT_VALUES` in
/// `crate::ops::types`), each spelled once here because the dispatch keys off it.
pub const PRE_CREATE_HOSTED_CONFIGURATION_VERSION: &str =
    "PRE_CREATE_HOSTED_CONFIGURATION_VERSION";
/// See [`PRE_CREATE_HOSTED_CONFIGURATION_VERSION`].
pub const PRE_START_DEPLOYMENT: &str = "PRE_START_DEPLOYMENT";
/// See [`PRE_CREATE_HOSTED_CONFIGURATION_VERSION`].
pub const AT_DEPLOYMENT_TICK: &str = "AT_DEPLOYMENT_TICK";
/// See [`PRE_CREATE_HOSTED_CONFIGURATION_VERSION`].
pub const ON_DEPLOYMENT_START: &str = "ON_DEPLOYMENT_START";
/// See [`PRE_CREATE_HOSTED_CONFIGURATION_VERSION`].
pub const ON_DEPLOYMENT_STEP: &str = "ON_DEPLOYMENT_STEP";
/// See [`PRE_CREATE_HOSTED_CONFIGURATION_VERSION`].
pub const ON_DEPLOYMENT_BAKING: &str = "ON_DEPLOYMENT_BAKING";
/// See [`PRE_CREATE_HOSTED_CONFIGURATION_VERSION`].
pub const ON_DEPLOYMENT_COMPLETE: &str = "ON_DEPLOYMENT_COMPLETE";
/// See [`PRE_CREATE_HOSTED_CONFIGURATION_VERSION`].
pub const ON_DEPLOYMENT_ROLLED_BACK: &str = "ON_DEPLOYMENT_ROLLED_BACK";

/// The `Directive` an `AT_DEPLOYMENT_TICK` target answers with to abort the deployment it
/// was invoked for.
pub const DIRECTIVE_ROLL_BACK: &str = "ROLL_BACK";

/// How long after the previous one the next `AT_DEPLOYMENT_TICK` invocation of an
/// in-flight deployment falls due.
///
/// The action point is invoked repeatedly for as long as a deployment is in flight, and
/// no interval is published for it, so this span is the EMULATOR's: short enough that a
/// caller watching a deployment sees the ticks that pace it, and never zero.
const TICK_INTERVAL: Duration = Duration::from_millis(500);

/// [`TICK_INTERVAL`], for the deployment record that arms the first tick.
pub fn tick_interval() -> Duration {
    TICK_INTERVAL
}

/// How many owed action points one [`drive_deployment`] call works through.
///
/// A bound rather than a loop to exhaustion: each round invokes one action point's
/// actions, and whatever is still owed afterwards is owed to the next call. It also keeps
/// a deployment nobody looked at for an hour from invoking an hour of ticks at once —
/// [`next_tick_due`] is moved to one interval past NOW rather than past the instant it was
/// last due, so the ticks nobody was there for are not replayed.
///
/// [`next_tick_due`]: Deployment::next_tick_due
const MAX_DRIVE_ROUNDS: usize = 8;

/// Which deployment event each action point of a deployment's own lifecycle is owed BY:
/// the event the deployment's clock-driven state machine logs when it reaches that stage
/// (`crate::state::Deployment::materialize`), so an action point fires exactly as often as
/// the stage it belongs to actually happened.
const LIFECYCLE_ACTION_POINTS: &[(&str, &str)] = &[
    (super::EVENT_DEPLOYMENT_STARTED, ON_DEPLOYMENT_START),
    (super::EVENT_PERCENTAGE_UPDATED, ON_DEPLOYMENT_STEP),
    (super::EVENT_BAKE_TIME_STARTED, ON_DEPLOYMENT_BAKING),
    (super::EVENT_DEPLOYMENT_COMPLETED, ON_DEPLOYMENT_COMPLETE),
    (super::EVENT_ROLLBACK_COMPLETED, ON_DEPLOYMENT_ROLLED_BACK),
];

/// Whether AppConfig consults this action point BEFORE performing the call that reached
/// it, so the call waits for the answer and a failed invocation refuses it.
///
/// The two `PRE_` points are those: each is "pre" the operation whose name it carries.
/// `AT_DEPLOYMENT_TICK` and the `ON_DEPLOYMENT_*` points belong to a deployment that is
/// already under way, so nothing is waiting on them.
pub fn consulted_before_the_call(action_point: &str) -> bool {
    action_point == PRE_CREATE_HOSTED_CONFIGURATION_VERSION || action_point == PRE_START_DEPLOYMENT
}

/// The `Type` the event carries: the action point in PascalCase rather than the
/// SCREAMING_SNAKE enum value — `PRE_START_DEPLOYMENT` is handed over as
/// `PreStartDeployment`.
pub fn event_type(action_point: &str) -> String {
    action_point
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut letters = word.chars();
            match letters.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &letters.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// The resource an action point's workflow is running on, as an association's
/// `ResourceArn` has to address for its actions to be bound there.
///
/// An association binds ONE of the three levels and a workflow runs all of the levels it
/// names, so an association bound to any of them is bound to the workflow. A level the
/// workflow does not name — the environment of a `CreateHostedConfigurationVersion`, which
/// deploys nothing — matches nothing.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub application_id: &'a str,
    pub environment_id: Option<&'a str>,
    pub configuration_profile_id: Option<&'a str>,
}

/// Whether the association whose `ResourceArn` is `resource_arn` is bound to `scope`.
///
/// Matched on the resource the ARN ADDRESSES rather than on the ARN text, so an
/// association created against one Region's ARN is still bound in another: this emulator
/// holds one tree, not one per Region.
pub fn association_binds(resource_arn: &str, scope: &Scope<'_>) -> bool {
    match super::parse_resource_arn(resource_arn) {
        Some(TaggedResource::Application { application_id }) => {
            application_id == scope.application_id
        }
        Some(TaggedResource::Environment {
            application_id,
            environment_id,
        }) => {
            application_id == scope.application_id
                && scope.environment_id == Some(environment_id)
        }
        Some(TaggedResource::ConfigurationProfile {
            application_id,
            configuration_profile_id,
        }) => {
            application_id == scope.application_id
                && scope.configuration_profile_id == Some(configuration_profile_id)
        }
        // Every other AppConfig resource — a deployment, a strategy, an extension,
        // another association — is not one of the three an extension associates to.
        _ => false,
    }
}

/// One action an association binds at one action point, resolved to everything its
/// invocation needs.
#[derive(Debug, Clone)]
pub struct ActionTarget {
    /// The action point this action is bound at, which decides the event's `Type` and
    /// whether the calling operation waits for the answer.
    pub action_point: String,
    /// The extension the action belongs to, as the refusal of a gated call names it and
    /// as the deployment's event log records it.
    pub extension_id: String,
    pub extension_name: String,
    pub action_name: Option<String>,
    pub uri: String,
    pub role_arn: Option<String>,
    /// The parameter values the association supplies, plus the dynamic values the call
    /// supplied for it.
    pub parameters: BTreeMap<String, String>,
    /// "A system-generated ID for this invocation"
    /// (`smithy_query(query="shape",shape="ActionInvocation")`), minted from this
    /// service's own id space because the member targets the model's `Id` shape.
    pub invocation_id: String,
}

impl ActionTarget {
    /// What the deployment's event log records about this invocation: the action that was
    /// invoked, and the error the target gave when it failed.
    pub fn invocation_record(&self, failure: Option<&Failure>) -> ActionInvocation {
        ActionInvocation {
            extension_identifier: Some(self.extension_name.clone()),
            action_name: self.action_name.clone(),
            uri: Some(self.uri.clone()),
            role_arn: self.role_arn.clone(),
            error_message: failure.map(|failure| failure.detail.clone()),
            error_code: failure.map(|failure| failure.code.clone()),
            invocation_id: Some(self.invocation_id.clone()),
        }
    }

    /// How AppConfig refuses the call an action point gated when the action's target did
    /// not answer: the service's own template, naming the extension whose action failed
    /// and, when the target reported one, the Lambda request id of the invocation.
    pub fn refusal_message(&self, failure: &Failure) -> String {
        let mut message = format!(
            "Error invoking extension {}: {}",
            self.extension_name, failure.detail
        );
        if let Some(request_id) = &failure.request_id {
            message.push_str(&lambda_request_id_clause(request_id));
        }
        message
    }
}

/// The clause a relayed Lambda failure carries the invocation's request id in.
///
/// `pub(crate)` because every refusal this service relays from a function carries it the
/// same way: a failed extension action above, and a configuration profile's `LAMBDA`
/// validator in [`super::validators`].
pub(crate) fn lambda_request_id_clause(request_id: &str) -> String {
    format!(", Lambda Request ID: {request_id}")
}

/// Where an action of an AWS-authored extension delivers the event it produces.
///
/// An extension an account creates names its own target on the action (`Uri`), but an
/// AWS-authored extension publishes no URI: AppConfig itself knows what its own extensions
/// do, and where a deployment-event notification extension sends one is the ARN the
/// ASSOCIATION that bound it supplies for that extension's published parameter. This is
/// that reading, one variant per published behaviour, carried on each row of the catalogue
/// (`super::AWS_AUTHORED_EXTENSIONS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthoredDestination {
    /// Nothing is delivered anywhere: the extension's published work happens inside
    /// AppConfig, on the configuration document a call is writing or serving.
    Internal,
    /// The ARN the association supplies for this parameter of the extension — the queue or
    /// the topic a deployment-event notification is sent to. An association that supplied
    /// no value for it names no destination, and nothing is sent.
    Parameter(&'static str),
    /// The account's default Amazon EventBridge event bus, which the EventBridge
    /// notification extension publishes to: its published sentence names that bus, so an
    /// association supplies no parameter for it.
    DefaultEventBus,
}

/// The name of the event bus every account has without creating one, which
/// [`AuthoredDestination::DefaultEventBus`] addresses.
const DEFAULT_EVENT_BUS_NAME: &str = "default";

impl AuthoredDestination {
    /// The ARN this destination names for an association supplying `parameters`, in
    /// `region`; `None` when it names none, in which case the action delivers nothing.
    pub fn target(self, parameters: &HashMap<String, String>, region: &str) -> Option<String> {
        match self {
            Self::Internal => None,
            Self::Parameter(name) => parameters
                .get(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            Self::DefaultEventBus => Some(::engine::emulator_arn(
                EVENTS_ARN_NAMESPACE,
                region,
                &format!("{EVENT_BUS_RESOURCE_TYPE}/{DEFAULT_EVENT_BUS_NAME}"),
            )),
        }
    }
}

/// The actions bound at `action_point` by the extensions associated to `scope`, in
/// association-id order so two calls invoke them in the same order.
///
/// `dynamic_parameters` are a call's own `DynamicExtensionParameters`: "a map of dynamic
/// extension parameter names to values to pass to associated extensions"
/// (`smithy_query(query="doc",shape="StartDeploymentRequest",member="DynamicExtensionParameters")`),
/// keyed `<extension name or association id>#<parameter name>` as the member's own
/// `DynamicParameterKey` pattern spells it, so each value reaches only the association it
/// names.
pub fn targets_at(
    store: &mut ServiceStore,
    action_point: &str,
    scope: &Scope<'_>,
    region: &str,
    dynamic_parameters: &HashMap<String, String>,
) -> Vec<ActionTarget> {
    // The bound associations first, as owned rows: resolving each one's extension reads
    // the store again, and minting the invocation ids writes to it.
    let bound: BTreeMap<String, (String, i32, HashMap<String, String>)> = store
        .extension_associations()
        .filter(|association| association_binds(&association.resource_arn, scope))
        .map(|association| {
            (
                association.id.clone(),
                (
                    association.extension_id.clone(),
                    association.extension_version_number,
                    association.parameters.clone(),
                ),
            )
        })
        .collect();

    let mut targets: Vec<ActionTarget> = Vec::new();
    for (association_id, (extension_id, version_number, parameters)) in bound {
        let Some((extension, _)) = store.resolve_extension(&extension_id, region) else {
            continue;
        };
        let Some(version) = extension.versions.get(&version_number) else {
            continue;
        };
        let Some(actions) = version.actions.get(action_point) else {
            continue;
        };
        for action in actions {
            // An action an account created names its own target. An AWS-authored
            // extension publishes no `Uri`, and what stands in its place is the
            // destination the catalogue publishes for that extension, resolved against
            // the values THIS association supplied: an association of the queue or topic
            // notification extension names the queue or the topic its parameter carries.
            // An action that resolves to no destination at all reaches nothing and is not
            // invoked.
            let Some(uri) = action
                .uri
                .clone()
                .filter(|uri| !uri.trim().is_empty())
                .or_else(|| {
                    super::aws_authored_extension_named(&extension_id)
                        .and_then(|authored| authored.destination.target(&parameters, region))
                })
            else {
                continue;
            };
            let mut merged: BTreeMap<String, String> = parameters
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            for (key, value) in dynamic_parameters {
                if let Some(name) =
                    dynamic_parameter_name(key, &association_id, &extension.name)
                {
                    merged.insert(name.to_string(), value.clone());
                }
            }
            targets.push(ActionTarget {
                action_point: action_point.to_string(),
                extension_id: extension.id.clone(),
                extension_name: extension.name.clone(),
                action_name: action.name.clone(),
                uri,
                role_arn: action.role_arn.clone(),
                parameters: merged,
                invocation_id: String::new(),
            });
        }
    }
    // Each invocation carries its own id, minted from the same id space every other
    // system-generated id of this service comes from.
    for target in &mut targets {
        target.invocation_id = store.mint_id();
    }
    targets
}

/// The parameter one `DynamicExtensionParameters` key supplies a value for, when the key
/// names this association (by its id) or its extension (by name); `None` when it names
/// another one.
fn dynamic_parameter_name<'key>(
    key: &'key str,
    association_id: &str,
    extension_name: &str,
) -> Option<&'key str> {
    let (named, parameter) = key.split_once('#')?;
    (named == association_id || named == extension_name).then_some(parameter)
}

/// The event one action point hands its targets, built member by member.
///
/// The members every event carries whatever the point — `Type`, `InvocationId` and
/// `Parameters` — are added per target by [`Self::document`], because each target has its
/// own invocation and its own parameter values.
#[derive(Debug, Clone, Default)]
pub struct ActionEvent {
    members: Map<String, Value>,
}

impl ActionEvent {
    /// An event carrying nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// A resource the workflow is running on, as the `{"Id": …, "Name": …}` object the
    /// `Application`, `Environment` and `ConfigurationProfile` members carry.
    pub fn resource(mut self, member: &str, id: &str, name: &str) -> Self {
        self.members.insert(
            member.to_string(),
            Value::Object(Map::from_iter([
                ("Id".to_string(), Value::from(id)),
                ("Name".to_string(), Value::from(name)),
            ])),
        );
        self
    }

    /// One member of this event.
    pub fn member(mut self, member: &str, value: Value) -> Self {
        self.members.insert(member.to_string(), value);
        self
    }

    /// The configuration content the workflow is handling: `ContentType`,
    /// `ContentVersion` as a STRING whatever the version is numbered, and `Content`
    /// base64-encoded.
    pub fn content(self, content_type: &str, content_version: &str, content: &[u8]) -> Self {
        self.member("ContentType", Value::from(content_type))
            .member("ContentVersion", Value::from(content_version))
            .member("Content", Value::from(encoded(content)))
    }

    /// The event as `target`'s invocation receives it.
    pub fn document(&self, target: &ActionTarget) -> Value {
        let mut members = self.members.clone();
        members.insert(
            "Type".to_string(),
            Value::from(event_type(&target.action_point)),
        );
        members.insert(
            "InvocationId".to_string(),
            Value::from(target.invocation_id.as_str()),
        );
        members.insert(
            "Parameters".to_string(),
            Value::Object(
                target
                    .parameters
                    .iter()
                    .map(|(name, value)| (name.clone(), Value::from(value.as_str())))
                    .collect(),
            ),
        );
        Value::Object(members)
    }
}

/// Content as an action-point event carries it: base64, the encoding the event's
/// `Content` member is documented to use.
pub fn encoded(content: &[u8]) -> String {
    ::base64::engine::general_purpose::STANDARD.encode(content)
}

/// The `PreviousContent` member: "the previously stored bytes", as the content, type and
/// version number of the version that was latest before the one being created.
pub fn previous_content(content_type: &str, version_number: i32, content: &[u8]) -> Value {
    Value::Object(Map::from_iter([
        ("ContentType".to_string(), Value::from(content_type)),
        (
            "ContentVersion".to_string(),
            Value::from(version_number.to_string()),
        ),
        ("Content".to_string(), Value::from(encoded(content))),
    ]))
}

/// Why an invocation produced no answer: what the deployment's event log records about it
/// and what the refusal of a gated call reports.
#[derive(Debug, Clone)]
pub struct Failure {
    /// The error code the target gave: the `FunctionError` of a function that raised, or
    /// the error code of the call that could not be made.
    pub code: String,
    /// What the refusal says went wrong, after `Error invoking extension <name>: `.
    pub detail: String,
    /// The Lambda request id of the invocation, when its answer carried one.
    pub request_id: Option<String>,
}

/// The wire error code an emulated service answers for an operation nobody has
/// implemented yet (`crate::scaffolded`, and the same constant EventBridge's target
/// delivery reads).
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The error codes a target's own service answers when the target COULD NOT BE RUN at all,
/// one per modelled failure that says so rather than reporting what the target decided.
///
/// Lambda answers each of these before the function's code runs — "Lambda could not unzip
/// the deployment package", a runtime it cannot run, a function not ready, or environment
/// variables it could not decrypt — so nothing is known about what the target would have
/// answered, exactly as for an operation this emulator has not implemented.
const NOT_RUN_CODES: &[&str] = &[
    NOT_IMPLEMENTED_CODE,
    "InvalidZipFileException",
    "InvalidRuntimeException",
    "ResourceNotReadyException",
    "KMSAccessDeniedException",
    "KMSDisabledException",
    "KMSInvalidStateException",
    "KMSNotFoundException",
];

impl Failure {
    /// The failure one refused call amounts to: the target service's own error code, the
    /// message it gave, and the request id its answer carried.
    pub fn new(code: &str, message: Option<&str>, request_id: Option<String>) -> Self {
        Self {
            code: code.to_string(),
            detail: match message {
                Some(message) => format!("{code}: {message}"),
                None => code.to_string(),
            },
            request_id,
        }
    }

    /// Whether this failure refuses the call the action point gated.
    ///
    /// A target that REFUSED the action does: that is the veto the action point exists
    /// for. A call that did not reach the target's own code does NOT ([`NOT_RUN_CODES`]) —
    /// nothing is known about what the target would have answered, so refusing a call the
    /// real service serves would report a veto no extension gave. The invocation is
    /// recorded as the failure it was either way.
    pub fn refuses_the_call(&self) -> bool {
        !NOT_RUN_CODES.contains(&self.code.as_str())
    }
}

/// What a target answered: the document a function returned (`None` from a target kind
/// that cannot answer), or the failure that means it did not run.
pub type Answer = Result<Option<Value>, Failure>;

/// The replacement content an answer supplies, when it carries a `Content` member: "a
/// response of `{"Content": "<base64>"}` replaces what is stored". `None` when the answer
/// asks for no replacement, and an `Err` when the member is there but is not base64.
pub fn replacement_content(answer: &Value) -> Option<Result<Vec<u8>, String>> {
    let encoded = answer.get("Content")?;
    let Some(text) = encoded.as_str() else {
        return Some(Err("the Content the extension answered is not a string".to_string()));
    };
    Some(
        ::base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|_| "the Content the extension answered is not base64".to_string()),
    )
}

/// The directive an `AT_DEPLOYMENT_TICK` answer gives, with the description it came with:
/// `{"Directive": "ROLL_BACK", "Description": "<text>"}`.
pub fn directive(answer: &Value) -> Option<(String, Option<String>)> {
    let directive = answer.get("Directive")?.as_str()?.to_string();
    let description = answer
        .get("Description")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((directive, description))
}

/// Invoke one action's target with `event` and answer what it gave back.
///
/// The target is reached in the Region ITS OWN ARN names, falling back to the Region this
/// request was signed for, over the in-process callback into this same emulator — so the
/// service that owns the target is the one that serves the invocation.
pub async fn invoke(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target: &ActionTarget,
    event: &Value,
    request_region: &str,
) -> Answer {
    let Some(kind) = TargetKind::of(&target.uri) else {
        return Err(Failure {
            code: "InvalidActionUri".to_string(),
            detail: format!(
                "the action URI {} names no Lambda function, Amazon SQS queue, Amazon SNS \
                 topic or Amazon EventBridge event bus",
                target.uri
            ),
            request_id: None,
        });
    };
    let region = ::service_common::arn::region_of(&target.uri, kind.vendor())
        .unwrap_or(request_region);
    let body = event.to_string();
    match kind {
        TargetKind::Function => invoke_function(ctx, &target.uri, region, body).await,
        TargetKind::Queue => {
            let Some(queue_url) = queue_url(&target.uri) else {
                return Err(Failure {
                    code: "InvalidActionUri".to_string(),
                    detail: format!("the action URI {} names no queue", target.uri),
                    request_id: None,
                });
            };
            answered_nothing(
                sqs_client(ctx, region)
                    .send_message()
                    .queue_url(queue_url)
                    .message_body(body)
                    .send()
                    .await,
            )
        }
        TargetKind::Topic => answered_nothing(
            sns_client(ctx, region)
                .publish()
                .topic_arn(&target.uri)
                .message(body)
                .send()
                .await,
        ),
        TargetKind::EventBus => answered_nothing(
            eventbridge_client(ctx, region)
                .put_events()
                .entries(
                    ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                        .event_bus_name(&target.uri)
                        // Every AWS service publishes its own events under
                        // `aws.<service>`, and the detail type is the action point the
                        // event belongs to, exactly as its `Type` member spells it.
                        .source(format!("aws.{}", super::SERVICE_NAMESPACE))
                        .detail_type(event_type(&target.action_point))
                        .detail(body)
                        .build(),
                )
                .send()
                .await,
        ),
    }
}

/// A Lambda function target: the one kind that can ANSWER, so it is invoked
/// synchronously ("RequestResponse") and its payload is read back.
///
/// `pub(crate)` because an action point is not the only place this service hands a
/// document to a function and reads the verdict back: a configuration profile's `LAMBDA`
/// validator does too ([`super::validators`]), and both go through this one invocation.
pub(crate) async fn invoke_function(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    function_name: &str,
    region: &str,
    body: String,
) -> Answer {
    let answer = lambda_client(ctx, region)
        .invoke()
        .function_name(function_name)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(body.into_bytes()))
        .send()
        .await;
    let output = match answer {
        Ok(output) => output,
        Err(error) => return Err(failure_of(&error)),
    };
    let request_id = ::aws_types::request_id::RequestId::request_id(&output).map(str::to_string);
    // "FunctionError: if present, indicates that an error occurred during function
    // execution. Details about the error are included in the response payload" — the
    // value is the error's handling (`Handled`, `Unhandled`), which is what AppConfig
    // reports as the reason the extension's action failed.
    if let Some(function_error) = output.function_error() {
        return Err(Failure {
            code: function_error.to_string(),
            detail: format!("{function_error} error"),
            request_id,
        });
    }
    let payload = output
        .payload()
        .map(|payload| payload.as_ref().to_vec())
        .unwrap_or_default();
    if payload.is_empty() {
        return Ok(None);
    }
    // A function that answered something other than a JSON document asked for nothing:
    // every directive and every replacement this service reads is a member of a JSON
    // object.
    Ok(::serde_json::from_slice::<Value>(&payload).ok())
}

/// What an invocation of a target kind that cannot answer amounts to: the send either
/// happened or it failed.
fn answered_nothing<T, E>(answer: Result<T, E>) -> Answer
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    match answer {
        Ok(_) => Ok(None),
        Err(error) => Err(failure_of(&error)),
    }
}

/// The failure one refused in-process call amounts to, read through the SDK's own error
/// metadata so the code and message are the target service's own.
fn failure_of<E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata>(
    error: &E,
) -> Failure {
    Failure::new(
        error.code().unwrap_or("InvocationFailed"),
        error.message(),
        None,
    )
}

/// The kind of target an action's `Uri` names: which service serves the invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    /// A Lambda function, invoked with the event as its payload.
    Function,
    /// An Amazon SQS queue, which receives the event as a message body.
    Queue,
    /// An Amazon SNS topic, which the event is published to.
    Topic,
    /// An Amazon EventBridge event bus, which the event is put on.
    EventBus,
}

/// The ARN namespace of AWS Lambda, whose functions an action invokes — and whose
/// functions a configuration profile's `LAMBDA` validator names, which is why this is
/// `pub(crate)`.
pub(crate) const LAMBDA_ARN_NAMESPACE: &str = "lambda";
/// The ARN namespace of Amazon SQS, whose queues an action sends to.
const SQS_ARN_NAMESPACE: &str = "sqs";
/// The ARN namespace of Amazon SNS, whose topics an action publishes to.
const SNS_ARN_NAMESPACE: &str = "sns";
/// The ARN namespace of Amazon EventBridge, whose event buses an action puts to.
const EVENTS_ARN_NAMESPACE: &str = "events";

/// The resource type a Lambda function ARN names.
const FUNCTION_RESOURCE_TYPE: &str = "function";
/// The resource type an EventBridge event bus ARN names.
const EVENT_BUS_RESOURCE_TYPE: &str = "event-bus";

impl TargetKind {
    /// The kind `uri` names, or `None` when it names no target this service can invoke —
    /// in which case nothing is sent and nothing is invented in its place.
    ///
    /// The ARN is read through the ONE shared parse (`service_common::arn`), so this crate
    /// does not split a caller's ARN its own way.
    fn of(uri: &str) -> Option<Self> {
        let parsed = ::service_common::arn::parse(uri)?;
        match parsed.vendor {
            LAMBDA_ARN_NAMESPACE if parsed.resource_type() == FUNCTION_RESOURCE_TYPE => {
                Some(Self::Function)
            }
            // A queue's ARN, like a topic's, names the resource directly: a resource
            // field carrying a type separator is not one.
            SQS_ARN_NAMESPACE if names_resource_directly(parsed.resource) => Some(Self::Queue),
            SNS_ARN_NAMESPACE if names_resource_directly(parsed.resource) => Some(Self::Topic),
            EVENTS_ARN_NAMESPACE if parsed.resource_type() == EVENT_BUS_RESOURCE_TYPE => {
                Some(Self::EventBus)
            }
            _ => None,
        }
    }

    /// The ARN namespace of the service this kind of target lives in.
    fn vendor(self) -> &'static str {
        match self {
            Self::Function => LAMBDA_ARN_NAMESPACE,
            Self::Queue => SQS_ARN_NAMESPACE,
            Self::Topic => SNS_ARN_NAMESPACE,
            Self::EventBus => EVENTS_ARN_NAMESPACE,
        }
    }
}

/// Whether an ARN's resource field names its resource directly rather than through a
/// type prefix.
fn names_resource_directly(resource: &str) -> bool {
    !resource.contains('/') && !resource.contains(':')
}

/// The queue url of an SQS queue ARN: SQS addresses a queue by
/// `<endpoint>/<account>/<QueueName>`, the form every SQS operation takes as its
/// `QueueUrl`.
fn queue_url(uri: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(uri)?;
    if parsed.vendor != SQS_ARN_NAMESPACE || !names_resource_directly(parsed.resource) {
        return None;
    }
    Some(format!(
        "https://sqs.{}.amazonaws.com/{}/{}",
        parsed.region, parsed.account, parsed.resource
    ))
}

/// Build one target service's client over the in-process callback into this same
/// emulator, against that service's endpoint in `region`.
///
/// The transport is in-process, so each client disables what a network client needs and
/// this one cannot have: retries, timeouts, stalled-stream protection and identity
/// caching each need a sleep implementation the callback transport is not given, and
/// there is nothing to retry behind it anyway.
///
/// The same four-client shape EventBridge's own target delivery builds
/// (`service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs`); its builder is
/// private to that operation's module, and the settings here must stay identical to it.
macro_rules! callback_client {
    ($name:ident, $sdk:ident, $endpoint:literal) => {
        fn $name(
            ctx: &smithy_server_runtime::OperationContext<'_>,
            region: &str,
        ) -> ::$sdk::Client {
            let config = ::$sdk::Config::builder()
                .behavior_version(::$sdk::config::BehaviorVersion::latest())
                .http_client(ctx.callback().http_client())
                .credentials_provider(ctx.callback().aws_credentials_provider())
                .region(::$sdk::config::Region::new(region.to_string()))
                .endpoint_url(format!("https://{}.{region}.amazonaws.com", $endpoint))
                .retry_config(::$sdk::config::retry::RetryConfig::disabled())
                .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::$sdk::config::IdentityCache::no_cache())
                .stalled_stream_protection(::$sdk::config::StalledStreamProtectionConfig::disabled())
                .build();
            ::$sdk::Client::from_conf(config)
        }
    };
}

callback_client!(lambda_client, aws_sdk_lambda, "lambda");
callback_client!(sqs_client, aws_sdk_sqs, "sqs");
callback_client!(sns_client, aws_sdk_sns, "sns");
callback_client!(eventbridge_client, aws_sdk_eventbridge, "events");

/// One action point a deployment owes the invocation of, with everything the invocation
/// and the recording of it need.
struct OwedPoint {
    /// The targets bound at the point, already marked as invoked in the store.
    targets: Vec<ActionTarget>,
    /// The event they are handed.
    event: ActionEvent,
    /// Where the deployment's event log records the invocations: the index of the event
    /// the action point belongs to, and `None` for a tick, which logs nothing of its own.
    event_index: Option<usize>,
}

/// Invoke the action points this deployment's own lifecycle owes, and apply what the
/// targets answer.
///
/// Every point it dispatches was owed by the CLOCK: each `ON_DEPLOYMENT_*` point is owed
/// by a deployment event the strategy's own durations already made due, and a tick by the
/// interval that has elapsed since the previous one — so a reporting operation may call
/// this. It cannot make a later caller see anything the clock had not already made true,
/// with the one exception the extension itself asks for: a target that answers
/// [`DIRECTIVE_ROLL_BACK`] rolls the deployment back, which is the point of the action
/// point.
pub async fn drive_deployment<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    application_id: &str,
    environment_id: &str,
    deployment_number: i32,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let region = super::request_region(ctx.parts());
    for _ in 0..MAX_DRIVE_ROUNDS {
        let now = SystemTime::now();
        let owed = {
            let mut store = state.lock();
            next_owed_point(
                &mut store,
                application_id,
                environment_id,
                deployment_number,
                &region,
                now,
            )
        };
        let Some(owed) = owed else {
            return;
        };
        let mut records: Vec<ActionInvocation> = Vec::new();
        let mut rollback: Option<Option<String>> = None;
        for target in &owed.targets {
            let answer = invoke(ctx, target, &owed.event.document(target), &region).await;
            match answer {
                Ok(answered) => {
                    records.push(target.invocation_record(None));
                    if target.action_point == AT_DEPLOYMENT_TICK {
                        if let Some((given, description)) =
                            answered.as_ref().and_then(directive)
                        {
                            if given == DIRECTIVE_ROLL_BACK {
                                rollback = Some(description);
                            }
                        }
                    }
                }
                // An action point of a deployment that is already under way gates
                // nothing, so a target that did not run is RECORDED rather than allowed
                // to change the deployment: the error it gave is what the event log
                // reports for that invocation.
                Err(failure) => records.push(target.invocation_record(Some(&failure))),
            }
        }
        if records.is_empty() && rollback.is_none() {
            continue;
        }
        let mut store = state.lock();
        let Some(deployment) = store
            .application_mut(application_id)
            .and_then(|application| application.environments.get_mut(environment_id))
            .and_then(|environment| environment.deployments.get_mut(&deployment_number))
        else {
            return;
        };
        if let Some(index) = owed.event_index {
            if let Some(event) = deployment.event_log.get_mut(index) {
                let mut invocations = event.action_invocations.take().unwrap_or_default();
                invocations.extend(records.iter().cloned());
                event.action_invocations = Some(invocations);
            }
        }
        if let Some(description) = rollback {
            apply_roll_back(deployment, description, records, now);
        }
    }
}

/// Apply an `AT_DEPLOYMENT_TICK` target's [`DIRECTIVE_ROLL_BACK`] to the deployment it
/// was invoked for, and answer whether the deployment was rolled back.
///
/// The directive is the extension's abort of a deployment that is still in flight, which
/// AppConfig itself then works through ([`Deployment::begin_rollback`]) rather than
/// settling in one call the way a caller's own stop does — so the deployment reports the
/// rollback in progress, and the event it logs records the invocations that asked for it,
/// the description the extension gave as its reason, and the EXTENSION as what triggered
/// it ([`super::TRIGGERED_BY_EXTENSION`]) rather than the caller who started the rollout
/// or AppConfig's own schedule. A deployment the service has already finished with is left
/// exactly as it was: there is nothing in flight to abort.
fn apply_roll_back(
    deployment: &mut Deployment,
    description: Option<String>,
    records: Vec<ActionInvocation>,
    now: SystemTime,
) -> bool {
    if deployment.begin_rollback(now).is_err() {
        return false;
    }
    if let Some(event) = deployment.event_log.last_mut() {
        event.action_invocations = Some(records);
        event.triggered_by = Some(super::TRIGGERED_BY_EXTENSION.to_string());
        if let Some(description) = description {
            event.description = Some(description);
        }
    }
    true
}

/// The next action point this deployment owes the invocation of, marked as invoked so the
/// same one is not dispatched twice, or `None` when it owes none.
fn next_owed_point(
    store: &mut ServiceStore,
    application_id: &str,
    environment_id: &str,
    deployment_number: i32,
    region: &str,
    now: SystemTime,
) -> Option<OwedPoint> {
    let environment = store
        .application(application_id)?
        .environments
        .get(environment_id)?;
    let deployment = environment.deployments.get(&deployment_number)?;
    let (action_point, event_index) = owed_point_of(deployment, now)?;
    let event = deployment_event(store, application_id, environment_id, deployment_number)?;
    // Marked BEFORE the invocation: an action point whose target does not answer has
    // still been reached, and the deployment's stage happened once however its actions
    // went.
    let deployment = store
        .application_mut(application_id)?
        .environments
        .get_mut(environment_id)?
        .deployments
        .get_mut(&deployment_number)?;
    let configuration_profile_id = deployment.configuration_profile_id.clone();
    if action_point == AT_DEPLOYMENT_TICK {
        deployment.next_tick_due = Some(now + TICK_INTERVAL);
    } else if let Some(index) = event_index {
        let event_type = deployment
            .event_log
            .get(index)
            .and_then(|event| event.event_type.clone())
            .unwrap_or_default();
        *deployment
            .notified_event_counts
            .entry(event_type)
            .or_insert(0) += 1;
    }
    let scope = Scope {
        application_id,
        environment_id: Some(environment_id),
        configuration_profile_id: Some(&configuration_profile_id),
    };
    // A deployment carries no dynamic parameters of its own: the values a call supplied
    // are passed to the `PRE_START_DEPLOYMENT` actions the call itself consulted.
    let targets = targets_at(store, action_point, &scope, region, &HashMap::new());
    Some(OwedPoint {
        targets,
        event,
        event_index,
    })
}

/// The action point this deployment owes next, with the index of the event that owes it.
///
/// The lifecycle points come first and in event order, so the actions of a deployment
/// that completed between two reads are invoked in the order its stages happened; the
/// tick is owed by the interval instead of by an event, so it carries no index.
fn owed_point_of(deployment: &Deployment, now: SystemTime) -> Option<(&'static str, Option<usize>)> {
    for (event_type, action_point) in LIFECYCLE_ACTION_POINTS {
        let notified = deployment
            .notified_event_counts
            .get(*event_type)
            .copied()
            .unwrap_or(0);
        let occurrence = deployment
            .event_log
            .iter()
            .enumerate()
            .filter(|(_, event)| event.event_type.as_deref() == Some(*event_type))
            .nth(notified);
        if let Some((index, _)) = occurrence {
            return Some((action_point, Some(index)));
        }
    }
    if deployment.is_in_flight()
        && deployment
            .next_tick_due
            .is_some_and(|due| now >= due)
    {
        return Some((AT_DEPLOYMENT_TICK, None));
    }
    None
}

/// The event every action point of a deployment's own workflow is handed: the resources
/// the deployment is running on, the configuration version it is rolling out, and the
/// deployment numbers.
fn deployment_event(
    store: &ServiceStore,
    application_id: &str,
    environment_id: &str,
    deployment_number: i32,
) -> Option<ActionEvent> {
    let application = store.application(application_id)?;
    let environment = application.environments.get(environment_id)?;
    let deployment = environment.deployments.get(&deployment_number)?;
    let mut event = ActionEvent::new()
        .resource("Application", &application.id, &application.name)
        .resource("Environment", &environment.id, &environment.name)
        .member(
            "DeploymentNumber",
            Value::from(deployment.deployment_number),
        )
        // The configuration version this deployment is rolling out, as the string the
        // `ConfigurationVersion` member of the deployment carries: a notification of a
        // deployment's lifecycle names the version the deployment is about, whether or not
        // this service holds the bytes of it.
        .member(
            "ConfigurationVersion",
            Value::from(deployment.configuration_version.as_str()),
        )
        .member(
            "PreviousDeploymentNumber",
            Value::from(previous_deployment_number(
                environment,
                deployment.deployment_number,
            )),
        )
        // Where the rollout stands when the point is dispatched: the share of targets the
        // configuration is available to, the quantity the API itself reports as
        // `Deployment$PercentageComplete` ("the percentage of targets for which the
        // deployment is available"). It is the position an `AT_DEPLOYMENT_TICK` target
        // rules on when it answers a directive, so the event states it rather than
        // leaving the target to guess how far the deployment it is steering has got.
        .member(
            "PercentageComplete",
            Value::from(deployment.percentage_complete),
        );
    if let Some(profile) = application.profiles.get(&deployment.configuration_profile_id) {
        event = event.resource("ConfigurationProfile", &profile.id, &profile.name);
        match profile.resolve_hosted_version(&deployment.configuration_version) {
            // The candidate configuration: the bytes a consumer of this profile is
            // served for the version being deployed, so an action sees what the
            // deployment would put in service.
            Some(version) => {
                event = event.content(
                    &version.content_type,
                    &deployment.configuration_version,
                    &profile.served_content(&version.content),
                );
            }
            // A configuration this service does not hold the bytes of — one stored in
            // S3, Parameter Store or Secrets Manager — has a version but no content
            // here, and content invented for it would be content no caller wrote.
            None => {
                event = event.member(
                    "ContentVersion",
                    Value::from(deployment.configuration_version.as_str()),
                );
            }
        }
    }
    Some(event)
}

/// The deployment number in force before `deployment_number`: the highest the environment
/// issued below it, and `0` before its first deployment.
fn previous_deployment_number(environment: &super::Environment, deployment_number: i32) -> i32 {
    environment
        .deployments
        .keys()
        .copied()
        .filter(|number| *number < deployment_number)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of [`a_replacement_content_is_the_base64_the_answer_carried`]: the case,
    /// the answer the target gave and the replacement it asks for, with the decode
    /// failure flattened to `Err(())` so a row is comparable.
    type ReplacementRow = (&'static str, Value, Option<Result<Vec<u8>, ()>>);

    /// One row of [`a_tick_directive_is_read_with_its_description`]: the case, the answer
    /// and the directive it gives with the description that came with it.
    type DirectiveRow = (&'static str, Value, Option<(String, Option<String>)>);

    /// One row of [`a_roll_back_directive_rolls_an_in_flight_deployment_back`]: the case,
    /// the strategy's rollout minutes and bake minutes, the description the extension
    /// gave, how long the deployment has been running, whether the directive rolls it
    /// back and the state it is left in.
    type RollBackRow = (
        &'static str,
        i32,
        i32,
        Option<String>,
        Duration,
        bool,
        &'static str,
    );

    /// The event's `Type` is the action point in PascalCase, over EVERY value of the
    /// model's `ActionPoint` enum (`crate::ops::types::ACTION_POINT_VALUES`) rather than
    /// the one the report cited.
    #[test]
    fn the_event_type_is_the_action_point_in_pascal_case() {
        let rows = [
            ("PRE_CREATE_HOSTED_CONFIGURATION_VERSION", "PreCreateHostedConfigurationVersion"),
            ("PRE_START_DEPLOYMENT", "PreStartDeployment"),
            ("AT_DEPLOYMENT_TICK", "AtDeploymentTick"),
            ("ON_DEPLOYMENT_START", "OnDeploymentStart"),
            ("ON_DEPLOYMENT_STEP", "OnDeploymentStep"),
            ("ON_DEPLOYMENT_BAKING", "OnDeploymentBaking"),
            ("ON_DEPLOYMENT_COMPLETE", "OnDeploymentComplete"),
            ("ON_DEPLOYMENT_ROLLED_BACK", "OnDeploymentRolledBack"),
        ];
        for (action_point, expected) in rows {
            let actual = event_type(action_point);
            assert_eq!(
                actual, expected,
                "case {action_point:?}: expected Type {expected:?}, got {actual:?}"
            );
        }
        let mut points: Vec<&str> = rows.iter().map(|(point, _)| *point).collect();
        points.sort_unstable();
        let mut modelled = crate::ops::types::ACTION_POINT_VALUES.to_vec();
        modelled.sort_unstable();
        assert_eq!(
            points, modelled,
            "case \"every modelled action point\": expected the rows to span \
             ACTION_POINT_VALUES, got {points:?} against {modelled:?}"
        );
    }

    /// A replacement `Content` is read back out of the answer, whatever bytes it carries,
    /// and an answer that carries none asks for no replacement.
    #[test]
    fn a_replacement_content_is_the_base64_the_answer_carried() {
        let rows: Vec<ReplacementRow> = vec![
            ("no Content member", ::serde_json::json!({}), None),
            (
                "an empty document",
                ::serde_json::json!({ "Content": encoded(b"") }),
                Some(Ok(Vec::new())),
            ),
            (
                "text",
                ::serde_json::json!({ "Content": encoded(b"{\"flag\":true}") }),
                Some(Ok(b"{\"flag\":true}".to_vec())),
            ),
            (
                "bytes that are not text",
                ::serde_json::json!({ "Content": encoded(&[0x00, 0xff, 0x10]) }),
                Some(Ok(vec![0x00, 0xff, 0x10])),
            ),
            (
                "a Content that is not base64",
                ::serde_json::json!({ "Content": "not base64 !!" }),
                Some(Err(())),
            ),
            (
                "a Content that is not a string",
                ::serde_json::json!({ "Content": 7 }),
                Some(Err(())),
            ),
        ];
        for (case, answer, expected) in rows {
            let actual = replacement_content(&answer);
            let shaped = actual.clone().map(|outcome| outcome.map_err(|_| ()));
            assert_eq!(
                shaped, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A directive is read with the description it came with, and only from an answer
    /// that carries one.
    #[test]
    fn a_tick_directive_is_read_with_its_description() {
        let rows: Vec<DirectiveRow> = vec![
            ("no Directive member", ::serde_json::json!({}), None),
            (
                "a roll back with a description",
                ::serde_json::json!({ "Directive": "ROLL_BACK", "Description": "alarm" }),
                Some(("ROLL_BACK".to_string(), Some("alarm".to_string()))),
            ),
            (
                "a roll back with no description",
                ::serde_json::json!({ "Directive": "ROLL_BACK" }),
                Some(("ROLL_BACK".to_string(), None)),
            ),
            (
                "another directive",
                ::serde_json::json!({ "Directive": "PROCEED" }),
                Some(("PROCEED".to_string(), None)),
            ),
            (
                "a Directive that is not a string",
                ::serde_json::json!({ "Directive": ["ROLL_BACK"] }),
                None,
            ),
        ];
        for (case, answer, expected) in rows {
            let actual = directive(&answer);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A dynamic parameter reaches the association its key names, by the association's id
    /// or by its extension's name, and no other.
    #[test]
    fn a_dynamic_parameter_reaches_the_association_its_key_names() {
        let rows = [
            ("by association id", "abc1234#limit", Some("limit")),
            ("by extension name", "gate#limit", Some("limit")),
            ("another association", "zzz9999#limit", None),
            ("no separator", "limit", None),
            ("a parameter name carrying a dot", "gate#team.limit", Some("team.limit")),
        ];
        for (case, key, expected) in rows {
            let actual = dynamic_parameter_name(key, "abc1234", "gate");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Which kind of target a `Uri` names, over a table spanning the four forms the
    /// member documents plus forms that name no target at all.
    #[test]
    fn a_uri_names_the_target_kind_its_arn_addresses() {
        let rows = [
            (
                "a function",
                "arn:aws:lambda:us-east-1:000000000000:function:gate",
                Some(TargetKind::Function),
            ),
            (
                "a function version",
                "arn:aws:lambda:us-east-1:000000000000:function:gate:live",
                Some(TargetKind::Function),
            ),
            (
                "a queue",
                "arn:aws:sqs:us-east-1:000000000000:observations",
                Some(TargetKind::Queue),
            ),
            (
                "a topic",
                "arn:aws:sns:us-east-1:000000000000:releases",
                Some(TargetKind::Topic),
            ),
            (
                "an event bus",
                "arn:aws:events:us-east-1:000000000000:event-bus/default",
                Some(TargetKind::EventBus),
            ),
            (
                "a lambda layer",
                "arn:aws:lambda:us-east-1:000000000000:layer:shared",
                None,
            ),
            (
                "an EventBridge rule",
                "arn:aws:events:us-east-1:000000000000:rule/nightly",
                None,
            ),
            ("an URL", "https://example.test/hook", None),
            ("nothing", "", None),
        ];
        for (case, uri, expected) in rows {
            let actual = TargetKind::of(uri);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The url a queue ARN is addressed by, and `None` for an ARN naming no queue.
    #[test]
    fn a_queue_arn_resolves_to_the_url_sqs_addresses_it_by() {
        let rows = [
            (
                "a queue",
                "arn:aws:sqs:eu-west-2:000000000000:observations",
                Some("https://sqs.eu-west-2.amazonaws.com/000000000000/observations"),
            ),
            (
                "a topic",
                "arn:aws:sns:eu-west-2:000000000000:observations",
                None,
            ),
        ];
        for (case, uri, expected) in rows {
            let actual = queue_url(uri);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Which action points gate the call that reached them.
    #[test]
    fn only_the_pre_action_points_gate_the_call() {
        for action_point in crate::ops::types::ACTION_POINT_VALUES {
            let expected = action_point.starts_with("PRE_");
            let actual = consulted_before_the_call(action_point);
            assert_eq!(
                actual, expected,
                "case {action_point:?}: expected gating {expected}, got {actual}"
            );
        }
    }

    /// Which invocation failures refuse the call an action point gated: the ones where
    /// the TARGET refused the action, and not the one where this emulator has not
    /// implemented the call the invocation makes.
    #[test]
    fn only_a_target_that_refused_the_action_refuses_the_gated_call() {
        let rows = [
            ("a function that raised", "Unhandled", true),
            ("a function nothing holds", "ResourceNotFoundException", true),
            ("an invocation the target refused", "AccessDeniedException", true),
            ("a URI naming no invocable target", "InvalidActionUri", true),
            ("a call this emulator does not serve", NOT_IMPLEMENTED_CODE, false),
        ];
        for (case, code, expected) in rows {
            let failure = Failure::new(code, Some("why"), None);
            let actual = failure.refuses_the_call();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected refusing {expected}, got {actual} for {failure:?}"
            );
            assert_eq!(
                failure.detail,
                format!("{code}: why"),
                "case {case:?}: expected the target's own code and message, got {:?}",
                failure.detail
            );
        }
    }

    /// The refusal of a gated call names the extension whose action failed, and the
    /// Lambda request id only when the invocation's answer carried one.
    #[test]
    fn the_refusal_of_a_gated_call_is_the_service_template() {
        let target = ActionTarget {
            action_point: PRE_START_DEPLOYMENT.to_string(),
            extension_id: "abc1234".to_string(),
            extension_name: "release-gate".to_string(),
            action_name: Some("gate".to_string()),
            uri: "arn:aws:lambda:us-east-1:000000000000:function:gate".to_string(),
            role_arn: None,
            parameters: BTreeMap::new(),
            invocation_id: "defg567".to_string(),
        };
        let rows = [
            (
                "a function that raised, with a request id",
                Failure {
                    code: "Unhandled".to_string(),
                    detail: "Unhandled error".to_string(),
                    request_id: Some("a-request-id".to_string()),
                },
                "Error invoking extension release-gate: Unhandled error, Lambda Request ID: \
                 a-request-id",
            ),
            (
                "a function that could not be invoked",
                Failure {
                    code: "ResourceNotFoundException".to_string(),
                    detail: "ResourceNotFoundException: Function not found".to_string(),
                    request_id: None,
                },
                "Error invoking extension release-gate: ResourceNotFoundException: Function \
                 not found",
            ),
        ];
        for (case, failure, expected) in rows {
            let actual = target.refusal_message(&failure);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The document a target receives carries the common members on top of the ones the
    /// action point put in it.
    #[test]
    fn the_event_document_carries_the_common_members() {
        let target = ActionTarget {
            action_point: PRE_CREATE_HOSTED_CONFIGURATION_VERSION.to_string(),
            extension_id: "abc1234".to_string(),
            extension_name: "stamp".to_string(),
            action_name: Some("stamp".to_string()),
            uri: "arn:aws:sqs:us-east-1:000000000000:observations".to_string(),
            role_arn: None,
            parameters: BTreeMap::from([("team".to_string(), "platform".to_string())]),
            invocation_id: "hij8901".to_string(),
        };
        let document = ActionEvent::new()
            .resource("Application", "app1234", "orders")
            .content("application/json", "2", b"{}")
            .document(&target);
        let expected = ::serde_json::json!({
            "Type": "PreCreateHostedConfigurationVersion",
            "InvocationId": "hij8901",
            "Parameters": { "team": "platform" },
            "Application": { "Id": "app1234", "Name": "orders" },
            "ContentType": "application/json",
            "ContentVersion": "2",
            "Content": encoded(b"{}"),
        });
        assert_eq!(
            document, expected,
            "case \"a PRE_CREATE_HOSTED_CONFIGURATION_VERSION event\": expected {expected}, \
             got {document}"
        );
    }

    /// A `ROLL_BACK` directive rolls a deployment that is still in flight back, recording
    /// the invocation that asked for it on the event that starts the rollback, and leaves
    /// a deployment the service has already finished with exactly as it was.
    #[test]
    fn a_roll_back_directive_rolls_an_in_flight_deployment_back() {
        let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let records = vec![ActionInvocation {
            invocation_id: Some("hij8901".to_string()),
            ..ActionInvocation::default()
        }];
        // Rows differ along the state the directive arrives in, and along whether the
        // extension gave a reason with it.
        let rows: Vec<RollBackRow> = vec![
            (
                "a deployment still rolling out, with a description",
                10,
                0,
                Some("the canary alarm fired".to_string()),
                Duration::from_secs(1),
                true,
                super::super::DEPLOYMENT_ROLLING_BACK,
            ),
            (
                "a deployment still rolling out, with no description",
                10,
                0,
                None,
                Duration::from_secs(1),
                true,
                super::super::DEPLOYMENT_ROLLING_BACK,
            ),
            (
                "a deployment that is baking",
                1,
                10,
                None,
                super::super::STAGE_DURATION + Duration::from_secs(1),
                true,
                super::super::DEPLOYMENT_ROLLING_BACK,
            ),
            (
                "a deployment that already completed",
                0,
                0,
                None,
                Duration::ZERO,
                false,
                super::super::DEPLOYMENT_COMPLETE,
            ),
        ];
        for (case, duration, bake, description, elapsed, expected_rolled_back, expected_state) in
            rows
        {
            let strategy = super::super::DeploymentStrategy {
                id: "strat1".to_string(),
                name: "strat".to_string(),
                description: None,
                deployment_duration_in_minutes: duration,
                growth_type: super::super::GROWTH_LINEAR.to_string(),
                growth_factor: 100.0,
                final_bake_time_in_minutes: bake,
                replicate_to: super::super::REPLICATE_TO_NONE.to_string(),
                tags: HashMap::new(),
            };
            let mut deployment = Deployment::start(
                super::super::NewDeployment {
                    deployment_number: 1,
                    application_id: "app1234".to_string(),
                    environment_id: "env1234".to_string(),
                    configuration_profile_id: "cfg1234".to_string(),
                    configuration_name: "config".to_string(),
                    configuration_location_uri: "hosted".to_string(),
                    configuration_version: "1".to_string(),
                    description: None,
                    kms_key_identifier: None,
                    version_label: None,
                    applied_extensions: Vec::new(),
                    tags: HashMap::new(),
                },
                &strategy,
                started_at,
            );
            let now = started_at + elapsed;
            deployment.materialize(now);
            let rolled_back =
                apply_roll_back(&mut deployment, description.clone(), records.clone(), now);
            assert_eq!(
                rolled_back, expected_rolled_back,
                "case {case:?}: expected rolled back {expected_rolled_back}, got {rolled_back}"
            );
            assert_eq!(
                deployment.state, expected_state,
                "case {case:?}: expected state {expected_state:?}, got {:?}",
                deployment.state
            );
            if !expected_rolled_back {
                continue;
            }
            let last = deployment
                .event_log
                .last()
                .unwrap_or_else(|| panic!("case {case:?}: the rollback logs an event"));
            assert_eq!(
                last.event_type.as_deref(),
                Some(super::super::EVENT_ROLLBACK_STARTED),
                "case {case:?}: expected a ROLLBACK_STARTED event, got {last:?}"
            );
            let invocations = last
                .action_invocations
                .as_ref()
                .unwrap_or_else(|| panic!("case {case:?}: the event records the invocations"));
            assert_eq!(
                invocations
                    .iter()
                    .filter_map(|invocation| invocation.invocation_id.clone())
                    .collect::<Vec<String>>(),
                vec!["hij8901".to_string()],
                "case {case:?}: expected the tick's invocation on the event, got {invocations:?}"
            );
            if let Some(expected_description) = &description {
                assert_eq!(
                    last.description.as_ref(),
                    Some(expected_description),
                    "case {case:?}: expected the extension's description, got {:?}",
                    last.description
                );
            }
        }
    }

    /// An AWS-authored extension's action delivers to the destination the catalogue
    /// publishes for it, resolved against the values the association supplied — and to
    /// nowhere at all for an extension whose work happens inside AppConfig or an
    /// association that named no destination.
    ///
    /// The rows span every row of the catalogue (`super::super::AWS_AUTHORED_EXTENSIONS`)
    /// rather than the one the report cited, and, for a row whose destination is a
    /// parameter, every shape of value an association can have supplied for it.
    #[test]
    fn an_authored_extensions_destination_is_the_one_its_association_names() {
        let region = "eu-west-2";
        let queue = "arn:aws:sqs:eu-west-2:123456789012:lifecycle-events";
        let topic = "arn:aws:sns:eu-west-2:123456789012:lifecycle-events";
        let default_bus = ::engine::emulator_arn(EVENTS_ARN_NAMESPACE, region, "event-bus/default");
        // Every published parameter value an association could carry, so one row can be
        // resolved against values meant for another extension as well as its own.
        let supplied: HashMap<String, String> = HashMap::from([
            ("queueArn".to_string(), queue.to_string()),
            ("topicArn".to_string(), topic.to_string()),
        ]);
        // The destination each catalogue row publishes, keyed by the published id: the
        // queue or the topic the association named, the default bus for the extension
        // whose own sentence names it, and nowhere for a row that delivers nothing.
        let published = |id: &str| -> Option<String> {
            match id {
                "AWS.AppConfig.DeploymentEventsToSqs" => Some(queue.to_string()),
                "AWS.AppConfig.DeploymentEventsToSns" => Some(topic.to_string()),
                "AWS.AppConfig.DeploymentNotificationsToEventBridge" => Some(default_bus.clone()),
                _ => None,
            }
        };
        for authored in super::super::AWS_AUTHORED_EXTENSIONS {
            let actual = authored.destination.target(&supplied, region);
            let wanted = published(authored.id);
            assert_eq!(
                actual, wanted,
                "case {:?}: expected the destination {wanted:?}, got {actual:?}",
                authored.id
            );
        }

        // An association that supplied nothing usable for the parameter its extension
        // publishes names no destination, so the action delivers nothing. The rows differ
        // along the shape of what was supplied.
        let unusable: Vec<(&str, HashMap<String, String>)> = vec![
            ("no parameter at all", HashMap::new()),
            (
                "another extension's parameter",
                HashMap::from([("topicArn".to_string(), topic.to_string())]),
            ),
            (
                "an empty value",
                HashMap::from([("queueArn".to_string(), String::new())]),
            ),
            (
                "a value that is only whitespace",
                HashMap::from([("queueArn".to_string(), "  ".to_string())]),
            ),
        ];
        let sqs_extension = super::super::aws_authored_extension_named(
            "AWS.AppConfig.DeploymentEventsToSqs",
        )
        .unwrap_or_else(|| panic!("the catalogue publishes the Amazon SQS notification extension"));
        for (case, parameters) in &unusable {
            let actual = sqs_extension.destination.target(parameters, region);
            assert_eq!(
                actual, None,
                "case {case:?}: expected no destination, got {actual:?}"
            );
        }
        // A value a caller DID supply is the destination, whatever ARN it names: the
        // parameter is read, never matched against a shape of its own.
        let elsewhere = HashMap::from([(
            "queueArn".to_string(),
            "arn:aws:sqs:us-west-2:210987654321:other".to_string(),
        )]);
        let actual = sqs_extension.destination.target(&elsewhere, region);
        assert_eq!(
            actual.as_deref(),
            Some("arn:aws:sqs:us-west-2:210987654321:other"),
            "case \"a queue in another Region\": expected the supplied ARN, got {actual:?}"
        );
    }
}
