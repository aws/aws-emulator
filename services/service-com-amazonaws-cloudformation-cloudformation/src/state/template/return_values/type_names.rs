// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The type of an IAM access key: the long-term credential of an IAM user, under the
/// namespace the IAM model's own `awsService.cloudFormationName` (`IAM`) spells.
///
/// "Ref returns the AWS::IAM::AccessKey resource's access key ID", and `Fn::GetAtt` reads
/// `SecretAccessKey` — "the secret access key for the specified AWS::IAM::AccessKey
/// resource". IAM mints both, which is why [`identity_is_service_assigned`] names this type
/// and why neither value is computed here ([`return_values`]).
pub const IAM_ACCESS_KEY: &str = "AWS::IAM::AccessKey";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Data Firehose stream.
/// The middle segment is the Firehose model's own `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")` on that service's model).
pub const FIREHOSE_DELIVERY_STREAM: &str = "AWS::KinesisFirehose::DeliveryStream";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Kinesis data stream — a
/// different service from the Firehose one above, whose middle segment is the Kinesis
/// model's own `awsService.cloudFormationName` (`Kinesis`).
///
/// "Ref returns the stream name", which is the name the owning service creates the stream
/// under, so a GENERATED one is held to that service's own rules for it ([`naming_rules`])
/// and the identity is not service-assigned.
pub const KINESIS_STREAM: &str = "AWS::Kinesis::Stream";

/// The `AWS::<cloudFormationName>::<Resource>` types of the Amazon Bedrock resources an
/// agent estate declares. The middle segment is that model's own
/// `awsService.cloudFormationName` (`Bedrock`).
///
/// Each one's owning API — Amazon Bedrock Agent's build-time service — identifies the
/// resource by a ten-character identifier IT mints (`CreateKnowledgeBase` answers a
/// `knowledgeBaseId`, `CreateAgent` an `agentId`) and takes a new name on the matching
/// update, which is why [`name_is_create_only`] names these types.
pub const BEDROCK_KNOWLEDGE_BASE: &str = "AWS::Bedrock::KnowledgeBase";
pub const BEDROCK_AGENT: &str = "AWS::Bedrock::Agent";
pub const BEDROCK_AGENT_ALIAS: &str = "AWS::Bedrock::AgentAlias";
pub const BEDROCK_DATA_SOURCE: &str = "AWS::Bedrock::DataSource";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Bedrock GUARDRAIL, the one
/// resource of that estate whose owning API is Bedrock's own control plane
/// (`CreateGuardrail`) rather than the agent build-time one.
pub const BEDROCK_GUARDRAIL: &str = "AWS::Bedrock::Guardrail";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Bedrock FLOW: the workflow
/// of nodes Amazon Bedrock Agent's `CreateFlow` makes, which `UpdateFlow` changes and
/// `DeleteFlow` removes, and which the Bedrock Agent RUNTIME plane's `InvokeFlow` runs.
///
/// "Ref returns the flow ARN" — the type's primary identifier — and `Fn::GetAtt` reads the
/// ARN, the `Id` that service minted, the flow's `Status`, its `Version` and the two instants
/// it keeps. That service mints the id (`^[0-9a-zA-Z]{10}$`) and the ARN around it, which is
/// why [`identity_is_service_assigned`] names this type.
pub const BEDROCK_FLOW: &str = "AWS::Bedrock::Flow";

/// Amazon Bedrock's ARN namespace, which is that model's own `awsService.arnNamespace`
/// (`bedrock`) — the namespace a guardrail ARN carries
/// (`arn:aws:bedrock:<region>:<account>:guardrail/<id>`, from `GuardrailArn`'s pattern).
pub const BEDROCK_ARN_NAMESPACE: &str = "bedrock";

/// The `AWS::<cloudFormationName>::<Resource>` types of the three Amazon Bedrock DATA
/// AUTOMATION resources a template declares: the blueprint that describes what to extract, the
/// data automation project an inference run names, and the data automation library a project
/// reads its entities from. The middle segment is `Bedrock`, the `cloudFormationName` every
/// type of that estate carries, while the owning API is Amazon Bedrock Data Automation's own
/// build-time service (`CreateBlueprint`, `CreateDataAutomationProject`,
/// `CreateDataAutomationLibrary`).
///
/// `Ref` returns each resource's ARN — the type's primary identifier — which that service
/// MINTS: every one of those creates takes the resource's NAME and answers an ARN whose
/// resource segment is 12 to 36 characters of `[a-zA-Z0-9-]` it drew (`BlueprintArn`,
/// `DataAutomationProjectArn` and `DataAutomationLibraryArn` each fix that form), which is why
/// [`identity_is_service_assigned`] names all three.
pub const BEDROCK_BLUEPRINT: &str = "AWS::Bedrock::Blueprint";
pub const BEDROCK_DATA_AUTOMATION_PROJECT: &str = "AWS::Bedrock::DataAutomationProject";
pub const BEDROCK_DATA_AUTOMATION_LIBRARY: &str = "AWS::Bedrock::DataAutomationLibrary";

/// The attribute each of those three types publishes the ARN its owning service minted as,
/// which is also the resource's physical id: one name, read by the return-value catalogue, the
/// primary-identifier table and the arms that provision the type
/// (`super::super::provisioner::bedrock_data_automation`).
pub const BLUEPRINT_ARN_ATTRIBUTE: &str = "BlueprintArn";
pub const PROJECT_ARN_ATTRIBUTE: &str = "ProjectArn";
pub const LIBRARY_ARN_ATTRIBUTE: &str = "LibraryArn";

/// The resource PATH each of those ARNs carries, which is the segment that service's own ARN
/// patterns fix before the identifier.
pub(super) const BLUEPRINT_PATH: &str = "blueprint";
pub(super) const DATA_AUTOMATION_PROJECT_PATH: &str = "data-automation-project";
pub(super) const DATA_AUTOMATION_LIBRARY_PATH: &str = "data-automation-library";

/// The salt the identifier stand-in of each of those resources is derived under, and how many
/// characters it carries: the three ARN patterns admit 12 to 36 characters of `[a-zA-Z0-9-]`,
/// so a stand-in of twelve lower-case hexadecimal digits is inside every one of them.
pub(super) const BEDROCK_DATA_AUTOMATION_ID_DIGITS: usize = 12;
pub(super) const BLUEPRINT_ID_SALT: &str = "bedrock/blueprint";
pub(super) const DATA_AUTOMATION_PROJECT_ID_SALT: &str = "bedrock/data-automation-project";
pub(super) const DATA_AUTOMATION_LIBRARY_ID_SALT: &str = "bedrock/data-automation-library";

/// The salt the guardrail-id stand-in is derived under, and how many characters it carries:
/// `GuardrailId` is `^[a-z0-9]+$` with `@length(max: 64)`, so the stand-in is lower-case
/// alphanumeric and carries no separator. Twelve characters is the length Amazon Bedrock's
/// own minted ids carry.
pub(super) const BEDROCK_GUARDRAIL_ID_SALT: &str = "bedrock/guardrail";
pub(super) const BEDROCK_GUARDRAIL_ID_DIGITS: usize = 12;

/// The version a freshly created guardrail carries: `CreateGuardrail`'s `version` is
/// `^DRAFT$` ("This value will always be DRAFT").
pub(super) const BEDROCK_GUARDRAIL_DRAFT_VERSION: &str = "DRAFT";

/// The salt the knowledge-base-id stand-in is derived under, and how many characters it
/// carries: a `knowledgeBaseId` is `^[0-9a-zA-Z]{10}$`, so the stand-in is exactly ten
/// alphanumeric characters and carries no separator.
pub(super) const BEDROCK_KNOWLEDGE_BASE_ID_SALT: &str = "bedrock/knowledge-base";
pub(super) const BEDROCK_KNOWLEDGE_BASE_ID_DIGITS: usize = 10;

/// The salt the data-source-id stand-in is derived under, and how many characters it carries:
/// a `dataSourceId` is `^[0-9a-zA-Z]{10}$` (the `Id` shape `CreateDataSource`'s answer and
/// every later call's `dataSourceId` target), so the stand-in is exactly ten alphanumeric
/// characters and carries no separator.
pub(super) const BEDROCK_DATA_SOURCE_ID_SALT: &str = "bedrock/data-source";
pub(super) const BEDROCK_DATA_SOURCE_ID_DIGITS: usize = 10;

/// The property an `AWS::Bedrock::DataSource` declaration states the knowledge base the data
/// source is attached to in, which is the `knowledgeBaseId` `CreateDataSource` binds as its
/// URI label.
pub(crate) const BEDROCK_DATA_SOURCE_KNOWLEDGE_BASE_ID_PROPERTY: &str = "KnowledgeBaseId";

/// The attribute carrying the id Amazon Bedrock Agent mints for a data source, which is the
/// second part of the identity the type is addressed by and the one return value these naming
/// rules compute a stand-in for.
pub(crate) const BEDROCK_DATA_SOURCE_ID_ATTRIBUTE: &str = "DataSourceId";

/// The physical resource id of an [`BEDROCK_DATA_SOURCE`]: the PAIR that type's primary
/// identifier is — the knowledge base the data source belongs to and the `dataSourceId` Amazon
/// Bedrock Agent minted, which are exactly the two labels `GetDataSource`,
/// `UpdateDataSource`, `DeleteDataSource` and `StartIngestionJob` each address one by — joined
/// with [`COMPOSITE_IDENTITY_SEPARATOR`], which is what the type's `Ref` returns.
///
/// Not a row of [`composite_identity_properties`]: that one builds an identity out of the
/// values a declaration STATES, and the second part of this one is the id Amazon Bedrock Agent
/// assigns, which no declaration carries. So the identity is built here as a stand-in and
/// again by [`crate::state::provisioner`] from the data source that service answered — the one
/// place both parts are really known.
pub(crate) fn bedrock_data_source_identity(knowledge_base_id: &str, data_source_id: &str) -> String {
    composite_identity_pair(knowledge_base_id, data_source_id)
}

/// The two parts of such an identity, where `identity` is one
/// ([`bedrock_data_source_identity`]); `None` for a value that is not.
pub(crate) fn bedrock_data_source_identity_parts(identity: &str) -> Option<(&str, &str)> {
    composite_identity_pair_parts(identity)
}

/// The physical resource id of an `AWS::BedrockAgentCore::GatewayTarget`: the PAIR that
/// type's primary identifier is — the gateway the target is served by and the `targetId` the
/// Amazon Bedrock AgentCore control plane minted, which are exactly the two labels
/// `GetGatewayTarget`, `UpdateGatewayTarget` and `DeleteGatewayTarget` each address one by —
/// joined with [`COMPOSITE_IDENTITY_SEPARATOR`], which is what the type's `Ref` returns.
///
/// Not a row of [`composite_identity_properties`]: that one builds an identity out of the
/// values a declaration STATES, and the second part of this one is the id that control plane
/// assigns, which no declaration carries. So the identity is built here as a stand-in and
/// again by [`crate::state::provisioner`] from the target that service answered — the one
/// place both parts are really known.
pub(crate) fn agentcore_gateway_target_identity(gateway_id: &str, target_id: &str) -> String {
    composite_identity_pair(gateway_id, target_id)
}

/// The two parts of such an identity, where `identity` is one
/// ([`agentcore_gateway_target_identity`]); `None` for a value that is not, which is what
/// keeps a call from being addressed with a value of another shape.
pub(crate) fn agentcore_gateway_target_identity_parts(identity: &str) -> Option<(&str, &str)> {
    composite_identity_pair_parts(identity)
}

/// The physical resource id a type whose primary identifier is a PAIR is known by: the two
/// values the owning service's own API addresses one resource by, joined with
/// [`COMPOSITE_IDENTITY_SEPARATOR`], which is what such a type's `Ref` returns.
///
/// The ONE join of such an identity, which every per-type function above delegates to, so the
/// readings of that separator cannot diverge — the pair counterpart of
/// [`composite_identity_triple`], and the one place a value that is not such a pair is told
/// apart from one that is ([`composite_identity_pair_parts`]).
pub(crate) fn composite_identity_pair(first: &str, second: &str) -> String {
    [first, second].join(COMPOSITE_IDENTITY_SEPARATOR)
}

/// The two parts of such an identity ([`composite_identity_pair`]); `None` for a value that is
/// not one, which is what keeps a call from being addressed with a value of another shape.
pub(crate) fn composite_identity_pair_parts(identity: &str) -> Option<(&str, &str)> {
    let mut parts = identity.split(COMPOSITE_IDENTITY_SEPARATOR);
    let (first, second) = (parts.next()?, parts.next()?);
    parts.next().is_none().then_some((first, second))
}

/// The salt the flow-id stand-in is derived under, and how many characters it carries: a
/// `FlowId` is `^[0-9a-zA-Z]{10}$`, so the stand-in is exactly ten alphanumeric characters
/// and carries no separator.
pub(super) const BEDROCK_FLOW_ID_SALT: &str = "bedrock/flow";
pub(super) const BEDROCK_FLOW_ID_DIGITS: usize = 10;

/// The version a freshly created flow carries: `CreateFlow`'s `version` is `^DRAFT$`, the
/// working copy every flow has before a numbered version is cut from it.
pub(super) const BEDROCK_FLOW_DRAFT_VERSION: &str = "DRAFT";

/// The resource path a flow ARN carries before the id that service minted, from
/// `FlowArn`'s own pattern (`arn:aws:bedrock:<region>:<account>:flow/<id>`).
pub(super) const BEDROCK_FLOW_PATH: &str = "flow";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Bedrock SESSION: the store
/// of a generative-AI conversation's checkpoints that the Bedrock Agent RUNTIME plane's
/// `CreateSession` mints, `UpdateSession` changes and `DeleteSession` removes.
///
/// `Ref` returns the session ARN — the type's primary identifier, and one of the two forms
/// `SessionIdentifier` admits, so every later call on a session is addressed by it — and
/// `Fn::GetAtt` reads that ARN and the `SessionId` that service minted. The id is a UUID that
/// service draws per session (`Uuid`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) and the ARN is built
/// around it, which is why [`identity_is_service_assigned`] names this type.
pub const BEDROCK_SESSION: &str = "AWS::Bedrock::Session";

/// The resource path a session ARN carries before the uuid that service minted, from
/// `SessionArn`'s own pattern
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]+:[0-9]{12}:session/<uuid>$`).
pub(super) const BEDROCK_SESSION_PATH: &str = "session";

/// The attribute carrying the ARN the Bedrock Agent runtime mints for a session, which is
/// this type's primary identifier.
pub(crate) const BEDROCK_SESSION_ARN_ATTRIBUTE: &str = "SessionArn";

/// The attribute carrying the uuid that service minted for the session, which is the other
/// form `SessionIdentifier` admits.
pub(crate) const BEDROCK_SESSION_ID_ATTRIBUTE: &str = "SessionId";

/// Every `Fn::GetAtt` attribute a [`BEDROCK_SESSION`] publishes that these naming rules can
/// compute a stand-in for, in the sorted order a resource's own attribute map holds them in.
///
/// The session's `SessionStatus` and the two instants it keeps are that service's own to
/// report and reach the resource off its create's own answer rather than being computed here,
/// exactly as a flow's status and instants do.
pub(crate) const BEDROCK_SESSION_ATTRIBUTES: &[&str] = &[
    BEDROCK_SESSION_ARN_ATTRIBUTE,
    BEDROCK_SESSION_ID_ATTRIBUTE,
];

/// The `AWS::<cloudFormationName>::<Resource>` type of an APPLICATION INFERENCE PROFILE: the
/// profile of Amazon Bedrock's own control plane that `CreateInferenceProfile` makes over a
/// foundation model or a system-defined profile, `GetInferenceProfile` reads and
/// `DeleteInferenceProfile` removes. The middle segment is `Bedrock`, the
/// `cloudFormationName` every type of that estate carries.
///
/// `Ref` returns the profile's identifier — the ARN, which is the type's primary identifier
/// and one of the two forms `InferenceProfileIdentifier` admits, so every later call on the
/// profile is addressed by it — and `Fn::GetAtt` reads that ARN and the
/// `inferenceProfileId` that service minted beside the status, the type and the two instants
/// it keeps. The create takes the profile's NAME and the model source and answers the
/// `inferenceProfileArn` IT minted (`InferenceProfileArn`:
/// `^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{0,20}):(|[0-9]{12}):\
/// (inference-profile|application-inference-profile)/[a-zA-Z0-9-:.]+$`), which is why
/// [`identity_is_service_assigned`] names this type.
pub const BEDROCK_APPLICATION_INFERENCE_PROFILE: &str =
    "AWS::Bedrock::ApplicationInferenceProfile";

/// The resource path an APPLICATION inference profile's ARN carries before the id that
/// service minted, which is the one of the two alternatives that pattern spells for a profile
/// an account creates (a `inference-profile/…` ARN names one Amazon Bedrock publishes).
pub(super) const BEDROCK_APPLICATION_INFERENCE_PROFILE_PATH: &str =
    "application-inference-profile";

/// The attributes carrying the two handles Amazon Bedrock mints for an application inference
/// profile: the ARN, which is this type's primary identifier, and the `InferenceProfileId`
/// inside it — the other form `InferenceProfileIdentifier` admits.
pub(crate) const BEDROCK_INFERENCE_PROFILE_ARN_ATTRIBUTE: &str = "InferenceProfileArn";
pub(crate) const BEDROCK_INFERENCE_PROFILE_ID_ATTRIBUTE: &str = "InferenceProfileId";

/// Every `Fn::GetAtt` attribute a [`BEDROCK_APPLICATION_INFERENCE_PROFILE`] publishes that
/// these naming rules can compute a stand-in for, in the sorted order a resource's own
/// attribute map holds them in.
///
/// The profile's `Status`, its `Type` and the two instants it keeps are that service's own to
/// report and reach the resource off its reads rather than being computed here, exactly as a
/// session's status and instants do.
pub(crate) const BEDROCK_INFERENCE_PROFILE_ATTRIBUTES: &[&str] = &[
    BEDROCK_INFERENCE_PROFILE_ARN_ATTRIBUTE,
    BEDROCK_INFERENCE_PROFILE_ID_ATTRIBUTE,
];

/// How many characters the id Amazon Bedrock mints for an application inference profile
/// carries, and the salt the STAND-IN computed before the create is derived under: 12, the
/// width that service's own minted ids have, inside the `^[a-zA-Z0-9-:.]+$` an
/// `InferenceProfileId` admits.
pub(super) const BEDROCK_INFERENCE_PROFILE_ID_DIGITS: usize = 12;
pub(super) const BEDROCK_INFERENCE_PROFILE_ID_SALT: &str = "bedrock/application-inference-profile";

/// The `AWS::BCM::<Resource>` types of the two Billing and Cost Management Dashboards
/// resources a template declares: the DASHBOARD that service's own `CreateDashboard` makes
/// under the name the declaration states, and the SCHEDULED REPORT its
/// `CreateScheduledReport` makes on one of those dashboards.
///
/// "Ref returns the Arn" for either type, and `Fn::GetAtt` reads
/// [`BCM_DASHBOARD_ATTRIBUTES`] / [`BCM_SCHEDULED_REPORT_ATTRIBUTES`]. That service MINTS
/// the ARN each create answers — neither create takes an identifier at all, and the
/// resource segment of the ARN carries an id of that service's own
/// (`DashboardArn`: `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$`,
/// and the same shape under `scheduled-report` for a report) — which is why
/// [`identity_is_service_assigned`] names both types, and why the instants beside the ARN
/// are that service's clock rather than anything a template states.
pub const BCM_DASHBOARD: &str = "AWS::BCM::Dashboard";
pub const BCM_SCHEDULED_REPORT: &str = "AWS::BCM::ScheduledReport";

/// That service's ARN namespace, which is the namespace both ARNs carry, with no REGION
/// segment at all: the ARN patterns above spell the region segment empty, so a dashboard
/// and a report are named account-wide.
pub const BCM_DASHBOARDS_ARN_NAMESPACE: &str = "bcm-dashboards";

/// The resource path each ARN carries before the id that service minted, from those same
/// patterns.
pub(super) const BCM_DASHBOARD_PATH: &str = "dashboard";
pub(super) const BCM_SCHEDULED_REPORT_PATH: &str = "scheduled-report";

/// How many characters the id inside one of those ARNs carries: that service renders a
/// resource id as 32 lowercase hexadecimal characters, which is inside the `[-a-z0-9]+`
/// resource segment the patterns admit — so the STAND-IN computed before the create is of
/// that same width.
pub(super) const BCM_RESOURCE_ID_DIGITS: usize = 32;

/// Every `Fn::GetAtt` attribute an [`BCM_DASHBOARD`] publishes, in the sorted order a
/// resource's own attribute map holds them in: the ARN that service minted, the two
/// instants it recorded for the dashboard and the kind of dashboard it records it as
/// (`GetDashboard` reports `createdAt`, `updatedAt` and `type` beside the record's own
/// members).
///
/// Only the ARN is computable here, and only as a stand-in of that service's own form;
/// every other value is read off its own report of the dashboard
/// ([`crate::state::provisioner`]).
pub(super) const BCM_DASHBOARD_ATTRIBUTES: &[&str] = &["Arn", "CreatedAt", "Type", "UpdatedAt"];

