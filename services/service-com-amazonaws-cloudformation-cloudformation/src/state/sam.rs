// SPDX-License-Identifier: CC0-1.0

//! The `AWS::Serverless-2016-10-31` transform: expanding a SAM template into the
//! CloudFormation resources it stands for.
//!
//! "The optional Transform section … specifies one or more macros that CloudFormation uses
//! to process your template", and CloudFormation processes those macros BEFORE it does
//! anything with the template's `Resources`: a change set plans, and a stack deploys, the
//! PROCESSED template. `AWS::Serverless-2016-10-31` is the macro the AWS Serverless
//! Application Model is published as, and what it processes is every `AWS::Serverless::*`
//! resource — a type that exists only as that macro's INPUT, never as a resource
//! CloudFormation creates. So a template declaring the transform is expanded here, by
//! [`expand`], before [`super::template::parse`] reads a single resource out of it; every
//! reader of a template body (a change set's plan, a deployment's resources, a stack's
//! outputs) therefore sees the concrete resources and never the macro's input types.
//!
//! What the expansion is, per the SAM resource reference ("AWS SAM resource and property
//! reference" and "Generated AWS CloudFormation resources" in the AWS SAM Developer
//! Guide):
//!
//! * `AWS::Serverless::Function` becomes an `AWS::Lambda::Function`, plus the
//!   `AWS::IAM::Role` it executes as when the declaration names no `Role` of its own, plus
//!   one resource per event the function is triggered by — an `AWS::Lambda::Permission` for
//!   each API event, an `AWS::Lambda::EventSourceMapping` for each stream or queue event,
//!   an `AWS::Events::Rule` for each schedule or event-pattern event, an
//!   `AWS::SNS::Subscription` for each topic event.
//! * An `Api` event that names no `RestApiId` is an IMPLICIT API: the transform synthesizes
//!   ONE `AWS::ApiGateway::RestApi` (`ServerlessRestApi`) whose OpenAPI body carries the
//!   path and method of every such event in the template, and deploys it — an
//!   `AWS::ApiGateway::Deployment` and an `AWS::ApiGateway::Stage` for the implicit `Prod`
//!   stage. That deployment declares a `StageName` of its own ([`EXTRA_STAGE`]), so the
//!   deployed REST API carries that stage beside the one its stage resource serves. A
//!   `HttpApi` event does the same through `AWS::ApiGatewayV2::Api` and its `$default` stage.
//! * `AWS::Serverless::Api` and `AWS::Serverless::HttpApi` become that same API trio from
//!   the definition the declaration states — and "if neither `DefinitionUri` nor
//!   `DefinitionBody` are specified, AWS SAM generates [one] based on your template
//!   configuration": the API's definition is generated from the routes of the events routed
//!   through it ([`api_definition_paths`]), whether that API is the implicit one or one a
//!   declaration spells, so an API the transform emits always carries the definition that
//!   NAMES it. An HTTP API is created by IMPORTING that document, so what the declaration
//!   says about the api itself — its name, description, CORS and endpoint configuration and
//!   its tags — travels inside it ([`http_definition_members`]).
//! * A declaration's `Policies` is what the role generated for it grants: every entry that
//!   is not an AWS managed policy becomes one INLINE policy on that role, named for the
//!   declaration and the entry's position ([`role_policies`]), and the role carries the
//!   managed policy each of the function's event sources needs
//!   ([`EVENT_SOURCE_POLICIES`]).
//! * `AWS::Serverless::SimpleTable` becomes an `AWS::DynamoDB::Table`,
//!   `AWS::Serverless::LayerVersion` an `AWS::Lambda::LayerVersion` and
//!   `AWS::Serverless::StateMachine` an `AWS::StepFunctions::StateMachine` (plus its
//!   execution role).
//! * `AWS::Serverless::WebSocketApi` becomes an `AWS::ApiGatewayV2::Api` of the `WEBSOCKET`
//!   protocol plus the `AWS::ApiGatewayV2::Stage` it is deployed to, and one route per entry of
//!   its `Routes` — with the integration and the invoke permission the function that entry
//!   names needs. A WebSocket api routes a
//!   MESSAGE rather than a request path, so there is no OpenAPI definition to generate for it
//!   ("to import an HTTP API, you must specify a Body or BodyS3Location" — importing a
//!   definition is HTTP-only): what the declaration states about the api travels as the
//!   `CreateApi` members beside it ([`WEBSOCKET_API_PASSTHROUGH`]), the route-selection
//!   expression that type requires included, and each route is wired to the function it names
//!   as that api's own `AWS_PROXY` integration ([`websocket_route`]).
//!
//! ## A deployment's logical id carries a content hash
//!
//! An `AWS::ApiGateway::Deployment` is immutable once created, so "AWS SAM … creates a new
//! deployment whenever the API's definition changes" by giving each deployment a logical id
//! of its own: the deployment's logical id is `<api>Deployment` followed by a hash of the
//! API's definition ([`DIGEST_CHARS`] hex characters), and the stage points at that id. The
//! same definition therefore plans the same single deployment, and an edited definition
//! plans a different one — which is the property [`digest`] gives it. The DIGEST FUNCTION
//! is not observable (nothing reads the hash back), so this module uses the crate's
//! existing SHA-256; what is observable is the shape of the id and that it is a function of
//! the definition's CONTENT, both of which hold here. An
//! `AWS::Serverless::LayerVersion` carries the same content hash on its own logical id, for
//! the same reason: a layer version cannot be updated in place. Both are derived through the
//! crate's one digest derivation ([`super::template::content_digest`]).
//!
//! An expansion that RENAMES a declaration re-points every reference to it
//! ([`rename_references`]): CloudFormation plans and deploys the macro's OUTPUT, so a `Ref`,
//! an `Fn::GetAtt`, an `Fn::Sub` variable or a `DependsOn` naming the declaration's own
//! logical id has to name the expanded one, in every section of the processed template.
//! Nothing else is renamed, so every other reference reads exactly as the template wrote it.
//!
//! * An `AWS::Serverless::Connector` is the GRANT between the two resources it names, not a
//!   resource of its own: the permissions it may ask for come from the profile of its
//!   source/destination type pair, and where that profile grants on the role the source
//!   executes as, the connector becomes the `AWS::IAM::ManagedPolicy` attached to that role
//!   ([`connector`]).
//!
//! ## What is left as declared
//!
//! An `AWS::Serverless::Application` resolves its template from the AWS Serverless
//! Application Repository, which needs an outbound connection this emulator does not make,
//! and an `AWS::Serverless::GraphQLApi` expansion is not
//! modelled here: those declarations are left exactly as the template spells them rather
//! than expanded into resources nothing computed. A connector whose grant is a policy on its
//! DESTINATION rather than on the source's role is left as declared for the same reason, with
//! its `Permissions` judged all the same ([`connector`]). The same
//! holds for an event type this
//! module does not carry — it contributes no resource, instead of a fabricated one. Every
//! OTHER macro (`AWS::Include`, a private one) is still unexpanded: this module answers for
//! the SAM transform alone, which is why [`super::resource_types`] keeps admitting the
//! types a template declaring any transform spells.
//!
//! ## Bounds
//!
//! The expansion is driven by a template body that is already bounded
//! ([`super::template::parse`] refuses an over-long body, too many nodes and too much
//! nesting before this module sees it), and it adds a bounded number of resources per
//! declaration — a fixed few, plus a fixed few per entry of a declared collection that
//! itself came out of that bounded body (the events routed through an api, the routes a
//! WEBSOCKET api declares) — so no caller-controlled input can make it loop or recurse
//! unboundedly: the
//! one recursive walk here ([`canonical`], over a value being hashed) carries its own depth
//! bound. The documented per-template resource quota is applied to the EXPANDED template,
//! which is the one CloudFormation validates.

use std::collections::BTreeMap;

use ::serde_json::{json, Map, Value};
// IAM's own catalogue of the managed policies AWS publishes: WHICH names AWS publishes a
// policy under, and WHERE it files each one, which is part of the ARN a policy a `Policies`
// entry names BY NAME is attached under ([`managed_policy_arn`]).
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::aws_managed_policy_named;

use super::template::{declared_transforms, MAX_TEMPLATE_DEPTH, PARTITION};

/// The policy templates a `Policies` entry's shorthand can name, and the policy document
/// each one stands for.
mod policy_templates;

/// What this transform REFUSES: the faults a submitted SAM document carries, and the
/// diagnostic its failure reports them in.
mod validation;

/// The OBJECT-EVENT source a function declares (`Events: { <name>: { Type: S3 } }`), which is
/// written into the BUCKET it names rather than becoming a resource of its own.
mod object_events;

/// The AUTHORIZATION an `AWS::Serverless::Api` declares, written into the OpenAPI definition
/// of the api the transform expands it into.
mod auth;

/// The CONNECTOR between two resources (`AWS::Serverless::Connector`): the permissions its
/// source/destination profile publishes, and what the macro refuses that asks for more.
mod connector;

/// `Some(reason)` for a template body whose declared Serverless transform cannot be applied
/// to it, worded as the failure the operation that submitted it records; `None` for a body
/// this macro processes, and for one that does not declare it at all.
///
/// The body is read through the crate's one JSON-or-YAML template reader, so a document is
/// judged the same however it was spelled — and under that reader's own bounds, so nothing
/// here reads a body no other path would.
pub fn transform_failure(body: &str) -> Option<String> {
    validation::failure(&super::template::document(body)?)
}

/// The macro name the AWS Serverless Application Model is published under, as a template's
/// `Transform` section names it.
pub const TRANSFORM: &str = "AWS::Serverless-2016-10-31";

/// The resource types the transform expands, spelled as the SAM resource reference spells
/// them.
const FUNCTION: &str = "AWS::Serverless::Function";
const API: &str = "AWS::Serverless::Api";
const HTTP_API: &str = "AWS::Serverless::HttpApi";
const WEBSOCKET_API: &str = "AWS::Serverless::WebSocketApi";
const SIMPLE_TABLE: &str = "AWS::Serverless::SimpleTable";
const LAYER_VERSION: &str = "AWS::Serverless::LayerVersion";
const STATE_MACHINE: &str = "AWS::Serverless::StateMachine";

/// The `ProtocolType` the api a WEBSOCKET declaration becomes is created with
/// (`CreateApi$ProtocolType`, "one of: HTTP | WEBSOCKET"), read from the one place this crate
/// spells it so the expansion and the `Ref`/`Fn::GetAtt` rules that read a v2 api's protocol
/// cannot disagree.
const WEBSOCKET_PROTOCOL: &str = super::template::APIGATEWAYV2_WEBSOCKET_PROTOCOL;

/// `CreateApi$RouteSelectionExpression` — "the route selection expression for the API … This
/// property is required for WebSocket APIs" — which is why a WEBSOCKET declaration that states
/// none is a document the macro cannot expand ([`validation`]).
pub(super) const ROUTE_SELECTION_EXPRESSION: &str = "RouteSelectionExpression";

/// The members an `AWS::Serverless::WebSocketApi` and the `AWS::ApiGatewayV2::Api` it becomes
/// spell the same way: every `CreateApi` member that configures the api ITSELF, less the
/// `ProtocolType` the declaration's own type fixes and the quick-create and OpenAPI-definition
/// members, which `ImportApi` is "supported only for HTTP APIs".
///
/// `Tags` is carried beside them, since the declaration states them as the same map the target
/// type takes.
const WEBSOCKET_API_PASSTHROUGH: &[&str] = &[
    "ApiKeySelectionExpression",
    "Description",
    "DisableExecuteApiEndpoint",
    "DisableSchemaValidation",
    "IpAddressType",
    "Name",
    ROUTE_SELECTION_EXPRESSION,
    "Version",
];

/// The members an `AWS::Serverless::WebSocketApi`'s STAGE takes from the declaration, spelled
/// the same way by `AWS::ApiGatewayV2::Stage`.
const WEBSOCKET_STAGE_PASSTHROUGH: &[&str] = &[
    "AccessLogSettings",
    "DefaultRouteSettings",
    "Description",
    "RouteSettings",
    "StageVariables",
];

/// The type a CONNECTOR between two resources is declared as, which the macro validates
/// ([`connector`]) and this emulator leaves as declared.
const CONNECTOR: &str = "AWS::Serverless::Connector";

/// The logical id of the REST API the transform synthesizes for the `Api` events that name
/// no `RestApiId` ("AWS SAM creates … an implicit API with the logical ID
/// `ServerlessRestApi`").
const IMPLICIT_REST_API: &str = "ServerlessRestApi";

/// The logical id of the HTTP API synthesized for `HttpApi` events that name no `ApiId`.
const IMPLICIT_HTTP_API: &str = "ServerlessHttpApi";

/// The stage the implicit REST API is deployed to ("the implicit API … is deployed to a
/// stage called `Prod`").
const IMPLICIT_STAGE: &str = "Prod";

/// The stage name the `AWS::ApiGateway::Deployment` generated for a REST API declares, which
/// is a stage of its own beside the one that deployment's stage resource serves: "AWS SAM
/// creates a stage called `Stage` by default", which is the stage that deployment's
/// `StageName` makes API Gateway create (`CreateDeployment$stageName` is "the name of the
/// Stage resource for the Deployment resource to create"). A REST API the transform expands
/// therefore carries TWO stages — this one and the stage the declaration's `StageName`
/// names — unless the declaration suppresses it ([`OPEN_API_VERSION_PROPERTY`]).
const EXTRA_STAGE: &str = "Stage";

/// `AWS::Serverless::Api$OpenApiVersion`: "the version of OpenApi to use … Setting this
/// property to any valid value will prevent the creation of the stage `Stage`". So the
/// generated deployment declares [`EXTRA_STAGE`] exactly when the declaration (or the
/// `Globals` section that stands in for it) states no version of its own — a property written
/// as `null` states nothing, so it reads as an absent one. A value that is no published
/// OpenAPI version fails the macro itself, which this module does not model, so the property's
/// PRESENCE is what the expansion reads.
const OPEN_API_VERSION_PROPERTY: &str = "OpenApiVersion";

/// The stage an HTTP API is deployed to by default, and the suffix its logical id carries:
/// `AWS::ApiGatewayV2` deploys to the `$default` stage, whose name is not a logical-id
/// fragment.
const HTTP_DEFAULT_STAGE: &str = "$default";
const HTTP_DEFAULT_STAGE_SUFFIX: &str = "ApiGatewayDefaultStage";

/// How many hex characters of the definition's digest a deployment's (or a layer version's)
/// logical id carries.
const DIGEST_CHARS: usize = 10;

/// `AWS::Serverless::LayerVersion$RetentionPolicy`: "specifies whether old versions of your
/// layer version are retained or deleted after an update", whose values are the
/// `DeletionPolicy` attribute's own `Retain` and `Delete` and whose default is to RETAIN —
/// which is what the expansion states on the generated layer version, since the hashed
/// logical id above makes every content a resource of its own and so makes every content
/// change a REMOVAL of the resource the stack had.
const RETENTION_POLICY_PROPERTY: &str = "RetentionPolicy";

/// `AWS::Serverless::Function$AutoPublishAlias`: "the name of the Lambda alias" the
/// transform publishes a version for and points at it.
const AUTO_PUBLISH_ALIAS_PROPERTY: &str = "AutoPublishAlias";

/// `AWS::Serverless::Function$AutoPublishCodeSha256`: the value a declaration states to
/// "address the following scenario … a new version is not published", which the generated
/// version's logical id carries instead of a hash of the code.
const AUTO_PUBLISH_SHA_PROPERTY: &str = "AutoPublishCodeSha256";