/// Every `Fn::GetAtt` attribute an [`BCM_SCHEDULED_REPORT`] publishes, in that same order:
/// the ARN that service minted and the two instants it recorded for the report
/// (`GetScheduledReport` reports `createdAt` and `updatedAt` beside the report's own
/// members).
pub(super) const BCM_SCHEDULED_REPORT_ATTRIBUTES: &[&str] = &["Arn", "CreatedAt", "UpdatedAt"];

/// The one attribute of either list these naming rules compute rather than stand in for:
/// the ARN, which is also the identity each type's `Ref` returns.
pub(super) const BCM_ARN_ATTRIBUTE: &str = "Arn";

/// The `AWS::<cloudFormationName>::<Resource>` types of the two Amazon VPC Lattice resources
/// an access-logging estate declares. The middle segment is that model's own
/// `awsService.cloudFormationName` (`VpcLattice`, `smithy_query(query="service_info")` on that
/// service's model).
///
/// * a SERVICE NETWORK — the "logical boundary for a collection of services" that service's
///   own `CreateServiceNetwork` makes under the name the template states. It MINTS the
///   network's id (`^sn-[0-9a-z]{17}$`) and the ARN built around it, and "Ref returns the ARN
///   of the service network", so neither value can be computed from the template and
///   [`identity_is_service_assigned`] names the type. `Fn::GetAtt` reads `Arn`, `Id`,
///   `CreatedAt` and `LastUpdatedAt`, the last two being that service's clock;
/// * an ACCESS LOG SUBSCRIPTION — the delivery `CreateAccessLogSubscription` sets up from one
///   such network (or one service) to a CloudWatch Logs log group, a Firehose delivery stream
///   or an Amazon S3 bucket. That create takes NO identifier at all and answers the
///   `^als-[0-9a-z]{17}$` id it drew with the ARN around it, which is what "Ref returns the
///   ARN of the access log subscription" publishes, so the same holds. `Fn::GetAtt` reads
///   `Arn`, `Id`, `ResourceArn` and `ResourceId` — the last two being the monitored resource,
///   as that service resolved it.
pub const VPCLATTICE_SERVICE_NETWORK: &str = "AWS::VpcLattice::ServiceNetwork";
pub const VPCLATTICE_ACCESS_LOG_SUBSCRIPTION: &str =
    "AWS::VpcLattice::AccessLogSubscription";

/// Amazon VPC Lattice's ARN namespace, which is the namespace both of those ARNs carry
/// (`ServiceNetworkArn`:
/// `^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:servicenetwork/sn-[0-9a-z]{17}$`).
pub const VPCLATTICE_ARN_NAMESPACE: &str = "vpc-lattice";

/// The resource paths those two ARNs carry before the identifier that service minted, from
/// those same patterns.
pub(super) const VPCLATTICE_SERVICE_NETWORK_PATH: &str = "servicenetwork";
pub(super) const VPCLATTICE_ACCESS_LOG_SUBSCRIPTION_PATH: &str = "accesslogsubscription";

/// How many characters follow the `sn-` / `als-` prefix of either identifier: 17, from those
/// same patterns.
pub(super) const VPCLATTICE_ID_CHARACTERS: usize = 17;

/// The attributes `AWS::VpcLattice::ServiceNetwork` publishes, and the ones its subscription
/// sibling publishes. Only each type's ARN and the id inside it are values these naming rules
/// compute a stand-in for; the rest reach the resource off that service's own report.
pub(super) const VPCLATTICE_SERVICE_NETWORK_ATTRIBUTES: &[&str] =
    &["Arn", "CreatedAt", "Id", "LastUpdatedAt"];
pub(super) const VPCLATTICE_ACCESS_LOG_SUBSCRIPTION_ATTRIBUTES: &[&str] =
    &["Arn", "Id", "ResourceArn", "ResourceId"];

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon CloudWatch investigations
/// INVESTIGATION GROUP: the per-Region group that service's own `CreateInvestigationGroup`
/// makes, which "help[s] you centrally manage the common properties of your investigations".
/// The middle segment is that model's own `awsService.sdkId` (`AIOps`).
///
/// "Ref returns the Arn", and `Fn::GetAtt` reads `Arn`, `CreatedBy`, `CreatedAt`,
/// `LastModifiedBy` and `LastModifiedAt`. That service MINTS the identifier the ARN carries
/// (`InvestigationGroupArn`:
/// `^arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b):aiops:[a-zA-Z0-9-]*:[0-9]{12}:\
/// investigation-group\/[A-Za-z0-9]{16}$`, for which its create takes no member at all), which
/// is why [`identity_is_service_assigned`] names this type; the caller it records and the two
/// instants of its clock are that service's own and reach the resource off its report of the
/// group rather than being computed here.
pub const AIOPS_INVESTIGATION_GROUP: &str = "AWS::AIOps::InvestigationGroup";

/// Amazon CloudWatch investigations' ARN namespace, which that model carries as
/// `awsService.arnNamespace` (`aiops`) and an investigation group's ARN is named under.
pub const AIOPS_ARN_NAMESPACE: &str = "aiops";

/// The resource path an investigation group's ARN carries before the identifier that service
/// minted, and how many characters that identifier holds — both read off the modelled
/// `InvestigationGroupArn` pattern above, so the STAND-IN computed before the create is of
/// that same form.
pub(super) const AIOPS_INVESTIGATION_GROUP_PATH: &str = "investigation-group";
pub(super) const AIOPS_INVESTIGATION_GROUP_ID_CHARACTERS: usize = 16;

/// The salt that keeps that stand-in apart from every other identifier derived for one
/// resource ([`derived_digits`]).
pub(super) const AIOPS_INVESTIGATION_GROUP_ID_SALT: &str = "aiops/investigation-group";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Braket SPENDING LIMIT: the
/// cost control that service's own `CreateSpendingLimit` makes for a quantum device. The
/// middle segment is that model's own `awsService.sdkId` (`Braket`).
///
/// "Ref returns the spending limit ARN", and `Fn::GetAtt` reads `SpendingLimitArn` and
/// `CreatedAt`. Amazon Braket mints that ARN and keeps that instant, which is why
/// [`identity_is_service_assigned`] names this type and why neither value can be recomputed
/// from the template.
pub const BRAKET_SPENDING_LIMIT: &str = "AWS::Braket::SpendingLimit";

/// Amazon Braket's ARN namespace, which is the namespace a spending-limit ARN carries
/// (`SpendingLimitArn`: `^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$`).
pub const BRAKET_ARN_NAMESPACE: &str = "braket";

/// The resource path a spending-limit ARN carries before the identifier Amazon Braket minted
/// for it, from that same pattern.
pub(super) const BRAKET_SPENDING_LIMIT_PATH: &str = "spending-limit";

/// The `AWS::<cloudFormationName>::<Resource>` types of the two AWS HealthLake resources a
/// template declares. The middle segment is that model's own `awsService.cloudFormationName`
/// (`HealthLake`).
///
/// * a FHIR DATASTORE — the FHIR-enabled data store that service's own `CreateFHIRDatastore`
///   makes. "Ref returns the DatastoreId", and `Fn::GetAtt` reads `DatastoreArn`,
///   `DatastoreEndpoint`, `DatastoreId` and `DatastoreStatus`. HealthLake MINTS that id (a
///   `DatastoreId` of 32 characters, which its create takes no member for) and answers the
///   ARN, the endpoint and the status built around it, so [`identity_is_service_assigned`]
///   names the type and none of those values can be recomputed from the template;
/// * a DATA TRANSFORMATION PROFILE — the profile `CreateDataTransformationProfile` makes in
///   DRAFT state. That service mints the profile's id the same way (`ProfileIdString`:
///   `^[a-f0-9]{32}$`, which its create takes no member for) and the type's `Ref` returns the
///   profile's ARN, built around that id, with `Fn::GetAtt` reading `ProfileArn` and
///   `ProfileId` — so the same holds.
pub const HEALTHLAKE_FHIR_DATASTORE: &str = "AWS::HealthLake::FHIRDatastore";
pub const HEALTHLAKE_DATA_TRANSFORMATION_PROFILE: &str =
    "AWS::HealthLake::DataTransformationProfile";

/// AWS HealthLake's ARN namespace, which that model carries as `awsService.arnNamespace`
/// (`healthlake`) and both resources' ARNs are named under.
pub const HEALTHLAKE_ARN_NAMESPACE: &str = "healthlake";

/// The resource paths the two ARNs carry before the identifier HealthLake minted, from the
/// modelled `AmazonResourceName` pattern — `…:healthlake:<region>:<account>:(datastore\/\
/// fhir\/.{32}|dataTransformationProfile\/[a-f0-9]{32})$` — which is also the
/// `DataTransformationProfile` resource's own `aws.api#arn` template
/// (`dataTransformationProfile/{ProfileId}`).
pub const HEALTHLAKE_DATASTORE_ARN_PATH: &str = "datastore/fhir";
pub const HEALTHLAKE_PROFILE_ARN_PATH: &str = "dataTransformationProfile";

/// How many lowercase hexadecimal characters each identifier HealthLake mints carries: 32,
/// which is the width the `ProfileIdString` pattern fixes and the one the `AmazonResourceName`
/// pattern's `.{32}` admits for a data store — so the STAND-IN computed before the create is
/// of that same width. The salts keep the two labels distinct for one resource.
pub(super) const HEALTHLAKE_ID_HEX_DIGITS: usize = 32;
pub(super) const HEALTHLAKE_DATASTORE_ID_SALT: &str = "healthlake/datastore";
pub(super) const HEALTHLAKE_PROFILE_ID_SALT: &str = "healthlake/data-transformation-profile";

/// Every `Fn::GetAtt` attribute each of the two types publishes, in the sorted order a
/// resource's own attribute map holds them in.
///
/// Each is a read-only value of its type: for the data store, the ARN, the FHIR endpoint, the
/// id and the status HealthLake reports for one; for the profile, the ARN its `Ref` returns
/// beside the members `CreateDataTransformationProfile` answers that a template does not
/// declare — the id, the version it was created at, the format it converts to and the instant
/// that service recorded.
///
/// Only the data store's ARN and its id, the profile's, and the two values the profile's own
/// create documents ([`HEALTHLAKE_PROFILE_TARGET_FORMAT`],
/// [`HEALTHLAKE_PROFILE_DRAFT_VERSION`]) are computable here, and the identifiers only as
/// stand-ins of HealthLake's own forms; the endpoint it serves a store at, the status it
/// reports the store in and the instant it recorded for a profile are that service's own and
/// reach the resource off its report
/// ([`crate::state::provisioner::healthlake_resources`]).
pub(super) const HEALTHLAKE_FHIR_DATASTORE_ATTRIBUTES: &[&str] = &[
    "DatastoreArn",
    "DatastoreEndpoint",
    "DatastoreId",
    "DatastoreStatus",
];
pub(super) const HEALTHLAKE_PROFILE_ATTRIBUTES: &[&str] = &[
    "LastUpdatedAt",
    "ProfileArn",
    "ProfileId",
    "TargetFormat",
    "Version",
];

/// The format a data transformation profile's content converts TO, which is the one value the
/// modelled `TargetFormat` enum declares and which
/// `CreateDataTransformationProfile$TargetFormat` documents as "Always FHIR_R4" — so it is a
/// value these naming rules compute rather than stand in for.
pub(super) const HEALTHLAKE_PROFILE_TARGET_FORMAT: &str = "FHIR_R4";

/// The version a CREATED profile carries: "The starting version is always 0, which indicates
/// the profile is in DRAFT state" (`CreateDataTransformationProfile$Version`, a
/// `ProfileVersion` of range 0..99). What the resource publishes is what the create answered;
/// this is that answer's own documented value.
pub(super) const HEALTHLAKE_PROFILE_DRAFT_VERSION: &str = "0";

/// The `AWS::<cloudFormationName>::<Resource>` types of the two AWS Direct Connect resources a
/// template declares. The middle segment is that model's own `awsService.cloudFormationName`
/// (`DirectConnect`).
///
/// * a DIRECT CONNECT GATEWAY — the "intermediate object that enables you to connect a set of
///   virtual interfaces and virtual private gateways" that service's own
///   `CreateDirectConnectGateway` makes. It MINTS the gateway's id (`directConnectGatewayId`,
///   an opaque uuid), which every Direct Connect call on a gateway takes, and the type's `Ref`
///   returns the gateway's ARN built around that id — so neither value can be computed from
///   the template and [`identity_is_service_assigned`] names the type;
/// * a RESILIENCY GROUP — the group `CreateResiliencyGroup` makes, which "lets you group
///   Direct Connect connections together and manage them as a single unit to meet a target
///   resiliency model". That service mints the group's id too (`ResiliencyGroupId`:
///   `^(dxrg|DXRG)-[0-9a-zA-Z]{17}$`) and the ARN around it (`ResiliencyGroupArn`), and the
///   type's `Ref` returns that ARN, so the same holds.
pub const DIRECT_CONNECT_GATEWAY: &str = "AWS::DirectConnect::DirectConnectGateway";
pub const DIRECT_CONNECT_RESILIENCY_GROUP: &str = "AWS::DirectConnect::ResiliencyGroup";

/// AWS Direct Connect's ARN namespace (that model's `awsService.arnNamespace`), which is the
/// namespace both ARNs carry — a resiliency group's from `ResiliencyGroupArn`'s own pattern
/// (`^arn:(aws|aws-cn|aws-us-gov):directconnect::[0-9]{12}:dx-resiliency-group/\
/// dxrg-[0-9a-z]{17}$`), which also shows that an ARN of this service carries NO Region.
pub const DIRECT_CONNECT_ARN_NAMESPACE: &str = "directconnect";

/// The resource paths the two ARNs carry before the identifier that service minted: the one
/// `ResiliencyGroupArn`'s pattern states for a group, and the one a Direct Connect gateway is
/// named under (`arn:<partition>:directconnect::<account>:dx-gateway/<id>`).
pub const DIRECT_CONNECT_GATEWAY_PATH: &str = "dx-gateway";
pub const DIRECT_CONNECT_RESILIENCY_GROUP_PATH: &str = "dx-resiliency-group";

/// Every `Fn::GetAtt` attribute each of the two types publishes: the identifier AWS Direct
/// Connect minted for the resource, which is what every call of that service on one takes and
/// the value the ARN that type's `Ref` returns is built around.
pub(super) const DIRECT_CONNECT_GATEWAY_ATTRIBUTES: &[&str] = &["DirectConnectGatewayId"];
pub(super) const DIRECT_CONNECT_RESILIENCY_GROUP_ATTRIBUTES: &[&str] = &["ResiliencyGroupId"];

/// How a resiliency group's identifier is SPELLED, from `ResiliencyGroupId`'s own pattern and
/// length: the `dxrg` prefix, a hyphen and 17 more characters, drawn from the lowercase
/// alphabet `ResiliencyGroupArn` admits. The salt keeps this label distinct from any other
/// identifier derived for the same resource.
pub(super) const DIRECT_CONNECT_RESILIENCY_GROUP_ID_PREFIX: &str = "dxrg";
pub(super) const DIRECT_CONNECT_RESILIENCY_GROUP_ID_SALT: &str = "directconnect/resiliency-group";
pub(super) const DIRECT_CONNECT_RESILIENCY_GROUP_ID_DIGITS: usize = 17;

/// The `AWS::<cloudFormationName>::<Resource>` types of the two Directory Service directories
/// a template declares: the AWS Managed Microsoft AD directory `CreateMicrosoftAD` makes, and
/// the Simple AD directory `CreateDirectory` makes. The middle segment is that model's own
/// `awsService.cloudFormationName` (`DirectoryService`).
///
/// "Ref returns the directory ID" for either — the `^d-[0-9a-f]{10}$` identifier that service
/// MINTS, which no naming rule could compute and which every operation on a directory takes,
/// so [`identity_is_service_assigned`] names both types. `Fn::GetAtt` reads the directory's
/// `Alias` ("if no alias exists, the alias is the directory identifier") and its
/// `DnsIpAddresses`, a LIST this catalogue therefore stands in for no value of; a Simple AD
/// publishes its `DirectoryId` beside them.
pub const DIRECTORY_SERVICE_MICROSOFT_AD: &str = "AWS::DirectoryService::MicrosoftAD";
pub const DIRECTORY_SERVICE_SIMPLE_AD: &str = "AWS::DirectoryService::SimpleAD";

/// Every `Fn::GetAtt` attribute either directory type publishes that these naming rules
/// compute a stand-in for, in the sorted order a resource's own attribute map holds them in.
///
/// The `DnsIpAddresses` both types publish is not among them: it is a LIST, and the attribute
/// map is string-valued, so it reaches the resource one element per indexed key off that
/// service's own description of the directory — the position a hosted zone's `NameServers` is
/// in.
pub(super) const DIRECTORY_SERVICE_MICROSOFT_AD_ATTRIBUTES: &[&str] = &["Alias"];
pub(super) const DIRECTORY_SERVICE_SIMPLE_AD_ATTRIBUTES: &[&str] = &["Alias", "DirectoryId"];

/// The `AWS::<cloudFormationName>::<Resource>` types of the three CloudWatch Application
/// Signals resources a template declares. The middle segment is that model's own
/// `awsService.sdkId` (`Application Signals`) with its space closed up, as the resource-type
/// namespace publishes it.
///
/// * a SERVICE LEVEL OBJECTIVE — the objective `CreateServiceLevelObjective` makes under the
///   `Name` the declaration states. "Ref returns the ARN", which that service builds from its
///   own namespace, the Region and that name (`ServiceLevelObjectiveArn`:
///   `^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/<name>$`), so the identity IS
///   computable from the template and [`identity_is_service_assigned`] does not name the type;
/// * a GROUPING CONFIGURATION — the one configuration `PutGroupingConfiguration` keeps per
///   account and Region ("Creates or updates the grouping configuration for this account"), so
///   a stack declares at most one and its identity is the account's rather than a name of its
///   own ([`APPLICATION_SIGNALS_GROUPING_CONFIGURATION_ATTRIBUTES`]);
/// * an INSTRUMENTATION CONFIG — the dynamic instrumentation point
///   `CreateInstrumentationConfiguration` records for one code location. Its primary identifier
///   is COMPOSITE ([`APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_IDENTITY`]): the four dimensions
///   every read and delete of one addresses it by, and the LOCATION HASH that service computes
///   from the location. The ARN it MINTS around that hash (`InstrumentationConfigurationArn`:
///   `^arn:[^:]+:application-signals:[^:]+:[0-9]{12}:instrumentationConfig/.+/[0-9a-f]{16}$`) is
///   a published attribute beside it. The hash is no value a naming rule could compute, so
///   [`identity_is_service_assigned`] names this one.
pub const APPLICATION_SIGNALS_SERVICE_LEVEL_OBJECTIVE: &str =
    "AWS::ApplicationSignals::ServiceLevelObjective";
pub const APPLICATION_SIGNALS_GROUPING_CONFIGURATION: &str =
    "AWS::ApplicationSignals::GroupingConfiguration";
pub const APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG: &str =
    "AWS::ApplicationSignals::InstrumentationConfig";

/// CloudWatch Application Signals' ARN namespace, which is that model's own
/// `awsService.arnNamespace` (`application-signals`) — the namespace both an SLO's ARN and an
/// instrumentation configuration's carry.
pub const APPLICATION_SIGNALS_ARN_NAMESPACE: &str = "application-signals";

/// Amazon CloudWatch Application Insights' own APPLICATION type (that model:
/// `cloudFormationName: ApplicationInsights`, `arnNamespace: applicationinsights`). This is a
/// type `crate::state::provisioner` creates in Application Insights itself, over the resource
/// group the declaration names.
///
/// "Ref returns the `ApplicationARN`", which that service builds from its own namespace, the
/// Region, the account and the resource group the application monitors — a total function of the
/// template, so [`identity_is_service_assigned`] does not name the type.
pub const APPLICATION_INSIGHTS_APPLICATION: &str = "AWS::ApplicationInsights::Application";

/// Application Insights' ARN namespace, which is that model's own `awsService.arnNamespace`
/// (`applicationinsights`) rather than the `ApplicationInsights` its resource type is spelled
/// with.
pub const APPLICATION_INSIGHTS_ARN_NAMESPACE: &str = "applicationinsights";

/// The resource path an application's ARN carries before the group it monitors:
/// `application/resource-group/<ResourceGroupName>`, the form that service publishes for an
/// application. Visible to the rest of this service's state because the arm that provisions the
/// type reads the group back out of that ARN
/// ([`crate::state::provisioner`]), which has to agree with the ARN written here.
pub const APPLICATION_INSIGHTS_APPLICATION_PATH: &str = "application/resource-group";

/// The one `Fn::GetAtt` attribute an [`APPLICATION_INSIGHTS_APPLICATION`] publishes, which is
/// also what its `Ref` returns: the application's own ARN.
pub const APPLICATION_INSIGHTS_APPLICATION_ARN_ATTRIBUTE: &str = "ApplicationARN";

/// The property an application declares the resource group it monitors in, which is the one
/// part of its ARN the template decides.
pub(super) const APPLICATION_INSIGHTS_RESOURCE_GROUP_PROPERTY: &str = "ResourceGroupName";

/// The resource path an SLO's ARN carries before the objective's name, and the one an
/// instrumentation configuration's carries before its four identity parts, from each member's
/// own pattern above.
pub(super) const APPLICATION_SIGNALS_SLO_PATH: &str = "slo";
pub(super) const APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_PATH: &str = "instrumentationConfig";

/// Every `Fn::GetAtt` attribute an [`APPLICATION_SIGNALS_SERVICE_LEVEL_OBJECTIVE`] publishes,
/// in the sorted order a resource's own attribute map holds them in: the read-only values the
/// `ServiceLevelObjective` record carries beside the configuration a declaration states.
///
/// The ARN is the one these naming rules compute — both this service and that one build it from
/// the account id this emulator mints, the Region and the objective's name — and the rest are
/// Application Signals' own: the two instants its clock recorded, which of the two indicators
/// the objective was created with and the metric source it derived from that indicator. Each of
/// those stands in ([`service_assigned_document`]) until
/// [`crate::state::provisioner`] has reported what that service really answered.
pub(super) const APPLICATION_SIGNALS_SLO_ATTRIBUTES: &[&str] = &[
    "Arn",
    "CreatedTime",
    "EvaluationType",
    "LastUpdatedTime",
    "MetricSourceType",
];

/// The one attribute of [`APPLICATION_SIGNALS_SLO_ATTRIBUTES`] these naming rules compute
/// rather than stand in for, which is also what the type's `Ref` returns.
pub(super) const APPLICATION_SIGNALS_SLO_ARN_ATTRIBUTE: &str = "Arn";

/// Every `Fn::GetAtt` attribute an [`APPLICATION_SIGNALS_GROUPING_CONFIGURATION`] publishes,
/// in the sorted order a resource's own attribute map holds them in: the ACCOUNT whose
/// configuration it is — the type's own primary identifier, which is also what its `Ref`
/// returns — and the instant that service recorded for the configuration
/// (`GroupingConfiguration$UpdatedAt`).
///
/// The account is no member of the wire at all: `PutGroupingConfiguration` takes only the
/// definitions and answers only them and the instant, so the configuration is the SINGLETON of
/// the account the call was signed for, and that account is what the type publishes as its
/// `AccountId`. The id is a value these naming rules compute (the one account this emulator
/// mints); the instant is that service's clock, so it stands in
/// ([`service_assigned_document`]) until [`crate::state::provisioner`] has reported what the
/// put answered.
pub(super) const APPLICATION_SIGNALS_GROUPING_CONFIGURATION_ATTRIBUTES: &[&str] =
    &["AccountId", "UpdatedAt"];

/// The attribute of [`APPLICATION_SIGNALS_GROUPING_CONFIGURATION_ATTRIBUTES`] that carries the
/// account whose configuration the resource is, which is the identity a `Ref` to one resolves
/// to. Visible to the rest of this service's state because it IS that type's primary identifier
/// ([`crate::state::primary_identifier`]).
pub const APPLICATION_SIGNALS_ACCOUNT_ID_ATTRIBUTE: &str = "AccountId";

/// Every `Fn::GetAtt` attribute an [`APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG`] publishes, in
/// the sorted order a resource's own attribute map holds them in: the three values the created
/// record carries that no declaration states — the ARN that service minted, the instant its
/// clock recorded and the `LocationHash` it computed from the location.
pub(super) const APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_ATTRIBUTES: &[&str] =
    &["Arn", "CreatedAt", "LocationHash"];

/// The attribute of [`APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_ATTRIBUTES`] that carries the
/// ARN that service minted, which the type publishes BESIDE its identity rather than as it
/// ([`APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_IDENTITY`]).
pub(super) const APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_ARN_ATTRIBUTE: &str = "Arn";

/// The attribute of [`APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_ATTRIBUTES`] that carries the
/// location hash that service computed, which is the last part of the type's own identity.
pub(super) const APPLICATION_SIGNALS_LOCATION_HASH_ATTRIBUTE: &str = "LocationHash";

/// How many lowercase hexadecimal characters an instrumentation configuration's `LocationHash`
/// carries: `LocationHash`'s own `@length(min: 16, max: 16)`, which the ARN pattern repeats as
/// its `[0-9a-f]{16}` final segment. The stand-in identity is built to that width
/// ([`return_values`]) so a value standing in for one is the shape that service answers.
pub(super) const APPLICATION_SIGNALS_LOCATION_HASH_CHARS: usize = 16;

/// The properties an instrumentation configuration's identity is spelled from: the four
/// dimensions `GetInstrumentationConfiguration` and `DeleteInstrumentationConfiguration` address
/// one by, and the LOCATION that service hashes into the fifth part of that identity.
pub(super) const APPLICATION_SIGNALS_INSTRUMENTATION_TYPE_PROPERTY: &str = "InstrumentationType";
pub(super) const APPLICATION_SIGNALS_SERVICE_PROPERTY: &str = "Service";
pub(super) const APPLICATION_SIGNALS_ENVIRONMENT_PROPERTY: &str = "Environment";
pub(super) const APPLICATION_SIGNALS_SIGNAL_TYPE_PROPERTY: &str = "SignalType";
pub(super) const APPLICATION_SIGNALS_LOCATION_PROPERTY: &str = "Location";

/// The COMPOSITE primary identifier of an [`APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG`], in the
/// order its parts are joined in: the four dimensions that address one configuration, and the
/// `LocationHash` that service computed from the location it watches.
///
/// Those five values are what every read and delete of one configuration takes
/// (`GetInstrumentationConfiguration`/`DeleteInstrumentationConfiguration` carry the four beside
/// a `LocationIdentifier` naming the location by its hash), and the type declares no name of its
/// own — so they are what a client names one by, which is what this type's `Ref` returns
/// ([`instrumentation_config_identity`]).
pub const APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_IDENTITY: &[&str] = &[
    APPLICATION_SIGNALS_INSTRUMENTATION_TYPE_PROPERTY,
    APPLICATION_SIGNALS_SERVICE_PROPERTY,
    APPLICATION_SIGNALS_ENVIRONMENT_PROPERTY,
    APPLICATION_SIGNALS_SIGNAL_TYPE_PROPERTY,
    APPLICATION_SIGNALS_LOCATION_HASH_ATTRIBUTE,
];

/// The physical resource id of one such configuration: the five parts of that identity joined
/// with [`COMPOSITE_IDENTITY_SEPARATOR`], which is what the type's `Ref` returns.
///
/// Not a row of [`composite_identity`]: that one builds an identity out of the values a
/// declaration STATES, and the last part of this one is the hash Application Signals computes,
/// which no declaration carries. So the identity is built here as a stand-in and again by
/// [`crate::state::provisioner`] from the record that service answered — the one place every
/// part of it is really known.
pub(crate) fn instrumentation_config_identity(
    instrumentation_type: &str,
    service: &str,
    environment: &str,
    signal_type: &str,
    location_hash: &str,
) -> String {
    [
        instrumentation_type,
        service,
        environment,
        signal_type,
        location_hash,
    ]
    .join(COMPOSITE_IDENTITY_SEPARATOR)
}

/// The `LocationHash` such an identity carries ([`instrumentation_config_identity`]); `None` for
/// a value that is not one, so a call is never addressed with a part of a value of another
/// shape.
pub(crate) fn instrumentation_config_location_hash(identity: &str) -> Option<&str> {
    let parts: Vec<&str> = identity.split(COMPOSITE_IDENTITY_SEPARATOR).collect();
    if parts.len() != APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_IDENTITY.len() {
        return None;
    }
    parts.last().copied()
}

/// The `AWS::<cloudFormationName>::<Resource>` type of an AWS Elemental MEDIACONVERT
/// CERTIFICATE ASSOCIATION: the trust between an AWS Certificate Manager certificate and the
/// account's MediaConvert service, which that service's own `AssociateCertificate` makes and
/// `DisassociateCertificate` takes away. The middle segment is that model's own
/// `awsService.cloudFormationName` (`MediaConvert`).
///
/// The type publishes ONE property, the `Arn` of the certificate, and its `Ref` is that ARN —
/// the only handle either of those calls takes. So the resource's physical id IS the declared
/// ARN ([`declared_name_property`] names the property it is read from), the type publishes no
/// `Fn::GetAtt` value of its own, and nothing about its identity is the owning service's to
/// mint.
pub const MEDIACONVERT_CERTIFICATE_ASSOCIATION: &str = "AWS::MediaConvert::CertificateAssociation";

/// The `AWS::<cloudFormationName>::<Resource>` type of a CLEAN ROOMS ML TRAINING DATASET: the
/// metadata record that service's own `CreateTrainingDataset` makes for a Glue table, which an
/// audience model is then trained from. The middle segment is that model's own
/// `awsService.sdkId` (`CleanRoomsML`).
///
/// "Ref returns the training dataset ARN" — the type's primary identifier — and `Fn::GetAtt`
/// reads [`CLEANROOMSML_TRAINING_DATASET_ARN_ATTRIBUTE`] and the dataset's `Status`. Clean Rooms
/// ML MINTS that ARN:
/// `CreateTrainingDataset` takes no identifier for the dataset — the declaration states its
/// name, the role to read the data with and the datasets themselves — and answers the
/// `trainingDatasetArn` it assigned, whose resource path carries an identifier of its own
/// (`TrainingDatasetArn`:
/// `^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:training-dataset/[-a-zA-Z0-9_/.]+$`), so
/// it cannot be computed from the template — that is what [`identity_is_service_assigned`]
/// names this type for, and it is the handle `GetTrainingDataset` and `DeleteTrainingDataset`
/// each address a dataset by.
pub const CLEANROOMSML_TRAINING_DATASET: &str = "AWS::CleanRoomsML::TrainingDataset";

/// Clean Rooms ML's ARN namespace, which is the namespace a training-dataset ARN carries (that
/// same pattern), and the resource path such an ARN carries before the identifier that service
/// minted.
pub const CLEANROOMSML_ARN_NAMESPACE: &str = "cleanrooms-ml";
pub(super) const CLEANROOMSML_TRAINING_DATASET_PATH: &str = "training-dataset";

/// The one `Fn::GetAtt` attribute a [`CLEANROOMSML_TRAINING_DATASET`] publishes that these
/// naming rules can compute a stand-in for: the ARN Clean Rooms ML minted, which is also what
/// the type's `Ref` returns. The dataset's `Status` is that service's own to report and reaches
/// the resource off its report of the created dataset
/// ([`crate::state::provisioner::Provisioner::CleanRoomsMlTrainingDataset`]), exactly as an
/// `AWS::Braket::SpendingLimit`'s `CreatedAt` does.
pub const CLEANROOMSML_TRAINING_DATASET_ARN_ATTRIBUTE: &str = "TrainingDatasetArn";

/// The `AWS::<Namespace>::<Resource>` type of a CLOUDHSM CLUSTER: the cluster of hardware
/// security modules CloudHSM's own `CreateCluster` places in the caller's subnets, which HSMs
/// are then added to and which is initialized before it serves any cryptographic operation.
///
/// The namespace is `CloudHSM`, as the "AWS resource and property types reference" publishes
/// it — NOT that model's own `awsService.cloudFormationName` (`CloudHSMV2`): the type is what a
/// template spells, the same position [`ECR_PUBLIC_REPOSITORY`] is in.
///
/// `Ref` returns the cluster's ARN — the type's primary identifier — and `Fn::GetAtt` reads
/// [`CLOUDHSM_CLUSTER_ATTRIBUTES`]. CloudHSM MINTS the cluster's identity: `CreateCluster`
/// takes no identifier at all — the declaration states the subnets, the HSM type and the mode
/// — and answers the `ClusterId` it assigned (`^cluster-[2-7a-zA-Z]{11,16}$`), which is the
/// handle every operation on a cluster takes and what the ARN below carries, so neither can be
/// computed from the template — that is what [`identity_is_service_assigned`] names this type
/// for.
pub const CLOUDHSM_CLUSTER: &str = "AWS::CloudHSM::Cluster";

/// CloudHSM's ARN namespace (that model's own `awsService.arnNamespace`, `cloudhsm`), and the
/// resource path a cluster's ARN carries before the id that service minted —
/// `arn:<partition>:cloudhsm:<region>:<account>:cluster/<clusterId>`.
pub const CLOUDHSM_ARN_NAMESPACE: &str = "cloudhsm";
pub const CLOUDHSM_CLUSTER_PATH: &str = "cluster";

/// Every `Fn::GetAtt` attribute a [`CLOUDHSM_CLUSTER`] publishes, in the sorted order a
/// resource's own attribute map holds them in: the ARN the type's `Ref` also returns, the
/// backup policy the cluster's backups are kept under, the id CloudHSM minted, the security
/// group it made for the cluster, the state it holds the cluster in and the VPC its subnets
/// are in.
///
/// Only the first two are values these naming rules can compute a stand-in of the right FORM
/// for; the rest are the owning service's own to report and reach the resource off its
/// description of the created cluster
/// ([`crate::state::provisioner::Provisioner::CloudHsmCluster`]), exactly as a Clean Rooms ML
/// training dataset's `Status` does.
pub const CLOUDHSM_CLUSTER_ATTRIBUTES: &[&str] = &[
    CLOUDHSM_CLUSTER_ARN_ATTRIBUTE,
    CLOUDHSM_CLUSTER_BACKUP_POLICY_ATTRIBUTE,
    CLOUDHSM_CLUSTER_ID_ATTRIBUTE,
    CLOUDHSM_CLUSTER_SECURITY_GROUP_ATTRIBUTE,
    CLOUDHSM_CLUSTER_STATE_ATTRIBUTE,
    CLOUDHSM_CLUSTER_VPC_ID_ATTRIBUTE,
];

/// The attributes above, each named where both the catalogue and the arms that provision the
/// type read it: every one but the ARN is a member of that service's own `Cluster` shape,
/// spelled the same way ([`crate::state::provisioner::cloudhsm_clusters`] reads them off a
/// described cluster).
pub const CLOUDHSM_CLUSTER_ARN_ATTRIBUTE: &str = "Arn";
pub const CLOUDHSM_CLUSTER_BACKUP_POLICY_ATTRIBUTE: &str = "BackupPolicy";
pub const CLOUDHSM_CLUSTER_ID_ATTRIBUTE: &str = "ClusterId";
pub const CLOUDHSM_CLUSTER_SECURITY_GROUP_ATTRIBUTE: &str = "SecurityGroup";
pub const CLOUDHSM_CLUSTER_STATE_ATTRIBUTE: &str = "State";
pub const CLOUDHSM_CLUSTER_VPC_ID_ATTRIBUTE: &str = "VpcId";

/// What a stand-in cluster id is built of: the prefix a `ClusterId` carries, the number of
/// characters after it and the alphabet those characters are drawn from — the modelled
/// `^cluster-[2-7a-zA-Z]{11,16}$`, which carries no `0`, `1`, `8` or `9` and so is not the
/// hexadecimal most service-assigned identifiers are written over
/// ([`derived_over`](super::super::naming_rules::derived_over) folds the digest into it).
pub(super) const CLOUDHSM_CLUSTER_ID_PREFIX: &str = "cluster-";
pub(super) const CLOUDHSM_CLUSTER_ID_CHARS: usize = 11;
pub(super) const CLOUDHSM_CLUSTER_ID_ALPHABET: &[u8] =
    b"234567abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The salt that stand-in is derived under, so the cluster id of one resource is not the digits
/// of anything else derived from the same name.
pub(super) const CLOUDHSM_CLUSTER_ID_SALT: &str = "cloudhsm-cluster";

/// The resource type of an IOT CORE DEVICE ADVISOR SUITE DEFINITION: the test suite AWS IoT
/// Core Device Advisor's own `CreateSuiteDefinition` records, which a suite RUN is then started
/// from.
///
/// The middle segment is `IoTCoreDeviceAdvisor`, as the AWS resource and property types
/// reference spells the namespace — NOT that model's own `awsService.cloudFormationName`
/// (`IotDeviceAdvisor`): the type is what a template spells, the same position
/// [`ECR_PUBLIC_REPOSITORY`] is in.
///
/// "Ref returns the suite definition id" and `Fn::GetAtt` reads
/// [`IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_ATTRIBUTES`]. That service MINTS the id:
/// `CreateSuiteDefinition` takes no identifier for the suite — the declaration states the
/// suite's configuration and its tags — and answers the `suiteDefinitionId` it drew (a `UUID`,
/// `@length 12..36`) with the `suitedefinition/<id>` ARN built around it, which is the handle
/// `GetSuiteDefinition`, `UpdateSuiteDefinition` and `DeleteSuiteDefinition` each address one
/// by. No name CloudFormation decides is such a uuid, so neither the id nor the ARN can be
/// computed from the template — that is what [`identity_is_service_assigned`] names this type
/// for.
pub const IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION: &str =
    "AWS::IoTCoreDeviceAdvisor::SuiteDefinition";

/// That service's ARN namespace, which is the namespace a suite-definition ARN carries (its
/// model's own `awsService.arnNamespace`, `iotdeviceadvisor`), and the resource path such an ARN
/// carries before the id that service minted.
pub const IOT_DEVICE_ADVISOR_ARN_NAMESPACE: &str = "iotdeviceadvisor";
pub(super) const IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_PATH: &str = "suitedefinition";

/// Every `Fn::GetAtt` attribute an [`IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION`] publishes, in
/// the sorted order a resource's own attribute map holds them in: the ARN that service built,
/// the id it minted and the version label it keeps the suite's live configuration under.
///
/// All three are values only AWS IoT Core Device Advisor produces, so each is a STAND-IN until
/// the deployment has re-pointed it at what that service really answered
/// ([`crate::state::provisioner::Provisioner::IotCoreDeviceAdvisorSuiteDefinition`]).
pub(super) const IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_ATTRIBUTES: &[&str] = &[
    "SuiteDefinitionArn",
    "SuiteDefinitionId",
    "SuiteDefinitionVersion",
];

/// The two of those attributes the naming rules compute from the id they stand in with: the
/// ARN built around it, and the id itself — which is also what the type's `Ref` returns.
pub const IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_ARN_ATTRIBUTE: &str = "SuiteDefinitionArn";
pub const IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_ID_ATTRIBUTE: &str = "SuiteDefinitionId";

/// The remaining one, which no naming rule can compute: the label that service keeps the
/// suite's live configuration under, reported off its own read of the suite.
pub const IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_VERSION_ATTRIBUTE: &str =
    "SuiteDefinitionVersion";

/// The `AWS::<cloudFormationName>::<Resource>` type of a BACKUP GATEWAY HYPERVISOR: the
/// on-premises host Backup Gateway reads virtual machines off, connected to by that service's
/// own `ImportHypervisorConfiguration`. The middle segment is that model's own
/// `awsService.cloudFormationName` (`BackupGateway`).
///
/// "Ref returns the hypervisor ARN" — the type's primary identifier — and `Fn::GetAtt` reads
/// `HypervisorArn`. Backup Gateway MINTS that ARN: the import takes no identifier for the
/// hypervisor at all and answers the `HypervisorArn` it assigned, which is the `ServerArn`
/// every later call on one takes, so it cannot be computed from the template — that is what
/// [`identity_is_service_assigned`] names this type for.
pub const BACKUP_GATEWAY_HYPERVISOR: &str = "AWS::BackupGateway::Hypervisor";

/// Backup Gateway's ARN namespace, which is the namespace a hypervisor ARN carries
/// (`ServerArn`: `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`),
/// and the resource path such an ARN carries before the identifier that service minted.
pub const BACKUP_GATEWAY_ARN_NAMESPACE: &str = "backup-gateway";
pub(super) const BACKUP_GATEWAY_HYPERVISOR_PATH: &str = "hypervisor";

/// The salt the hypervisor-id stand-in is derived under, how many characters it carries and
/// the prefix it opens with: a hypervisor identity of that service is `hype-` followed by
/// eight lower-case hexadecimal digits, which is the form the ARN's own
/// `[a-zA-Z-0-9]+` resource segment carries.
pub(super) const BACKUP_GATEWAY_HYPERVISOR_ID_SALT: &str = "backup-gateway/hypervisor";
pub(super) const BACKUP_GATEWAY_HYPERVISOR_ID_DIGITS: usize = 8;
pub(super) const BACKUP_GATEWAY_HYPERVISOR_ID_PREFIX: &str = "hype-";

/// The one `Fn::GetAtt` attribute a [`BACKUP_GATEWAY_HYPERVISOR`] publishes: the ARN Backup
/// Gateway minted, which is also what the type's `Ref` returns.
pub(super) const BACKUP_GATEWAY_HYPERVISOR_ATTRIBUTES: &[&str] = &["HypervisorArn"];

/// The `AWS::<cloudFormationName>::<Resource>` type of an AWS Amplify APP: the hosting
/// application that service's own `CreateApp` makes, and the parent every other Amplify
/// resource hangs off. The middle segment is that model's own `awsService.cloudFormationName`
/// (`Amplify`).
///
/// "Ref returns the ARN of the app" — the type's primary identifier — and `Fn::GetAtt` reads
/// [`AMPLIFY_APP_ATTRIBUTES`]. AWS Amplify MINTS the `appId` (`^d[a-z0-9]+$`, which no name
/// CloudFormation decides satisfies) and the ARN built around it, and serves the app's branches
/// under a default domain of its own, so none of those values can be recomputed from the
/// template: that is what [`identity_is_service_assigned`] names this type for.
pub const AMPLIFY_APP: &str = "AWS::Amplify::App";

/// Every `Fn::GetAtt` attribute an [`AMPLIFY_APP`] publishes, in the sorted order a resource's
/// own attribute map holds them in: the id that service minted, the app's name, its ARN and the
/// default domain it is served under — each of them a value only AWS Amplify can produce, so
/// each stands in ([`service_assigned_document`]) until that service has reported what it
/// really answered.
pub(super) const AMPLIFY_APP_ATTRIBUTES: &[&str] =
    &["AppId", "AppName", "Arn", "DefaultDomain"];