/// `AWS::Serverless::Function$VersionDescription`: "a description of the function version",
/// which `AWS::Lambda::Version` spells as its own `Description`.
const VERSION_DESCRIPTION_PROPERTY: &str = "VersionDescription";

/// `AWS::Serverless::Function$ProvisionedConcurrencyConfig`: "the provisioned concurrency
/// configuration of a function's alias", which belongs on the generated ALIAS — "must be
/// specified with AutoPublishAlias" — under the same name `AWS::Lambda::Alias` spells it.
const PROVISIONED_CONCURRENCY_PROPERTY: &str = "ProvisionedConcurrencyConfig";

/// The logical-id fragments the generated version and alias carry, between the function's
/// own logical id and the hash (or the alias's name): "AWS SAM generates … a
/// `<function-logical-id>Version<sha>` and a `<function-logical-id>Alias<alias-name>`".
const VERSION_ID_INFIX: &str = "Version";
const ALIAS_ID_INFIX: &str = "Alias";

/// The resource attributes a reference walk reads: the ways a template names another
/// resource ([`rename_references`]).
const REF: &str = "Ref";
const FN_SUB: &str = "Fn::Sub";
const DEPENDS_ON: &str = "DependsOn";
const DELETION_POLICY: &str = "DeletionPolicy";

/// The members of an OpenAPI definition the expansion writes into: the document's `paths`,
/// and the `info` block that carries the api's name and description — "`info.title`" is
/// what an imported api's `Name` comes from, which is why a generated definition always
/// states one.
const DEFINITION_PATHS: &str = "paths";
const DEFINITION_INFO: &str = "info";
const DEFINITION_TITLE: &str = "title";
const DEFINITION_DESCRIPTION: &str = "description";

/// The document-level extension an OpenAPI definition carries an HTTP api's CORS
/// configuration in — where an `AWS::Serverless::HttpApi`'s `CorsConfiguration` belongs,
/// since such an api is created by importing its definition.
const CORS_EXTENSION: &str = "x-amazon-apigateway-cors";

/// The `CorsConfiguration` members of an API declaration, against the names the definition's
/// CORS extension spells them with: the extension's members are the modelled `Cors` ones
/// with a lower-cased initial letter.
const CORS_MEMBERS: &[(&str, &str)] = &[
    ("AllowCredentials", "allowCredentials"),
    ("AllowHeaders", "allowHeaders"),
    ("AllowMethods", "allowMethods"),
    ("AllowOrigins", "allowOrigins"),
    ("ExposeHeaders", "exposeHeaders"),
    ("MaxAge", "maxAge"),
];

/// The document-level extension an OpenAPI definition carries an api's endpoint
/// configuration in, and the member of it that answers for `DisableExecuteApiEndpoint`.
const ENDPOINT_CONFIGURATION_EXTENSION: &str = "x-amazon-apigateway-endpoint-configuration";
const DISABLE_EXECUTE_API_ENDPOINT: &str = "disableExecuteApiEndpoint";

/// The root member an OpenAPI definition carries an api's tags in, and the members one
/// entry of it spells a tag's name and value under: an imported api's tags travel inside
/// its definition.
const DEFINITION_TAGS: &str = "tags";
const DEFINITION_TAG_NAME: &str = "name";
const DEFINITION_TAG_VALUE: &str = "x-amazon-apigateway-tag-value";

/// The property an API declaration states its stage's name under, which the `Globals`
/// section of neither API type carries for the IMPLICIT API: that one is deployed to the
/// stage the macro names for it.
const STAGE_NAME_PROPERTY: &str = "StageName";

/// The properties an API declaration states its definition under.
const DEFINITION_BODY_PROPERTY: &str = "DefinitionBody";
const DEFINITION_URI_PROPERTY: &str = "DefinitionUri";

/// What the content hash below is derived under, so it cannot be the same digits as another
/// identifier this crate derives from the same text (see
/// [`super::template::content_digest`]).
const DIGEST_SALT: &str = "sam/definition";

/// The tag the transform puts on the Lambda functions and roles it creates
/// (`lambda:createdBy: SAM`, as a synthesized function's tags carry).
const CREATED_BY_TAG: (&str, &str) = ("lambda:createdBy", "SAM");

/// The managed policy the execution role the transform creates carries: "AWS SAM creates
/// … an IAM role with the `AWSLambdaBasicExecutionRole` managed policy".
///
/// Spelled with the partition rather than `${AWS::Partition}`, which is how the recorded
/// expansion spells every ARN it writes: the transform runs in one partition and names it.
const BASIC_EXECUTION_POLICY: &str = "iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// What an ARN begins with, which is what tells an ARN from a NAME in the one `Policies`
/// entry that can be either ([`managed_policy_arn`]).
const ARN_PREFIX: &str = "arn:";

/// The managed policy the role a function executes as needs for one EVENT SOURCE TYPE:
/// an event source the Lambda service READS FROM on the function's behalf is polled with
/// the function's own permissions, so the transform attaches the execution-role policy
/// published for that source ("AWS SAM … adds the permissions the event source needs to the
/// function's execution role").
///
/// Keyed by the event type an `Events` entry declares, spelled as
/// [`EVENT_SOURCE_MAPPINGS`] spells it. A pull source whose permissions the published
/// expansion grants as policy STATEMENTS rather than as one managed policy (an Amazon MQ
/// broker, a self-managed cluster, a DocumentDB change stream — each of which needs the
/// secret and the network of the resource it reads) has no entry here: nothing is attached
/// for it, rather than a policy nothing published.
const EVENT_SOURCE_POLICIES: &[(&str, &str)] = &[
    (
        "DynamoDB",
        "iam::aws:policy/service-role/AWSLambdaDynamoDBExecutionRole",
    ),
    (
        "Kinesis",
        "iam::aws:policy/service-role/AWSLambdaKinesisExecutionRole",
    ),
    (
        "SQS",
        "iam::aws:policy/service-role/AWSLambdaSQSQueueExecutionRole",
    ),
    (
        "MSK",
        "iam::aws:policy/service-role/AWSLambdaMSKExecutionRole",
    ),
];

/// The function timeout the transform states when the declaration states none (SAM's
/// documented default for `AWS::Serverless::Function$Timeout`, which is Lambda's own
/// default of 3 seconds).
const DEFAULT_TIMEOUT: u64 = 3;

/// The members an `AWS::Serverless::Function` and an `AWS::Lambda::Function` spell the same
/// way, which the expansion carries straight through.
const FUNCTION_PASSTHROUGH: &[&str] = &[
    "Architectures",
    "CodeSigningConfigArn",
    "Description",
    "Environment",
    "EphemeralStorage",
    "FileSystemConfigs",
    "FunctionName",
    "Handler",
    "ImageConfig",
    "KmsKeyArn",
    "Layers",
    "LoggingConfig",
    "MemorySize",
    "PackageType",
    "RecursiveLoop",
    "ReservedConcurrentExecutions",
    "Runtime",
    "RuntimeManagementConfig",
    "SnapStart",
    "Timeout",
    "VpcConfig",
];

/// The members an `AWS::Serverless::Api` and an `AWS::ApiGateway::RestApi` spell the same
/// way.
const REST_API_PASSTHROUGH: &[&str] = &[
    "BinaryMediaTypes",
    "Description",
    "DisableExecuteApiEndpoint",
    "EndpointConfiguration",
    "MinimumCompressionSize",
    "Mode",
    "Name",
    "Policy",
];

/// The members an `AWS::Serverless::Api`'s stage takes from the declaration, spelled the
/// same way on `AWS::ApiGateway::Stage`.
const REST_STAGE_PASSTHROUGH: &[&str] = &[
    "AccessLogSetting",
    "CacheClusterEnabled",
    "CacheClusterSize",
    "CanarySetting",
    "MethodSettings",
    "TracingEnabled",
    "Variables",
];

/// The members an `AWS::Serverless::SimpleTable` and an `AWS::DynamoDB::Table` spell the
/// same way.
const TABLE_PASSTHROUGH: &[&str] = &[
    "ProvisionedThroughput",
    "SSESpecification",
    "TableName",
    "PointInTimeRecoverySpecification",
];

/// The event types that become an `AWS::Lambda::EventSourceMapping`, each with the member
/// the event's source ARN is spelled under on that resource: a stream source is an
/// `EventSourceArn`, a self-managed broker a `SelfManagedEventSource`.
const EVENT_SOURCE_MAPPINGS: &[(&str, &str)] = &[
    ("SQS", "Queue"),
    ("DynamoDB", "Stream"),
    ("Kinesis", "Stream"),
    ("MSK", "Stream"),
    ("MQ", "Broker"),
    ("DocumentDB", "Cluster"),
];

/// The members an event-source-mapping event and `AWS::Lambda::EventSourceMapping` spell
/// the same way.
const EVENT_SOURCE_PASSTHROUGH: &[&str] = &[
    "BatchSize",
    "BisectBatchOnFunctionError",
    "DestinationConfig",
    "Enabled",
    "FilterCriteria",
    "FunctionResponseTypes",
    "MaximumBatchingWindowInSeconds",
    "MaximumRecordAgeInSeconds",
    "MaximumRetryAttempts",
    "ParallelizationFactor",
    "ScalingConfig",
    "StartingPosition",
    "StartingPositionTimestamp",
    "TumblingWindowInSeconds",
];

/// The `RuleState` values an `AWS::Events::Rule` the transform synthesizes is put in force
/// with: "the state of the rule" EventBridge takes at `PutRule$State`, where "ENABLED"
/// means the rule matches published events and "DISABLED" means it matches none.
const RULE_ENABLED: &str = "ENABLED";
const RULE_DISABLED: &str = "DISABLED";

/// Expand the template document in place, where it declares [`TRANSFORM`].
///
/// Answers whether the expansion CHANGED anything — a document declaring any other
/// transform, or none, is left exactly as it was read, because this is the one macro this
/// emulator processes, and so is one whose declarations the macro has nothing to expand.
/// Its caller needs to know, because a processed template that differs from the submitted
/// one is the only reason to report anything but the submitted body itself.
pub fn expand(document: &mut Value) -> bool {
    expand_with_parameters(document, &BTreeMap::new())
}

/// [`expand`] with the PARAMETER VALUES in force on the deployment, which is how the stage
/// a stack deploys is expanded.
///
/// One property of the expansion reads a parameter: the `AutoPublishCodeSha256` the
/// generated version's logical id carries, which a declaration states as `!Ref` of a
/// parameter precisely so that each deployment can state a different value for it
/// ([`stated_code_sha`]). Every other part of the expansion is a function of the document
/// alone, so a caller with no parameter values in hand expands the same document the same
/// way.
pub fn expand_with_parameters(
    document: &mut Value,
    parameters: &BTreeMap<String, String>,
) -> bool {
    if !declares_the_transform(document) {
        return false;
    }
    let Some(resources) = document.get("Resources").and_then(Value::as_object).cloned() else {
        return false;
    };
    let globals = document.get("Globals").cloned().unwrap_or(Value::Null);
    let expansion = expanded_resources(&resources, &globals, parameters);
    if expansion.resources == resources && expansion.renamed.is_empty() {
        return false;
    }
    match document.as_object_mut() {
        Some(sections) => {
            sections.insert("Resources".to_string(), Value::Object(expansion.resources));
            // `Globals` is the macro's own section, and the expansion resolved it into
            // every resource it applies to: the processed template no longer carries it.
            sections.remove("Globals");
            // A declaration the expansion RENAMED is named by its own logical id wherever
            // the template refers to it, and the template CloudFormation goes on to plan
            // and deploy is this one: every such reference is re-pointed at the id the
            // expansion gave the resource, in every section that carries one.
            for section in sections.values_mut() {
                rename_references(section, &expansion.renamed, 0);
            }
            true
        }
        None => false,
    }
}

/// What the transform made of a template's `Resources` section.
#[derive(Debug, Default)]
struct Expansion {
    /// The `Resources` section the expansion produced.
    resources: Map<String, Value>,
    /// Each declaration whose logical id the expansion CHANGED, against the id its
    /// resource now carries — what [`rename_references`] re-points the references to.
    renamed: BTreeMap<String, String>,
}

/// Whether the `Transform` section names the SAM macro, in any of the spellings the section
/// admits — read by the one reader of that section
/// ([`super::template::declared_transforms`]), so the two can never disagree about what a
/// template declares.
fn declares_the_transform(document: &Value) -> bool {
    declared_transforms(document.get("Transform"))
        .iter()
        .any(|name| name == TRANSFORM)
}

/// The `Resources` section the transform produces from the one the template declares, with
/// the logical ids it renamed on the way.
fn expanded_resources(
    resources: &Map<String, Value>,
    globals: &Value,
    parameters: &BTreeMap<String, String>,
) -> Expansion {
    // The functions' properties, resolved against `Globals` once: the API events are read
    // off them, and so is each function's own expansion.
    let functions: Vec<(String, Value)> = resources
        .iter()
        .filter(|(_, declaration)| resource_type(declaration) == Some(FUNCTION))
        .map(|(logical_id, declaration)| {
            (
                logical_id.clone(),
                merged(globals.get("Function"), declaration.get("Properties"), 0),
            )
        })
        .collect();
    let stages = api_stages(resources, globals);
    let events = api_events(&functions, &stages);

    // `primary` carries the resource each declaration BECOMES (and every resource the
    // template declares itself); `added` the ones the transform synthesizes beside it. They
    // are kept apart so a synthesized id can never displace a resource the template
    // declares under that id — the declaration wins, which is what merging `added` under
    // `or_insert` below does.
    let mut primary: Map<String, Value> = Map::new();
    let mut added: Map<String, Value> = Map::new();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for (logical_id, declaration) in resources {
        let condition = declaration.get("Condition");
        match resource_type(declaration) {
            Some(FUNCTION) => {
                let properties = functions
                    .iter()
                    .find(|(id, _)| id == logical_id)
                    .map(|(_, properties)| properties.clone())
                    .unwrap_or(Value::Null);
                expand_function(
                    &mut primary,
                    &mut added,
                    (logical_id, declaration, &properties),
                    &events,
                    parameters,
                );
            }
            Some(API) => {
                let properties = merged(globals.get("Api"), declaration.get("Properties"), 0);
                expand_rest_api(
                    &mut primary,
                    &mut added,
                    (logical_id, declaration, &properties),
                    &api_definition_paths(&events, logical_id, true),
                );
            }
            Some(HTTP_API) => {
                let properties =
                    merged(globals.get("HttpApi"), declaration.get("Properties"), 0);
                expand_http_api(
                    &mut primary,
                    &mut added,
                    (logical_id, declaration, &properties),
                    &api_definition_paths(&events, logical_id, false),
                );
            }
            // A WEBSOCKET api is the one v2 api that is not imported from a definition: its
            // declaration states the api's own members, and the stage beside it is what
            // deploys the api.
            Some(WEBSOCKET_API) => {
                expand_websocket_api(
                    &mut primary,
                    &mut added,
                    (
                        logical_id,
                        declaration,
                        &declaration.get("Properties").cloned().unwrap_or(Value::Null),
                    ),
                );
            }
            Some(SIMPLE_TABLE) => {
                let properties =
                    merged(globals.get("SimpleTable"), declaration.get("Properties"), 0);
                primary.insert(
                    logical_id.clone(),
                    carried_over(
                        declaration,
                        "AWS::DynamoDB::Table",
                        expanded_table(&properties),
                    ),
                );
            }
            Some(LAYER_VERSION) => {
                let properties = object(declaration.get("Properties"));
                let expanded = expanded_layer_version(&properties);
                // "AWS SAM … appends a hash to the logical ID": a layer version cannot be
                // updated in place, so each content is its own resource — and every
                // reference to the declaration is re-pointed at the id that gives it
                // ([`rename_references`]).
                let hashed = format!("{logical_id}{}", digest(&Value::Object(expanded.clone())));
                let mut layer = carried_over(declaration, "AWS::Lambda::LayerVersion", expanded);
                retained(&mut layer, &properties);
                primary.insert(hashed.clone(), layer);
                if &hashed != logical_id {
                    renamed.insert(logical_id.clone(), hashed);
                }
            }
            Some(STATE_MACHINE) => {
                let properties = object(declaration.get("Properties"));
                let (expanded, role) = expanded_state_machine(logical_id, &properties);
                primary.insert(
                    logical_id.clone(),
                    carried_over(declaration, "AWS::StepFunctions::StateMachine", expanded),
                );
                if let Some(role) = role {
                    added.insert(format!("{logical_id}Role"), conditioned(role, condition));
                }
            }
            // A CONNECTOR is the GRANT between the two resources it names rather than a
            // resource of its own: where its profile publishes a policy on the role the source
            // executes as, that policy is what the declaration becomes ([`connector`]) and the
            // declaration itself is gone, as every expanded type is.
            Some(CONNECTOR) => match connector::expanded(logical_id, declaration, resources) {
                Some((policy_id, policy)) => {
                    added.insert(policy_id, conditioned(policy, condition));
                }
                None => {
                    primary.insert(logical_id.clone(), declaration.clone());
                }
            },
            // A type the transform does not expand (an `AWS::Serverless::Application`,
            // whose template lives in the Serverless Application Repository) stays exactly
            // as declared, and so does every resource the template writes out itself.
            _ => {
                primary.insert(logical_id.clone(), declaration.clone());
            }
        }
    }

    // The implicit APIs: one REST API for every `Api` event that named no `RestApiId`, one
    // HTTP API for every `HttpApi` event that named no `ApiId`.
    implicit_api(&mut added, &events, globals, true);
    implicit_api(&mut added, &events, globals, false);

    // The OBJECT events: each one is written into the bucket resource it names, which is a
    // resource of this same template, so it is done once every declaration has taken its
    // place in `primary` ([`object_events`]).
    expand_object_events(&mut primary, &mut added, resources, &functions);

    let mut out = primary;
    for (logical_id, resource) in added {
        out.entry(logical_id).or_insert(resource);
    }
    Expansion {
        resources: out,
        renamed,
    }
}