/// The `AWS::<cloudFormationName>::<Resource>` type of an AWS Amplify BRANCH: the branch of an
/// [`AMPLIFY_APP`] that service's own `CreateBranch` makes, and the unit Amplify hosts — every
/// `GetBranch`, `ListBranches`, `UpdateBranch` and deployment call addresses one.
///
/// "Ref returns the ARN of the branch" — the type's primary identifier — and `Fn::GetAtt` reads
/// [`AMPLIFY_BRANCH_ATTRIBUTES`]. That ARN is built around the `appId` AWS Amplify minted for
/// the app the branch belongs to, so it cannot be recomputed from the template: that is what
/// [`identity_is_service_assigned`] names this type for. The branch's NAME is the declaration's
/// own `BranchName`, and every call of that service on a branch addresses it BY that name, so a
/// re-declared name asks for a different branch.
pub const AMPLIFY_BRANCH: &str = "AWS::Amplify::Branch";

/// Every `Fn::GetAtt` attribute an [`AMPLIFY_BRANCH`] publishes, in the sorted order a
/// resource's own attribute map holds them in: the ARN AWS Amplify built for the branch, which
/// stands in ([`service_assigned_document`]) until that service has reported it, and the
/// branch's own name, which the declaration states.
pub(super) const AMPLIFY_BRANCH_ATTRIBUTES: &[&str] = &["Arn", "BranchName"];

/// The `AWS::<cloudFormationName>::<Resource>` types of the three user-interface entities of
/// an Amplify app environment: the component, the form and the theme Amplify UI
/// Builder's own `CreateComponent`, `CreateForm` and `CreateTheme` make. The middle segment is
/// that model's own `awsService.cloudFormationName` (`AmplifyUIBuilder`) and the resource
/// segment each entity's own `@cfnResource` name (`Component`, `Form`, `Theme`).
///
/// Each type's identity is that service's to mint: every one of those creates takes the app
/// and the environment as its URI labels and the entity's NAME in its payload, takes no
/// identifier at all, and answers the `id` Amplify UI Builder assigned — a `Uuid`, which is
/// what `GetComponent`, `UpdateForm`, `DeleteTheme` and every other call on one is addressed
/// by. That is what [`identity_is_service_assigned`] names all three for.
///
/// The values a resource of one of these types publishes are the three identifiers of its own
/// resource shape (`smithy_query(query="shape", shape="ComponentResource")`:
/// `identifiers: {id, appId, environmentName}`) — the app and the environment the declaration
/// states, and the id that service minted — beside the members the entity itself carries as
/// READ-ONLY (`cfnMutability: "read"`), which are the two instants a component and a theme are
/// reported with ([`AMPLIFYUIBUILDER_INSTANT_ATTRIBUTES`]); a form's own structure carries
/// neither.
pub const AMPLIFYUIBUILDER_COMPONENT: &str = "AWS::AmplifyUIBuilder::Component";
pub const AMPLIFYUIBUILDER_FORM: &str = "AWS::AmplifyUIBuilder::Form";
pub const AMPLIFYUIBUILDER_THEME: &str = "AWS::AmplifyUIBuilder::Theme";

/// Every `Fn::GetAtt` attribute an `AWS::AmplifyUIBuilder::Form` publishes, in the sorted order
/// a resource's own attribute map holds them in: the identifiers of the type's resource shape
/// ([`AMPLIFYUIBUILDER_COMPONENT`]).
///
/// Two of them are the declaration's own values, and the `Id` is the one value only Amplify UI
/// Builder can produce, so it stands in as a uuid of that member's own form
/// ([`generated_uuid`]) until `super::provisioning` has reported the id the create really
/// answered.
pub(super) const AMPLIFYUIBUILDER_ATTRIBUTES: &[&str] = &["AppId", "EnvironmentName", "Id"];

/// The two further attributes a COMPONENT and a THEME publish: the instants their own
/// structures carry `cfnMutability: "read"` on (`Component$createdAt`, `Component$modifiedAt`
/// and the same pair on `Theme`), which is that model's own statement that they are read-only
/// properties of the type. A `Form`'s structure carries neither member, so a form publishes
/// only [`AMPLIFYUIBUILDER_ATTRIBUTES`].
///
/// Both are Amplify UI Builder's clock, so neither is computed here: each stands in
/// ([`service_assigned_document`]) until that service has reported the entity it created.
pub(super) const AMPLIFYUIBUILDER_INSTANT_ATTRIBUTES: &[&str] = &["CreatedAt", "ModifiedAt"];

/// [`AMPLIFYUIBUILDER_ATTRIBUTES`] and [`AMPLIFYUIBUILDER_INSTANT_ATTRIBUTES`] together, in
/// that sorted order: what a component and a theme publish.
pub(super) const AMPLIFYUIBUILDER_TIMED_ATTRIBUTES: &[&str] =
    &["AppId", "CreatedAt", "EnvironmentName", "Id", "ModifiedAt"];

/// The properties a declaration of one of those types states the app environment the entity
/// belongs to in, which are also two of the values it publishes
/// ([`AMPLIFYUIBUILDER_ATTRIBUTES`]).
pub(super) const AMPLIFYUIBUILDER_APP_ID: &str = "AppId";
pub(super) const AMPLIFYUIBUILDER_ENVIRONMENT_NAME: &str = "EnvironmentName";

/// The attribute carrying the identifier Amplify UI Builder mints for the entity.
pub(super) const AMPLIFYUIBUILDER_ID_ATTRIBUTE: &str = "Id";

/// The physical resource id of one of those three entities: the TRIPLE its own resource shape
/// is identified by — the app, the environment and the id that service minted — joined with
/// [`COMPOSITE_IDENTITY_SEPARATOR`], which is what the type's `Ref` returns.
///
/// Not a row of [`composite_identity_properties`]: that one builds an identity out of the
/// values a declaration STATES, and the last part of this one is the id Amplify UI Builder
/// assigns, which no declaration carries. So the identity is built here as a stand-in and
/// again by [`crate::state::provisioner`] from the entity that service answered — the one
/// place each of the three parts is really known.
pub(crate) fn amplifyuibuilder_identity(app_id: &str, environment_name: &str, id: &str) -> String {
    composite_identity_triple(app_id, environment_name, id)
}

/// The three parts of such an identity, where `identity` is one
/// ([`amplifyuibuilder_identity`]); `None` for a value that is not.
pub(crate) fn amplifyuibuilder_identity_parts(identity: &str) -> Option<(&str, &str, &str)> {
    composite_identity_triple_parts(identity)
}

/// The physical resource id a type whose primary identifier is a TRIPLE is known by: the three
/// values the owning service's own API addresses one resource by, joined with
/// [`COMPOSITE_IDENTITY_SEPARATOR`], which is what such a type's `Ref` returns.
///
/// The ONE join of that identity, which every per-type function above and below delegates to, so
/// the readings of that separator cannot diverge — and the one place a value that is not such a
/// triple is told apart from one that is ([`composite_identity_triple_parts`]).
pub(crate) fn composite_identity_triple(first: &str, second: &str, third: &str) -> String {
    [first, second, third].join(COMPOSITE_IDENTITY_SEPARATOR)
}

/// The three parts of such an identity ([`composite_identity_triple`]); `None` for a value that
/// is not one, which is what keeps a call from being addressed with a value of another shape.
pub(crate) fn composite_identity_triple_parts(identity: &str) -> Option<(&str, &str, &str)> {
    let mut parts = identity.split(COMPOSITE_IDENTITY_SEPARATOR);
    let (first, second, third) = (parts.next()?, parts.next()?, parts.next()?);
    parts.next().is_none().then_some((first, second, third))
}

/// The attribute of [`AMPLIFY_APP_ATTRIBUTES`] that carries the app's ARN, which is the
/// identity a `Ref` to one resolves to.
pub(super) const APP_ARN_STAND_IN_ATTRIBUTE: &str = "Arn";

/// The attribute of [`AMPLIFY_BRANCH_ATTRIBUTES`] that carries the branch's ARN, which is the
/// identity a `Ref` to one resolves to, beside the one that carries its name.
pub(super) const BRANCH_ARN_STAND_IN_ATTRIBUTE: &str = "Arn";
pub(super) const BRANCH_NAME_STAND_IN_ATTRIBUTE: &str = "BranchName";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Managed Workflows for
/// Apache Airflow ENVIRONMENT: the Apache Airflow deployment that service's own
/// `CreateEnvironment` makes. The middle segment is that model's own `awsService.sdkId`
/// (`MWAA`).
///
/// "Ref returns the name of the Amazon MWAA environment", which is the name the owning
/// service creates the environment under — so a GENERATED one is held to that service's own
/// rules for it ([`naming_rules`]) and the identity is not service-assigned.
pub const MWAA_ENVIRONMENT: &str = "AWS::MWAA::Environment";

/// Amazon MWAA's ARN namespace (that model's `awsService.arnNamespace`), which is `airflow`
/// rather than the `MWAA` its resource type is spelled with — the namespace an environment
/// ARN carries (`EnvironmentArn`:
/// `^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+`).
pub(super) const MWAA_ARN_NAMESPACE: &str = "airflow";

/// The resource path an environment ARN carries before the environment's name, from that
/// same pattern.
pub(super) const MWAA_ENVIRONMENT_PATH: &str = "environment";

/// Every `Fn::GetAtt` attribute an `AWS::MWAA::Environment` publishes, in the sorted order a
/// resource's own attribute map holds them in.
///
/// Source: the type's own "Return values" section — the environment's ARN, the Apache Airflow
/// web server's URL, the Celery Executor's queue, the two VPC endpoint services, and the
/// CloudWatch Logs group each Apache Airflow log type publishes to, which that section spells
/// `LoggingConfiguration.<log type>.CloudWatchLogGroupArn`.
///
/// Only the ARN is a value these naming rules can compute. Every other one is Amazon MWAA's
/// own — the web server host it brings up, the queue and the endpoint services it makes for
/// the environment, the log groups it creates for the log types the declaration enabled — so
/// each stands in ([`service_assigned_document`]) until that service has reported what it
/// really answered.
pub(super) const MWAA_ENVIRONMENT_ATTRIBUTES: &[&str] = &[
    "Arn",
    "CeleryExecutorQueue",
    "DatabaseVpcEndpointService",
    "LoggingConfiguration.DagProcessingLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.SchedulerLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.TaskLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.WebserverLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.WorkerLogs.CloudWatchLogGroupArn",
    "WebserverUrl",
    "WebserverVpcEndpointService",
];

/// The one attribute of [`MWAA_ENVIRONMENT_ATTRIBUTES`] these naming rules compute rather
/// than stand in for.
pub(super) const MWAA_ENVIRONMENT_ARN_ATTRIBUTE: &str = "Arn";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Connect Customer Profiles
/// DOMAIN: "a container for all customer data, such as customer profile attributes, object
/// types, profile keys, and encryption keys" (`CreateDomain`), which that service's own
/// `CreateDomain` makes. The middle segment is that model's own
/// `awsService.cloudFormationName` (`CustomerProfiles`).
///
/// "Ref returns the DomainName", which is the name the owning service creates the domain under
/// and the `@httpLabel` every operation on one takes — so a GENERATED one is held to that
/// service's own rules for it ([`naming_rules`]) and the identity is not service-assigned.
pub const CUSTOMER_PROFILES_DOMAIN: &str = "AWS::CustomerProfiles::Domain";

/// Every `Fn::GetAtt` attribute an `AWS::CustomerProfiles::Domain` publishes, in the sorted
/// order a resource's own attribute map holds them in.
///
/// Source: the type's own "Return values" section — the two instants the owning service
/// recorded for the domain, the status its rule-based matching configuration is in, and the
/// four usage statistics that service keeps over the domain's data, which that section spells
/// `Stats.<count>`. Every one of them is a read-only value of that service's own: none is
/// computable from a template, so each stands in ([`service_assigned_document`]) until
/// [`crate::state::provisioner`] has reported what `GetDomain` really answered.
pub(super) const CUSTOMER_PROFILES_DOMAIN_ATTRIBUTES: &[&str] = &[
    "CreatedAt",
    "LastUpdatedAt",
    "RuleBasedMatching.Status",
    "Stats.MeteringProfileCount",
    "Stats.ObjectCount",
    "Stats.ProfileCount",
    "Stats.TotalSize",
];

/// The `AWS::<Namespace>::<Resource>` type of a QUICK SETUP CONFIGURATION MANAGER: "a
/// collection of desired state configurations for multiple configuration definitions and
/// summaries describing the deployments of those definitions"
/// (`CreateConfigurationManager`), which that service's own create makes.
///
/// The namespace is `SSMQuickSetup`, as the "AWS resource and property types reference"
/// publishes it — that model's own `awsService.sdkId` is `SSM QuickSetup`.
///
/// "Ref returns the ManagerArn" — the type's primary identifier — and `Fn::GetAtt` reads
/// [`SSM_QUICKSETUP_CONFIGURATION_MANAGER_ARN_ATTRIBUTE`] beside the two instants and the
/// status summaries that service keeps for the manager. Quick Setup MINTS that ARN:
/// `CreateConfigurationManager` takes no identifier for the manager — the declaration states
/// its name, its description and the definitions it deploys — and answers the `ManagerArn` it
/// assigned, whose resource path carries a uuid of its own
/// (`^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}$`),
/// so it cannot be computed from the template — that is what [`identity_is_service_assigned`]
/// names this type for, and it is the handle `GetConfigurationManager`,
/// `UpdateConfigurationManager` and `DeleteConfigurationManager` each address a manager by.
pub const SSM_QUICKSETUP_CONFIGURATION_MANAGER: &str = "AWS::SSMQuickSetup::ConfigurationManager";

/// Quick Setup's ARN namespace, which is the namespace a configuration manager's ARN carries
/// (that same pattern), and the resource path such an ARN carries before the uuid that service
/// minted.
pub const SSM_QUICKSETUP_ARN_NAMESPACE: &str = "ssm-quicksetup";
pub(super) const SSM_QUICKSETUP_CONFIGURATION_MANAGER_PATH: &str = "configuration-manager";

/// The one `Fn::GetAtt` attribute an [`SSM_QUICKSETUP_CONFIGURATION_MANAGER`] publishes that
/// these naming rules can compute a stand-in for: the ARN Quick Setup minted, which is also
/// what the type's `Ref` returns.
///
/// The manager's `CreatedAt`, `LastModifiedAt` and `StatusSummaries` are that service's own to
/// report — its clock and its deployment's state — and reach the resource off its report of
/// the created manager
/// ([`crate::state::provisioner::Provisioner::SsmQuickSetupConfigurationManager`]), exactly as
/// an `AWS::Braket::SpendingLimit`'s `CreatedAt` does.
pub const SSM_QUICKSETUP_CONFIGURATION_MANAGER_ARN_ATTRIBUTE: &str = "ManagerArn";

/// The `AWS::<cloudFormationName>::<Resource>` type of a RECYCLE BIN retention rule: the
/// account-wide guardrail that keeps a deleted resource recoverable for a period, which that
/// service's own `CreateRule` makes. The middle segment is that model's own `awsService.sdkId`
/// (`rbin`) with the initial capitalised, as the resource-type namespace publishes it.
///
/// "Ref returns the rule ARN" — the type's primary identifier — and `Fn::GetAtt` reads
/// [`RBIN_RULE_ATTRIBUTES`]. Recycle Bin MINTS the rule's `Identifier`
/// (`^[0-9a-zA-Z]{11}$`, which no name CloudFormation decides satisfies) and the `RuleArn`
/// built around it, and the create takes no identifier at all, which is why
/// [`identity_is_service_assigned`] names this type: the identity in force is the one that
/// service answered.
pub const RBIN_RULE: &str = "AWS::Rbin::Rule";

/// Every `Fn::GetAtt` attribute an [`RBIN_RULE`] publishes, in the sorted order a resource's
/// own attribute map holds them in: the rule's ARN, the identifier Recycle Bin minted and the
/// lock state it keeps.
///
/// Source: the type's own "Return values" section. The ARN and the identifier are that
/// service's to mint, so both are STAND-INS of the forms its model fixes until
/// [`crate::state::provisioner`] has reported what `CreateRule` really answered; the lock state
/// follows from the declaration itself — a rule declared with a lock configuration is created
/// locked and one declared without it has never been locked
/// ([`crate::state::provisioner::rbin_rules::declared_lock_state`]).
pub(super) const RBIN_RULE_ATTRIBUTES: &[&str] = &["Arn", "Identifier", "LockState"];

/// Recycle Bin's ARN namespace, which is the namespace a retention rule's ARN carries
/// (`RuleArn`: `^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}…$`).
pub const RBIN_ARN_NAMESPACE: &str = "rbin";

/// The resource path a rule ARN carries before the identifier Recycle Bin minted for it, from
/// that same pattern.
pub(super) const RBIN_RULE_PATH: &str = "rule";

/// The salt the rule-identifier stand-in is derived under, and how many characters it carries:
/// a `RuleIdentifier` is `^[0-9a-zA-Z]{11}$`, so the stand-in is exactly eleven alphanumeric
/// characters and carries no separator.
pub(super) const RBIN_RULE_ID_SALT: &str = "rbin/rule";
pub(super) const RBIN_RULE_ID_DIGITS: usize = 11;

/// The `AWS::<cloudFormationName>::<Resource>` type of an AMAZON DATA LIFECYCLE MANAGER
/// LIFECYCLE POLICY: the policy that automates the creation, retention and deletion of EBS
/// snapshots and EBS-backed AMIs, which that service's own `CreateLifecyclePolicy` makes. The
/// middle segment is that model's own `awsService.cloudFormationName` (`DLM`).
///
/// "Ref returns the policy id" — what every call of that service on a policy takes as its
/// `PolicyId` — and `Fn::GetAtt` reads [`DLM_LIFECYCLE_POLICY_ATTRIBUTES`]. Amazon Data
/// Lifecycle Manager MINTS that id (`^policy-[a-f0-9]+$`, which no name CloudFormation decides
/// satisfies) and builds the `PolicyArn` around it, and the create takes no identifier at all,
/// which is why [`identity_is_service_assigned`] names this type: the identity in force is the
/// one that service answered.
pub const DLM_LIFECYCLE_POLICY: &str = "AWS::DLM::LifecyclePolicy";

/// The one `Fn::GetAtt` attribute a [`DLM_LIFECYCLE_POLICY`] publishes: the ARN of the policy,
/// from the type's own "Return values" section.
///
/// It is that service's to build — around the id it minted — so it is a STAND-IN of the form its
/// own model fixes (`PolicyArn`:
/// `^arn:aws(-[a-z]{1,3}){0,2}:dlm:[A-Za-z0-9_/.-]{0,63}:\d+:policy/[0-9A-Za-z_-]{1,128}$`) until
/// [`crate::state::provisioner`] has reported the ARN that service really holds the policy under.
pub(super) const DLM_LIFECYCLE_POLICY_ATTRIBUTES: &[&str] = &["Arn"];

/// Amazon Data Lifecycle Manager's ARN namespace, which is the namespace a policy ARN carries
/// (that model's own `awsService.arnNamespace`, and that same `PolicyArn` pattern), and the
/// resource path such an ARN carries before the id that service minted.
pub(super) const DLM_ARN_NAMESPACE: &str = "dlm";
pub(super) const DLM_LIFECYCLE_POLICY_PATH: &str = "policy";

/// The prefix a policy id carries and how many hexadecimal characters follow it: a `PolicyId` is
/// `^policy-[a-f0-9]+$`, so the stand-in opens with that prefix and carries sixteen lower-case
/// hexadecimal digits.
pub(super) const DLM_POLICY_ID_PREFIX: &str = "policy";
pub(super) const DLM_POLICY_ID_DIGITS: usize = 16;

/// The `AWS::<cloudFormationName>::<Resource>` type of a NEPTUNE ANALYTICS graph: the graph
/// database that service's own `CreateGraph` makes. The middle segment is the one the
/// resource-type namespace publishes (`NeptuneGraph`), which is that model's own
/// `awsService.sdkId` (`Neptune Graph`) written as one word.
///
/// "Ref returns the graph id" — the type's primary identifier — and `Fn::GetAtt` reads
/// [`NEPTUNE_GRAPH_GRAPH_ATTRIBUTES`]. Neptune Analytics MINTS that id
/// (`^g-[a-z0-9]{10}$`, which no name CloudFormation decides satisfies — the pattern's own
/// negative lookahead keeps a graph NAME out of that form) and builds the ARN and the endpoint
/// around it, which is why [`identity_is_service_assigned`] names this type: the identity in
/// force is the one that service answered.
pub const NEPTUNE_GRAPH_GRAPH: &str = "AWS::NeptuneGraph::Graph";

/// The `AWS::<cloudFormationName>::<Resource>` type of a SNAPSHOT of such a graph, which that
/// service's own `CreateGraphSnapshot` takes of the graph a declaration names.
///
/// "Ref returns the snapshot id", which that service MINTS (`^gs-[a-z0-9]{10}$`) and which
/// `GetGraphSnapshot`, `DeleteGraphSnapshot` and `RestoreGraphFromSnapshot` each address one
/// by — so [`identity_is_service_assigned`] names this type too — and `Fn::GetAtt` reads
/// [`NEPTUNE_GRAPH_SNAPSHOT_ATTRIBUTES`].
pub const NEPTUNE_GRAPH_GRAPH_SNAPSHOT: &str = "AWS::NeptuneGraph::GraphSnapshot";