/// Expand every `S3` event source the functions declare: the grant Amazon S3 needs on the
/// function, and the notification configuration written into the bucket the source names.
///
/// Done after every declaration has taken its place in `primary`, because what an object event
/// produces is a change to ANOTHER resource of the same template — the bucket's own
/// `NotificationConfiguration` — rather than a resource of its own ([`object_events`] states
/// the whole rule). A source whose bucket this template does not declare contributes nothing,
/// so a stack is never left with a grant for a configuration nothing carries.
fn expand_object_events(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    resources: &Map<String, Value>,
    functions: &[(String, Value)],
) {
    for (logical_id, properties) in functions {
        let condition = resources
            .get(logical_id)
            .and_then(|declaration| declaration.get("Condition"))
            .cloned();
        for (event, declaration) in object(properties.get("Events")) {
            if declaration.get("Type").and_then(Value::as_str) != Some(object_events::S3_EVENT) {
                continue;
            }
            let event_properties = object(declaration.get("Properties"));
            let Some(source) = object_events::object_event(
                logical_id,
                &event,
                &event_properties,
                condition.as_ref(),
            ) else {
                continue;
            };
            // The bucket the source names has to be one this template declares, since the
            // configuration is written into it; a source naming anything else is left alone.
            let Some(bucket) = primary.get_mut(&source.bucket) else {
                continue;
            };
            object_events::inject(bucket, &source);
            added.insert(
                source.permission_id.clone(),
                conditioned(
                    resource(
                        "AWS::Lambda::Permission",
                        object_events::invoke_permission(&source),
                    ),
                    source.condition.as_ref(),
                ),
            );
        }
    }
}

/// One API event a function declares: what it contributes to its API's definition, and the
/// permission that lets API Gateway invoke the function.
#[derive(Debug, Clone)]
struct ApiEvent {
    /// The function the event triggers.
    function: String,
    /// The key the function's `Events` map carries the event under.
    event: String,
    /// `true` for an `Api` event (REST, `AWS::ApiGateway`), `false` for a `HttpApi` one.
    rest: bool,
    /// The logical id of the API the event is routed through.
    api: String,
    /// Whether that API is one the transform synthesizes.
    implicit: bool,
    /// `Path`, as the event states it.
    path: String,
    /// `Method`, lowercased; `any` for the event that answers every method.
    method: String,
    /// The stage the API is deployed to, where its name is known: what the permission's
    /// logical id ends with.
    stage: Option<String>,
}

impl ApiEvent {
    /// The logical id of the permission: the function's id, the event's key, `Permission`
    /// and the stage's name (`MyFunctionApiEventPermissionProd`).
    fn permission_id(&self) -> String {
        format!(
            "{}{}Permission{}",
            self.function,
            self.event,
            self.stage.as_deref().unwrap_or_default()
        )
    }
}

/// Every API event the template's functions declare, in the order the functions and their
/// `Events` maps are walked.
fn api_events(
    functions: &[(String, Value)],
    stages: &Map<String, Value>,
) -> Vec<ApiEvent> {
    let mut events = Vec::new();
    for (function, properties) in functions {
        for (event, declaration) in object(properties.get("Events")) {
            let kind = declaration.get("Type").and_then(Value::as_str);
            let rest = match kind {
                Some("Api") => true,
                Some("HttpApi") => false,
                _ => continue,
            };
            let event_properties = object(declaration.get("Properties"));
            // "Path … Method" are what a route IS; an event stating neither routes nothing,
            // and an `HttpApi` event may state `Path`/`Method` or neither (it then answers
            // every route of the API, which is not a route this expansion can write).
            let (Some(path), Some(method)) = (
                event_properties.get("Path").and_then(Value::as_str),
                event_properties.get("Method").and_then(Value::as_str),
            ) else {
                continue;
            };
            let declared_api = event_properties
                .get(if rest { "RestApiId" } else { "ApiId" })
                .and_then(referenced_logical_id);
            let implicit = declared_api.is_none();
            let api = declared_api.unwrap_or_else(|| {
                if rest {
                    IMPLICIT_REST_API.to_string()
                } else {
                    IMPLICIT_HTTP_API.to_string()
                }
            });
            let stage = if implicit && rest {
                Some(IMPLICIT_STAGE.to_string())
            } else if implicit {
                None
            } else {
                stages.get(&api).and_then(Value::as_str).map(logical_fragment)
            };
            events.push(ApiEvent {
                function: function.clone(),
                event: event.clone(),
                rest,
                api,
                implicit,
                path: path.to_string(),
                method: method.to_ascii_lowercase(),
                stage,
            });
        }
    }
    events
}

/// Each explicitly declared API's stage name, as the declaration states it: the name a
/// stage's logical id and an API permission's logical id are built from.
fn api_stages(resources: &Map<String, Value>, globals: &Value) -> Map<String, Value> {
    let mut stages = Map::new();
    for (logical_id, declaration) in resources {
        let section = match resource_type(declaration) {
            Some(API) => "Api",
            Some(HTTP_API) => "HttpApi",
            _ => continue,
        };
        let properties = merged(globals.get(section), declaration.get("Properties"), 0);
        if let Some(name) = properties.get(STAGE_NAME_PROPERTY) {
            stages.insert(logical_id.clone(), name.clone());
        }
    }
    stages
}

/// Expand one `AWS::Serverless::Function`: the Lambda function, the role it executes as,
/// and one resource per event source it declares.
fn expand_function(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    declared: (&String, &Value, &Value),
    events: &[ApiEvent],
    parameters: &BTreeMap<String, String>,
) {
    let (logical_id, declaration, properties) = declared;
    let condition = declaration.get("Condition");
    let mut function = Map::new();
    if let Some(code) = function_code(properties) {
        function.insert("Code".to_string(), code);
    }
    copy_through(&mut function, properties, FUNCTION_PASSTHROUGH);
    // "Tracing: … the function's X-Ray tracing mode", which Lambda spells as a
    // `TracingConfig$Mode`.
    if let Some(mode) = properties.get("Tracing") {
        function.insert("TracingConfig".to_string(), json!({ "Mode": mode }));
    }
    // "DeadLetterQueue: … TargetArn: the ARN of the queue or topic", which Lambda spells as
    // a `DeadLetterConfig$TargetArn`.
    if let Some(target) = properties.get("DeadLetterQueue").and_then(|queue| queue.get("TargetArn"))
    {
        function.insert(
            "DeadLetterConfig".to_string(),
            json!({ "TargetArn": target }),
        );
    }
    if let Some(uri) = properties.get("ImageUri") {
        function.insert("Code".to_string(), json!({ "ImageUri": uri }));
        function.insert("PackageType".to_string(), json!("Image"));
    }
    function
        .entry("Timeout".to_string())
        .or_insert_with(|| json!(DEFAULT_TIMEOUT));
    // "AutoPublishCodeSha256 … use this property to … publish a new version": AWS Lambda
    // "doesn't publish a version if the function's configuration and code haven't changed
    // since the last version", so what makes the publication below a NEW version is that
    // the value the declaration states travels into the FUNCTION's own description — a
    // configuration that differs exactly when that value does. It is stated as the
    // declaration wrote it, so a value spelled as a `Ref` to a parameter is the one
    // CloudFormation resolves for this deployment. A description the declaration states
    // itself wins, as every other property of the function does.
    if let Some(sha) = auto_published_sha(properties) {
        function
            .entry("Description".to_string())
            .or_insert_with(|| sha.clone());
    }
    function.insert(
        "Tags".to_string(),
        tag_list(properties.get("Tags"), Some(CREATED_BY_TAG)),
    );
    // "Role: … If this property isn't specified, AWS SAM creates an IAM role for you."
    match properties.get("Role") {
        Some(role) => {
            function.insert("Role".to_string(), role.clone());
        }
        None => {
            function.insert(
                "Role".to_string(),
                json!({ "Fn::GetAtt": [format!("{logical_id}Role"), "Arn"] }),
            );
            added.insert(
                format!("{logical_id}Role"),
                conditioned(execution_role(logical_id, properties), condition),
            );
        }
    }
    let published_code = function.get("Code").cloned();
    primary.insert(
        logical_id.clone(),
        carried_over(declaration, "AWS::Lambda::Function", function),
    );
    auto_published_version(
        added,
        (logical_id, condition, properties),
        published_code.as_ref(),
        parameters,
    );

    for event in events.iter().filter(|event| &event.function == logical_id) {
        added.insert(
            event.permission_id(),
            conditioned(invoke_permission(event, logical_id), condition),
        );
    }
    for (event, declaration) in object(properties.get("Events")) {
        let Some(kind) = declaration.get("Type").and_then(Value::as_str) else {
            continue;
        };
        let event_properties = object(declaration.get("Properties"));
        let logical = format!("{logical_id}{event}");
        if let Some((_, source)) = EVENT_SOURCE_MAPPINGS
            .iter()
            .find(|(event_type, _)| *event_type == kind)
        {
            let Some(mapping) = event_source_mapping(logical_id, source, &event_properties) else {
                continue;
            };
            added.insert(
                logical,
                conditioned(
                    resource("AWS::Lambda::EventSourceMapping", mapping),
                    condition,
                ),
            );
            continue;
        }
        match kind {
            "Schedule" | "ScheduleV2" | "CloudWatchEvent" | "EventBridgeRule" => {
                let Some(rule) = event_rule(logical_id, &logical, &event_properties) else {
                    continue;
                };
                added.insert(
                    logical.clone(),
                    conditioned(resource("AWS::Events::Rule", rule), condition),
                );
                added.insert(
                    format!("{logical}Permission"),
                    conditioned(
                        resource(
                            "AWS::Lambda::Permission",
                            principal_permission(
                                logical_id,
                                "events.amazonaws.com",
                                json!({ "Fn::GetAtt": [logical.clone(), "Arn"] }),
                            ),
                        ),
                        condition,
                    ),
                );
            }
            "SNS" => {
                let Some(topic) = event_properties.get("Topic") else {
                    continue;
                };
                let mut subscription = Map::new();
                subscription.insert("Protocol".to_string(), json!("lambda"));
                subscription.insert("TopicArn".to_string(), topic.clone());
                subscription.insert(
                    "Endpoint".to_string(),
                    json!({ "Fn::GetAtt": [logical_id.clone(), "Arn"] }),
                );
                copy_through(
                    &mut subscription,
                    &Value::Object(event_properties.clone()),
                    &["FilterPolicy", "FilterPolicyScope", "RedrivePolicy", "Region"],
                );
                added.insert(
                    logical.clone(),
                    conditioned(resource("AWS::SNS::Subscription", subscription), condition),
                );
                added.insert(
                    format!("{logical}Permission"),
                    conditioned(
                        resource(
                            "AWS::Lambda::Permission",
                            principal_permission(logical_id, "sns.amazonaws.com", topic.clone()),
                        ),
                        condition,
                    ),
                );
            }
            // Every other event type (a Cognito trigger) rewrites a resource beside the
            // function rather than adding one of its own; nothing is synthesized for it here.
            // An S3 event is one of those — the bucket's own `NotificationConfiguration` is
            // what it writes — and it is expanded once every declaration has taken its place
            // ([`expand_object_events`]).
            _ => {}
        }
    }
}