/// Every `Fn::GetAtt` attribute a [`NEPTUNE_GRAPH_GRAPH`] publishes, in the sorted order a
/// resource's own attribute map holds them in: the endpoint the graph is reached at, its ARN
/// and the id Neptune Analytics minted.
///
/// Source: the type's own "Return values" section. All three are that service's to produce —
/// the id it mints, the ARN it builds around it and the endpoint it serves the graph on — so
/// each is a STAND-IN of the form its own model fixes until [`crate::state::provisioner`] has
/// reported what `CreateGraph` really answered.
pub(super) const NEPTUNE_GRAPH_GRAPH_ATTRIBUTES: &[&str] =
    &["Endpoint", "GraphArn", "GraphId"];

/// Every `Fn::GetAtt` attribute a [`NEPTUNE_GRAPH_GRAPH_SNAPSHOT`] publishes, in that same
/// sorted order: the ARN Neptune Analytics built for the snapshot and the id it minted.
///
/// Both are that service's to produce — `CreateGraphSnapshot` takes the graph and a name and
/// answers the `id` it assigned with the `arn` built around it — so each is a STAND-IN of the
/// form its own model fixes until [`crate::state::provisioner`] has reported what that create
/// really answered.
pub(super) const NEPTUNE_GRAPH_SNAPSHOT_ATTRIBUTES: &[&str] = &["Arn", "Id"];

/// Neptune Analytics' ARN namespace, which is the namespace a graph's ARN carries (that
/// model's own `awsService.arnNamespace`), and the resource path a graph ARN carries before
/// the id that service minted.
pub(super) const NEPTUNE_GRAPH_ARN_NAMESPACE: &str = "neptune-graph";
pub(super) const NEPTUNE_GRAPH_GRAPH_PATH: &str = "graph";

/// The resource path a SNAPSHOT's ARN carries before the id that service minted, from the same
/// place: it is the path that service's own `arn` member spells for a snapshot.
pub(super) const NEPTUNE_GRAPH_SNAPSHOT_PATH: &str = "graph-snapshot";

/// The prefixes a graph id and a snapshot id carry, and how many characters follow either:
/// `GraphId` is `^g-[a-z0-9]{10}$` and `SnapshotId` is `^gs-[a-z0-9]{10}$`
/// (`smithy_query(query="shape",shape="GraphId,SnapshotId")` on that service's model), so a
/// stand-in is that prefix and exactly ten lower-case characters.
pub(super) const NEPTUNE_GRAPH_ID_PREFIX: &str = "g";
pub(super) const NEPTUNE_SNAPSHOT_ID_PREFIX: &str = "gs";
pub(super) const NEPTUNE_GRAPH_ID_DIGITS: usize = 10;

/// The prefix a Systems Manager patch-baseline id carries and how many characters follow it:
/// the ids that service mints read `pb-` and 17 hexadecimal characters, which is the shortest
/// value its `BaselineId` pattern admits (`^[a-zA-Z0-9_\-:/]{20,128}$`,
/// `smithy_query(query="shape", shape="BaselineId")` on that service's model) — so a stand-in
/// for one is that prefix and exactly seventeen characters.
pub(super) const SSM_PATCH_BASELINE_ID_PREFIX: &str = "pb";
pub(super) const SSM_PATCH_BASELINE_ID_DIGITS: usize = 17;

/// The `AWS::<cloudFormationName>::<Resource>` types of the OpenSearch Serverless
/// resources a stack creates. The middle segment is that service's own
/// `awsService.cloudFormationName` (`OpenSearchServerless`).
pub const AOSS_COLLECTION: &str = "AWS::OpenSearchServerless::Collection";
pub const AOSS_SECURITY_POLICY: &str = "AWS::OpenSearchServerless::SecurityPolicy";
pub const AOSS_VPC_ENDPOINT: &str = "AWS::OpenSearchServerless::VpcEndpoint";

/// The type of an Amazon OpenSearch Service domain — a different service from the
/// serverless one above, whose own model is reached by the `es` signing name. Its middle
/// segment is the one the resource-type namespace publishes (`OpenSearchService`) rather
/// than that model's shorter `awsService.cloudFormationName` (`OpenSearch`).
///
/// "Ref returns the domain name", which is the name the owning service creates the domain
/// under — so a GENERATED one is held to that service's own rules for it
/// ([`naming_rules`]) and the identity is not service-assigned.
pub const OPENSEARCH_DOMAIN: &str = "AWS::OpenSearchService::Domain";

/// The type of an Amazon S3 bucket, whose middle segment is that model's own
/// `cloudFormationName` the same way. "Ref returns the bucket name" for this type, so a
/// bucket's physical resource id is the name its owning service creates it under — which
/// is why [`naming_rules`] holds a GENERATED one to S3's bucket naming rules.
pub const S3_BUCKET: &str = "AWS::S3::Bucket";

/// The type of an Amazon S3 TABLE bucket, the third bucket variant beside `AWS::S3::Bucket`
/// and `AWS::S3Vectors::VectorBucket`. The middle segment is that model's own sdk identity
/// (`smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/\
/// com-amazonaws-s3tables-s3tablebuckets.json")` reports `sdkId: S3Tables`).
///
/// This type's primary identifier is its own ARN (`bucket/<name>` under the ARN namespace
/// below), so `Ref` resolves to the ARN its owning service keys the bucket by — which is
/// what `GetTableBucket`, `PutTableBucketPolicy` and every other S3 Tables operation on a
/// table bucket take as `tableBucketARN`.
pub const S3TABLES_TABLE_BUCKET: &str = "AWS::S3Tables::TableBucket";

/// The type of an Amazon S3 Tables NAMESPACE: the logical grouping of tables a table bucket
/// holds, which `CreateNamespace` creates IN the bucket its declaration names and
/// `DeleteNamespace` removes from it.
///
/// It declares no resource of its own beyond that pairing, so its primary identifier is
/// COMPOSITE ([`S3TABLES_NAMESPACE_IDENTITY`]): the bucket's ARN and the namespace's own
/// name, which is what every S3 Tables operation on one takes together
/// (`GetNamespace`/`DeleteNamespace` bind `tableBucketARN` and `namespace` as the two labels
/// of `/namespaces/{tableBucketARN}/{namespace}`) — and the type publishes no `Fn::GetAtt`
/// value, both of its properties being its identity.
pub const S3TABLES_NAMESPACE: &str = "AWS::S3Tables::Namespace";

/// The property an `AWS::S3Tables::Namespace` states the namespace's own NAME as, which the
/// owning service takes as `CreateNamespace`'s `namespace` member.
pub const S3TABLES_NAMESPACE_PROPERTY: &str = "Namespace";

/// The pair that identifies one namespace, in the order the parts of a composite physical
/// resource id are joined in: the table bucket it lives in and its own name.
pub const S3TABLES_NAMESPACE_IDENTITY: &[&str] = &[
    S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE,
    S3TABLES_NAMESPACE_PROPERTY,
];

/// Amazon S3 Tables' ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`s3tables`) rather than the `S3Tables` its resource types are spelled with.
pub const S3TABLES_ARN_NAMESPACE: &str = "s3tables";

/// The segment a table bucket occupies in an Amazon S3 Tables ARN, from the
/// `TableBucketARN` pattern that model declares
/// (`…:s3tables:<region>:<account>:bucket/<name>`).
///
/// The same literal as the Amazon S3 Vectors one beside it, and a separate constant all the
/// same: each is its own model's statement about its own ARNs, so neither follows from the
/// other and a change to one is not a change to both.
pub const S3TABLES_BUCKET_SEGMENT: &str = "bucket/";

/// The one `Fn::GetAtt` attribute an `AWS::S3Tables::TableBucket` publishes, spelled with
/// the upper-case `ARN` its own model spells the member with (`TableBucketARN`).
pub const S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE: &str = "TableBucketARN";

/// The types of the two Amazon S3 Vectors resources a stack creates. The middle segment is
/// that model's own sdk identity (`smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/\
/// com-amazonaws-s3vectors-s3vectors.json")` reports `sdkId: S3Vectors`), beside the
/// sibling bucket variants `AWS::S3::Bucket` and `AWS::S3Tables::TableBucket`.
///
/// Each type's primary identifier is its own ARN — a vector bucket is
/// `bucket/<name>` and an index `bucket/<bucket>/index/<name>` under the ARN namespace
/// below — so `Ref` resolves to the ARN its owning service keys the resource by, which is
/// what `GetVectorBucket`/`GetIndex` take as `vectorBucketArn`/`indexArn`.
pub const S3VECTORS_VECTOR_BUCKET: &str = "AWS::S3Vectors::VectorBucket";
pub const S3VECTORS_INDEX: &str = "AWS::S3Vectors::Index";

/// Amazon S3 Vectors' ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`s3vectors`) rather than the `S3Vectors` its resource types are spelled with.
pub const S3VECTORS_ARN_NAMESPACE: &str = "s3vectors";

/// The segment a vector bucket occupies in an Amazon S3 Vectors ARN, from the
/// `VectorBucketArn` pattern that model declares
/// (`…:s3vectors:<region>:<account>:bucket/<name>`).
pub const S3VECTORS_BUCKET_SEGMENT: &str = "bucket/";

/// The type of an Amazon EMR cluster: the EMR model carries
/// `awsService.cloudFormationName: EMR`. "Ref returns the cluster ID", the `j-` job-flow id
/// `RunJobFlow` mints, which is why [`identity_is_service_assigned`] names this type.
pub const EMR_CLUSTER: &str = "AWS::EMR::Cluster";

/// The type of an AWS Transfer Family server: that model carries
/// `awsService.cloudFormationName: Transfer`. "Ref returns the Amazon Resource Name (ARN)
/// of the server", and `Fn::GetAtt` reads `Arn` and `ServerId` — "the service-assigned ID
/// of the server that is created". Transfer Family MINTS both (`CreateServer` takes no
/// identifier and answers the `ServerId` it assigned), which is why
/// [`identity_is_service_assigned`] names this type: the ARN a `Ref` resolves to is the one
/// that service created the server under, not one these naming rules computed.
pub const TRANSFER_SERVER: &str = "AWS::Transfer::Server";

/// The types of the two kinds of Amazon MSK cluster a stack creates: the Amazon MSK model
/// carries `awsService.cloudFormationName: MSK`, and the namespace publishes one type per
/// kind of cluster — a provisioned one, whose brokers the declaration sizes, and a
/// serverless one, which has none.
///
/// For both, "Ref returns the ARN of the cluster", which is also each type's one published
/// `Fn::GetAtt` attribute. Amazon MSK MINTS that ARN (its resource path is
/// `cluster/<name>/<id>`, where the id is the service's own), which is why
/// [`identity_is_service_assigned`] names both types.
pub const MSK_CLUSTER: &str = "AWS::MSK::Cluster";
pub const MSK_SERVERLESS_CLUSTER: &str = "AWS::MSK::ServerlessCluster";

/// The type of an Amazon MSK CONFIGURATION: the `server.properties` document a cluster of
/// either kind can be given, declared in that same `MSK` namespace.
///
/// "Ref returns the ARN of the configuration" — the type's primary identifier — and
/// `Fn::GetAtt` reads that ARN beside the configuration's LATEST REVISION, whose members are
/// read under the nested names the type publishes them at
/// (`LatestRevision.CreationTime`, `LatestRevision.Revision`). Amazon MSK MINTS that ARN
/// (its resource path is `configuration/<name>/<id>-<ordinal>`, where the id and the ordinal
/// are the service's own) and mints the revision, so [`identity_is_service_assigned`] names
/// this type too: what a completed stack reports is what `CreateConfiguration` answered.
pub const MSK_CONFIGURATION: &str = "AWS::MSK::Configuration";

/// The `Fn::GetAtt` attribute an [`MSK_CONFIGURATION`] publishes the creation instant of its
/// latest revision at, as the type spells the nested read.
pub const MSK_CONFIGURATION_REVISION_CREATION_TIME: &str = "LatestRevision.CreationTime";

/// The same for that revision's NUMBER, which a created configuration's first revision is
/// `1` of.
pub const MSK_CONFIGURATION_REVISION_NUMBER: &str = "LatestRevision.Revision";

/// The revision number a configuration Amazon MSK has just created carries: its FIRST, which
/// `CreateConfiguration` answers as its `LatestRevision.Revision` and
/// `ListConfigurationRevisions` then reports as the only revision there is.
pub(super) const MSK_CONFIGURATION_FIRST_REVISION: u32 = 1;

/// The same for that revision's DESCRIPTION, which is the declaration's own `Description`
/// where it states one.
///
/// Not among the names [`published_attribute_names`] carries for the type: a revision has a
/// description only where the create was given one (`ConfigurationRevision$Description` is
/// optional), so the value comes from what Amazon MSK reported for the revision it created
/// rather than from a stand-in computed before the create, which for a configuration
/// described by nothing would be a value no read of it answers.
pub const MSK_CONFIGURATION_REVISION_DESCRIPTION: &str = "LatestRevision.Description";

/// Amazon MSK's ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`kafka`) rather than the `MSK` its resource types are spelled with.
pub const MSK_ARN_NAMESPACE: &str = "kafka";

/// The type of an Amazon MQ broker: that middle segment is the Amazon MQ model's own
/// `awsService.cloudFormationName` (`AmazonMQ`).
///
/// "Ref returns the Amazon MQ broker ID", and `Fn::GetAtt` reads `Arn` — the broker's own
/// ARN. Amazon MQ MINTS that id (`CreateBroker` takes the broker's name and answers with
/// the `brokerId` it generated, which is the `@httpLabel` every operation on a broker is
/// addressed by), which is why [`identity_is_service_assigned`] names this type: the id a
/// `Ref` resolves to is the one that service created the broker under, not one these naming
/// rules computed.
pub const MQ_BROKER: &str = "AWS::AmazonMQ::Broker";

/// Amazon MQ's ARN namespace, which is its model's own `awsService.arnNamespace` (`mq`)
/// rather than the `AmazonMQ` its resource type is spelled with.
pub const MQ_ARN_NAMESPACE: &str = "mq";

/// What an Amazon MQ broker id begins with (`b-` followed by 128 bits in the 8-4-4-4-12
/// grouping `BrokerId`'s own documented form carries).
///
/// The id built on it here is a PLACEHOLDER, and one that is never reported for a broker
/// that exists: the id a created broker carries is the one Amazon MQ minted, which
/// [`super::provisioning::reconcile`] adopts over this one. What this value is for is the
/// shape of the identifier a declaration reports BEFORE its create answers.
pub(super) const MQ_BROKER_ID_PREFIX: &str = "b-";

/// What an Amazon EC2 Spot Fleet request id begins with (`sfr-` followed by 128 bits in the
/// 8-4-4-4-12 grouping that service's own ids carry,
/// `sfr-73fbd2ce-aa30-494c-8788-1cee4ffcbc91`).
///
/// The id built on it here is a PLACEHOLDER for the same reason the broker's is: the id a
/// requested fleet carries is the one Amazon EC2 minted, which the deployment adopts over this
/// one ([`identity_is_service_assigned`] names the type).
pub(super) const SPOT_FLEET_REQUEST_ID_PREFIX: &str = "sfr-";

/// The type of an Amazon MQ CONFIGURATION: the engine configuration document a broker of
/// that service can be given, declared in the same `AmazonMQ` namespace the broker is.
///
/// "Ref returns the Amazon MQ configuration ID", and `Fn::GetAtt` reads `Arn` (the
/// configuration's own ARN), `Id` (that same configuration id) and `Revision` (the number of
/// its latest revision). Amazon MQ MINTS the id and the ARN — `CreateConfiguration` takes
/// the configuration's name and answers with the `Id` it generated and the `Arn` it built
/// around it, and it NUMBERS each revision — which is why [`identity_is_service_assigned`]
/// names this type: the id a `Ref` resolves to is the one that service created the
/// configuration under, not one these naming rules computed.
pub const MQ_CONFIGURATION: &str = "AWS::AmazonMQ::Configuration";

/// The `Fn::GetAtt` attribute an [`MQ_CONFIGURATION`] publishes the number of its latest
/// revision at, as the type's return values section spells it.
pub const MQ_CONFIGURATION_REVISION_ATTRIBUTE: &str = "Revision";

/// What an Amazon MQ configuration id begins with (`c-` followed by the same 128 bits in the
/// 8-4-4-4-12 grouping a broker id carries), for the same reason
/// [`MQ_BROKER_ID_PREFIX`] carries the broker's: the id built on it is a PLACEHOLDER no
/// configuration that exists is reported under, since
/// [`super::provisioning::reconcile`] adopts the id Amazon MQ minted over it.
pub(super) const MQ_CONFIGURATION_ID_PREFIX: &str = "c-";

/// The revision number the configuration an `AWS::AmazonMQ::Configuration` declares carries
/// once it is created: its SECOND.
///
/// `CreateConfiguration` takes no document at all — it "creates a configuration from the
/// engine's default settings", which is revision 1 — so the `Data` the type declares reaches
/// the configuration as the revision `UpdateConfiguration` adds, which is revision 2
/// ([`crate::state::provisioner::mq_configurations`] makes both calls). A PLACEHOLDER like
/// every other value here: what the stack reports is the revision number Amazon MQ answered
/// for the document it really stored.
pub(super) const MQ_CONFIGURATION_DECLARED_REVISION: u32 = 2;

/// The 1-based ordinal an Amazon MSK cluster or configuration ARN ends with
/// (`cluster/<name>/<id>-<ordinal>`, `configuration/<name>/<id>-<ordinal>`), as the stand-in
/// identifier computed here carries it.
///
/// A PLACEHOLDER, and one that is never reported for a resource that exists: the ARN a
/// created cluster or configuration carries is the one Amazon MSK minted, where the ordinal
/// is that service's own count of the resources it has registered, and
/// [`super::provisioning::reconcile`] adopts that ARN over this one. What this value is
/// for is the shape of the identifier a declaration reports BEFORE its create answers.
pub(super) const MSK_RESOURCE_ARN_ORDINAL: u32 = 1;

/// The same for an Amazon SQS queue: the SQS model carries
/// `awsService.cloudFormationName: SQS`.
pub const SQS_QUEUE: &str = "AWS::SQS::Queue";

/// The provisioned-cluster type of Amazon Redshift: the Redshift model carries
/// `awsService.cloudFormationName: Redshift`.
///
/// "Ref returns the name of the cluster" — a cluster's `ClusterIdentifier` — so this
/// resource's identity is CloudFormation's to compute, under that property
/// ([`declared_name_property`]) and Amazon Redshift's own rules for it
/// ([`naming_rules`]).
pub const REDSHIFT_CLUSTER: &str = "AWS::Redshift::Cluster";

/// The cluster SUBNET GROUP of Amazon Redshift: the subnets of a VPC a cluster is
/// launched into, which a cluster's `ClusterSubnetGroupName` names.
///
/// "Ref returns the name of the cluster subnet group" and the type declares no name
/// property of its own — its three properties are `Description`, `SubnetIds` and `Tags` —
/// so the name is one CloudFormation GENERATES, held to Amazon Redshift's own rules for
/// it ([`naming_rules`]), and that generated name is the physical resource id a dependent
/// cluster resolves the group by.
pub const REDSHIFT_CLUSTER_SUBNET_GROUP: &str = "AWS::Redshift::ClusterSubnetGroup";

/// The two resource types that carry a QUEUE's access policy: the one that applies a
/// document to a list of queues, and the one that belongs to a single queue.
///
/// Neither is a resource of Amazon SQS's own — a queue's policy is its `Policy` ATTRIBUTE —
/// so neither carries a [`return_values`] arm: the default applies and the physical
/// resource id is the name CloudFormation generated for the resource, which is what "Ref
/// returns the policy ID" for these types means here.
pub const SQS_QUEUE_POLICY: &str = "AWS::SQS::QueuePolicy";
pub const SQS_QUEUE_INLINE_POLICY: &str = "AWS::SQS::QueueInlinePolicy";

/// The `AWS::<cloudFormationName>::<Resource>` type of a SageMaker model package group —
/// the model registry's container for the versioned model packages registered into it, and
/// the carrier of the registry's resource policy. The middle segment is the SageMaker
/// model's own `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")` on that service's model reports `SageMaker`).
///
/// This type deliberately carries NO [`return_values`] arm, so the default applies: its
/// physical resource id is the group's NAME. That is what every SageMaker model-registry
/// operation addresses the group by — `ModelPackageGroupName` is modelled `EntityName`,
/// name-only on the policy operations — so the name is the identifier a client can use,
/// and it is what the provisioner passes to `CreateModelPackageGroup`. The group's ARN is
/// not computed here either: `Arn`/`ModelPackageGroupArn` are the ARN SageMaker itself
/// answered, which the provisioner records on the resource, rather than one minted from a
/// pattern. SageMaker's own model says nothing about this type's `Ref`, so the identifier a
/// client can resolve is preferred over a guess at CloudFormation's.
pub const SAGEMAKER_MODEL_PACKAGE_GROUP: &str = "AWS::SageMaker::ModelPackageGroup";

/// An Amazon CloudWatch metric stream's resource type (the CloudWatch model:
/// `cloudFormationName: CloudWatch`). This is the type `super::provisioning` creates in
/// CloudWatch itself, through that service's own `PutMetricStream`.
pub const CLOUDWATCH_METRIC_STREAM: &str = "AWS::CloudWatch::MetricStream";