/// The version and the alias an `AutoPublishAlias` asks for, beside the function.
///
/// "AutoPublishAlias: the name of the Lambda alias … AWS SAM … creates and publishes an
/// `AWS::Lambda::Version` … and creates and updates an `AWS::Lambda::Alias` with the name
/// you provide, pointing to that version." A published version is IMMUTABLE, so each
/// snapshot is a resource of its own: the generated version's logical id carries a hash,
/// exactly as a generated layer version's does, and a deployment whose code changed
/// therefore declares a DIFFERENT version resource — which is what publishes a new version
/// and what the alias is then pointed at.
///
/// The hash is the one SAM's logical-id generator produces: the declaration's
/// `AutoPublishCodeSha256` where it states one — that property exists precisely so a caller
/// can decide when a new version is published — and otherwise a digest of the function's own
/// `Code`, so a changed deployment package is a changed id. Either way it is
/// [`DIGEST_CHARS`] characters, the length SAM appends.
///
/// Nothing is generated for a declaration that states no alias: the version publication is
/// what the alias property asks for.
fn auto_published_version(
    added: &mut Map<String, Value>,
    declared: (&String, Option<&Value>, &Value),
    code: Option<&Value>,
    parameters: &BTreeMap<String, String>,
) {
    let (logical_id, condition, properties) = declared;
    let Some(alias) = properties
        .get(AUTO_PUBLISH_ALIAS_PROPERTY)
        .filter(|alias| !alias.is_null())
    else {
        return;
    };
    let hash = stated_code_sha(properties, parameters).unwrap_or_else(|| {
        digest(&with_resolved_parameters(
            code.unwrap_or(&Value::Null),
            parameters,
            0,
        ))
    });
    let version_id = format!("{logical_id}{VERSION_ID_INFIX}{hash}");

    let mut version = Map::new();
    version.insert("FunctionName".to_string(), json!({ REF: logical_id }));
    if let Some(description) = properties.get(VERSION_DESCRIPTION_PROPERTY) {
        version.insert("Description".to_string(), description.clone());
    }
    let mut published = resource("AWS::Lambda::Version", version);
    // The hashed logical id above makes each snapshot a resource of its own, so a changed
    // code content REMOVES the resource the stack had — the event `DeletionPolicy` governs.
    // A published version is what an alias can be pointed BACK at, so the versions a stack
    // published are retained rather than taken away by the deployment that publishes the
    // next one, exactly as the generated layer version's are ([`retained`]).
    if let Some(members) = published.as_object_mut() {
        members.insert(
            DELETION_POLICY.to_string(),
            json!(super::deletion_policy::RETAIN),
        );
    }
    added.insert(version_id.clone(), conditioned(published, condition));

    let mut generated = Map::new();
    generated.insert("Name".to_string(), alias.clone());
    generated.insert("FunctionName".to_string(), json!({ REF: logical_id }));
    // The alias invokes the version this expansion just declared, read off that resource —
    // "Version: the version number" — so the alias is pointed at the version Lambda
    // published rather than at a number the template guessed.
    generated.insert(
        "FunctionVersion".to_string(),
        json!({ "Fn::GetAtt": [version_id, "Version"] }),
    );
    if let Some(config) = properties.get(PROVISIONED_CONCURRENCY_PROPERTY) {
        generated.insert(PROVISIONED_CONCURRENCY_PROPERTY.to_string(), config.clone());
    }
    let named = alias.as_str().map(logical_fragment).unwrap_or_default();
    added.insert(
        format!("{logical_id}{ALIAS_ID_INFIX}{named}"),
        conditioned(resource("AWS::Lambda::Alias", generated), condition),
    );
}

/// `value` with every reference to a template PARAMETER replaced by a FINGERPRINT of the
/// value in force for it ([`parameter_fingerprint`]): a `Ref` to one, and a parameter
/// interpolated into an `Fn::Sub`'s text.
///
/// What the generated version's hash stands for is the function's CODE, and the code a
/// deployment really deploys is the one those values resolve: a pipeline that restates the
/// handler's source — or the asset object it points at — as a parameter changes the code
/// without changing a character of the template. SAM resolves the template's parameters
/// before it hashes, so the generated id is a function of the code itself and not of the way
/// the template spells it, and a deployment that changes only such a parameter declares a
/// DIFFERENT version resource — which is what publishes the next version.
///
/// Read ONLY to be hashed ([`digest`]), which is why a fingerprint serves: two deployments
/// differ here exactly when a value they read differs, and nothing reads the text itself.
///
/// A reference to anything else — another resource, a pseudo parameter — is left exactly as
/// written: its value is not this expansion's to know, and it is the same text in every
/// deployment of the template anyway. Bounded by [`MAX_TEMPLATE_DEPTH`], as every other walk
/// over a caller-supplied document here is.
fn with_resolved_parameters(
    value: &Value,
    parameters: &BTreeMap<String, String>,
    depth: usize,
) -> Value {
    if depth > MAX_TEMPLATE_DEPTH {
        return value.clone();
    }
    match value {
        Value::Object(members) => {
            if members.len() == 1 {
                if let Some((name, in_force)) = members
                    .get(REF)
                    .and_then(Value::as_str)
                    .and_then(|name| parameters.get(name).map(|in_force| (name, in_force)))
                {
                    return Value::String(parameter_fingerprint(name, in_force));
                }
                if let Some(substitution) = members.get(FN_SUB) {
                    return substituted_value(substitution, parameters, depth);
                }
            }
            Value::Object(
                members
                    .iter()
                    .map(|(name, member)| {
                        (
                            name.clone(),
                            with_resolved_parameters(member, parameters, depth + 1),
                        )
                    })
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| with_resolved_parameters(item, parameters, depth + 1))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// One `Fn::Sub`'s value with its source string substituted, in BOTH forms the function takes
/// ([`rename_substitution`] reads the same two): the source string on its own, or
/// `[ String, { Name: Value, … } ]`.
///
/// The variables of the source string are walked by the crate's own `Fn::Sub` variable walk
/// ([`renamed_substitution`], which trims the variable, reads the name off the dotted
/// `${Name.Attribute}` form and keeps the function's own `${!Literal}` escape), with each
/// parameter in force mapped to its fingerprint — so there is one reader of that form here
/// rather than a second scanner beside it.
///
/// A variable the substitution supplies ITSELF shadows a parameter of the same name, so it is
/// left as written and the supplied value — which may be a `Ref` to a parameter — is
/// fingerprinted where it stands, at this value's own depth.
fn substituted_value(
    substitution: &Value,
    parameters: &BTreeMap<String, String>,
    depth: usize,
) -> Value {
    let fingerprints: BTreeMap<String, String> = parameters
        .iter()
        .map(|(name, value)| (name.clone(), parameter_fingerprint(name, value)))
        .collect();
    match substitution {
        Value::String(source) => {
            Value::String(renamed_substitution(source, &fingerprints, &[]))
        }
        Value::Array(parts) => {
            let supplied: Vec<String> = parts
                .get(1)
                .and_then(Value::as_object)
                .map(|variables| variables.keys().cloned().collect())
                .unwrap_or_default();
            let mut resolved: Vec<Value> = Vec::with_capacity(parts.len());
            for (at, part) in parts.iter().enumerate() {
                match (at, part) {
                    (0, Value::String(source)) => resolved.push(Value::String(
                        renamed_substitution(source, &fingerprints, &supplied),
                    )),
                    _ => resolved.push(with_resolved_parameters(part, parameters, depth + 1)),
                }
            }
            Value::Array(resolved)
        }
        _ => substitution.clone(),
    }
}

/// What stands in for one parameter's VALUE in the text that is hashed: the parameter's name
/// and a digest of the value in force for it.
///
/// Two deployments of one template therefore differ here exactly when a value they read
/// differs, which is all the hash has to tell apart — while the stand-in's own length is
/// [`DIGEST_CHARS`] whatever the value's is. That is what keeps the work BOUNDED: both the
/// text and the values are the REQUESTER's, so inlining a value would make the produced text
/// the product of the occurrence count and the value's length, neither of which this service
/// bounds.
fn parameter_fingerprint(name: &str, value: &str) -> String {
    format!(
        "{name}={}",
        super::template::content_digest(PARAMETER_DIGEST_SALT, value, DIGEST_CHARS)
    )
}

/// What a parameter value's fingerprint is derived under, which keeps it apart from every
/// other digest this module takes.
const PARAMETER_DIGEST_SALT: &str = "sam-parameter";

/// The `AutoPublishCodeSha256` a declaration states, as it wrote it, where the declaration
/// asks for the automatic publication at all — the property has meaning only beside
/// `AutoPublishAlias`, which is what publishes a version.
fn auto_published_sha(properties: &Value) -> Option<&Value> {
    properties
        .get(AUTO_PUBLISH_ALIAS_PROPERTY)
        .filter(|alias| !alias.is_null())?;
    properties
        .get(AUTO_PUBLISH_SHA_PROPERTY)
        .filter(|sha| !sha.is_null())
}

/// The hash the generated version's logical id carries where the declaration STATES one:
/// [`DIGEST_CHARS`] characters of its `AutoPublishCodeSha256`.
///
/// The property is "a string that, when changed, triggers a new version to be published",
/// and what a caller changes per deployment is normally a template PARAMETER — so a value
/// stated as `!Ref` of one is read as the value in force for that parameter, which is the
/// same resolution SAM performs before it hashes. `None` where the declaration states none,
/// or states one from something no parameter value resolves: the hash is then the digest of
/// the function's own code, which changes exactly when the code does.
fn stated_code_sha(properties: &Value, parameters: &BTreeMap<String, String>) -> Option<String> {
    let stated = properties.get(AUTO_PUBLISH_SHA_PROPERTY)?;
    let stated = match stated {
        Value::String(stated) => stated.clone(),
        Value::Object(_) => {
            let name = stated.get(REF).and_then(Value::as_str)?;
            parameters.get(name)?.clone()
        }
        _ => return None,
    };
    let hash: String = stated.chars().take(DIGEST_CHARS).collect();
    (!hash.is_empty()).then_some(hash)
}

/// The `Code` a function's declaration states: "CodeUri: The Amazon S3 URI … or a local
/// path", "InlineCode: the function code itself".
fn function_code(properties: &Value) -> Option<Value> {
    if let Some(code) = properties.get("InlineCode") {
        return Some(json!({ "ZipFile": code }));
    }
    lambda_s3_location(properties.get("CodeUri")?)
}

/// The execution role the transform creates for a function that names none: the role Lambda
/// assumes, carrying the basic execution managed policy, the managed policies the
/// declaration names, the one each of its event sources needs and one inline policy per
/// other `Policies` entry.
fn execution_role(logical_id: &str, properties: &Value) -> Value {
    let (declared_managed, inline) = role_policies(logical_id, properties);
    let mut managed = vec![json!(arn(BASIC_EXECUTION_POLICY))];
    managed.extend(declared_managed);
    for policy in event_source_policies(properties) {
        // A policy is either attached or it is not, so two events needing the same one
        // attach it once.
        if !managed.contains(&policy) {
            managed.push(policy);
        }
    }
    let mut role = Map::new();
    role.insert(
        "AssumeRolePolicyDocument".to_string(),
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Action": ["sts:AssumeRole"],
                "Effect": "Allow",
                "Principal": { "Service": ["lambda.amazonaws.com"] },
            }],
        }),
    );
    role.insert("ManagedPolicyArns".to_string(), Value::Array(managed));
    if !inline.is_empty() {
        role.insert("Policies".to_string(), Value::Array(inline));
    }
    if let Some(boundary) = properties.get("PermissionsBoundary") {
        role.insert("PermissionsBoundary".to_string(), boundary.clone());
    }
    role.insert(
        "Tags".to_string(),
        tag_list(properties.get("Tags"), Some(CREATED_BY_TAG)),
    );
    resource("AWS::IAM::Role", role)
}

/// The managed policies the function's own EVENT SOURCES need on the role it executes as,
/// one per source type that publishes one ([`EVENT_SOURCE_POLICIES`]), in the order the
/// `Events` entries are walked and without repeats.
fn event_source_policies(properties: &Value) -> Vec<Value> {
    let mut policies: Vec<Value> = Vec::new();
    for declaration in object(properties.get("Events")).values() {
        let Some(kind) = declaration.get("Type").and_then(Value::as_str) else {
            continue;
        };
        let Some((_, policy)) = EVENT_SOURCE_POLICIES
            .iter()
            .find(|(event_type, _)| *event_type == kind)
        else {
            continue;
        };
        let policy = json!(arn(policy));
        if !policies.contains(&policy) {
            policies.push(policy);
        }
    }
    policies
}

/// `Policies` as the list of entries it states, whichever of its spellings the declaration
/// uses: "This property accepts a single value or a list of values."
fn declared_policies(policies: Option<&Value>) -> Vec<Value> {
    match policies {
        Some(Value::Array(entries)) => entries.clone(),
        Some(entry) => vec![entry.clone()],
        None => Vec::new(),
    }
}

/// What a `Policies` declaration puts on the role the transform generates for it: the
/// managed policy ARNs to attach, and the INLINE policies to embed.
///
/// Each entry is one of the three things the property accepts, and every entry that is not
/// a managed policy becomes exactly ONE inline policy, named for the resource the role
/// belongs to and the entry's POSITION in the declared list
/// (`<LogicalId>RolePolicy<position>`), so the role embeds the entries in declaration
/// order:
///
/// * "a list of … AWS managed policy names, or … policy ARNs" — a string names a policy to
///   attach, which is a `ManagedPolicyArns` entry and no inline policy at all, under the ARN
///   AWS publishes that policy at where the entry names it by NAME
///   ([`managed_policy_arn`]);
/// * an IAM policy document, which `Statement` is what makes a value one — embedded as it
///   is stated;
/// * "a list of AWS SAM policy templates" — a single-member object naming a published
///   template and the parameters it is applied with, embedded as the document that template
///   grants ([`policy_templates`]).
///
/// An entry that is none of those three (an object that is no document and names no
/// template this module carries) contributes no inline policy, rather than a document
/// nothing computed.
fn role_policies(logical_id: &str, properties: &Value) -> (Vec<Value>, Vec<Value>) {
    let mut managed: Vec<Value> = Vec::new();
    let mut inline: Vec<Value> = Vec::new();
    for (position, policy) in declared_policies(properties.get("Policies"))
        .iter()
        .enumerate()
    {
        let document = match policy {
            Value::String(declared) => {
                managed.push(json!(managed_policy_arn(declared)));
                continue;
            }
            Value::Object(_) if policy.get("Statement").is_some() => Some(policy.clone()),
            Value::Object(entry) => named_policy_template(entry)
                .and_then(|(name, parameters)| policy_templates::document(name, parameters)),
            _ => None,
        };
        if let Some(document) = document {
            inline.push(json!({
                "PolicyName": format!("{logical_id}RolePolicy{position}"),
                "PolicyDocument": document,
            }));
        }
    }
    (managed, inline)
}

/// The ARN the managed policy a `Policies` STRING entry names is attached under.
///
/// "Each value can be the name of an AWS managed policy" or that policy's ARN, so an entry
/// that is already an ARN is attached exactly as the declaration spells it, and a bare NAME
/// is resolved to the ARN AWS publishes that policy under:
/// `arn:<partition>:iam::aws:policy<path><name>`, which is the value `ManagedPolicyArns`
/// takes — a name is not one, so a role declared with one is a role IAM refuses.
///
/// WHERE AWS files a published policy is part of that ARN (`/service-role/` for the ones it
/// files there, `/` for the rest), so a name is resolved through IAM's own catalogue of the
/// policies AWS publishes ([`aws_managed_policy_named`]) — the one reading of where a policy
/// lives, so the name and the ARN of one policy never name two different policies.
///
/// A name that catalogue publishes NO policy under resolves to nothing, and is left exactly as
/// the declaration spelled it: `ManagedPolicyArns` takes ARNs, so the role IAM is then asked to
/// create is one it refuses, and the stack rolls back — which is what becomes of a template
/// naming a managed policy that does not exist.
fn managed_policy_arn(declared: &str) -> String {
    if declared.is_empty() || declared.starts_with(ARN_PREFIX) {
        return declared.to_string();
    }
    match aws_managed_policy_named(declared) {
        // The catalogue's own spelling of the policy, which is the identity the ARN names.
        Some(entry) => arn(&format!(
            "iam::aws:policy{}{}",
            entry.path, entry.policy_name
        )),
        None => declared.to_string(),
    }
}

/// The template a policy-template entry names and the parameters it states for it: a
/// policy template is applied as a ONE-member object, the member's name being the
/// template's and its value the parameters.
fn named_policy_template(entry: &Map<String, Value>) -> Option<(&str, &Value)> {
    match entry.len() {
        1 => entry
            .iter()
            .next()
            .map(|(name, parameters)| (name.as_str(), parameters)),
        _ => None,
    }
}

/// The permission that lets API Gateway invoke the function this event routes to.
fn invoke_permission(event: &ApiEvent, function: &str) -> Value {
    let mut permission = Map::new();
    permission.insert("Action".to_string(), json!("lambda:InvokeFunction"));
    permission.insert(
        "FunctionName".to_string(),
        json!({ "Fn::GetAtt": [function, "Arn"] }),
    );
    permission.insert("Principal".to_string(), json!("apigateway.amazonaws.com"));
    // "SourceArn: … the ARN of the API's method", with the stage left open (`*`) so the
    // permission holds for the deployment the stage points at now and after a redeploy.
    let method = if event.method == "any" {
        "*".to_string()
    } else {
        event.method.to_ascii_uppercase()
    };
    let path = event
        .path
        .trim_start_matches('/')
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "*"
            } else {
                segment
            }
        })
        .collect::<Vec<&str>>()
        .join("/");
    permission.insert(
        "SourceArn".to_string(),
        json!({ "Fn::Sub": [
            arn(&format!(
                "execute-api:${{AWS::Region}}:${{AWS::AccountId}}:\
                 ${{__ApiId__}}/${{__Stage__}}/{method}/{path}"
            )),
            { "__ApiId__": { "Ref": event.api.clone() }, "__Stage__": "*" },
        ]}),
    );
    resource("AWS::Lambda::Permission", permission)
}

/// The permission that lets another AWS service invoke the function.
fn principal_permission(function: &str, principal: &str, source: Value) -> Map<String, Value> {
    let mut permission = Map::new();
    permission.insert("Action".to_string(), json!("lambda:InvokeFunction"));
    permission.insert(
        "FunctionName".to_string(),
        json!({ "Fn::GetAtt": [function, "Arn"] }),
    );
    permission.insert("Principal".to_string(), json!(principal));
    permission.insert("SourceArn".to_string(), source);
    permission
}

/// The event-source mapping a stream, queue or broker event becomes.
fn event_source_mapping(
    function: &str,
    source: &str,
    event_properties: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut mapping = Map::new();
    // `AWS::Lambda::EventSourceMapping$FunctionName` is "the name or ARN of the Lambda
    // function", and the generated mapping names the function by the reference that resolves
    // to the identifier CloudFormation gives it: "Ref returns the function name" for an
    // `AWS::Lambda::Function`, which is the form the expanded mapping carries.
    mapping.insert("FunctionName".to_string(), json!({ "Ref": function }));
    // The event states its source under its own name (`Queue`, `Stream`, `Broker`); the
    // mapping spells every one of them `EventSourceArn`.
    mapping.insert(
        "EventSourceArn".to_string(),
        event_properties.get(source)?.clone(),
    );
    copy_through(
        &mut mapping,
        &Value::Object(event_properties.clone()),
        EVENT_SOURCE_PASSTHROUGH,
    );
    Some(mapping)
}

/// The EventBridge rule a schedule or event-pattern event becomes, with the function as its
/// target.
fn event_rule(
    function: &str,
    logical_id: &str,
    event_properties: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut rule = Map::new();
    if let Some(schedule) = event_properties.get("Schedule") {
        rule.insert("ScheduleExpression".to_string(), schedule.clone());
    }
    if let Some(pattern) = event_properties.get("Pattern") {
        rule.insert("EventPattern".to_string(), pattern.clone());
    }
    if rule.is_empty() {
        // Neither a schedule nor a pattern: nothing states when the rule fires.
        return None;
    }
    copy_through(
        &mut rule,
        &Value::Object(event_properties.clone()),
        &["Description", "Name", "State", "EventBusName"],
    );
    // "Enabled: indicates whether the rule is enabled" — the enablement an event source
    // declares as a BOOLEAN is the generated rule's own `State`, which EventBridge spells
    // as one of its named values ([`RULE_ENABLED`] / [`RULE_DISABLED`]), so a declaration
    // that asks for a paused trigger expands into a rule that is paused. A declaration that
    // states `State` itself states that member directly, and it is carried through above, so
    // nothing re-derives it; a value that is no boolean states no enablement at all, and the
    // rule then takes the state EventBridge defaults an omitted `State` to.
    if !rule.contains_key("State") {
        if let Some(enabled) = declared_flag(event_properties.get("Enabled")) {
            rule.insert(
                "State".to_string(),
                json!(if enabled { RULE_ENABLED } else { RULE_DISABLED }),
            );
        }
    }
    rule.insert(
        "Targets".to_string(),
        json!([{
            "Arn": { "Fn::GetAtt": [function, "Arn"] },
            "Id": format!("{logical_id}LambdaTarget"),
        }]),
    );
    Some(rule)
}

/// Expand one `AWS::Serverless::Api` (or the implicit REST API) into the API, its
/// deployment and its stage.
fn expand_rest_api(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    declared: (&String, &Value, &Value),
    paths: &Map<String, Value>,
) {
    let (logical_id, declaration, properties) = declared;
    let condition = declaration.get("Condition");
    let mut api = Map::new();
    copy_through(&mut api, properties, REST_API_PASSTHROUGH);
    // "DefinitionBody: The OpenAPI specification that describes your API", which
    // `AWS::ApiGateway::RestApi` spells `Body`; "DefinitionUri: The Amazon S3 URI … of the
    // OpenAPI document", which it spells `BodyS3Location`. "If neither … is specified, AWS
    // SAM generates [the definition] based on your template configuration" — and the part of
    // that configuration an API's definition is generated from is the routes of the events
    // routed through it, which are merged INTO a definition the declaration states as well:
    // an event routed through an API is a route of it however that API's definition was
    // written. A definition held in Amazon S3 is the one case nothing can be merged into
    // here, since the document is not in the template.
    match properties.get(DEFINITION_URI_PROPERTY).and_then(s3_location) {
        Some(uri) if properties.get(DEFINITION_BODY_PROPERTY).is_none() => {
            api.insert("BodyS3Location".to_string(), uri);
        }
        _ => {
            let mut body = properties
                .get(DEFINITION_BODY_PROPERTY)
                .cloned()
                .unwrap_or_else(rest_definition);
            merge_definition_paths(&mut body, paths);
            // ...and what the declaration says about the AUTHORIZATION of the api's methods,
            // which is part of the document an api is imported from ([`auth`]).
            auth::apply(&mut body, properties);
            api.insert("Body".to_string(), body);
        }
    }
    if let Some(tags) = properties.get("Tags") {
        api.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    let definition = api
        .get("Body")
        .or_else(|| api.get("BodyS3Location"))
        .cloned()
        .unwrap_or_else(|| Value::Object(api.clone()));
    primary.insert(
        logical_id.clone(),
        carried_over(declaration, "AWS::ApiGateway::RestApi", api),
    );

    // The deployment and the stage: "AWS SAM creates an `AWS::ApiGateway::Deployment` and an
    // `AWS::ApiGateway::Stage` for the API."
    let digest = digest(&definition);
    let deployment_id = format!("{logical_id}Deployment{digest}");
    let mut deployment = Map::from_iter([
        (
            "Description".to_string(),
            json!(format!("RestApi deployment id: {digest}")),
        ),
        ("RestApiId".to_string(), json!({ "Ref": logical_id })),
    ]);
    // The deployment declares a stage of its own ([`EXTRA_STAGE`]) unless the declaration
    // states an OpenAPI version, which is what suppresses it. A property written as `null`
    // states nothing, so it reads as an absent one.
    let states_open_api_version = properties
        .get(OPEN_API_VERSION_PROPERTY)
        .is_some_and(|version| !version.is_null());
    if !states_open_api_version {
        deployment.insert(STAGE_NAME_PROPERTY.to_string(), json!(EXTRA_STAGE));
    }
    added.insert(
        deployment_id.clone(),
        conditioned(
            resource("AWS::ApiGateway::Deployment", deployment),
            condition,
        ),
    );
    let stage_name = properties
        .get(STAGE_NAME_PROPERTY)
        .cloned()
        .unwrap_or_else(|| json!(IMPLICIT_STAGE));
    let mut stage = Map::new();
    copy_through(&mut stage, properties, REST_STAGE_PASSTHROUGH);
    stage.insert("RestApiId".to_string(), json!({ "Ref": logical_id }));
    stage.insert("DeploymentId".to_string(), json!({ "Ref": deployment_id }));
    stage.insert("StageName".to_string(), stage_name.clone());
    if let Some(tags) = properties.get("Tags") {
        stage.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    let suffix = stage_name.as_str().map(logical_fragment).unwrap_or_default();
    added.insert(
        format!("{logical_id}{suffix}Stage"),
        conditioned(resource("AWS::ApiGateway::Stage", stage), condition),
    );
}

/// Expand one `AWS::Serverless::HttpApi` (or the implicit HTTP API) into the API and its
/// stage. An HTTP API deploys itself (`AutoDeploy`), so there is no deployment resource.
///
/// An HTTP API is created by IMPORTING its OpenAPI definition, so the definition is the
/// api: it carries the api's name (`info.title`), its description, its CORS configuration
/// and its tags, and "if neither `DefinitionUri` nor `DefinitionBody` are specified, AWS SAM
/// generates [it] based on your template configuration" — the routes of the events routed
/// through this api. That is why the generated resource always carries a definition: an
/// `AWS::ApiGatewayV2::Api` that carries neither a definition nor a `Name` is one the API
/// Gateway v2 create refuses for a null name.
fn expand_http_api(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    declared: (&String, &Value, &Value),
    routes: &Map<String, Value>,
) {
    let (logical_id, declaration, properties) = declared;
    let condition = declaration.get("Condition");
    let mut api = Map::new();
    match properties.get(DEFINITION_URI_PROPERTY).and_then(s3_location) {
        // A definition held in Amazon S3 is not in the template, so nothing can be written
        // into it: what the declaration states about the api travels beside it instead.
        Some(uri) if properties.get(DEFINITION_BODY_PROPERTY).is_none() => {
            api.insert("BodyS3Location".to_string(), uri);
            copy_through(&mut api, properties, &["Description", "Name"]);
            if let Some(tags) = properties.get("Tags") {
                api.insert("Tags".to_string(), tags.clone());
            }
        }
        _ => {
            let mut body = properties
                .get(DEFINITION_BODY_PROPERTY)
                .cloned()
                .unwrap_or_else(http_definition);
            merge_definition_paths(&mut body, routes);
            http_definition_members(&mut body, properties);
            api.insert("Body".to_string(), body);
        }
    }
    // "FailOnWarnings: … whether to roll back the API creation when a warning is
    // encountered" is how the definition is READ rather than part of it, so it is stated on
    // the resource either way.
    copy_through(&mut api, properties, &["FailOnWarnings"]);
    primary.insert(
        logical_id.clone(),
        carried_over(declaration, "AWS::ApiGatewayV2::Api", api),
    );

    let stage_name = properties
        .get(STAGE_NAME_PROPERTY)
        .cloned()
        .unwrap_or_else(|| json!(HTTP_DEFAULT_STAGE));
    let mut stage = Map::new();
    stage.insert("ApiId".to_string(), json!({ "Ref": logical_id }));
    stage.insert("StageName".to_string(), stage_name.clone());
    stage.insert("AutoDeploy".to_string(), json!(true));
    copy_through(
        &mut stage,
        properties,
        &[
            "AccessLogSettings",
            "DefaultRouteSettings",
            "Description",
            "RouteSettings",
            "StageVariables",
        ],
    );
    if let Some(tags) = properties.get("Tags") {
        stage.insert("Tags".to_string(), tags.clone());
    }
    // The `$default` stage's name is not a logical-id fragment, so its stage carries the
    // suffix AWS SAM gives the default stage.
    let suffix = match stage_name.as_str() {
        Some(HTTP_DEFAULT_STAGE) | None => HTTP_DEFAULT_STAGE_SUFFIX.to_string(),
        Some(name) => format!("{}Stage", logical_fragment(name)),
    };
    added.insert(
        format!("{logical_id}{suffix}"),
        conditioned(resource("AWS::ApiGatewayV2::Stage", stage), condition),
    );
}

/// Expand one `AWS::Serverless::WebSocketApi` into the api and the stage it is deployed to.
///
/// A WEBSOCKET api selects a route by evaluating its `RouteSelectionExpression` over each
/// inbound MESSAGE, so nothing about it is written as an OpenAPI document — "to import an HTTP
/// API, you must specify a Body or BodyS3Location" is HTTP-only, and `CreateApi` takes every
/// member such an api carries directly. The expansion is therefore the declaration's own
/// members re-stated on an `AWS::ApiGatewayV2::Api` of that protocol
/// ([`WEBSOCKET_API_PASSTHROUGH`]), whose `Ref` is the `ApiId` API Gateway v2 minted — the api a
/// client reads back through that service's own control plane.
///
/// An api answers no message until a stage of it is deployed, so the expansion deploys it the
/// way it deploys an HTTP api: one `AWS::ApiGatewayV2::Stage` that auto-deploys, on the stage
/// name the declaration states or [`HTTP_DEFAULT_STAGE`] where it states none.
///
/// `Name` is what identifies the api in that control plane and `CreateApi` requires it, so a
/// declaration that states none is named by its own logical id — the only name such a
/// declaration carries.
///
/// Each entry of the declaration's `Routes` is one route of that api, keyed by the route key a
/// message selects it with: the function behind it becomes the api's `AWS_PROXY` integration,
/// the entry becomes the `AWS::ApiGatewayV2::Route` wired to it, and API Gateway is granted
/// permission to invoke that function ([`websocket_route`]), which is what makes the route
/// answer rather than merely exist.
fn expand_websocket_api(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    declared: (&String, &Value, &Value),
) {
    let (logical_id, declaration, properties) = declared;
    let condition = declaration.get("Condition");
    let mut api = Map::new();
    api.insert("ProtocolType".to_string(), json!(WEBSOCKET_PROTOCOL));
    copy_through(&mut api, properties, WEBSOCKET_API_PASSTHROUGH);
    api.entry("Name".to_string())
        .or_insert_with(|| json!(logical_id));
    if let Some(tags) = properties.get("Tags") {
        api.insert("Tags".to_string(), tags.clone());
    }
    primary.insert(
        logical_id.clone(),
        carried_over(declaration, "AWS::ApiGatewayV2::Api", api),
    );

    let stage_name = properties
        .get(STAGE_NAME_PROPERTY)
        .cloned()
        .unwrap_or_else(|| json!(HTTP_DEFAULT_STAGE));
    let mut stage = Map::new();
    stage.insert("ApiId".to_string(), json!({ "Ref": logical_id }));
    stage.insert("StageName".to_string(), stage_name.clone());
    stage.insert("AutoDeploy".to_string(), json!(true));
    copy_through(&mut stage, properties, WEBSOCKET_STAGE_PASSTHROUGH);
    if let Some(tags) = properties.get("Tags") {
        stage.insert("Tags".to_string(), tags.clone());
    }
    // The `$default` stage's name is not a logical-id fragment, so its stage carries the same
    // suffix the default stage of an HTTP api does.
    let suffix = match stage_name.as_str() {
        Some(HTTP_DEFAULT_STAGE) | None => HTTP_DEFAULT_STAGE_SUFFIX.to_string(),
        Some(name) => format!("{}Stage", logical_fragment(name)),
    };
    added.insert(
        format!("{logical_id}{suffix}"),
        conditioned(resource("AWS::ApiGatewayV2::Stage", stage), condition),
    );

    let declared_routes = object(properties.get(WEBSOCKET_ROUTES_PROPERTY));
    let fragments = route_fragments(&declared_routes);
    for (route_key, route) in &declared_routes {
        websocket_route(
            added,
            logical_id,
            condition,
            (route_key, fragments.get(route_key).map_or("", String::as_str)),
            route,
            properties
                .get(WEBSOCKET_AUTH_PROPERTY)
                .and_then(|auth| auth.get(WEBSOCKET_AUTH_TYPE)),
        );
    }
}

/// The logical-id fragment each declared route key contributes, keyed by that route key.
///
/// A route key is the caller's own text and a logical id "must be alphanumeric", so the
/// fragment is the key's alphanumeric characters ([`logical_fragment`]) — which two distinct
/// keys can share (`$connect` and `connect`, `sendMessage` and `send-Message`). A logical id is
/// what separates one expanded route from another, so a fragment two or more declared keys
/// would collapse to carries a digest of the key itself: every declared route key then expands
/// into resources of its own instead of one displacing another's.
fn route_fragments(routes: &Map<String, Value>) -> BTreeMap<String, String> {
    let mut shared: BTreeMap<String, usize> = BTreeMap::new();
    for route_key in routes.keys() {
        *shared
            .entry(capitalized(&logical_fragment(route_key)))
            .or_insert(0) += 1;
    }
    routes
        .keys()
        .map(|route_key| {
            let fragment = capitalized(&logical_fragment(route_key));
            let unique = match shared.get(&fragment) {
                Some(count) if *count > 1 => {
                    format!("{fragment}{}", digest(&json!(route_key)))
                }
                _ => fragment,
            };
            (route_key.clone(), unique)
        })
        .collect()
}

/// The property one WEBSOCKET declaration states its AUTHORIZATION under, and the member of it
/// that names the authorization type.
const WEBSOCKET_AUTH_PROPERTY: &str = "Auth";
const WEBSOCKET_AUTH_TYPE: &str = "AuthType";

/// The route key of the request that OPENS a WebSocket connection: "$connect", the route whose
/// authorization decides whether the connection is opened at all.
///
/// A WEBSOCKET api authorizes the CONNECTION rather than each message — API Gateway accepts an
/// authorization type on this route of such an api, and the routes that carry messages over a
/// connection already opened are reached only through it — so what the declaration states about
/// authorization is stated on this route (`Route$AuthorizationType`: "the authorization type
/// for the route … NONE for open access, AWS_IAM for using AWS IAM permissions").
const WEBSOCKET_CONNECT_ROUTE_KEY: &str = "$connect";

/// The property one WEBSOCKET declaration states its routes under, keyed by the ROUTE KEY each
/// is selected by — `$connect`, `$default` and the keys a message's own selection expression
/// evaluates to.
const WEBSOCKET_ROUTES_PROPERTY: &str = "Routes";

/// The member one route entry names the function behind it with.
const WEBSOCKET_ROUTE_FUNCTION_ARN: &str = "FunctionArn";

/// Expand ONE entry of a WEBSOCKET api's `Routes` into the resources that make that route
/// answer: the api's `AWS_PROXY` integration with the function the entry names, the route under
/// its own key wired to that integration, and the grant that lets API Gateway invoke the
/// function.
///
/// A route is wired to its integration by `Target`, which "names the integration it is wired to
/// as `integrations/<IntegrationId>`" — an identifier API Gateway v2 mints, so the value is
/// composed around a reference to the integration rather than written out.
///
/// The integration addresses the function through the path form an `AWS_PROXY` integration of a
/// WEBSOCKET api carries (`arn:<partition>:apigateway:<region>:lambda:path/2015-03-31/functions/
/// <function-arn>/invocations`), with the ARN the entry states substituted in: that ARN is not
/// known until the function exists, so it travels as the expression the declaration wrote.
///
/// The grant is the one API Gateway needs on the function (`lambda:InvokeFunction` for
/// `apigateway.amazonaws.com`), narrowed to this ROUTE's own `execute-api` ARN —
/// `<api-id>/<stage>/<route-key>` — with the stage left open so it holds for whichever stage of
/// the api serves the route. Without it the function refuses the invocation and the route
/// answers an error instead of the function's response.
///
/// `authorization_type`, where the declaration states one, is stated on the CONNECT route
/// ([`WEBSOCKET_CONNECT_ROUTE_KEY`]) and on no other.
///
/// An entry that names NO function states a route key and no backend for it, which is a route
/// of the api all the same — `CreateRoute` takes a route with no `Target` — so the route is
/// expanded with nothing wired to it rather than the declared key being dropped from the api.
/// There is nothing to grant and nothing to proxy to, so neither the integration nor the
/// permission is synthesized for it.
///
/// `declared` is the entry's route key and the logical-id fragment it was given
/// ([`route_fragments`]), which is unique among the keys of the declaration it came from.
fn websocket_route(
    added: &mut Map<String, Value>,
    api: &str,
    condition: Option<&Value>,
    declared: (&str, &str),
    route: &Value,
    authorization_type: Option<&Value>,
) {
    let (route_key, fragment) = declared;
    let function_arn = route.get(WEBSOCKET_ROUTE_FUNCTION_ARN);
    let integration_id = format!("{api}{fragment}Integration");

    if let Some(function_arn) = function_arn {
        let mut integration = Map::new();
        integration.insert("ApiId".to_string(), json!({ "Ref": api }));
        integration.insert("IntegrationType".to_string(), json!("AWS_PROXY"));
        integration.insert(
            "IntegrationUri".to_string(),
            json!({ "Fn::Sub": [
                arn(
                    "apigateway:${AWS::Region}:lambda:path/2015-03-31/functions/\
                     ${__FunctionArn__}/invocations",
                ),
                { "__FunctionArn__": function_arn.clone() },
            ]}),
        );
        added.insert(
            integration_id.clone(),
            conditioned(
                resource("AWS::ApiGatewayV2::Integration", integration),
                condition,
            ),
        );
    }

    let mut route_resource = Map::new();
    route_resource.insert("ApiId".to_string(), json!({ "Ref": api }));
    route_resource.insert("RouteKey".to_string(), json!(route_key));
    if function_arn.is_some() {
        route_resource.insert(
            "Target".to_string(),
            json!({ "Fn::Join": ["/", ["integrations", { "Ref": integration_id }]] }),
        );
    }
    if let Some(authorization_type) = authorization_type {
        if route_key == WEBSOCKET_CONNECT_ROUTE_KEY {
            route_resource.insert(
                "AuthorizationType".to_string(),
                authorization_type.clone(),
            );
        }
    }
    added.insert(
        format!("{api}{fragment}Route"),
        conditioned(resource("AWS::ApiGatewayV2::Route", route_resource), condition),
    );

    let Some(function_arn) = function_arn else {
        return;
    };
    let mut permission = Map::new();
    permission.insert("Action".to_string(), json!("lambda:InvokeFunction"));
    permission.insert("FunctionName".to_string(), function_arn.clone());
    permission.insert("Principal".to_string(), json!("apigateway.amazonaws.com"));
    permission.insert(
        "SourceArn".to_string(),
        json!({ "Fn::Sub": [
            arn(
                "execute-api:${AWS::Region}:${AWS::AccountId}:\
                 ${__ApiId__}/${__Stage__}/${__RouteKey__}",
            ),
            {
                "__ApiId__": { "Ref": api },
                "__Stage__": "*",
                "__RouteKey__": route_key,
            },
        ]}),
    );
    added.insert(
        format!("{api}{fragment}Permission"),
        conditioned(resource("AWS::Lambda::Permission", permission), condition),
    );
}

/// A logical-id fragment with its first character upper-cased, so the ids the expansion builds
/// out of one read as the CamelCase a logical id is written in.
fn capitalized(fragment: &str) -> String {
    let mut characters = fragment.chars();
    match characters.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), characters.as_str()),
        None => String::new(),
    }
}