/// The `AWS::DynamoDB::Table` property that enables the table's stream, whose presence is
/// what says the table publishes a `StreamArn` at all ("You must specify the
/// StreamSpecification property to use this attribute").
pub(super) const DYNAMODB_STREAM_SPECIFICATION: &str = "StreamSpecification";

/// `AWS::DynamoDB::GlobalTable`: the same table declared for a GROUP of Regions, which
/// `crate::state::provisioning` creates in Amazon DynamoDB itself. Named by the module that
/// provisions it, so the type is spelled once in this crate.
pub(crate) use crate::state::provisioning::dynamodb_table::global_table::DYNAMODB_GLOBAL_TABLE;

/// The salt and length of the stand-in LABEL a declared stream's ARN carries until the
/// table's own create answers the one Amazon DynamoDB minted
/// (`TableDescription$LatestStreamLabel`, "a timestamp, in ISO 8601 format").
pub(super) const DYNAMODB_STREAM_LABEL_SALT: &str = "dynamodb/stream-label";
pub(super) const DYNAMODB_STREAM_LABEL_DIGITS: usize = 16;

/// The flag the API's own `StreamSpecification` carries, which a template may state beside
/// the view type: "indicates whether DynamoDB Streams is enabled (true) or disabled (false)
/// on the table".
pub(super) const DYNAMODB_STREAM_ENABLED: &str = "StreamEnabled";

/// Whether a declared `StreamSpecification` enables a stream — which is what decides whether
/// the table publishes a `StreamArn` at all.
///
/// A declaration that states the property asks for a stream, and one that states the flag
/// keeps what it states: a specification turning the stream OFF leaves the table with no
/// stream, and so with no stream ARN for anything to read. The flag is read as the TEXT it
/// resolved to ([`crate::state::scalar_text`], this service's one reading of a scalar
/// property), so a template that writes it as the string a YAML or an intrinsic can yield is
/// read the same way as one that writes the JSON boolean.
pub(super) fn stream_is_enabled(declared: Option<&Value>) -> bool {
    let Some(declared) = declared.filter(|value| !value.is_null()) else {
        return false;
    };
    declared
        .get(DYNAMODB_STREAM_ENABLED)
        .and_then(crate::state::scalar_text)
        .is_none_or(|enabled| !enabled.eq_ignore_ascii_case("false"))
}

/// An Amazon CloudWatch dashboard's resource type. This is the type `super::provisioning`
/// creates in CloudWatch itself, through that service's own `PutDashboard`.
pub const CLOUDWATCH_DASHBOARD: &str = "AWS::CloudWatch::Dashboard";

/// The same for an Amazon SNS topic: the SNS model carries
/// `awsService.cloudFormationName: SNS`.
pub const SNS_TOPIC: &str = "AWS::SNS::Topic";

/// An Amazon SNS subscription's resource type, under the same namespace: the type a
/// template declares a subscriber of a topic with, beside the topic's own inline
/// `Subscription` list.
///
/// It carries no [`return_values`] arm, so the computed physical id is the generated name
/// — a stand-in that the create replaces with the subscription ARN Amazon SNS mints, which
/// is this type's published return value ("Ref returns the subscription ARN") and the
/// identifier `Unsubscribe`/`SetSubscriptionAttributes` address a subscription by.
pub const SNS_SUBSCRIPTION: &str = "AWS::SNS::Subscription";

/// The two resource types that carry a TOPIC's access policy: the one that applies a
/// document to a list of topics, and the one that belongs to a single topic.
///
/// Neither is a resource of Amazon SNS's own — a topic's policy is its `Policy` ATTRIBUTE —
/// so neither carries a [`return_values`] arm: the default applies and the physical resource
/// id is the name CloudFormation generated for the resource, exactly as it is for the two
/// queue-policy types ([`SQS_QUEUE_POLICY`]).
pub const SNS_TOPIC_POLICY: &str = "AWS::SNS::TopicPolicy";
pub const SNS_TOPIC_INLINE_POLICY: &str = "AWS::SNS::TopicInlinePolicy";

/// An Amazon SES configuration set's resource type (the SES model:
/// `awsService.cloudFormationName: SES`). This is the type `super::provisioning` creates
/// in Amazon SES itself, through that service's own `CreateConfigurationSet`.
///
/// It deliberately carries NO [`return_values`] arm, so the default applies and its
/// physical resource id is the set's NAME — which is that type's published return value
/// ("Ref returns the configuration set name") and the identifier every operation of the
/// owning service addresses a configuration set by (`ConfigurationSetName`, on
/// `DescribeConfigurationSet` and `DeleteConfigurationSet` alike). The type publishes no
/// `Fn::GetAtt` attribute, so it carries no entry in [`published_attribute_names`] either.
pub const SES_CONFIGURATION_SET: &str = "AWS::SES::ConfigurationSet";

/// An Amazon SES email IDENTITY's resource type, under the same namespace. This is the type
/// `super::provisioning` creates in Amazon SES itself, through the API v2 that owns the
/// type's properties (`CreateEmailIdentity`).
///
/// It carries NO [`return_values`] arm, so the default applies and its physical resource id
/// is the identity's own text — the domain or the address the declaration states, which is
/// that type's published return value ("Ref returns the email identity") and the identifier
/// every operation of the owning service addresses an identity by (`EmailIdentity`, the
/// `@httpLabel` of `GetEmailIdentity`, `DeleteEmailIdentity` and each
/// `PutEmailIdentity*Attributes` alike). The property it is read from is the type's own
/// `EmailIdentity`, which neither naming convention finds, so the type is registered in
/// [`declared_name_property`].
pub const SES_EMAIL_IDENTITY: &str = "AWS::SES::EmailIdentity";

/// An Amazon SES receipt rule set's resource type, under the same namespace. This is the
/// type `super::provisioning` creates in Amazon SES itself, through that service's own
/// `CreateReceiptRuleSet`.
///
/// It carries NO [`return_values`] arm either, so its physical resource id is the set's
/// NAME — that type's published return value ("Ref returns the rule set name") and the
/// identifier every operation of the owning service addresses a receipt rule set by
/// (`RuleSetName`, on `DescribeReceiptRuleSet`, `SetActiveReceiptRuleSet` and
/// `DeleteReceiptRuleSet` alike). The name property it is read from is unconventional, so
/// the type is registered in [`declared_name_property`].
pub const SES_RECEIPT_RULE_SET: &str = "AWS::SES::ReceiptRuleSet";

/// One receipt rule OF such a set, under the same namespace: the type a template declares a
/// rule of a rule set with, created through the owning service's own `CreateReceiptRule`.
///
/// Its published return value is the RULE's name ("Ref returns the receipt rule name"),
/// which a declaration states INSIDE the type's `Rule` property rather than through a name
/// property of its own — so no naming rule here computes it and
/// [`identity_is_service_assigned`] names this type: the identifier in force is the one the
/// create answered with.
pub const SES_RECEIPT_RULE: &str = "AWS::SES::ReceiptRule";

/// The `AWS::ECR::PublicRepository` resource type: an Amazon ECR Public repository.
///
/// Spelled as AWS's own CloudFormation resource reference publishes it, under the `ECR`
/// namespace beside the private registry's `AWS::ECR::Repository`, rather than from the
/// ECR Public model's `awsService.cloudFormationName` (`ECRPUBLIC`): the type is what a
/// template spells.
pub const ECR_PUBLIC_REPOSITORY: &str = "AWS::ECR::PublicRepository";

/// An Amazon ECR repository's resource type (ECR's model: `cloudFormationName: ECR`).
pub const ECR_REPOSITORY: &str = "AWS::ECR::Repository";

/// The `AWS::ECR::PullTimeUpdateExclusion` resource type, under that same namespace: ONE
/// entry of the private registry's pull-time-update exclusion list — the IAM principal
/// Amazon ECR records no image pull time for.
///
/// The resource IS the principal it excludes: `RegisterPullTimeUpdateExclusion` takes that
/// `principalArn` and no identifier of its own, and the deregister addresses the record by
/// the same ARN, so the declared `PrincipalArn` is the resource's physical id and what its
/// `Ref` returns. The name property is unconventional — the type's last segment is
/// `PullTimeUpdateExclusion`, so neither `PullTimeUpdateExclusionName` nor `Name` finds it —
/// so the type is registered in [`declared_name_property`], and
/// [`crate::state::provisioner`] is what registers the principal in Amazon ECR itself.
pub const ECR_PULL_TIME_UPDATE_EXCLUSION: &str = "AWS::ECR::PullTimeUpdateExclusion";

/// The SIGNING CONFIGURATION of that private registry, under the same namespace: the one
/// configuration an account's registry holds, which `PutSigningConfiguration` is documented to
/// create or update ("Creates or updates the registry's signing configuration") and
/// `DeleteSigningConfiguration` removes.
///
/// Neither call takes an identifier, and the two that report one answer the `registryId` the
/// configuration belongs to — so the resource's identity is that REGISTRY rather than a name of
/// its own, exactly as the account's grouping configuration
/// ([`APPLICATION_SIGNALS_GROUPING_CONFIGURATION`]) is identified by the account whose it is. A
/// second declaration in the same account therefore names the same resource, which is why a
/// deployment of one probes the registry before putting it.
pub const ECR_SIGNING_CONFIGURATION: &str = "AWS::ECR::SigningConfiguration";

/// Every `Fn::GetAtt` attribute an [`ECR_SIGNING_CONFIGURATION`] publishes, in the sorted order
/// a resource's own attribute map holds them in: the registry whose configuration it is — the
/// `registryId` both `GetSigningConfiguration` and `DeleteSigningConfiguration` answer, which is
/// also what this type's `Ref` returns.
///
/// It is a value these naming rules compute: a registry id is the owning account's id
/// (`RegistryId`: `^[0-9]{12}$`), which is the one account this emulator mints.
pub(super) const ECR_SIGNING_CONFIGURATION_ATTRIBUTES: &[&str] = &[ECR_REGISTRY_ID_ATTRIBUTE];

/// The one attribute of [`ECR_SIGNING_CONFIGURATION_ATTRIBUTES`], which is the identity a `Ref`
/// to one resolves to.
pub(crate) const ECR_REGISTRY_ID_ATTRIBUTE: &str = "RegistryId";

/// An AWS CodeBuild report group's resource type (CodeBuild's model:
/// `awsService.cloudFormationName: CodeBuild`). This is the type `super::provisioning`
/// creates in CodeBuild itself, through that service's own `CreateReportGroup`.
///
/// Its physical resource id is the report group's NAME — that type's published return
/// value ("Ref returns the report group name"), which is also the `Name` property a
/// declaration states it under ([`physical_name`]) and the one value
/// `CreateReportGroup` takes to key the group by. [`return_values`] carries its one
/// published attribute, `Arn`: the ARN CodeBuild forms for a report group is that
/// service's own `report-group/<name>` path under the single account id this emulator
/// mints, so the value computed here and the one the create answers with are the same
/// ARN — the position [`SNS_TOPIC`] is in.
pub const CODEBUILD_REPORT_GROUP: &str = "AWS::CodeBuild::ReportGroup";

/// An AWS CodeBuild build project's resource type, under that same namespace. This is the
/// type `super::provisioning` creates in CodeBuild itself, through that service's own
/// `CreateProject`.
///
/// Its physical resource id is the project's NAME — that type's published return value
/// ("Ref returns the project name"), which is also the `Name` property a declaration states
/// it under ([`physical_name`]) and the one value `CreateProject` takes to key the project
/// by, which is what every CodeBuild operation on a project (`StartBuild`,
/// `BatchGetProjects`, `DeleteProject`) addresses it with. [`return_values`] carries its one
/// published attribute, `Arn`: the ARN CodeBuild forms for a project is that service's own
/// `project/<name>` path under the single account id this emulator mints, so the value
/// computed here and the one the create answers with are the same ARN — the position
/// [`CODEBUILD_REPORT_GROUP`] is in.
pub const CODEBUILD_PROJECT: &str = "AWS::CodeBuild::Project";

/// The ARN namespace of the CodeBuild service, from its model's `aws.api#service`
/// `arnNamespace` — the service segment of every ARN it forms.
pub const CODEBUILD_ARN_NAMESPACE: &str = "codebuild";

/// An AWS CodeCommit repository's resource type (that model:
/// `awsService.cloudFormationName: CodeCommit`). This is the type `super::provisioning`
/// creates in CodeCommit itself, through that service's own `CreateRepository`.
///
/// It deliberately carries NO [`return_values`] arm, so the default applies and its
/// physical resource id is the repository's NAME — which is that type's published return
/// value ("Ref returns the repository name") and the identifier every CodeCommit operation
/// addresses a repository by (`repositoryName`, on `GetRepository`,
/// `PutRepositoryTriggers` and `DeleteRepository` alike). Its published attributes are not
/// computed here either: the ARN and the two clone URLs are the ones CodeCommit itself
/// answered the create with — the clone URLs name the endpoint this emulator serves the
/// repository's git transport on, which no pattern here could mint — and the provisioner
/// records them on the resource.
pub const CODECOMMIT_REPOSITORY: &str = "AWS::CodeCommit::Repository";

/// An AWS CodeArtifact domain's resource type (that model:
/// `awsService.cloudFormationName: CodeArtifact`). This is the type `super::provisioning`
/// creates in CodeArtifact itself, through that service's own `CreateDomain`.
///
/// It carries a [`return_values`] arm because its `Ref` is NOT its name: "Ref returns the
/// Amazon Resource Name (ARN) of the domain", and the name is one of the three values
/// `Fn::GetAtt` reads off it (`Arn`, `Name`, `Owner` — the type's read-only properties).
/// That is what a repository's own declaration names the domain holding it by, since a
/// `Ref` to the domain would hand `CreateRepository` an ARN rather than the domain name
/// that call takes.
pub const CODEARTIFACT_DOMAIN: &str = "AWS::CodeArtifact::Domain";

/// An AWS CodeArtifact repository's resource type, under the same namespace, created
/// through that service's own `CreateRepository` in the domain its declaration names.
///
/// Its `Ref` is its ARN too ("Ref returns the Amazon Resource Name (ARN) of the
/// repository"), and its published attributes are `Arn`, `DomainOwner` and `Name`. The
/// `DomainName` a declaration states is a create-only PROPERTY rather than one of them.
pub const CODEARTIFACT_REPOSITORY: &str = "AWS::CodeArtifact::Repository";

/// An AWS CodePipeline pipeline's resource type (that model:
/// `awsService.cloudFormationName: CodePipeline`). This is the type `super::provisioning`
/// creates in CodePipeline itself, through that service's own `CreatePipeline`.
///
/// Its physical resource id is the pipeline's NAME — "Ref returns the pipeline name", which
/// is also the `Name` property a declaration states it under ([`physical_name`]) and the one
/// value every CodePipeline operation on a pipeline takes. It carries a [`return_values`]
/// arm for the values `Fn::GetAtt` reads off it, neither of which follows from the resource's
/// own identity the way the uniform convention's do: the pipeline's `Arn`, whose resource
/// path is the bare pipeline name (`PipelineArn`'s pattern,
/// `^arn:aws(-[\w]+)*:codepipeline:.+:[0-9]{12}:.+$`, in CodePipeline's own model) rather
/// than the `pipeline/<name>` path the convention would mint, and its `Version`.
pub const CODEPIPELINE_PIPELINE: &str = "AWS::CodePipeline::Pipeline";

/// CodePipeline's ARN namespace, which its own model carries as `awsService.arnNamespace`
/// (`codepipeline`) — the service segment of every ARN it forms, and the one that service's
/// own state module builds a pipeline's ARN under.
pub const CODEPIPELINE_ARN_NAMESPACE: &str = "codepipeline";

/// The version a pipeline carries the moment CodePipeline has created it: "A new pipeline
/// always has a version number of 1. This number is incremented when a pipeline is updated"
/// (`PipelineDeclaration$version`, CodePipeline's own model).
///
/// It is the stand-in [`return_values`] gives the pipeline's `Version` attribute, which the
/// create then replaces with the version that service really answered
/// (`super::super::provisioning` re-points an attribute exactly as it re-points a minted
/// identifier), so a pipeline an update has given a further version reports that version
/// rather than this one.
pub const CODEPIPELINE_FIRST_PIPELINE_VERSION: &str = "1";

/// The type of an AWS CodePipeline CUSTOM ACTION TYPE: the registration of an action a job
/// worker of the account runs, which `CreateCustomActionType` makes and
/// `DeleteCustomActionType` removes.
///
/// Its primary identifier is COMPOSITE ([`CODEPIPELINE_ACTION_TYPE_IDENTITY`]): the
/// registration is no named resource of that service's own, and every call on one addresses
/// it by the triple its declaration states — which is also how `ListActionTypes` keys the
/// action types an account holds (`ActionTypeId`'s `category`, `provider` and `version`). So
/// `Ref` resolves to that triple rather than to a name, and the type publishes no
/// `Fn::GetAtt` value, all three parts of its identity being properties of its own.
pub const CODEPIPELINE_CUSTOM_ACTION_TYPE: &str = "AWS::CodePipeline::CustomActionType";

/// The three properties that identify one custom action type, in the order the parts of a
/// composite physical resource id are joined in ([`composite_identity`]) — which is the order
/// the registration's own identifier is written in (`Build|<provider>|1`).
pub const CODEPIPELINE_ACTION_TYPE_IDENTITY: &[&str] = &["Category", "Provider", "Version"];

/// The type of an AWS CodePipeline WEBHOOK: the trigger URL a third party POSTs to in order
/// to start one pipeline, which `PutWebhook` creates and `DeleteWebhook` removes.
///
/// Its physical resource id is the webhook's NAME — the `Name` property a declaration states
/// it under and the one value `DeleteWebhook` takes (`DeleteWebhook$name`). It carries a
/// [`return_values`] arm for the one published value that is not read off that identity: the
/// `Url` CodePipeline generates for the webhook (`ListWebhookItem$url`, "A unique URL
/// generated by CodePipeline"), which carries a token that service minted and so stands in
/// ([`service_assigned_document`]) until `PutWebhook` has reported it.
pub const CODEPIPELINE_WEBHOOK: &str = "AWS::CodePipeline::Webhook";

/// The resource types a template declares a CloudFormation HOOK with: the Guard Hook, which
/// evaluates CloudFormation Guard rules against a stack operation's targets, and the Lambda
/// Hook, which hands those targets to a function. Both are among the types this service
/// admits ([`crate::state::resource_types`]).
///
/// Each carries a [`return_values`] arm because its `Ref` is NOT its name: "Ref returns the
/// Hook ARN", and `HookArn` — the ARN of the activated Hook — is what `Fn::GetAtt` reads off
/// it. Those are the two values a declaration publishes about a Hook, and a stack that
/// outputs either hands its caller the identifier the extension APIs address that Hook by.
pub const CLOUDFORMATION_GUARD_HOOK: &str = "AWS::CloudFormation::GuardHook";
/// See [`CLOUDFORMATION_GUARD_HOOK`].
pub const CLOUDFORMATION_LAMBDA_HOOK: &str = "AWS::CloudFormation::LambdaHook";

/// CodeArtifact's ARN namespace, which its own model carries as `awsService.arnNamespace`
/// (`codeartifact`) — the service segment of a domain's and a repository's ARN, and the
/// one that service's own state module builds both ARNs under.
pub(super) const CODEARTIFACT_ARN_NAMESPACE: &str = "codeartifact";

/// The type of an Amazon Cognito identity pool (the cognito-identity model's
/// `awsService.cloudFormationName: Cognito`). This is the type `super::super::provisioning`
/// creates in Amazon Cognito Identity itself.
///
/// It deliberately carries NO [`return_values`] arm: "Ref returns the Amazon Cognito
/// identity pool ID" and that id is the owning service's to mint
/// ([`identity_is_service_assigned`] names the type), so the identifier the naming rules
/// compute is only the stand-in a declaration is resolved against until `CreateIdentityPool`
/// answers — and the `Name` the type publishes through `Fn::GetAtt` is what that create
/// reports for the pool.
pub const COGNITO_IDENTITY_POOL: &str = "AWS::Cognito::IdentityPool";

/// The type of the role attachment that binds an identity pool's IAM roles and role
/// mappings to it — the `SetIdentityPoolRoles` call in template form.
///
/// It carries no [`return_values`] arm either, and its identity is NOT service-assigned: the
/// attachment is not a resource of its own in the owning API (a pool has at most one role
/// configuration, which each call replaces), so CloudFormation's own generated identifier is
/// the physical id and the pool it configures stays a declared PROPERTY.
pub const COGNITO_IDENTITY_POOL_ROLE_ATTACHMENT: &str =
    "AWS::Cognito::IdentityPoolRoleAttachment";

/// The type of the principal-tag attribute map of ONE identity provider of ONE identity
/// pool — the `SetPrincipalTagAttributeMap` call in template form.
///
/// It declares no entity of its own: the mapping belongs to the
/// `(IdentityPoolId, IdentityProviderName)` pair that call is addressed by, and that pair is
/// the type's primary identifier — so its physical resource id, which is what `Ref` returns,
/// is the two values joined ([`composite_identity_properties`]) rather than a name
/// CloudFormation generated, which carries neither half and no call of the owning service
/// takes.
pub const COGNITO_IDENTITY_POOL_PRINCIPAL_TAG: &str = "AWS::Cognito::IdentityPoolPrincipalTag";