/// The implicit API the `Api` (or `HttpApi`) events that named no API of their own are
/// routed through, where the template has any.
fn implicit_api(
    added: &mut Map<String, Value>,
    events: &[ApiEvent],
    globals: &Value,
    rest: bool,
) {
    if !events
        .iter()
        .any(|event| event.implicit && event.rest == rest)
    {
        return;
    }
    let logical_id = if rest {
        IMPLICIT_REST_API.to_string()
    } else {
        IMPLICIT_HTTP_API.to_string()
    };
    // The implicit API is declared by nothing, so it carries no resource attributes: what it
    // states is what the `Globals` section states for every API of its type, and its
    // definition is the routes its events state.
    let declaration = Value::Object(Map::new());
    let properties = implicit_api_properties(globals, rest);
    let paths = api_definition_paths(events, &logical_id, rest);
    let mut primary = Map::new();
    if rest {
        expand_rest_api(
            &mut primary,
            added,
            (&logical_id, &declaration, &properties),
            &paths,
        );
    } else {
        expand_http_api(
            &mut primary,
            added,
            (&logical_id, &declaration, &properties),
            &paths,
        );
    }
    for (id, resource) in primary {
        added.entry(id).or_insert(resource);
    }
}

/// What the implicit API of one type states: the `Globals` section of that type —
/// "properties that you specify in the Globals section … apply to all your serverless
/// functions and APIs", and the API the macro adds is one of them — less the stage's name,
/// which neither section's published property set carries and which for this API is the one
/// the macro names ([`IMPLICIT_STAGE`] for a REST API, [`HTTP_DEFAULT_STAGE`] for an HTTP
/// one).
fn implicit_api_properties(globals: &Value, rest: bool) -> Value {
    let mut properties = object(globals.get(if rest { "Api" } else { "HttpApi" }));
    properties.remove(STAGE_NAME_PROPERTY);
    Value::Object(properties)
}