/// The type of an Amazon Cognito user-pool group: the cognito-idp model carries
/// `awsService.cloudFormationName: Cognito`.
///
/// This type deliberately carries NO [`return_values`] arm, so the default applies and its
/// physical resource id is the group's NAME — the identifier every operation of the owning
/// service addresses a group by (`GetGroup`, `UpdateGroup` and `DeleteGroup` each take a
/// `GroupName` beside the `UserPoolId` of the pool that holds it), and the one a template
/// declares through [`declared_name_property`]. Its `UserPoolId` is the other half of that
/// identity and stays a declared PROPERTY, which is what the provisioner's calls carry.
pub const COGNITO_USER_POOL_GROUP: &str = "AWS::Cognito::UserPoolGroup";

/// The three attributes `AWS::Cognito::UserPool` publishes, read off the IDENTITY in force:
/// the pool's ARN, and the issuer a pool's tokens name it by, as a name
/// (`cognito-idp.<region>.amazonaws.com/<id>`) and as the URL of that name.
///
/// The ONE place that formula is written, because the identity it is read off changes: the
/// name CloudFormation computed stands in until `CreateUserPool` has answered, and the id
/// Amazon Cognito minted is the identity from then on — so the provisioner recomputes the
/// three from that id ([`crate::state::provisioning::cognito_user_pool`]) rather than keeping
/// values built on a name that identifies no pool.
pub fn user_pool_attributes(region: &str, identity: &str) -> Vec<(String, String)> {
    let host = format!("{COGNITO_IDP_ARN_NAMESPACE}.{region}.{URL_SUFFIX}");
    vec![
        (
            "Arn".to_string(),
            ::emulator_server::emulator_arn(
                COGNITO_IDP_ARN_NAMESPACE,
                region,
                &format!("userpool/{identity}"),
            ),
        ),
        ("ProviderName".to_string(), format!("{host}/{identity}")),
        (
            "ProviderURL".to_string(),
            format!("https://{host}/{identity}"),
        ),
    ]
}

/// The type of an Amazon Cognito user pool (the cognito-idp model's
/// `awsService.cloudFormationName: Cognito`), whose published return values
/// [`return_values`] carries: a pool is how a federated identity pool, an app client and an
/// OpenID relying party all name the same user pool, and each of them names it through one
/// of those values.
pub const COGNITO_USER_POOL: &str = "AWS::Cognito::UserPool";

/// The type of an APP CLIENT of a user pool, whose published return values
/// [`return_values`] carries: the client id its `Ref` returns is what every sign-in and
/// every `DescribeUserPoolClient` names the client by, and the secret its `ClientSecret`
/// publishes is what a confidential client signs its token requests with.
pub const COGNITO_USER_POOL_CLIENT: &str = "AWS::Cognito::UserPoolClient";

/// The type of a DOMAIN of a user pool: the host the pool's managed login pages, its OAuth2
/// endpoints and its OpenID Connect discovery document are served on. Its identity is the
/// `Domain` its declaration states — the string `DescribeUserPoolDomain` resolves a domain by,
/// and what "Ref returns" for the type — which is what [`declared_name_property`] names the
/// property it is stated in. The one attribute it publishes,
/// `CloudFrontDistribution`, is read off the domain itself by the provisioner
/// ([`crate::state::provisioning::cognito_user_pool`]), since a prefix domain's create answers
/// none.
pub const COGNITO_USER_POOL_DOMAIN: &str = "AWS::Cognito::UserPoolDomain";

/// The type of a third-party IDENTITY PROVIDER of a user pool. Its identity is the
/// `ProviderName` its declaration states — the name that service addresses a provider by,
/// beside the pool that holds it — which is what [`declared_name_property`] names the
/// property it is stated in.
pub const COGNITO_USER_POOL_IDENTITY_PROVIDER: &str = "AWS::Cognito::UserPoolIdentityProvider";

/// The type of a RESOURCE SERVER of a user pool: the API whose custom scopes an app client of
/// the pool may be permitted. Its identity is the `Identifier` its declaration states — the
/// value every call on one addresses it by beside its pool, and the one a scope is spelled
/// `<identifier>/<scope>` with — which is what [`declared_name_property`] names the property
/// it is stated in.
pub const COGNITO_USER_POOL_RESOURCE_SERVER: &str = "AWS::Cognito::UserPoolResourceServer";

/// Amazon Cognito user pools' ARN namespace and endpoint prefix, which its own model
/// carries as `awsService.arnNamespace` and `endpointPrefix` (`cognito-idp`) — the service
/// segment of a pool's ARN and the host its pool endpoints are served under.
pub(super) const COGNITO_IDP_ARN_NAMESPACE: &str = "cognito-idp";

/// An Amazon ECS cluster's resource type (ECS's model: `cloudFormationName: ECS`). This
/// is the type `super::provisioning` creates in Amazon ECS itself.
pub const ECS_CLUSTER: &str = "AWS::ECS::Cluster";

/// The four AWS Elastic Beanstalk resource types `super::provisioning` creates in
/// Elastic Beanstalk itself (that model: `cloudFormationName: ElasticBeanstalk`).
///
/// Each deliberately carries NO [`return_values`] arm, so the default applies and the
/// physical resource id is the resource's NAME — which is what each type's published
/// return value is ("Ref returns the Elastic Beanstalk application name", "Ref returns
/// the Elastic Beanstalk environment name", "Ref returns the auto-generated version
/// label", "Ref returns the name of the configuration template") and what every Elastic
/// Beanstalk operation addresses the resource by. The attribute each of the two types that
/// publishes one carries — the environment's `EndpointURL` and the configuration
/// template's `TemplateName` — is not computed here either: it is the value Elastic
/// Beanstalk itself answered the create with, which the provisioner records on the
/// resource.
pub const ELASTIC_BEANSTALK_APPLICATION: &str = "AWS::ElasticBeanstalk::Application";
/// See [`ELASTIC_BEANSTALK_APPLICATION`].
pub const ELASTIC_BEANSTALK_APPLICATION_VERSION: &str =
    "AWS::ElasticBeanstalk::ApplicationVersion";
/// See [`ELASTIC_BEANSTALK_APPLICATION`].
pub const ELASTIC_BEANSTALK_ENVIRONMENT: &str = "AWS::ElasticBeanstalk::Environment";
/// See [`ELASTIC_BEANSTALK_APPLICATION`]. The SAVED CONFIGURATION a stack creates on an
/// application, which `DescribeApplications` then reports among that application's
/// `ConfigurationTemplates` and `DescribeConfigurationSettings` reports the settings of.
pub const ELASTIC_BEANSTALK_CONFIGURATION_TEMPLATE: &str =
    "AWS::ElasticBeanstalk::ConfigurationTemplate";

/// An Amazon ECS task definition's resource type.
pub const ECS_TASK_DEFINITION: &str = "AWS::ECS::TaskDefinition";

/// An AWS Batch compute environment's resource type (that model:
/// `cloudFormationName: Batch`). "Ref returns the compute environment ARN"; `Fn::GetAtt`:
/// `ComputeEnvironmentArn`.
pub const BATCH_COMPUTE_ENVIRONMENT: &str = "AWS::Batch::ComputeEnvironment";
/// An AWS Batch job queue's resource type. "Ref returns the job queue ARN";
/// `Fn::GetAtt`: `JobQueueArn`.
pub const BATCH_JOB_QUEUE: &str = "AWS::Batch::JobQueue";
/// An AWS Batch JOB DEFINITION's resource type. "Ref returns the job definition ARN", which
/// is revision-qualified (`…:job-definition/<name>:<revision>`); `Fn::GetAtt`:
/// `JobDefinitionArn`.
///
/// The revision is AWS Batch's to allocate, so the ARN this emulator computes for a
/// declaration is only the stand-in it is resolved against until that service has registered
/// the revision ([`identity_is_service_assigned`] names the type, and
/// [`crate::state::provisioner::batch_job_definitions`] is what registers it).
pub const BATCH_JOB_DEFINITION: &str = "AWS::Batch::JobDefinition";
/// An AWS Batch service environment's resource type. "Ref returns the service environment
/// ARN"; `Fn::GetAtt`: `ServiceEnvironmentArn`.
pub const BATCH_SERVICE_ENVIRONMENT: &str = "AWS::Batch::ServiceEnvironment";
/// An AWS Batch SCHEDULING POLICY's resource type. "Ref returns the scheduling policy ARN";
/// `Fn::GetAtt`: `Arn`.
///
/// That ARN is the identifier `CreateJobQueue$schedulingPolicyArn` takes ("the format is
/// aws:Partition:batch:Region:Account:scheduling-policy/Name"), so it is the one value a
/// queue of the same stack can be created against
/// ([`crate::state::provisioner::batch_share_policies`] creates the policy).
pub const BATCH_SCHEDULING_POLICY: &str = "AWS::Batch::SchedulingPolicy";
/// An AWS Batch QUOTA SHARE's resource type. "Ref returns the quota share ARN";
/// `Fn::GetAtt`: `QuotaShareArn`.
///
/// A share is created ON a job queue and AWS Batch composes its ARN under that queue's own
/// (`…:job-queue/<queue>/quota-share/<share>`), which is the only identifier
/// `DescribeQuotaShare`, `UpdateQuotaShare` and `DeleteQuotaShare` take — so the ARN is
/// built around the queue the declaration points at, and the identity in force is the one
/// that service answered ([`crate::state::provisioner::batch_share_policies`] creates the
/// share).
pub const BATCH_QUOTA_SHARE: &str = "AWS::Batch::QuotaShare";
/// AWS Batch's ARN namespace, from its model's `awsService.arnNamespace`.
pub(super) const BATCH_ARN_NAMESPACE: &str = "batch";

/// Amazon EKS's ARN namespace, from its model's `awsService.arnNamespace`, which the ARNs
/// of a cluster and of a pod identity association are built under.
pub(super) const EKS_ARN_NAMESPACE: &str = "eks";

/// The three CodeDeploy resource types `super::provisioning` creates in CodeDeploy itself
/// (that model: `cloudFormationName: CodeDeploy`).
///
/// Each deliberately carries NO [`return_values`] arm, so the default applies and the
/// physical resource id is the resource's NAME — which is what each type's published return
/// value is ("Ref returns the application name", "Ref returns the deployment group name",
/// "Ref returns the deployment configuration name") and the identifier every CodeDeploy
/// operation addresses the resource by (`GetApplication` takes an `applicationName`,
/// `GetDeploymentGroup` that name and a `deploymentGroupName`, `GetDeploymentConfig` a
/// `deploymentConfigName`). A deployment group's `ApplicationName` is the other half of its
/// identity and stays a declared PROPERTY, which is what the provisioner's calls carry.
pub const CODEDEPLOY_APPLICATION: &str = "AWS::CodeDeploy::Application";
/// See [`CODEDEPLOY_APPLICATION`].
pub const CODEDEPLOY_DEPLOYMENT_GROUP: &str = "AWS::CodeDeploy::DeploymentGroup";
/// See [`CODEDEPLOY_APPLICATION`].
pub const CODEDEPLOY_DEPLOYMENT_CONFIG: &str = "AWS::CodeDeploy::DeploymentConfig";

/// An Elastic Load Balancing v2 load balancer's resource type (that model's
/// `cloudFormationName: ElasticLoadBalancingV2`).
pub const ELB_LOAD_BALANCER: &str = "AWS::ElasticLoadBalancingV2::LoadBalancer";

/// An Elastic Load Balancing v2 target group's resource type.
pub const ELB_TARGET_GROUP: &str = "AWS::ElasticLoadBalancingV2::TargetGroup";

/// An AWS Fault Injection Service experiment template's resource type (the FIS model:
/// `cloudFormationName: FIS`). This is the type `super::provisioning` creates in FIS
/// itself, so the id a stack's output reports for it is the id FIS minted.
pub const FIS_EXPERIMENT_TEMPLATE: &str = "AWS::FIS::ExperimentTemplate";

/// An AWS WAF Classic regional IPSet's resource type (the AWS WAF Regional model:
/// `cloudFormationName: WAFRegional`). This is a type `super::provisioning` creates in that
/// service itself, and "Ref returns the IPSet ID" — an id only AWS WAF Classic mints — so
/// the id a stack's output reports for it is the one that service answered with.
pub const WAF_REGIONAL_IP_SET: &str = "AWS::WAFRegional::IPSet";

/// The four WAF (WAFv2) resource types a stack creates in that service (the WAFv2 model:
/// `cloudFormationName: WAFv2`).
///
/// Each one's identity is WAF's OWN to mint: its create takes the NAME and the scope the
/// template declares and answers a `Summary` carrying the `Id` it assigned and the `ARN` it
/// built from the scope, that name and that id — "a unique identifier for the … This ID is
/// returned in the responses to create and list commands. You provide it to operations like
/// update and delete" (`WebACLSummary$Id` and its siblings). So the identifiers a stack
/// publishes for one are the ones that service answered with, which is what
/// `GetWebACL`/`GetIPSet`/`GetRuleGroup`/`GetRegexPatternSet` resolve and what
/// `AssociateWebACL` takes, rather than values these naming rules computed.
pub const WAFV2_WEB_ACL: &str = "AWS::WAFv2::WebACL";
pub const WAFV2_RULE_GROUP: &str = "AWS::WAFv2::RuleGroup";
pub const WAFV2_IP_SET: &str = "AWS::WAFv2::IPSet";
pub const WAFV2_REGEX_PATTERN_SET: &str = "AWS::WAFv2::RegexPatternSet";

/// The properties whose values identify ONE of those four entities, in the order each type's
/// own schema declares its primary identifier in: the NAME the entity was created under, the
/// `Id` WAF assigned and the `Scope` of the namespace it lives in — exactly the triple
/// `GetWebACL`, `DeleteWebACL` and every sibling call take to address one entity
/// ([`composite_identity_properties`]).
///
/// So the physical resource id of one is those three values joined
/// ([`wafv2_entity_identity`]): a client reads the entity out of the stack and splits the id to
/// recover the `Id` and the `Scope` it has to pass to that service.
///
/// Not a row [`composite_identity`] can compute: the middle part is WAF's to mint, which no
/// declaration carries, so the identity is built from what the create answered instead
/// ([`crate::state::provisioner::create_wafv2_entity`]).
pub(super) const WAFV2_ENTITY_IDENTITY: &[&str] = &["Name", "Id", "Scope"];

/// The physical resource id of one of those four entities: the triple above joined with
/// [`COMPOSITE_IDENTITY_SEPARATOR`], which is what the type's `Ref` returns and what the stack
/// reports the resource under.
pub(crate) fn wafv2_entity_identity(name: &str, id: &str, scope: &str) -> String {
    composite_identity_triple(name, id, scope)
}

/// The three parts of such an identity, where `identity` is one
/// ([`wafv2_entity_identity`]); `None` for a value that is not.
pub(crate) fn wafv2_entity_identity_parts(identity: &str) -> Option<(&str, &str, &str)> {
    composite_identity_triple_parts(identity)
}

/// The type that puts one of those web ACLs IN FRONT OF a protected resource (the same
/// model, `cloudFormationName: WAFv2`).
///
/// It is no entity of AWS WAF's own: it is the BINDING `AssociateWebACL` makes and
/// `DisassociateWebACL` removes, and what identifies the binding is the pair it binds — so
/// its physical resource id is that pair rather than a name no call of that service takes
/// ([`composite_identity_properties`]).
pub const WAFV2_WEB_ACL_ASSOCIATION: &str = "AWS::WAFv2::WebACLAssociation";

/// The type of an Amazon EKS access entry: the access one IAM principal has to one cluster
/// (the Amazon EKS model's `awsService.cloudFormationName: EKS`).
///
/// Another binding identified by the pair it binds: every operation on an access entry
/// addresses it by the cluster and the principal, which are that operation's two
/// `@httpLabel`s, so this type's physical resource id is that pair too
/// ([`composite_identity_properties`]).
pub const EKS_ACCESS_ENTRY: &str = "AWS::EKS::AccessEntry";

/// The type of an Amazon EKS cluster: the Kubernetes control plane that service runs (the
/// Amazon EKS model's `awsService.cloudFormationName: EKS`).
///
/// "Ref returns the name of the cluster", which is also the handle every Amazon EKS
/// operation on one takes (`DescribeCluster$name`) and the `Name` property a declaration
/// states, so the physical resource id the naming rules compute for the type is already the
/// identifier in force.
pub const EKS_CLUSTER: &str = "AWS::EKS::Cluster";

/// The type of an Amazon EKS pod identity association: the IAM role one Kubernetes service
/// account of one cluster assumes (same model).
///
/// `CreatePodIdentityAssociation` takes no identifier — a declaration states the cluster,
/// the namespace, the service account and the role — and answers the association's own ARN
/// and id, which is why this type's identity is the owning service's
/// ([`identity_is_service_assigned`]).
pub const EKS_POD_IDENTITY_ASSOCIATION: &str = "AWS::EKS::PodIdentityAssociation";

/// The type of an Amazon EKS ADD-ON: one piece of operational software installed on a
/// cluster, which `CreateAddon` installs under the add-on's own published name and
/// `DeleteAddon` removes (same model).
///
/// An add-on lives INSIDE a cluster: every operation of that service on one addresses it by
/// the cluster and the add-on name, which are that operation's two `@httpLabel`s. The
/// add-on's name is the `AddonName` the declaration states, which is also the physical
/// resource id the naming rules compute for the type, so what the stack publishes for the
/// resource is the name Amazon EKS knows it by.
pub const EKS_ADDON: &str = "AWS::EKS::Addon";

/// The type of an Amazon EKS managed NODE GROUP: the Auto Scaling group of Amazon EC2
/// instances that joins a cluster as its nodes, which `CreateNodegroup` creates and
/// `DeleteNodegroup` removes (same model).
///
/// Another resource inside a cluster, addressed by the cluster and the node group's own
/// name — that operation's two `@httpLabel`s — with the name the `NodegroupName` the
/// declaration states or the one CloudFormation computed for it.
pub const EKS_NODEGROUP: &str = "AWS::EKS::Nodegroup";

/// The `AWS::<cloudFormationName>::<Resource>` types of the AWS AppConfig resources a
/// configuration estate declares. The middle segment is that model's own
/// `awsService.cloudFormationName` (`AppConfig`).
///
/// Each one's identity is AppConfig's OWN to mint: every create takes the resource's NAME and
/// no identifier, and answers the `Id` that service assigned — modelled `Id` with pattern
/// `^[a-z0-9]{4,7}$`, which no name CloudFormation decides could satisfy. "Ref returns the
/// application ID" and "Ref returns the deployment strategy ID", and the ids a child's
/// `Fn::GetAtt EnvironmentId`/`ConfigurationProfileId` reports are the ones its create
/// answered — which is also the `ApplicationId`, `EnvironmentId`, `ConfigurationProfileId`
/// and `DeploymentStrategyId` every later AppConfig call takes
/// (`CreateHostedConfigurationVersion`, `StartDeployment`). That is what
/// [`identity_is_service_assigned`] names all four for, and
/// [`crate::state::provisioning::appconfig_resources`] is what creates them.
pub const APPCONFIG_APPLICATION: &str = "AWS::AppConfig::Application";
pub const APPCONFIG_ENVIRONMENT: &str = "AWS::AppConfig::Environment";
pub const APPCONFIG_CONFIGURATION_PROFILE: &str = "AWS::AppConfig::ConfigurationProfile";
pub const APPCONFIG_DEPLOYMENT_STRATEGY: &str = "AWS::AppConfig::DeploymentStrategy";

/// The `AWS::<cloudFormationName>::<Resource>` types of the two AWS AppConfig resources a
/// ROLLOUT is declared with: the configuration content hosted for a profile, and the deployment
/// of one version of it into an environment
/// ([`crate::state::provisioning::appconfig_rollout`] is what creates them).
///
/// Neither one's identity is a name at all: `CreateHostedConfigurationVersion` takes the content
/// and answers the `VersionNumber` AppConfig assigned, and `StartDeployment` takes no identifier
/// and answers the `DeploymentNumber` it allocated. Each type's primary identifier is therefore
/// the TRIPLE its own API addresses one resource by — the application and the profile or the
/// environment, and that number — which is what a `Ref` to one resolves to
/// ([`appconfig_rollout_identity`]) and what [`identity_is_service_assigned`] names both types
/// for; `Fn::GetAtt` reads the number itself, and for a deployment the `State` AppConfig reports
/// the rollout in.
pub const APPCONFIG_HOSTED_CONFIGURATION_VERSION: &str =
    "AWS::AppConfig::HostedConfigurationVersion";
pub const APPCONFIG_DEPLOYMENT: &str = "AWS::AppConfig::Deployment";

/// The attribute `AWS::AppConfig::HostedConfigurationVersion` publishes: the version number
/// AppConfig minted for the content, which is the `ConfigurationVersion` a `StartDeployment`
/// deploys.
pub(super) const APPCONFIG_VERSION_NUMBER_ATTRIBUTE: &str = "VersionNumber";

/// The attributes `AWS::AppConfig::Deployment` publishes, in the sorted order a resource's own
/// attribute map holds them in: the number AppConfig allocated for the deployment, and the
/// `DeploymentState` it reports the rollout in.
pub(super) const APPCONFIG_DEPLOYMENT_ATTRIBUTES: &[&str] = &["DeploymentNumber", "State"];