/// The OpenAPI path items the events routed through ONE api contribute to its definition,
/// keyed by the path each states and, inside that, by its method.
///
/// Every event names the api it is routed through — the implicit one, or the one its
/// `RestApiId`/`ApiId` spells — so this is read for an API the macro adds and for one a
/// declaration spells alike: "if neither `DefinitionUri` nor `DefinitionBody` are specified,
/// AWS SAM generates [the definition] based on your template configuration", and these
/// routes are that configuration.
fn api_definition_paths(events: &[ApiEvent], api: &str, rest: bool) -> Map<String, Value> {
    let mut paths: Map<String, Value> = Map::new();
    for event in events
        .iter()
        .filter(|event| event.rest == rest && event.api == api)
    {
        let item = if rest {
            rest_path_item(&event.function)
        } else {
            http_path_item(&event.function)
        };
        let methods = paths
            .entry(event.path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(methods) = methods.as_object_mut() {
            methods.insert(method_key(&event.method), item);
        }
    }
    paths
}

/// Merge the path items of the events routed through an api into that api's definition.
///
/// A path item the DEFINITION already states WINS: the document is what the client wrote
/// about that route, and the generated item would describe a different integration for it.
/// Each method is merged on its own, so an event adds `post` to a path whose `get` the
/// definition declares. A definition that is not an object (one left as an expression for
/// CloudFormation to resolve) is left exactly as it was written.
fn merge_definition_paths(body: &mut Value, paths: &Map<String, Value>) {
    if paths.is_empty() {
        return;
    }
    let Some(document) = body.as_object_mut() else {
        return;
    };
    let declared = document
        .entry(DEFINITION_PATHS.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(declared) = declared.as_object_mut() else {
        return;
    };
    for (path, methods) in paths {
        let item = declared
            .entry(path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if let Some(methods) = methods.as_object() {
            for (method, operation) in methods {
                item.entry(method.clone())
                    .or_insert_with(|| operation.clone());
            }
        }
    }
}

/// State on an HTTP api's OpenAPI definition what the declaration says about the API
/// ITSELF, since that api is created by importing the document rather than from properties
/// beside it: its `Name` is the definition's `info.title` (which is where an imported api's
/// name comes from), its `Description` the `info.description`, its `CorsConfiguration` the
/// document's CORS extension, its `DisableExecuteApiEndpoint` the endpoint-configuration
/// extension's own member, and its `Tags` the document's `tags`.
///
/// What the DEFINITION states wins everywhere: a declaration that carries a `DefinitionBody`
/// has already said these things inside it.
fn http_definition_members(body: &mut Value, properties: &Value) {
    if let Some(name) = properties.get("Name") {
        state_info_member(body, DEFINITION_TITLE, name.clone());
    }
    if let Some(description) = properties.get("Description") {
        state_info_member(body, DEFINITION_DESCRIPTION, description.clone());
    }
    let Some(document) = body.as_object_mut() else {
        return;
    };
    if let Some(cors) = properties.get("CorsConfiguration").and_then(cors_extension) {
        document
            .entry(CORS_EXTENSION.to_string())
            .or_insert(cors);
    }
    if let Some(disabled) = properties.get("DisableExecuteApiEndpoint") {
        let mut configuration = Map::new();
        configuration.insert(
            DISABLE_EXECUTE_API_ENDPOINT.to_string(),
            disabled.clone(),
        );
        document
            .entry(ENDPOINT_CONFIGURATION_EXTENSION.to_string())
            .or_insert_with(|| Value::Object(configuration));
    }
    if let Some(tags) = properties.get("Tags").and_then(definition_tags) {
        document.entry(DEFINITION_TAGS.to_string()).or_insert(tags);
    }
}

/// State one member of a definition's `info` block, where the document carries one that can
/// hold it: the block is created when the definition states none, and a definition whose
/// `info` is something other than an object is left as it was written.
fn state_info_member(body: &mut Value, member: &str, value: Value) {
    let Some(document) = body.as_object_mut() else {
        return;
    };
    let info = document
        .entry(DEFINITION_INFO.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(members) = info.as_object_mut() {
        members.insert(member.to_string(), value);
    }
}

/// The CORS extension a `CorsConfiguration` becomes: the members the declaration states,
/// under the names a definition spells them with ([`CORS_MEMBERS`]).
///
/// "You can also provide a string of the allowed origin", which is a shorthand for that one
/// member. Anything else states no configuration this can read, so it contributes none
/// rather than one nothing declared.
fn cors_extension(declared: &Value) -> Option<Value> {
    let mut cors = Map::new();
    match declared {
        Value::String(origin) => {
            cors.insert("allowOrigins".to_string(), json!([origin]));
        }
        Value::Object(members) => {
            for (property, member) in CORS_MEMBERS {
                if let Some(value) = members.get(*property) {
                    cors.insert((*member).to_string(), value.clone());
                }
            }
        }
        _ => return None,
    }
    (!cors.is_empty()).then_some(Value::Object(cors))
}

/// The `tags` an api's definition carries, from the tag MAP the declaration states: one
/// entry per tag, its name beside the extension a tag's value travels in.
fn definition_tags(tags: &Value) -> Option<Value> {
    let declared = tags.as_object()?;
    let entries: Vec<Value> = declared
        .iter()
        .map(|(key, value)| {
            let mut entry = Map::new();
            entry.insert(DEFINITION_TAG_NAME.to_string(), json!(key));
            entry.insert(DEFINITION_TAG_VALUE.to_string(), value.clone());
            Value::Object(entry)
        })
        .collect();
    (!entries.is_empty()).then_some(Value::Array(entries))
}

/// The OpenAPI 2.0 document a REST API's generated definition starts from — the skeleton
/// AWS SAM generates for an API whose declaration states none, whose `info.title` is the
/// stack's own name.
fn rest_definition() -> Value {
    json!({
        "swagger": "2.0",
        "info": { "title": { "Fn::Sub": "${AWS::StackName}" }, "version": "1.0" },
        "paths": {},
    })
}

/// The OpenAPI 3.0 document an HTTP API's generated definition starts from.
fn http_definition() -> Value {
    json!({
        "openapi": "3.0.1",
        "info": { "title": { "Fn::Sub": "${AWS::StackName}" }, "version": "1.0" },
        "paths": {},
    })
}

/// The key a method is spelled under in an OpenAPI path item: the method itself, or the
/// API Gateway extension that answers every method.
fn method_key(method: &str) -> String {
    if method == "any" {
        "x-amazon-apigateway-any-method".to_string()
    } else {
        method.to_string()
    }
}

/// One REST path item: the Lambda proxy integration that routes the method to the function.
fn rest_path_item(function: &str) -> Value {
    json!({
        "responses": {},
        "x-amazon-apigateway-integration": {
            "httpMethod": "POST",
            "type": "aws_proxy",
            "uri": { "Fn::Sub": lambda_integration_uri(function) },
        },
    })
}

/// One HTTP API path item: the same proxy integration, with the payload format version an
/// HTTP API's integration carries.
fn http_path_item(function: &str) -> Value {
    json!({
        "responses": {},
        "x-amazon-apigateway-integration": {
            "httpMethod": "POST",
            "type": "aws_proxy",
            "payloadFormatVersion": "2.0",
            "uri": { "Fn::Sub": lambda_integration_uri(function) },
        },
    })
}

/// The integration URI a Lambda proxy integration carries: the API Gateway path that
/// invokes the function, left as the `Fn::Sub` the expansion writes — the function's ARN is
/// not known until the function exists, so what the API's definition carries is the
/// expression for it.
fn lambda_integration_uri(function: &str) -> String {
    arn(&format!(
        "apigateway:${{AWS::Region}}:lambda:path/2015-03-31/functions/${{{function}.Arn}}/\
         invocations"
    ))
}

/// An ARN the expansion writes, in the partition this emulator's ARNs belong to.
///
/// The recorded expansion spells the partition out rather than leaving an
/// `${AWS::Partition}` for CloudFormation to substitute, so these ARNs do too; the
/// `${…}` expressions that remain are the ones whose value is not known until the
/// resources exist, which is why they are carried as an `Fn::Sub`.
fn arn(rest: &str) -> String {
    format!("arn:{PARTITION}:{rest}")
}

/// Expand one `AWS::Serverless::SimpleTable` into the DynamoDB table it declares: "a
/// DynamoDB table with a single attribute primary key".
fn expanded_table(properties: &Value) -> Map<String, Value> {
    let key = properties.get("PrimaryKey");
    // "PrimaryKey … Name: The name of the primary key. Default: id", "Type … Default:
    // String".
    let name = key
        .and_then(|key| key.get("Name"))
        .cloned()
        .unwrap_or_else(|| json!("id"));
    let declared_type = key
        .and_then(|key| key.get("Type"))
        .and_then(Value::as_str)
        .unwrap_or("String");
    let mut table = Map::new();
    table.insert(
        "KeySchema".to_string(),
        json!([{ "AttributeName": name, "KeyType": "HASH" }]),
    );
    table.insert(
        "AttributeDefinitions".to_string(),
        json!([{ "AttributeName": name, "AttributeType": attribute_type(declared_type) }]),
    );
    copy_through(&mut table, properties, TABLE_PASSTHROUGH);
    // "If you don't specify the ProvisionedThroughput property, the table is created with
    // on-demand capacity."
    if !table.contains_key("ProvisionedThroughput") {
        table.insert("BillingMode".to_string(), json!("PAY_PER_REQUEST"));
    }
    if let Some(tags) = properties.get("Tags") {
        table.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    table
}

/// The DynamoDB attribute type a `PrimaryKey$Type` names: "String", "Number" and "Binary"
/// are what the property admits, and DynamoDB's own `AttributeDefinition$AttributeType`
/// spells them `S`, `N` and `B`. Anything else is passed through as the declaration spells
/// it, for DynamoDB to judge.
fn attribute_type(declared: &str) -> &str {
    match declared {
        "String" => "S",
        "Number" => "N",
        "Binary" => "B",
        other => other,
    }
}

/// Expand one `AWS::Serverless::LayerVersion` into the Lambda layer version it declares.
fn expanded_layer_version(properties: &Map<String, Value>) -> Map<String, Value> {
    let mut layer = Map::new();
    // `AWS::Lambda::LayerVersion$Content` is the deployment package, whose members AWS
    // Lambda spells the way `PublishLayerVersion`'s own `Content` spells them
    // ([`lambda_s3_location`]) — not the `Bucket`/`Key`/`Version` of the `S3Location` an
    // API definition carries.
    if let Some(content) = properties.get("ContentUri").and_then(lambda_s3_location) {
        layer.insert("Content".to_string(), content);
    }
    copy_through(
        &mut layer,
        &Value::Object(properties.clone()),
        &[
            "CompatibleArchitectures",
            "CompatibleRuntimes",
            "Description",
            "LicenseInfo",
        ],
    );
    if let Some(name) = properties.get("LayerName") {
        layer.insert("LayerName".to_string(), name.clone());
    }
    layer
}

/// State the generated layer version's `DeletionPolicy` from the declaration's
/// [`RETENTION_POLICY_PROPERTY`], whose default is to retain the version.
///
/// The hashed logical id above makes each content a resource of its own, so a changed
/// content REMOVES the resource the stack had — the event `DeletionPolicy` governs — and
/// retaining the old version is exactly what the declaration's retention policy asks for.
/// A policy the declaration states itself wins: [`carried_over`] has already put it on the
/// resource, and this states one only where the declaration left the attribute out.
fn retained(layer: &mut Value, properties: &Map<String, Value>) {
    let Some(members) = layer.as_object_mut() else {
        return;
    };
    if members.contains_key(DELETION_POLICY) {
        return;
    }
    let policy = match properties
        .get(RETENTION_POLICY_PROPERTY)
        .and_then(Value::as_str)
    {
        Some(stated) if stated.eq_ignore_ascii_case(super::deletion_policy::DELETE) => {
            super::deletion_policy::DELETE
        }
        _ => super::deletion_policy::RETAIN,
    };
    members.insert(DELETION_POLICY.to_string(), json!(policy));
}

/// Re-point every reference to a logical id the expansion RENAMED at the id that resource
/// now carries.
///
/// A macro may rename what it expands — a layer version's logical id carries a content hash
/// — and CloudFormation plans and deploys the macro's OUTPUT, so it is the OUTPUT's
/// references that have to resolve: a `Ref` to the declaration's own id would otherwise name
/// a resource the processed template no longer declares. Each of the four ways a template
/// names a resource is re-pointed: `Ref`, `Fn::GetAtt` (both spellings), an `Fn::Sub`
/// variable and the `DependsOn` attribute. A name no expansion renamed is left exactly as
/// the template wrote it.
///
/// Bounded by [`MAX_TEMPLATE_DEPTH`], like every other walk over a caller-supplied document.
fn rename_references(node: &mut Value, renamed: &BTreeMap<String, String>, depth: usize) {
    if renamed.is_empty() || depth > MAX_TEMPLATE_DEPTH {
        return;
    }
    match node {
        Value::Array(items) => {
            for item in items.iter_mut() {
                rename_references(item, renamed, depth + 1);
            }
        }
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                match key.as_str() {
                    // "Ref returns the value of the specified … resource", and "DependsOn
                    // … specif[ies] that the creation of a specific resource follows
                    // another": both name a resource by its logical id alone.
                    REF | DEPENDS_ON => rename_named(member, renamed, depth + 1),
                    // "Fn::GetAtt returns the value of an attribute from a resource in the
                    // template": the resource is the first argument, or everything before
                    // the first dot of the short form.
                    super::template::FN_GET_ATT => rename_attribute(member, renamed),
                    // "Fn::Sub substitutes variables in an input string": a `${Name}` names
                    // what a `Ref` names and a `${Name.Attribute}` what an `Fn::GetAtt`
                    // does.
                    FN_SUB => rename_substitution(member, renamed),
                    _ => {}
                }
                rename_references(member, renamed, depth + 1);
            }
        }
        _ => {}
    }
}

/// Re-point a reference that names a resource by its logical id alone — one name, or a list
/// of them, which is the form `DependsOn` also takes.
///
/// Bounded by [`MAX_TEMPLATE_DEPTH`], like every other walk over a caller-supplied document:
/// the value a `Ref` or a `DependsOn` carries is the submitted template's, so its NESTING is
/// the caller's to choose and cannot be allowed to decide how deep this recurses.
fn rename_named(member: &mut Value, renamed: &BTreeMap<String, String>, depth: usize) {
    if depth > MAX_TEMPLATE_DEPTH {
        return;
    }
    match member {
        Value::String(name) => {
            if let Some(expanded) = renamed.get(name.as_str()) {
                *name = expanded.clone();
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                rename_named(item, renamed, depth + 1);
            }
        }
        _ => {}
    }
}

/// Re-point the resource an `Fn::GetAtt` reads its attribute off, in both spellings the
/// function takes: `[logicalId, attribute]` and `logicalId.attribute`.
fn rename_attribute(member: &mut Value, renamed: &BTreeMap<String, String>) {
    match member {
        Value::Array(parts) => {
            if let Some(Value::String(name)) = parts.first_mut() {
                if let Some(expanded) = renamed.get(name.as_str()) {
                    *name = expanded.clone();
                }
            }
        }
        Value::String(path) => {
            // The resource is read the way the crate's one reader of the short form reads
            // it, so the two can never disagree about which resource a path names.
            let resource = super::template::resource_of(path).to_string();
            if let Some(expanded) = renamed.get(resource.as_str()) {
                let repointed = format!("{expanded}{}", &path[resource.len()..]);
                *path = repointed;
            }
        }
        _ => {}
    }
}

/// Re-point the `${Name}` and `${Name.Attribute}` variables of an `Fn::Sub`, in both forms
/// the function takes: the source string on its own, or `[ String, { Name: Value, … } ]`.
fn rename_substitution(member: &mut Value, renamed: &BTreeMap<String, String>) {
    // A variable the substitution supplies ITSELF is not a reference to a resource at all,
    // so a name the map carries is left exactly as written.
    let supplied: Vec<String> = match &*member {
        Value::Array(parts) => parts
            .get(1)
            .and_then(Value::as_object)
            .map(|variables| variables.keys().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let source = match member {
        Value::String(source) => source,
        Value::Array(parts) => match parts.first_mut() {
            Some(Value::String(source)) => source,
            _ => return,
        },
        _ => return,
    };
    let repointed = renamed_substitution(source, renamed, &supplied);
    *source = repointed;
}

/// One `Fn::Sub` source string with every variable naming a renamed resource re-pointed.
///
/// The variables are read the way the crate's one reader of them reads them
/// ([`super::template::substitution_names`]): everything between `${` and the next `}`, the
/// name being what precedes the first dot. A `${!Literal}` is the function's own escape and
/// names nothing, so it never matches a renamed id; every other run of the string is copied
/// through byte for byte.
fn renamed_substitution(
    source: &str,
    renamed: &BTreeMap<String, String>,
    supplied: &[String],
) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start + 2]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(after);
            return out;
        };
        let variable = &after[..end];
        let trimmed = variable.trim();
        let name = trimmed.split('.').next().unwrap_or(trimmed);
        match renamed.get(name) {
            Some(expanded) if !supplied.iter().any(|variable| variable == name) => {
                // The attribute path the variable read the name off is kept as it was.
                out.push_str(expanded);
                out.push_str(&trimmed[name.len()..]);
            }
            _ => out.push_str(variable),
        }
        out.push('}');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Expand one `AWS::Serverless::StateMachine` into the state machine it declares, and the
/// role it executes as where the declaration names none.
fn expanded_state_machine(
    logical_id: &str,
    properties: &Map<String, Value>,
) -> (Map<String, Value>, Option<Value>) {
    let mut machine = Map::new();
    // "Definition: The state machine definition", which Step Functions takes as a
    // `DefinitionString`; "DefinitionUri: The Amazon S3 URI … of the state machine
    // definition", which it takes as a `DefinitionS3Location`.
    if let Some(definition) = properties.get("Definition") {
        machine.insert(
            "DefinitionString".to_string(),
            json!({ "Fn::Sub": canonical(definition, 0) }),
        );
    } else if let Some(uri) = properties.get("DefinitionUri").and_then(s3_location) {
        machine.insert("DefinitionS3Location".to_string(), uri);
    }
    if let Some(name) = properties.get("Name") {
        machine.insert("StateMachineName".to_string(), name.clone());
    }
    if let Some(kind) = properties.get("Type") {
        machine.insert("StateMachineType".to_string(), kind.clone());
    }
    if let Some(logging) = properties.get("Logging") {
        machine.insert("LoggingConfiguration".to_string(), logging.clone());
    }
    if let Some(tracing) = properties.get("Tracing") {
        machine.insert("TracingConfiguration".to_string(), tracing.clone());
    }
    copy_through(
        &mut machine,
        &Value::Object(properties.clone()),
        &["DefinitionSubstitutions"],
    );
    if let Some(tags) = properties.get("Tags") {
        machine.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    match properties.get("Role") {
        Some(role) => {
            machine.insert("RoleArn".to_string(), role.clone());
            (machine, None)
        }
        None => {
            machine.insert(
                "RoleArn".to_string(),
                json!({ "Fn::GetAtt": [format!("{logical_id}Role"), "Arn"] }),
            );
            let mut role = Map::new();
            role.insert(
                "AssumeRolePolicyDocument".to_string(),
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Action": ["sts:AssumeRole"],
                        "Effect": "Allow",
                        "Principal": { "Service": ["states.amazonaws.com"] },
                    }],
                }),
            );
            // A state machine states its permissions the same way a function does, and the
            // role generated for it carries them the same way ([`role_policies`]).
            let (managed, inline) =
                role_policies(logical_id, &Value::Object(properties.clone()));
            if !managed.is_empty() {
                role.insert("ManagedPolicyArns".to_string(), Value::Array(managed));
            }
            if !inline.is_empty() {
                role.insert("Policies".to_string(), Value::Array(inline));
            }
            (machine, Some(resource("AWS::IAM::Role", role)))
        }
    }
}

/// One synthesized resource: its type and its properties.
fn resource(resource_type: &str, properties: Map<String, Value>) -> Value {
    json!({ "Type": resource_type, "Properties": properties })
}

/// A synthesized resource under the same `Condition` as the declaration it came from: "you
/// can … create resources conditionally", and a resource the transform adds for a
/// declaration exists exactly when that declaration's resource does.
fn conditioned(mut resource: Value, condition: Option<&Value>) -> Value {
    if let (Some(condition), Some(members)) = (condition, resource.as_object_mut()) {
        members.insert("Condition".to_string(), condition.clone());
    }
    resource
}

/// The resource a declaration BECOMES: its new type and properties, carrying over the
/// resource ATTRIBUTES beside `Properties` that CloudFormation itself reads
/// (`DependsOn`, `Condition`, the policies and `Metadata`), because those are stated about
/// the resource the declaration stands for.
fn carried_over(declaration: &Value, resource_type: &str, properties: Map<String, Value>) -> Value {
    let mut out = Map::new();
    out.insert("Type".to_string(), json!(resource_type));
    out.insert("Properties".to_string(), Value::Object(properties));
    for attribute in [
        "Condition",
        "DeletionPolicy",
        "DependsOn",
        "Metadata",
        "UpdateReplacePolicy",
    ] {
        if let Some(value) = declaration.get(attribute) {
            out.insert(attribute.to_string(), value.clone());
        }
    }
    Value::Object(out)
}