/// The physical resource id of one of those two resources: the triple its own API addresses one
/// by — the application, the profile or the environment, and the number AppConfig minted —
/// joined with [`COMPOSITE_IDENTITY_SEPARATOR`], which is what the type's `Ref` returns.
///
/// Not a row of [`composite_identity`]: that one builds an identity out of the values a
/// declaration STATES, and the last part of this one is a number AppConfig assigns, which no
/// declaration carries. So the identity is built by
/// [`crate::state::provisioning::appconfig_rollout`] from what the create answered — the one
/// place all three parts are really known — and the naming rules leave the resource the
/// identifier they computed until then ([`identity_is_service_assigned`] names both types, so
/// that identifier is only the stand-in a declaration is resolved against).
pub(crate) fn appconfig_rollout_identity(first: &str, second: &str, number: &str) -> String {
    composite_identity_triple(first, second, number)
}

/// The three parts of such an identity, where `identity` is one
/// ([`appconfig_rollout_identity`]); `None` for a value that is not.
pub(crate) fn appconfig_rollout_identity_parts(identity: &str) -> Option<(&str, &str, &str)> {
    composite_identity_triple_parts(identity)
}

/// The types of the two Amazon MemoryDB GROUPS a cluster is configured out of (that
/// model's `awsService.cloudFormationName: MemoryDB`): the parameter group that carries
/// the engine settings, and the subnet group that places a cluster in a VPC.
///
/// "Ref returns the name" for each — the `ParameterGroupName`/`SubnetGroupName` the
/// declaration states, which is the identifier every MemoryDB call on one takes
/// (`DescribeParameterGroups$ParameterGroupName`, `DescribeSubnetGroups$SubnetGroupName`)
/// — so neither identity is service-assigned. Each publishes its own ARN, under the
/// upper-case member name that model spells it with (`ParameterGroup$ARN`,
/// `SubnetGroup$ARN`: "the Amazon Resource Name (ARN) of the parameter group" / "of the
/// subnet group").
pub const MEMORYDB_PARAMETER_GROUP: &str = "AWS::MemoryDB::ParameterGroup";
pub const MEMORYDB_SUBNET_GROUP: &str = "AWS::MemoryDB::SubnetGroup";

/// The `AWS::<cloudFormationName>::<Resource>` types of the AWS Elemental MediaConvert
/// transcoding estate a stack declares. The middle segment is that model's own
/// `awsService.cloudFormationName` (`MediaConvert`,
/// `smithy_query(query="service_info")` on that service's model).
///
/// "Ref returns the name" for each — the `Name` the declaration states, which is the
/// identifier every MediaConvert call on one takes (`GetQueue$Name`,
/// `GetJobTemplate$Name` and `GetPreset$Name` are each that operation's one `@httpLabel`)
/// — so none of the three identities is service-assigned
/// ([`identity_is_service_assigned`]). Each publishes its own `Arn`, built from the
/// resource-type segment that service's ARNs carry
/// ([`MEDIACONVERT_QUEUE_SEGMENT`] and its siblings), and
/// [`crate::state::provisioning::mediaconvert_resources`] is what creates them.
pub const MEDIACONVERT_QUEUE: &str = "AWS::MediaConvert::Queue";
pub const MEDIACONVERT_JOB_TEMPLATE: &str = "AWS::MediaConvert::JobTemplate";
pub const MEDIACONVERT_PRESET: &str = "AWS::MediaConvert::Preset";

/// AWS Elemental MediaConvert's ARN namespace, which is its model's own
/// `awsService.arnNamespace` (`mediaconvert`).
pub const MEDIACONVERT_ARN_NAMESPACE: &str = "mediaconvert";

/// The resource-type segments a MediaConvert ARN spells these three with: the PLURAL noun,
/// `queues/<name>`, `jobTemplates/<name>` and `presets/<name>` — the segments that service's
/// own answers carry in each resource's `Arn` member, and the ones its tagging calls resolve
/// — where the uniform convention ([`super::super::conventional_attributes`]) would give them
/// the singular, hyphenated type segment (`queue`, `job-template`).
pub const MEDIACONVERT_QUEUE_SEGMENT: &str = "queues";
pub const MEDIACONVERT_JOB_TEMPLATE_SEGMENT: &str = "jobTemplates";
pub const MEDIACONVERT_PRESET_SEGMENT: &str = "presets";

/// The `AWS::NetworkSecurityManager::<Resource>` types of the AWS Network Security Manager
/// estate a stack declares: the rule that carries one firewall configuration, the template
/// that groups rules, the policy that enforces templates and rules with its own settings, and
/// the scope that selects the accounts and resources a deployment applies to.
///
/// The middle segment is that model's own `awsService.sdkId` with its spaces dropped
/// (`Network Security Manager`, `smithy_query(query="service_info")` on
/// `services/service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice/\
/// smithy-model/com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json`), and the
/// resource segment is the entity each of that service's `Create…`/`Get…`/`Delete…`
/// operations is named for.
///
/// `Ref` returns the resource's ARN — the type's primary identifier — and `Fn::GetAtt` reads
/// that ARN and the id inside it ([`NETWORK_SECURITY_MANAGER_RULE_ATTRIBUTES`] and its
/// siblings). That service MINTS both: each create takes no identifier for the entity and
/// answers the id it assigned (`^[a-z0-9]{1,64}$`) with the ARN built around it, which is the
/// `…Identifier` every later call on the entity takes (its ARN member carries the model's own
/// `resourceIdentifier`), so neither value can be computed from the template — that is what
/// [`identity_is_service_assigned`] names these four types for.
pub const NETWORK_SECURITY_MANAGER_RULE: &str = "AWS::NetworkSecurityManager::Rule";
pub const NETWORK_SECURITY_MANAGER_TEMPLATE: &str = "AWS::NetworkSecurityManager::Template";
pub const NETWORK_SECURITY_MANAGER_POLICY: &str = "AWS::NetworkSecurityManager::Policy";
pub const NETWORK_SECURITY_MANAGER_SCOPE: &str = "AWS::NetworkSecurityManager::Scope";

/// AWS Network Security Manager's ARN namespace, which is that model's own
/// `awsService.arnNamespace` (`network-security-manager`), and the resource paths its ARNs
/// carry before the id that service minted — the paths its own ARNs are built from
/// (`rule/<ruleId>`, `template/<templateId>`, `policy/<policyId>`, `scope/<scopeId>`).
pub const NETWORK_SECURITY_MANAGER_ARN_NAMESPACE: &str = "network-security-manager";
pub(super) const NETWORK_SECURITY_MANAGER_RULE_PATH: &str = "rule";
pub(super) const NETWORK_SECURITY_MANAGER_TEMPLATE_PATH: &str = "template";
pub(super) const NETWORK_SECURITY_MANAGER_POLICY_PATH: &str = "policy";
pub(super) const NETWORK_SECURITY_MANAGER_SCOPE_PATH: &str = "scope";

/// How many characters the stand-in for one of those minted ids carries: the 32 lower-case
/// hexadecimal digits that service's own ids are drawn as, which is inside the
/// `^[a-z0-9]{1,64}$` its `RuleId`, `TemplateId`, `PolicyId` and `ScopeId` shapes declare.
pub(super) const NETWORK_SECURITY_MANAGER_ID_DIGITS: usize = 32;

/// Every `Fn::GetAtt` attribute one of those four types publishes, in the sorted order a
/// resource's own attribute map holds them in: the ARN that service minted and the id inside
/// it, which are the two read-only values its create answers the entity's identity in.
pub(super) const NETWORK_SECURITY_MANAGER_RULE_ATTRIBUTES: &[&str] = &["RuleArn", "RuleId"];
pub(super) const NETWORK_SECURITY_MANAGER_TEMPLATE_ATTRIBUTES: &[&str] =
    &["TemplateArn", "TemplateId"];
pub(super) const NETWORK_SECURITY_MANAGER_POLICY_ATTRIBUTES: &[&str] = &["PolicyArn", "PolicyId"];
pub(super) const NETWORK_SECURITY_MANAGER_SCOPE_ATTRIBUTES: &[&str] = &["ScopeArn", "ScopeId"];

/// Amazon MemoryDB's ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`memorydb`) rather than the `MemoryDB` its resource types are spelled with.
pub const MEMORYDB_ARN_NAMESPACE: &str = "memorydb";

/// The resource-type segments an Amazon MemoryDB ARN spells these two groups with:
/// `parametergroup/<name>` and `subnetgroup/<name>`, written as ONE word rather than the
/// hyphenated `parameter-group` the uniform convention would give them — the segments that
/// service's own ARNs carry, which is what its `DescribeParameterGroups` and
/// `DescribeSubnetGroups` report in each group's `ARN` member.
pub const MEMORYDB_PARAMETER_GROUP_SEGMENT: &str = "parametergroup";
pub const MEMORYDB_SUBNET_GROUP_SEGMENT: &str = "subnetgroup";

/// The `AWS::<cloudFormationName>::<Resource>` types of the Amazon VPC Lattice
/// RESOURCE-ACCESS estate a template declares, beside the service network and the access log
/// subscription above. The middle segment is that model's own
/// `awsService.cloudFormationName` (`VpcLattice`).
///
/// * a RESOURCE GATEWAY — "a point of ingress into the VPC where a resource resides"
///   (`CreateResourceGateway`'s own documentation), opened in a declared VPC's subnets;
/// * a RESOURCE CONFIGURATION — what one resource reached through such a gateway IS, as a
///   client of the service network addresses it;
/// * a SERVICE-NETWORK/RESOURCE ASSOCIATION — the binding that shares one configuration
///   into one service network;
/// * a SERVICE-NETWORK/VPC ASSOCIATION — the binding that lets a VPC's resources reach the
///   services in a service network.
///
/// Each type's `Ref` returns the resource's ARN (its primary identifier) and `Fn::GetAtt`
/// reads that ARN and the `Id` inside it. Amazon VPC Lattice MINTS both, exactly as it does
/// for the two types above: every one of those creates takes a name or a pair of identifiers
/// and answers the `rgw-`/`rcfg-`/`snra-`/`snva-` id it drew with the ARN built around it, so
/// no naming rule could compute either and [`identity_is_service_assigned`] names all four.
pub const VPCLATTICE_RESOURCE_GATEWAY: &str = "AWS::VpcLattice::ResourceGateway";
pub const VPCLATTICE_RESOURCE_CONFIGURATION: &str = "AWS::VpcLattice::ResourceConfiguration";
pub const VPCLATTICE_SERVICE_NETWORK_RESOURCE_ASSOCIATION: &str =
    "AWS::VpcLattice::ServiceNetworkResourceAssociation";
pub const VPCLATTICE_SERVICE_NETWORK_VPC_ASSOCIATION: &str =
    "AWS::VpcLattice::ServiceNetworkVpcAssociation";

/// The resource path each of those ARNs carries before the id that service minted, and the
/// prefix of that id — both from the modelled ARN and id patterns of each resource
/// (`resourcegateway/rgw-…`, `resourceconfiguration/rcfg-…`,
/// `servicenetworkresourceassociation/snra-…`, `servicenetworkvpcassociation/snva-…`). The
/// namespace they sit in and the width of the id are that service's own
/// ([`VPCLATTICE_ARN_NAMESPACE`], [`VPCLATTICE_ID_CHARACTERS`]).
pub(super) const VPCLATTICE_RESOURCE_GATEWAY_PATH: &str = "resourcegateway";
pub(super) const VPCLATTICE_RESOURCE_CONFIGURATION_PATH: &str = "resourceconfiguration";
pub(super) const VPCLATTICE_RESOURCE_ASSOCIATION_PATH: &str =
    "servicenetworkresourceassociation";
pub(super) const VPCLATTICE_VPC_ASSOCIATION_PATH: &str = "servicenetworkvpcassociation";
pub(super) const VPCLATTICE_RESOURCE_GATEWAY_ID_PREFIX: &str = "rgw";
pub(super) const VPCLATTICE_RESOURCE_CONFIGURATION_ID_PREFIX: &str = "rcfg";
pub(super) const VPCLATTICE_RESOURCE_ASSOCIATION_ID_PREFIX: &str = "snra";
pub(super) const VPCLATTICE_VPC_ASSOCIATION_ID_PREFIX: &str = "snva";

/// Every `Fn::GetAtt` attribute each of those four types publishes, in the sorted order a
/// resource's own attribute map holds them in: the ARN that service minted and the id inside
/// it, which are the two read-only values each create answers the resource's identity in.
pub(super) const VPCLATTICE_RESOURCE_ACCESS_ATTRIBUTES: &[&str] = &["Arn", "Id"];

/// The two attributes by name, so the arms that compute them and the provisioner that
/// re-points them at what the create really answered cannot spell them differently.
pub const VPCLATTICE_ARN_ATTRIBUTE: &str = "Arn";
pub const VPCLATTICE_ID_ATTRIBUTE: &str = "Id";

/// The `AWS::<cloudFormationName>::<Resource>` types of the two Amazon GameLift Servers
/// resources a template ships game code as. The middle segment is that model's own
/// `awsService.cloudFormationName` (`GameLift`).
///
/// * a BUILD — the custom game server build `CreateBuild` records for the zip file at the
///   declared Amazon S3 storage location. "Ref returns the build ID", and `Fn::GetAtt` reads
///   the `BuildId` and the `Arn` that service built around it;
/// * a SCRIPT — the Realtime script `CreateScript` records the same way. "Ref returns the
///   script ID", and `Fn::GetAtt` reads the `Id` and the `Arn`.
///
/// That service MINTS each identity — neither create takes an identifier for the resource,
/// and each answers the `build-…`/`script-…` id it drew with the ARN built around it
/// (`Build$BuildArn`: `^arn:.*:build\/build-\S+$`, `Script$ScriptArn`:
/// `^arn:.*:script\/script-\S+$`, each documented "the resource ID matches the
/// *BuildId*/*ScriptId* value") — so [`identity_is_service_assigned`] names both types and
/// neither value can be recomputed from the template.
pub const GAMELIFT_BUILD: &str = "AWS::GameLift::Build";
pub const GAMELIFT_SCRIPT: &str = "AWS::GameLift::Script";

/// Amazon GameLift Servers' ARN namespace, which that model carries as
/// `awsService.arnNamespace` (`gamelift`) and both resources' ARNs are named under.
pub const GAMELIFT_ARN_NAMESPACE: &str = "gamelift";

/// The resource paths those two ARNs carry before the id that service minted, from those
/// same patterns.
pub(super) const GAMELIFT_BUILD_PATH: &str = "build";
pub(super) const GAMELIFT_SCRIPT_PATH: &str = "script";

/// The prefix each of those minted ids carries (`BuildId`: `^build-\S+$`, `ScriptId`:
/// `^script-\S+$`), which the STAND-IN computed before the create carries as well.
pub(super) const GAMELIFT_BUILD_ID_PREFIX: &str = "build-";
pub(super) const GAMELIFT_SCRIPT_ID_PREFIX: &str = "script-";

/// Every `Fn::GetAtt` attribute each of the two types publishes, in the sorted order a
/// resource's own attribute map holds them in: the ARN that service minted and the id
/// inside it, which are the two read-only values its create answers the resource's identity
/// in.
pub(super) const GAMELIFT_BUILD_ATTRIBUTES: &[&str] =
    &[GAMELIFT_ARN_ATTRIBUTE, GAMELIFT_BUILD_ID_ATTRIBUTE];
pub(super) const GAMELIFT_SCRIPT_ATTRIBUTES: &[&str] =
    &[GAMELIFT_ARN_ATTRIBUTE, GAMELIFT_ID_ATTRIBUTE];

/// Those three attributes by name, so the arms that compute a stand-in and the provisioner
/// that re-points them at what the create really answered cannot spell them differently.
pub const GAMELIFT_ARN_ATTRIBUTE: &str = "Arn";
pub const GAMELIFT_BUILD_ID_ATTRIBUTE: &str = "BuildId";
pub const GAMELIFT_ID_ATTRIBUTE: &str = "Id";

/// The seven AWS End User Messaging SMS and Voice resource types a template declares, under
/// the `SMSVOICE` namespace the CloudFormation resource and property types reference
/// publishes them under. The owning service's own model is `Pinpoint SMS Voice V2`
/// (`smithy_query(query="service_info")` on that model), which signs and names its ARNs
/// `sms-voice`; the types are created in that control plane
/// ([`crate::state::provisioner::sms_voice`]).
///
/// A CONFIGURATION SET and an OPT-OUT LIST are NAMED by the declaration: each type's `Ref`
/// returns that name, which is the identifier every call of the owning service addresses one
/// by (`ConfigurationSetName`, `OptOutListName`), and the convention finds the property it is
/// read from. The other five are not: four are identified by what their create MINTED
/// ([`identity_is_service_assigned`]) and a RESOURCE POLICY by the ARN of the resource it is
/// attached to, which is the type's own `ResourceArn` property
/// ([`declared_name_property`]).
pub const SMSVOICE_CONFIGURATION_SET: &str = "AWS::SMSVOICE::ConfigurationSet";
pub const SMSVOICE_OPT_OUT_LIST: &str = "AWS::SMSVOICE::OptOutList";
pub const SMSVOICE_PROTECT_CONFIGURATION: &str = "AWS::SMSVOICE::ProtectConfiguration";
pub const SMSVOICE_RESOURCE_POLICY: &str = "AWS::SMSVOICE::ResourcePolicy";
pub const SMSVOICE_RCS_AGENT: &str = "AWS::SMSVOICE::RcsAgent";
pub const SMSVOICE_REGISTRATION: &str = "AWS::SMSVOICE::Registration";
pub const SMSVOICE_REGISTRATION_ATTACHMENT: &str = "AWS::SMSVOICE::RegistrationAttachment";

/// That service's ARN namespace, which its model carries as `awsService.arnNamespace`
/// (`sms-voice`) and every one of these resources' ARNs is named under.
pub(super) const SMSVOICE_ARN_NAMESPACE: &str = "sms-voice";

/// The resource path each of those ARNs carries before the resource's name or minted id,
/// which is the `ResourceType` the owning service's own exceptions name the resource by
/// (`smithy_query(query="shape",shape="ResourceType")` on that model).
pub(super) const SMSVOICE_CONFIGURATION_SET_PATH: &str = "configuration-set";
pub(super) const SMSVOICE_OPT_OUT_LIST_PATH: &str = "opt-out-list";
pub(super) const SMSVOICE_PROTECT_CONFIGURATION_PATH: &str = "protect-configuration";
pub(super) const SMSVOICE_RCS_AGENT_PATH: &str = "rcs-agent";
pub(super) const SMSVOICE_REGISTRATION_PATH: &str = "registration";
pub(super) const SMSVOICE_REGISTRATION_ATTACHMENT_PATH: &str = "registration-attachment";

/// How wide the identifier each of the four service-identified types carries is, and the
/// prefix it carries before it: that service mints an identifier as its resource kind
/// followed by hexadecimal digits, so the STAND-IN computed before the create has that form
/// too.
pub(super) const SMSVOICE_ID_HEX_DIGITS: usize = 32;
pub(super) const SMSVOICE_PROTECT_CONFIGURATION_ID_PREFIX: &str = "protect-configuration-";
pub(super) const SMSVOICE_RCS_AGENT_ID_PREFIX: &str = "rcsagent-";
pub(super) const SMSVOICE_REGISTRATION_ID_PREFIX: &str = "registration-";
pub(super) const SMSVOICE_REGISTRATION_ATTACHMENT_ID_PREFIX: &str = "attachment-";

/// Every `Fn::GetAtt` attribute each of those types publishes, in the sorted order a
/// resource's own attribute map holds them in: the ARN that service minted, and for a type
/// whose identity that service assigns the identifier inside it.
///
/// A RESOURCE POLICY carries no entry: its one return value is the ARN its `Ref` returns,
/// which is the physical resource id rather than an attribute.
pub(super) const SMSVOICE_NAMED_ATTRIBUTES: &[&str] = &[SMSVOICE_ARN_ATTRIBUTE];
pub(super) const SMSVOICE_PROTECT_CONFIGURATION_ATTRIBUTES: &[&str] = &[
    SMSVOICE_ARN_ATTRIBUTE,
    SMSVOICE_PROTECT_CONFIGURATION_ID_ATTRIBUTE,
];
pub(super) const SMSVOICE_RCS_AGENT_ATTRIBUTES: &[&str] =
    &[SMSVOICE_ARN_ATTRIBUTE, SMSVOICE_RCS_AGENT_ID_ATTRIBUTE];
pub(super) const SMSVOICE_REGISTRATION_ATTRIBUTES: &[&str] =
    &[SMSVOICE_ARN_ATTRIBUTE, SMSVOICE_REGISTRATION_ID_ATTRIBUTE];
pub(super) const SMSVOICE_REGISTRATION_ATTACHMENT_ATTRIBUTES: &[&str] = &[
    SMSVOICE_ARN_ATTRIBUTE,
    SMSVOICE_REGISTRATION_ATTACHMENT_ID_ATTRIBUTE,
];

/// Those attributes by name, so the arms that compute a stand-in and the provisioner that
/// re-points them at what the create really answered cannot spell them differently.
pub const SMSVOICE_ARN_ATTRIBUTE: &str = "Arn";
pub const SMSVOICE_PROTECT_CONFIGURATION_ID_ATTRIBUTE: &str = "ProtectConfigurationId";
pub const SMSVOICE_RCS_AGENT_ID_ATTRIBUTE: &str = "RcsAgentId";
pub const SMSVOICE_REGISTRATION_ID_ATTRIBUTE: &str = "RegistrationId";
pub const SMSVOICE_REGISTRATION_ATTACHMENT_ID_ATTRIBUTE: &str = "RegistrationAttachmentId";