/// Copy the members `names` lists from `properties` into `into`, where the declaration
/// states them: the members two types spell the same way.
fn copy_through(into: &mut Map<String, Value>, properties: &Value, names: &[&str]) {
    for name in names {
        if let Some(value) = properties.get(*name) {
            into.insert((*name).to_string(), value.clone());
        }
    }
}

/// A declared boolean in either spelling a template gives one: JSON's boolean, or the text a
/// template spells one as, read case-insensitively. `None` where the property is absent or
/// is no boolean at all — an intrinsic, say — which states nothing the expansion can map, so
/// the member it would have decided is left for the target type's own default.
fn declared_flag(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// A SAM `Tags` map as the `[{Key, Value}]` list a CloudFormation resource takes, with the
/// tag the transform adds where there is one.
///
/// A declaration that already states its tags as a LIST is left as it is: that is the form
/// the target resource takes.
fn tag_list(tags: Option<&Value>, extra: Option<(&str, &str)>) -> Value {
    let mut list: Vec<Value> = match tags {
        Some(Value::Array(entries)) => entries.clone(),
        Some(Value::Object(entries)) => entries
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
        _ => Vec::new(),
    };
    if let Some((key, value)) = extra {
        list.push(json!({ "Key": key, "Value": value }));
    }
    Value::Array(list)
}

/// The `{Bucket, Key, Version}` location an `s3://` URI or an object naming those members
/// states, as a resource's S3-location property takes it.
fn s3_location(value: &Value) -> Option<Value> {
    let (bucket, key, version) = s3_parts(value)?;
    let mut location = Map::new();
    location.insert("Bucket".to_string(), bucket);
    location.insert("Key".to_string(), key);
    if let Some(version) = version {
        location.insert("Version".to_string(), version);
    }
    Some(Value::Object(location))
}

/// The Amazon S3 location a LAMBDA package property states: a function's `Code` and a layer
/// version's `Content`, whose members AWS Lambda's own calls spell `S3Bucket`, `S3Key` and
/// `S3ObjectVersion` — against the `Bucket`, `Key` and `Version` of the `S3Location` an API
/// definition and a state machine definition carry ([`s3_location`]).
fn lambda_s3_location(value: &Value) -> Option<Value> {
    let (bucket, key, version) = s3_parts(value)?;
    let mut location = Map::new();
    location.insert("S3Bucket".to_string(), bucket);
    location.insert("S3Key".to_string(), key);
    if let Some(version) = version {
        location.insert("S3ObjectVersion".to_string(), version);
    }
    Some(Value::Object(location))
}

/// The bucket, key and object version a SAM URI property names: "`s3://bucket/key`" (with
/// an optional `?versionId=`), or an object stating `Bucket`, `Key` and `Version`.
///
/// `None` for anything else — a local path, which is what `sam package` replaces with an S3
/// URI before the template is deployed, is not a location this expansion can state. The URI
/// spelling is read by the crate's one reader of it
/// ([`super::template::s3_uri_parts`]).
fn s3_parts(value: &Value) -> Option<(Value, Value, Option<Value>)> {
    match value {
        Value::String(uri) => {
            let (bucket, key, version) = super::template::s3_uri_parts(uri)?;
            Some((json!(bucket), json!(key), version.map(|version| json!(version))))
        }
        Value::Object(members) => Some((
            members.get("Bucket")?.clone(),
            members.get("Key")?.clone(),
            members.get("Version").cloned(),
        )),
        _ => None,
    }
}

/// The logical id a `{"Ref": "Api"}` (or a plain logical id) names, which is how an event
/// states the API it is routed through.
fn referenced_logical_id(value: &Value) -> Option<String> {
    match value {
        Value::String(id) => Some(id.clone()),
        Value::Object(_) => value
            .get("Ref")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

/// A name as a logical-id fragment: "the logical ID must be alphanumeric (A-Za-z0-9) and
/// unique within the template", so only the alphanumeric characters of a stage's name can
/// be part of one.
fn logical_fragment(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// The resource type a declaration states.
fn resource_type(declaration: &Value) -> Option<&str> {
    declaration.get("Type").and_then(Value::as_str)
}

/// A value's members, or an empty map when it has none — the one reading of an optional
/// object section (`Properties`, `Events`).
fn object(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// A resource's properties resolved against the `Globals` section: "properties that you
/// specify in the Globals section … apply to all your serverless functions and APIs", and
/// "the resource-level property overrides the global".
///
/// Two objects are merged member by member, so a resource stating one environment variable
/// keeps the global ones beside it; anything else the resource states REPLACES the global
/// value.
///
/// Bounded by [`MAX_TEMPLATE_DEPTH`], as every other walk over a caller-supplied document
/// here is ([`canonical`]): both sides are sub-trees of the submitted template, so the
/// nesting is the REQUESTER's, and past that depth the resource's own value stands as it
/// wrote it rather than being recursed into.
fn merged(global: Option<&Value>, declared: Option<&Value>, depth: usize) -> Value {
    match (global, declared) {
        (Some(Value::Object(global)), Some(Value::Object(declared)))
            if depth <= MAX_TEMPLATE_DEPTH =>
        {
            let mut out = global.clone();
            for (name, value) in declared {
                match out.get(name) {
                    Some(global) if global.is_object() && value.is_object() => {
                        out.insert(name.clone(), merged(Some(global), Some(value), depth + 1));
                    }
                    _ => {
                        out.insert(name.clone(), value.clone());
                    }
                }
            }
            Value::Object(out)
        }
        (_, Some(declared)) => declared.clone(),
        (Some(global), None) => global.clone(),
        (None, None) => Value::Object(Map::new()),
    }
}

/// The hash a deployment's (or a layer version's) logical id carries: [`DIGEST_CHARS`]
/// lowercase hex characters of a digest of the value's CONTENT.
///
/// The same content always gives the same id and different content a different one, which
/// is the whole of what the id has to do: it makes an immutable resource's identity a
/// function of what it holds, so an edited API definition is planned as a NEW deployment
/// rather than an update of the one that exists. Nothing reads the hash itself, so the
/// digest is the crate's own derivation ([`super::template::content_digest`]) rather than a
/// published one.
fn digest(value: &Value) -> String {
    super::template::content_digest(DIGEST_SALT, &canonical(value, 0), DIGEST_CHARS)
}

/// A value as text that depends on its CONTENT alone: object members in sorted order, so
/// the same definition hashes the same way whatever order the template reader built its
/// members in (`serde_json`'s map preserves insertion order under one build feature and
/// sorts under another).
///
/// Bounded by [`MAX_TEMPLATE_DEPTH`]: the value is caller-supplied, so a deeper tree is
/// reported as elided rather than recursed into.
fn canonical(value: &Value, depth: usize) -> String {
    if depth > MAX_TEMPLATE_DEPTH {
        return "...".to_string();
    }
    match value {
        Value::Object(members) => {
            let mut names: Vec<&String> = members.keys().collect();
            names.sort();
            let members: Vec<String> = names
                .iter()
                .map(|name| {
                    format!(
                        "{}:{}",
                        Value::String((*name).clone()),
                        canonical(&members[name.as_str()], depth + 1)
                    )
                })
                .collect();
            format!("{{{}}}", members.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|item| canonical(item, depth + 1))
                .collect();
            format!("[{}]", items.join(","))
        }
        scalar => scalar.to_string(),
    }
}

#[cfg(test)]
mod auto_publish_tests {
    use super::*;

    /// A serverless function declaring `properties`, as a whole template the transform is
    /// asked to expand.
    fn document(properties: Value, parameters: Option<Value>) -> Value {
        let mut sections = json!({
            "Transform": TRANSFORM,
            "Resources": { "Fn": { "Type": FUNCTION, "Properties": properties } }
        });
        if let Some(parameters) = parameters {
            sections["Parameters"] = parameters;
        }
        sections
    }

    /// The properties of one resource of the expansion, or `Value::Null` for a logical id
    /// the expansion did not produce.
    fn expanded(document: &Value, logical_id: &str) -> Value {
        document
            .pointer(&format!("/Resources/{logical_id}"))
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Every logical id of the expansion carrying `resource_type`.
    fn of_type(document: &Value, resource_type: &str) -> Vec<String> {
        document
            .get("Resources")
            .and_then(Value::as_object)
            .map(|resources| {
                resources
                    .iter()
                    .filter(|(_, declared)| resource_type_of(declared) == Some(resource_type))
                    .map(|(logical_id, _)| logical_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// [`resource_type`] under another name: the test module's own `resource_type` binding
    /// would otherwise shadow the function.
    fn resource_type_of(declared: &Value) -> Option<&str> {
        super::resource_type(declared)
    }

    /// The code digest a row states, which the generated logical id carries ten characters
    /// of. Any string does: the property is one "that, when changed, triggers a new version
    /// to be published".
    const SHA: &str = "9f2c3b7a1d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8";

    /// One row: a declaration, the parameter values in force on the deployment, and what the
    /// expansion is to make of it.
    struct Row {
        case: &'static str,
        /// The `AWS::Serverless::Function`'s declared properties.
        properties: Value,
        /// The template's own `Parameters` section, for a row that states the digest through
        /// one.
        parameters: Option<Value>,
        /// The parameter values in force, as a deployment supplies them.
        supplied: Vec<(&'static str, &'static str)>,
        /// The logical id the generated version is to carry, or `None` where the hash is a
        /// digest of the code — which is this crate's own derivation, so the row states its
        /// SHAPE instead.
        version_id: Option<String>,
        /// The function's description as the expansion leaves it: a digest stated as an
        /// intrinsic travels into the description as the declaration wrote it, which is what
        /// CloudFormation resolves for the deployment.
        description: Option<Value>,
    }

    /// `AutoPublishAlias` expands into one `AWS::Lambda::Version` under a hashed logical id,
    /// retained, and one `AWS::Lambda::Alias` invoking it — and the digest the declaration
    /// states, in whichever form it states it, is what that hash is and what the function's
    /// own description carries.
    ///
    /// The rows differ along what the rule generalises over: how the digest is stated (a
    /// literal, a `Ref` to a parameter, not at all), whether the declaration describes the
    /// function itself, and whether it asks for the automatic publication at all.
    #[test]
    fn auto_publish_alias_expands_into_a_retained_version_and_an_alias() {
        let rows: Vec<Row> = vec![
            Row {
                case: "the digest written out in the declaration",
                properties: json!({
                    "InlineCode": "handler",
                    "AutoPublishAlias": "live",
                    "AutoPublishCodeSha256": SHA,
                }),
                parameters: None,
                supplied: vec![],
                version_id: Some(format!("FnVersion{}", &SHA[..DIGEST_CHARS])),
                description: Some(json!(SHA)),
            },
            Row {
                case: "the digest stated as a parameter of the template",
                properties: json!({
                    "InlineCode": "handler",
                    "AutoPublishAlias": "live",
                    "AutoPublishCodeSha256": { "Ref": "CodeSha" },
                }),
                parameters: Some(json!({ "CodeSha": { "Type": "String" } })),
                supplied: vec![("CodeSha", SHA)],
                version_id: Some(format!("FnVersion{}", &SHA[..DIGEST_CHARS])),
                description: Some(json!({ "Ref": "CodeSha" })),
            },
            Row {
                case: "no digest at all, so the hash is one of the code itself",
                properties: json!({ "InlineCode": "handler", "AutoPublishAlias": "live" }),
                parameters: None,
                supplied: vec![],
                version_id: None,
                description: None,
            },
            Row {
                case: "a description the declaration states itself, beside a digest",
                properties: json!({
                    "InlineCode": "handler",
                    "AutoPublishAlias": "live",
                    "AutoPublishCodeSha256": SHA,
                    "Description": "what this function does",
                }),
                parameters: None,
                supplied: vec![],
                version_id: Some(format!("FnVersion{}", &SHA[..DIGEST_CHARS])),
                description: Some(json!("what this function does")),
            },
        ];

        for row in rows {
            let Row {
                case,
                properties,
                parameters,
                supplied,
                version_id,
                description,
            } = row;
            let mut expanded_document = document(properties, parameters);
            let values: BTreeMap<String, String> = supplied
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            let changed = expand_with_parameters(&mut expanded_document, &values);
            assert!(
                changed,
                "case {case:?}: expected the transform to expand the declaration"
            );

            let versions = of_type(&expanded_document, "AWS::Lambda::Version");
            match &version_id {
                Some(expected) => assert_eq!(
                    versions,
                    vec![expected.clone()],
                    "case {case:?}: expected exactly the version {expected:?}, got \
                     {versions:?} in {expanded_document}"
                ),
                // A hash of the code is this crate's own digest, so what the row states
                // about it is its SHAPE: the ten hexadecimal characters a generated logical
                // id carries.
                None => {
                    let hash = versions
                        .first()
                        .and_then(|id| id.strip_prefix("FnVersion"))
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        versions.len() == 1
                            && hash.len() == DIGEST_CHARS
                            && hash.chars().all(|digit| digit.is_ascii_hexdigit()),
                        "case {case:?}: expected one version hashed over the code, got \
                         {versions:?}"
                    );
                }
            }
            let version = versions.first().cloned().unwrap_or_default();
            assert_eq!(
                expanded(&expanded_document, &version)
                    .get(DELETION_POLICY)
                    .and_then(Value::as_str),
                Some(super::super::deletion_policy::RETAIN),
                "case {case:?}: expected the generated version to be retained, got \
                 {expanded_document}"
            );

            // The alias is named for the alias the declaration asks for, belongs to the same
            // function, and invokes the version the hash names.
            let alias = expanded(&expanded_document, "FnAliaslive");
            assert_eq!(
                alias.pointer("/Properties/FunctionVersion").cloned(),
                Some(json!({ "Fn::GetAtt": [version, "Version"] })),
                "case {case:?}: expected the alias to invoke the generated version, got \
                 {alias}"
            );
            assert_eq!(
                alias.pointer("/Properties/Name").and_then(Value::as_str),
                Some("live"),
                "case {case:?}: expected the alias to carry the declared name, got {alias}"
            );

            // ...and the function's own description is what makes the publication a NEW
            // version where the declaration states a digest, unless the declaration
            // describes the function itself. Compared as the whole VALUE: a digest stated as
            // an intrinsic is a description that is an intrinsic, and a row that only read
            // it as text would pass on a description the expansion had dropped.
            let described = expanded(&expanded_document, "Fn");
            assert_eq!(
                described.pointer("/Properties/Description").cloned(),
                description,
                "case {case:?}: expected the function's description to be {description:?}, \
                 got {described}"
            );
        }
    }

    /// A declaration that asks for no alias has no version and no alias generated for it:
    /// the publication is what `AutoPublishAlias` asks for.
    #[test]
    fn a_function_without_the_alias_property_publishes_nothing() {
        let mut expanded_document = document(
            json!({ "InlineCode": "handler", "AutoPublishCodeSha256": SHA }),
            None,
        );
        expand_with_parameters(&mut expanded_document, &BTreeMap::new());
        for resource_type in ["AWS::Lambda::Version", "AWS::Lambda::Alias"] {
            let generated = of_type(&expanded_document, resource_type);
            assert!(
                generated.is_empty(),
                "case {resource_type:?}: expected no generated resource, got {generated:?}"
            );
        }
    }
}

