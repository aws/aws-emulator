// SPDX-License-Identifier: CC0-1.0

//! @op PutConformancePack
//!
//! Creates or updates a conformance pack — "a collection of Config rules that can be
//! easily deployed in an account and a region" — and the CloudFormation stack it is
//! deployed as.
//!
//! What this handler enforces, and where each rule comes from:
//!
//! * "For custom conformance packs, you must specify only one of TemplateS3Uri,
//!   TemplateBody, TemplateSSMDocumentDetails": a request carrying none of them, or
//!   more than one, is refused with the modelled `InvalidParameterValueException`;
//! * "For managed conformance packs, you must specify Source. Do not specify
//!   TemplateS3Uri, TemplateBody, TemplateSSMDocumentDetails, DeliveryS3Bucket, or
//!   DeliveryS3KeyPrefix": a request naming a `Source` is a MANAGED deployment, so one
//!   that names a template or a delivery location beside it is refused with the same
//!   error, as is a `Scope` named without a `Source` ("supported only for managed
//!   conformance packs ... if you specify a scope, you must also specify Source");
//! * "You cannot convert an existing conformance pack between a managed conformance pack
//!   and a custom conformance pack": the kind a pack is was fixed when it was created,
//!   so a request that would deploy an existing pack as the other kind is refused and
//!   leaves it as it is;
//! * a `TemplateBody` is READ as the YAML (or JSON, which YAML 1.2 is a superset of)
//!   template it must be, and must declare the resource types a conformance pack
//!   template declares ([`CONFORMANCE_PACK_TEMPLATE_RESOURCE_TYPES`]); a body that is
//!   neither is refused with the modelled `ConformancePackTemplateValidationException`
//!   ("You have specified a template that is not valid or supported") and leaves NO
//!   pack behind for that name — worded with the service's fixed text for the reading
//!   that refused it: [`INVALID_TEMPLATE_MESSAGE`] for a body that is not the
//!   CloudFormation template a pack is deployed from, and the errors Config's own
//!   conformance pack validation found in a template it could read
//!   ([`invalid_conformance_pack_template_message`]), neither reporting any part of the
//!   body;
//! * "PutConformancePack is an idempotent API. Subsequent requests won't create a
//!   duplicate resource if one was already created": a second call for the same name
//!   redeploys that pack rather than creating another, and "tags will not be updated,
//!   even if they are different";
//! * "For PutConformancePack ..., a conformance pack creation, update, and deletion is
//!   in progress. Try your request again later." (`ResourceInUseException`): the
//!   deployment this call starts is still in flight when it answers, so a further call
//!   for a pack whose deployment has not settled is refused rather than restarting work
//!   in progress — the idempotency above is what a call for a SETTLED pack gets, and the
//!   refusal NAMES the pack whose workflow holds it
//!   ([`crate::state::conformance_pack_workflow_in_progress_message`]), rather than
//!   repeating the error shape's documentation sentence;
//! * a NEW pack has to fit under this Region's conformance pack quota
//!   ([`crate::state::CONFORMANCE_PACKS_QUOTA`]), else
//!   `MaxNumberOfConformancePacksExceededException`;
//! * the deployment is not finished when this call answers: the pack is
//!   `CREATE_IN_PROGRESS` until the clock makes `CREATE_COMPLETE` due
//!   ([`crate::state::CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER`]), which is what
//!   `DescribeConformancePackStatus` reports.
//!
//! A template Config has to FETCH to read — `TemplateS3Uri`, whose body is an Amazon S3
//! object, and `TemplateSSMDocumentDetails`, whose body is a Systems Manager document —
//! is not read here: this handler cannot see either store, so rather than deploy a pack
//! from a template it never read it answers the scaffold's 501 for those two legs alone.
//! A MANAGED pack is the third such leg: what its `Source` names is "a predefined
//! identifier for a managed conformance pack" standing for a collection of Config rules
//! "created and maintained by Amazon Web Services", which this emulator holds none of,
//! so rather than report a managed pack deployed whose rules exist nowhere this handler
//! answers 501 for a request carrying a `Source` too. A managed pack that IS in state —
//! carrying its `Source`, its `Scope` and the `MANAGED` type — is reported by the
//! conformance pack read paths like any other.
//!
//! ## The values the template is deployed WITH
//!
//! A conformance pack is deployed as a CloudFormation stack, and the request's
//! `ConformancePackInputParameters` are that stack's parameter values ("input parameters
//! in the form of key-value pairs for the conformance pack",
//! `ConformancePackInputParameter`). So every reference the template makes to one of its
//! own parameters RESOLVES — to the value the request gave that parameter, or, where the
//! request gave none, to the `Default` the template declares for it
//! ([`resolve_parameter_references`]) — before the deployment reads what the template
//! declares. A pack whose template scopes a rule to one resource through a parameter
//! therefore deploys a rule scoped to that resource, rather than one scoped to the
//! parameter's own name. A reference nothing resolves is left as the reference it is, so
//! the member it stands in for carries no value rather than a made-up one. The pack's
//! STORED template body is what was sent, byte for byte: resolving is how the declarations
//! are READ, not a rewrite of the template.
//!
//! ## What a deployment here does
//!
//! The deployment CREATES what the template declares: each `AWS::Config::ConfigRule` is
//! written into this account's Config rules, under the name Config gives a pack's rule
//! ([`crate::state::conformance_pack_rule_name`]) and carrying the pack's service
//! principal as its `CreatedBy`, and each `AWS::Config::RemediationConfiguration` is
//! written as the remediation of the rule it names. So a pack this operation reports
//! `CREATE_COMPLETE` is one whose rules `DescribeConfigRules` reports and whose
//! remediation actions `DescribeRemediationConfigurations` reports; a REDEPLOY of the
//! same pack replaces exactly the resources its previous deployment created.
//!
//! A deployment can only create a resource THIS ACCOUNT COULD CREATE DIRECTLY, so every
//! resource the template declares is judged by the preconditions the operation that
//! creates that kind of resource enforces: a remediation action by
//! `PutRemediationConfigurations`' own ([`crate::state::unusable_remediation`]), a Config
//! rule by `PutConfigRule`'s own ([`crate::state::unusable_config_rule`]) and the declared
//! Config rules together by this Region's Config rules quota. A deployment that cannot
//! create what the template declares creates NOTHING — leaving a previous deployment of
//! the same pack standing — and is reported in the modelled `CREATE_FAILED` state with
//! the reason (`ConformancePackStatusReason`) rather than as a completion of work nothing
//! performed.
//!
//! What the deployment does NOT do is evaluate those rules itself: the evaluation is the
//! pass Config owes a rule it has just created ("when you create a rule, Config evaluates
//! your resources against the rule automatically", `PutConfigRule`), which the in-process
//! pump runs ([`crate::state::drive_recorder_clock`]) — so a rule whose implementation
//! this emulator has is placed at a verdict a span after this call answers, and the pack's
//! own compliance surface (`DescribeConformancePackCompliance`,
//! `GetConformancePackComplianceDetails`, `GetConformancePackComplianceSummary`) reports
//! those results. A rule whose implementation this emulator does not have records no
//! verdict at all rather than a synthesised one, and reports the `INSUFFICIENT_DATA` of a
//! rule nothing has evaluated.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `PutConformancePack::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutConformancePack`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json")
//! ```
//!
//! Creates or updates a conformance pack. A conformance pack is a collection of Config
//! rules that can be easily deployed in an account and a region and across an organization.
//! For information on how many conformance packs you can have per account, see Service
//! Limits in the Config Developer Guide. When you use PutConformancePack to deploy
//! conformance packs in your account, the operation can create Config rules and remediation
//! actions without requiring config:PutConfigRule or config:PutRemediationConfigurations
//! permissions in your account IAM policies. This API uses the
//! AWSServiceRoleForConfigConforms service-linked role in your account to create
//! conformance pack resources. This service-linked role includes the permissions to create
//! Config rules and remediation configurations, even if your account IAM policies
//! explicitly deny these actions. This API creates a service-linked role
//! AWSServiceRoleForConfigConforms in your account. The service-linked role is created only
//! when the role does not exist in your account. You must specify only one of the follow
//! parameters: TemplateS3Uri, TemplateBody or TemplateSSMDocumentDetails. Tags are added at
//! creation and cannot be updated with this operation PutConformancePack is an idempotent
//! API. Subsequent requests won't create a duplicate resource if one was already created.
//! If a following request has different tags values, Config will ignore these differences
//! and treat it as an idempotent request of the previous. In this case, tags will not be
//! updated, even if they are different. Use TagResource and UntagResource to update tags
//! after creation.
//!
//! ## Input
//!
//! * `ConformancePackInputParameters` (ConformancePackInputParameters, len 0..60) — A list of
//!   ConformancePackInputParameter objects.
//! * `ConformancePackName` (ConformancePackName, len 1..256, pattern ^[a-zA-Z][-a-zA-Z0-9]*$)
//!   **required** — The unique name of the conformance pack you want to deploy.
//! * `DeliveryS3Bucket` (DeliveryS3Bucket, len 0..63) — The name of the Amazon S3 bucket
//!   where Config stores conformance pack templates. This field is optional.
//! * `DeliveryS3KeyPrefix` (DeliveryS3KeyPrefix, len 0..1024) — The prefix for the Amazon S3
//!   bucket. This field is optional.
//! * `Tags` (TagsList, len 0..50) — The tags for the conformance pack. Each tag consists of a
//!   key and an optional value, both of which you define.
//! * `TemplateBody` (TemplateBody, len 1..51200) — A string that contains the full
//!   conformance pack template body. The structure containing the template body has a minimum
//!   length of 1 byte and a maximum length of 51,200 bytes. You can use a YAML template with
//!   two resource types: Config rule (AWS::Config::ConfigRule) and remediation action
//!   (AWS::Config::RemediationConfiguration).
//! * `TemplateS3Uri` (TemplateS3Uri, len 1..1024, pattern ^s3://) — The location of the file
//!   containing the template body (s3://bucketname/prefix). The uri must point to a
//!   conformance pack template (max size: 300 KB) that is located in an Amazon S3 bucket in
//!   the same Region as the conformance pack. You must have access to read Amazon S3 bucket.
//!   In addition, in order to ensure a successful deployment, the template object must not be
//!   in an archived storage class if this parameter is passed.
//! * `TemplateSSMDocumentDetails` (TemplateSSMDocumentDetails) — An object of type
//!   TemplateSSMDocumentDetails, which contains the name or the Amazon Resource Name (ARN) of
//!   the Amazon Web Services Systems Manager document (SSM document) and the version of the
//!   SSM document that is used to create a conformance pack.
//!
//! ## Output
//!
//! * `ConformancePackArn` (ConformancePackArn, len 1..2048) — ARN of the conformance pack.
//!
//! ## Errors
//!
//! * `ConformancePackTemplateValidationException` (HTTP 400) — You have specified a template
//!   that is not valid or supported.
//! * `InsufficientPermissionsException` (HTTP 400) — Indicates one of the following errors:
//!   For PutConfigRule, the rule cannot be created because the IAM role assigned to Config
//!   lacks permissions to perform the config:Put* action. For PutConfigRule, the Lambda
//!   function cannot be invoked. Check the function ARN, and check the function's
//!   permissions. For PutOrganizationConfigRule, organization Config rule cannot be created
//!   because you do not have permissions to call IAM GetRole action or create a
//!   service-linked role. For PutConformancePack and PutOrganizationConformancePack, a
//!   conformance pack cannot be created because you do not have the following permissions:
//!   You do not have permission to call IAM GetRole action or create a service-linked …
//! * `InvalidParameterValueException` (HTTP 400) — One or more of the specified parameters
//!   are not valid. Verify that your parameters are valid and try again.
//! * `MaxNumberOfConformancePacksExceededException` (HTTP 400) — You have reached the limit
//!   of the number of conformance packs you can create in an account. For more information,
//!   see Service Limits in the Config Developer Guide.
//! * `ResourceInUseException` (HTTP 400) — You see this exception in the following cases: For
//!   DeleteConfigRule, Config is deleting this rule. Try your request again later. For
//!   DeleteConfigRule, the rule is deleting your evaluation results. Try your request again
//!   later. For DeleteConfigRule, a remediation action is associated with the rule and Config
//!   cannot delete this rule. Delete the remediation action associated with the rule before
//!   deleting the rule and try your request again later. For PutConfigOrganizationRule,
//!   organization Config rule deletion is in progress. Try your request again later. For
//!   DeleteOrganizationConfigRule, organization Config rule creation is in progress. Try your
//!   request again later. For PutConformancePack and …

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutConformancePackError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-configservice-starlingdoveservice.json");

/// The resource types a conformance pack template declares (this operation's own doc:
/// "You can use a YAML template with two resource types: Config rule
/// (AWS::Config::ConfigRule) and remediation action
/// (AWS::Config::RemediationConfiguration)").
pub const CONFORMANCE_PACK_TEMPLATE_RESOURCE_TYPES: [&str; 2] = [
    "AWS::Config::ConfigRule",
    "AWS::Config::RemediationConfiguration",
];

/// The two resource types of [`CONFORMANCE_PACK_TEMPLATE_RESOURCE_TYPES`], named so the
/// reading can tell one declared resource from the other.
const CONFIG_RULE_RESOURCE_TYPE: &str = CONFORMANCE_PACK_TEMPLATE_RESOURCE_TYPES[0];
/// See [`CONFIG_RULE_RESOURCE_TYPE`].
const REMEDIATION_RESOURCE_TYPE: &str = CONFORMANCE_PACK_TEMPLATE_RESOURCE_TYPES[1];

/// The properties of a declared `AWS::Config::ConfigRule`, spelled as CloudFormation
/// spells them — which, for this resource type, is how the `ConfigRule` shape spells its
/// own members.
const RULE_NAME_MEMBER: &str = "ConfigRuleName";
/// See [`RULE_NAME_MEMBER`].
const RULE_DESCRIPTION_MEMBER: &str = "Description";
/// See [`RULE_NAME_MEMBER`].
const RULE_SOURCE_MEMBER: &str = "Source";
/// See [`RULE_NAME_MEMBER`].
const RULE_INPUT_PARAMETERS_MEMBER: &str = "InputParameters";
/// See [`RULE_NAME_MEMBER`].
const RULE_MAXIMUM_EXECUTION_FREQUENCY_MEMBER: &str = "MaximumExecutionFrequency";
/// See [`RULE_NAME_MEMBER`].
const RULE_SCOPE_MEMBER: &str = "Scope";
/// See [`RULE_NAME_MEMBER`].
const SOURCE_OWNER_MEMBER: &str = "Owner";
/// See [`RULE_NAME_MEMBER`].
const SOURCE_IDENTIFIER_MEMBER: &str = "SourceIdentifier";
/// See [`RULE_NAME_MEMBER`].
const SCOPE_COMPLIANCE_RESOURCE_TYPES_MEMBER: &str = "ComplianceResourceTypes";
/// See [`RULE_NAME_MEMBER`].
const SCOPE_COMPLIANCE_RESOURCE_ID_MEMBER: &str = "ComplianceResourceId";
/// See [`RULE_NAME_MEMBER`].
const SCOPE_TAG_KEY_MEMBER: &str = "TagKey";
/// See [`RULE_NAME_MEMBER`].
const SCOPE_TAG_VALUE_MEMBER: &str = "TagValue";

/// The properties of a declared `AWS::Config::RemediationConfiguration`, spelled the
/// same way (the members of the `RemediationConfiguration` shape).
const REMEDIATION_RULE_NAME_MEMBER: &str = "ConfigRuleName";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const REMEDIATION_TARGET_TYPE_MEMBER: &str = "TargetType";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const REMEDIATION_TARGET_ID_MEMBER: &str = "TargetId";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const REMEDIATION_TARGET_VERSION_MEMBER: &str = "TargetVersion";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const REMEDIATION_PARAMETERS_MEMBER: &str = "Parameters";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const REMEDIATION_RESOURCE_TYPE_MEMBER: &str = "ResourceType";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const REMEDIATION_AUTOMATIC_MEMBER: &str = "Automatic";
/// See [`REMEDIATION_RULE_NAME_MEMBER`]: the two forms one remediation parameter takes.
const PARAMETER_RESOURCE_VALUE_MEMBER: &str = "ResourceValue";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const PARAMETER_STATIC_VALUE_MEMBER: &str = "StaticValue";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const PARAMETER_VALUE_MEMBER: &str = "Value";
/// See [`REMEDIATION_RULE_NAME_MEMBER`].
const PARAMETER_VALUES_MEMBER: &str = "Values";

/// How Config words `ConformancePackTemplateValidationException` for a body that is not
/// the CloudFormation template a conformance pack is deployed from at all — one that is
/// no single YAML (or JSON) mapping document, that declares no resource, or that declares
/// a resource without the type and properties CloudFormation requires of it.
///
/// A FIXED service sentence, carried WHOLE and carrying no part of the body: the recorded
/// refusal of a body that is not a template is exactly this text and reports nothing of
/// what was sent, so neither does any other refusal of this class. The errors Config's
/// own conformance pack validation finds in a template it COULD read are worded the other
/// way ([`invalid_conformance_pack_template_message`]).
pub const INVALID_TEMPLATE_MESSAGE: &str = "Template passed in the input parameter is invalid";

/// How Config words `ConformancePackTemplateValidationException` for a template it read as
/// a CloudFormation template but whose contents its own conformance pack validation found
/// errors in.
///
/// RECORDED behaviour: a fixed opening sentence, the errors that validation found rendered
/// as the bracketed list they are, and a fixed closing sentence. The only variable part is
/// the LIST — each entry states a class of fault and names no part of the body, so the
/// refusal reports neither the logical id nor the type of the resource it was found on.
pub fn invalid_conformance_pack_template_message(errors: &[&str]) -> String {
    format!(
        "Invalid conformance pack template provided. Errors found: [{}]. Fix your template and \
         try again.",
        errors.join(", ")
    )
}

/// The template error Config reports for a template declaring a resource of a type a
/// conformance pack does not deploy ([`CONFORMANCE_PACK_TEMPLATE_RESOURCE_TYPES`]).
///
/// RECORDED wording, and ONE entry however many such resources the template declares:
/// the error is about the type(s) the template provided, which is why it names none of
/// them and reads plurally.
const UNSUPPORTED_RESOURCE_TYPE_TEMPLATE_ERROR: &str =
    "Unsupported resource type(s) provided in the template";

/// The section of a template that declares the resources it deploys, and the member of
/// one declared resource that names its type. Both are CloudFormation's own spelling,
/// which is the template language a conformance pack template is written in.
const TEMPLATE_RESOURCES_SECTION: &str = "Resources";
/// See [`TEMPLATE_RESOURCES_SECTION`].
const TEMPLATE_RESOURCE_TYPE_MEMBER: &str = "Type";

/// The most nodes the TREE this reader builds may carry, and the deepest it may nest.
///
/// These bound what the READER holds: `TemplateBody` is `@length(1, 51200)`, which the
/// boundary validator enforces before this handler runs, so the parse is over at most
/// 51 200 characters, and a body that would build a larger or deeper tree than this is
/// read as no template. They do NOT bound the loader's own recursion — an
/// `EventReceiver` cannot abort `load`, because `on_event` returns `()` — which is what
/// [`guard_template_nesting`] bounds BEFORE the loader is entered. A conformance pack
/// template declaring sixty rules nests a handful of levels and a few thousand nodes, so
/// both sit an order of magnitude above any template.
const MAX_TEMPLATE_NODES: usize = 100_000;
/// See [`MAX_TEMPLATE_NODES`].
const MAX_TEMPLATE_DEPTH: usize = 64;

/// Pre-flight bounds on an untrusted template body, applied BEFORE the loader builds a
/// tree from it.
///
/// What has to be bounded is how deep the loader can RECURSE: an `EventReceiver` cannot
/// abort `load` (`on_event` returns `()`), and exhausting the stack aborts the process
/// rather than unwinding, so the reader's own [`MAX_TEMPLATE_DEPTH`] can only refuse a
/// tree it was already handed. The two sibling readings of caller-supplied YAML in this
/// tree guard their loads the same way, for the same reason
/// (`services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs`
/// and `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/put_rest_api.rs`).
///
/// Each measure is a bound on NESTING rather than on how big the body is, so a large,
/// flat template passes all of them:
///
/// * [`MAX_TEMPLATE_FLOW_DEPTH`] — the flow collections (`[`, `{`) open at once, with
///   closers subtracted: the nesting a well-formed body actually reaches.
/// * [`MAX_TEMPLATE_FLOW_OPENERS`] — the total flow openers, the backstop for the one way
///   the measure above can be fooled: this is a byte scan, so a `]` inside a quoted
///   scalar lowers the running depth while the loader's own depth keeps climbing. However
///   many closers a body hides, its real flow nesting cannot exceed its opener COUNT.
/// * [`MAX_TEMPLATE_INDENT`] columns of leading indentation and
///   [`MAX_TEMPLATE_COMPACT_PER_LINE`] compact `- `/`? ` indicators on one line — the two
///   ways BLOCK nesting descends (a column per level across lines, or a compact entry
///   within one line).
///
/// Over-counting (a `[` inside a quoted scalar) is the only failure mode, and each bound
/// sits far above the nesting a conformance pack template reaches.
fn guard_template_nesting(body: &str) -> Option<()> {
    let mut flow_depth = 0usize;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    let mut previous = b'\n';

    for byte in body.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_TEMPLATE_INDENT {
                    return None;
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_depth += 1;
                flow_openers += 1;
                if flow_depth > MAX_TEMPLATE_FLOW_DEPTH || flow_openers > MAX_TEMPLATE_FLOW_OPENERS
                {
                    return None;
                }
            }
            b']' | b'}' => {
                counting_indent = false;
                flow_depth = flow_depth.saturating_sub(1);
            }
            _ => counting_indent = false,
        }
        // A `-` or `?` is a block indicator only where a token may start — at the start of
        // a line or after whitespace — which is what keeps a hyphen inside a name, an ARN
        // or a description from counting as one.
        if matches!(byte, b'-' | b'?') && matches!(previous, b'\n' | b'\r' | b' ' | b'\t') {
            compact_on_line += 1;
            if compact_on_line > MAX_TEMPLATE_COMPACT_PER_LINE {
                return None;
            }
        }
        previous = byte;
    }
    Some(())
}

/// The most flow collections (`[`, `{`) a template body may have open at once. Chosen,
/// far past the property nesting any conformance pack template reaches.
const MAX_TEMPLATE_FLOW_DEPTH: usize = 64;

/// The most flow openers a template body may carry in total — the backstop for
/// [`MAX_TEMPLATE_FLOW_DEPTH`] described in [`guard_template_nesting`]. Chosen: a body of
/// this many openers with every closer hidden inside a quoted scalar is the worst case
/// this bound permits, and it sits far above the flow collections a template uses.
const MAX_TEMPLATE_FLOW_OPENERS: usize = 2_048;

/// The most columns of leading indentation one line of a template body may carry, which is
/// where block nesting descends across lines. Chosen.
const MAX_TEMPLATE_INDENT: usize = 256;

/// The most compact block indicators (`- `, `? `) one line may carry, which is where block
/// nesting (`- - - x`) descends without costing a column. Chosen.
const MAX_TEMPLATE_COMPACT_PER_LINE: usize = 256;

/// The section of a template that declares a resource's properties, and the members of
/// a declared Config rule and remediation action this deployment reads out of it. All
/// are CloudFormation's own spelling, which is the template language a conformance pack
/// template is written in, and each is the member of the corresponding Config API shape.
const TEMPLATE_PROPERTIES_SECTION: &str = "Properties";

/// One resource a conformance pack template declares, as the deployment reads it.
#[derive(Debug)]
enum DeclaredResource {
    /// A Config rule (`AWS::Config::ConfigRule`).
    Rule(DeclaredRule),
    /// A remediation action (`AWS::Config::RemediationConfiguration`) for one of the
    /// rules the same template declares.
    Remediation(DeclaredRemediation),
}

/// A Config rule a template declares: the name it gives the rule and the rule's own
/// definition, read out of the resource's `Properties`.
#[derive(Debug)]
struct DeclaredRule {
    /// `Properties.ConfigRuleName`, or the resource's logical id when the template names
    /// none — which is how CloudFormation names a resource whose name it is not given.
    declared_name: String,
    description: Option<String>,
    source: Source,
    input_parameters: Option<String>,
    maximum_execution_frequency: Option<String>,
    scope: Option<Scope>,
}

/// A remediation action a template declares, naming the rule (by the name the TEMPLATE
/// gives it) that it remediates.
#[derive(Debug)]
struct DeclaredRemediation {
    rule_declared_name: String,
    requested: RemediationConfiguration,
}

/// The resources `body` declares, or how Config words its refusal of a body that is not
/// a conformance pack template.
///
/// The body is READ, not matched: `yaml-rust2` parses it as the YAML 1.2 document a
/// CloudFormation template is written as (and JSON is a subset of), so a body is judged
/// by the template it IS. What a conformance pack template has to be, beyond parsing: a
/// single document, a mapping, carrying a non-empty `Resources` section each of whose
/// declared resources is one of the two resource types a conformance pack deploys and
/// declares the properties CloudFormation requires for that type — a `Source` for a
/// Config rule, and a `ConfigRuleName`, `TargetType` and `TargetId` for a remediation
/// action, whose rule the same template must declare.
///
/// Which of the service's two fixed refusal texts the `Err` carries follows WHICH of
/// those readings the body fails: a body that is not the CloudFormation template a pack
/// is deployed from is refused with [`INVALID_TEMPLATE_MESSAGE`], while a template that
/// IS one, declaring a resource of a type a conformance pack does not deploy, is refused
/// with the errors Config's own conformance pack validation found in it
/// ([`invalid_conformance_pack_template_message`]). Neither carries any part of the body.
fn read_conformance_pack_template(
    body: &str,
    input_parameters: &[ConformancePackInputParameter],
) -> Result<Vec<DeclaredResource>, String> {
    let Some(mut read) = read_template(body) else {
        return Err(INVALID_TEMPLATE_MESSAGE.to_string());
    };
    // The template is deployed WITH the values the request gave its parameters, so every
    // reference to one resolves before the declarations below are read.
    let parameters = resolved_template_parameters(&read, input_parameters);
    resolve_parameter_references(&mut read, &parameters);
    let ::serde_json::Value::Object(template) = read else {
        return Err(INVALID_TEMPLATE_MESSAGE.to_string());
    };
    let Some(::serde_json::Value::Object(resources)) = template.get(TEMPLATE_RESOURCES_SECTION)
    else {
        return Err(INVALID_TEMPLATE_MESSAGE.to_string());
    };
    if resources.is_empty() {
        return Err(INVALID_TEMPLATE_MESSAGE.to_string());
    }

    // Config's own validation of the template it read: every resource a conformance pack
    // template declares is one of the two resource types a pack deploys, and a template
    // declaring any other is refused with the error that validation found — reported ONCE
    // for the template, whichever resource carries it and however many do, which is why
    // this is judged over the whole `Resources` section before any resource is read.
    let declares_an_unsupported_type = resources.values().any(|resource| {
        resource
            .as_object()
            .and_then(|resource| resource.get(TEMPLATE_RESOURCE_TYPE_MEMBER))
            .and_then(::serde_json::Value::as_str)
            .is_some_and(|resource_type| {
                !CONFORMANCE_PACK_TEMPLATE_RESOURCE_TYPES.contains(&resource_type)
            })
    });
    if declares_an_unsupported_type {
        return Err(invalid_conformance_pack_template_message(&[
            UNSUPPORTED_RESOURCE_TYPE_TEMPLATE_ERROR,
        ]));
    }

    let mut declared = Vec::new();
    for (logical_id, resource) in resources {
        let resource = resource.as_object();
        let resource_type = resource
            .and_then(|resource| resource.get(TEMPLATE_RESOURCE_TYPE_MEMBER))
            .and_then(::serde_json::Value::as_str);
        // CloudFormation requires every declared resource to carry a `Type`, so a
        // resource declaring none — or declaring one that is no string — is not a
        // resource of a template a pack is deployed from.
        let Some(resource_type) = resource_type else {
            return Err(INVALID_TEMPLATE_MESSAGE.to_string());
        };
        let properties = resource
            .and_then(|resource| resource.get(TEMPLATE_PROPERTIES_SECTION))
            .and_then(::serde_json::Value::as_object);
        let text = |member: &str| -> Option<String> {
            properties
                .and_then(|properties| properties.get(member))
                .and_then(::serde_json::Value::as_str)
                .map(str::to_string)
        };

        match resource_type {
            CONFIG_RULE_RESOURCE_TYPE => {
                // CloudFormation requires `Source` for a Config rule, and Config
                // requires the source's `Owner`: a rule that names neither cannot be
                // deployed, so the template that declares it is not one Config deploys.
                let source = properties
                    .and_then(|properties| properties.get(RULE_SOURCE_MEMBER))
                    .and_then(::serde_json::Value::as_object);
                let owner = source
                    .and_then(|source| source.get(SOURCE_OWNER_MEMBER))
                    .and_then(::serde_json::Value::as_str);
                let Some(owner) = owner else {
                    return Err(INVALID_TEMPLATE_MESSAGE.to_string());
                };
                declared.push(DeclaredResource::Rule(DeclaredRule {
                    declared_name: text(RULE_NAME_MEMBER).unwrap_or_else(|| logical_id.clone()),
                    description: text(RULE_DESCRIPTION_MEMBER),
                    source: Source {
                        owner: owner.to_string(),
                        source_identifier: source
                            .and_then(|source| source.get(SOURCE_IDENTIFIER_MEMBER))
                            .and_then(::serde_json::Value::as_str)
                            .map(str::to_string),
                        // A rule's `SourceDetails` and `CustomPolicyDetails` are not
                        // read here: neither is required to deploy the rule, and this
                        // reading reports only what the template states.
                        source_details: None,
                        custom_policy_details: None,
                    },
                    // "A string, in JSON format, that is passed to the Config rule
                    // Lambda function": a template may write it as the JSON string it
                    // is, or as the mapping that document is, and both are stored as
                    // that document.
                    input_parameters: properties
                        .and_then(|properties| properties.get(RULE_INPUT_PARAMETERS_MEMBER))
                        .map(|value| match value {
                            ::serde_json::Value::String(text) => text.clone(),
                            other => other.to_string(),
                        }),
                    maximum_execution_frequency: text(RULE_MAXIMUM_EXECUTION_FREQUENCY_MEMBER),
                    scope: properties
                        .and_then(|properties| properties.get(RULE_SCOPE_MEMBER))
                        .and_then(::serde_json::Value::as_object)
                        .map(|scope| Scope {
                            compliance_resource_types: scope
                                .get(SCOPE_COMPLIANCE_RESOURCE_TYPES_MEMBER)
                                .and_then(::serde_json::Value::as_array)
                                .map(|types| {
                                    types
                                        .iter()
                                        .filter_map(::serde_json::Value::as_str)
                                        .map(str::to_string)
                                        .collect()
                                }),
                            tag_key: scope
                                .get(SCOPE_TAG_KEY_MEMBER)
                                .and_then(::serde_json::Value::as_str)
                                .map(str::to_string),
                            tag_value: scope
                                .get(SCOPE_TAG_VALUE_MEMBER)
                                .and_then(::serde_json::Value::as_str)
                                .map(str::to_string),
                            compliance_resource_id: scope
                                .get(SCOPE_COMPLIANCE_RESOURCE_ID_MEMBER)
                                .and_then(::serde_json::Value::as_str)
                                .map(str::to_string),
                            service_principals: None,
                        }),
                }));
            }
            REMEDIATION_RESOURCE_TYPE => {
                // CloudFormation requires all three of these for a remediation action.
                let (Some(rule_declared_name), Some(target_type), Some(target_id)) = (
                    text(REMEDIATION_RULE_NAME_MEMBER),
                    text(REMEDIATION_TARGET_TYPE_MEMBER),
                    text(REMEDIATION_TARGET_ID_MEMBER),
                ) else {
                    return Err(INVALID_TEMPLATE_MESSAGE.to_string());
                };
                let automatic = properties
                    .and_then(|properties| properties.get(REMEDIATION_AUTOMATIC_MEMBER))
                    .map(|value| match value {
                        ::serde_json::Value::Bool(flag) => *flag,
                        other => other.as_str() == Some("true"),
                    })
                    .unwrap_or_default();
                declared.push(DeclaredResource::Remediation(DeclaredRemediation {
                    rule_declared_name,
                    requested: RemediationConfiguration {
                        // The rule's deployed name is not known until the pack has an
                        // id, so the deployment fills it in.
                        config_rule_name: String::new(),
                        target_type,
                        target_id,
                        target_version: text(REMEDIATION_TARGET_VERSION_MEMBER),
                        parameters: properties
                            .and_then(|properties| {
                                properties.get(REMEDIATION_PARAMETERS_MEMBER)
                            })
                            .and_then(::serde_json::Value::as_object)
                            .map(|parameters| {
                                parameters
                                    .iter()
                                    .map(|(name, value)| {
                                        (name.clone(), remediation_parameter(value))
                                    })
                                    .collect()
                            }),
                        resource_type: text(REMEDIATION_RESOURCE_TYPE_MEMBER),
                        automatic,
                        execution_controls: None,
                        maximum_automatic_attempts: None,
                        retry_attempt_seconds: None,
                        arn: None,
                        created_by_service: None,
                    },
                }));
            }
            // A type that is not one of the two a conformance pack deploys: the reading
            // above refuses such a template over the whole `Resources` section, and this
            // arm words the same refusal the same way for the match that is over text.
            _ => {
                return Err(invalid_conformance_pack_template_message(&[
                    UNSUPPORTED_RESOURCE_TYPE_TEMPLATE_ERROR,
                ]))
            }
        }
    }

    // "The specified Config rule ... must exist": a remediation action remediates one of
    // the rules the pack deploys, so a template whose remediation names a rule it does
    // not declare cannot be deployed.
    let rule_names: Vec<&str> = declared
        .iter()
        .filter_map(|resource| match resource {
            DeclaredResource::Rule(rule) => Some(rule.declared_name.as_str()),
            DeclaredResource::Remediation(_) => None,
        })
        .collect();
    for resource in &declared {
        if let DeclaredResource::Remediation(remediation) = resource {
            if !rule_names.contains(&remediation.rule_declared_name.as_str()) {
                return Err(INVALID_TEMPLATE_MESSAGE.to_string());
            }
        }
    }
    Ok(declared)
}

/// One remediation parameter as the template writes it: the resource member the value is
/// taken from (`ResourceValue.Value`), or the static values given for it
/// (`StaticValue.Values`).
fn remediation_parameter(value: &::serde_json::Value) -> RemediationParameterValue {
    let member = |outer: &str, inner: &str| {
        value
            .get(outer)
            .and_then(|outer| outer.get(inner))
            .cloned()
            .unwrap_or(::serde_json::Value::Null)
    };
    RemediationParameterValue {
        resource_value: member(PARAMETER_RESOURCE_VALUE_MEMBER, PARAMETER_VALUE_MEMBER)
            .as_str()
            .map(|value| ResourceValue {
                value: value.to_string(),
            }),
        static_value: member(PARAMETER_STATIC_VALUE_MEMBER, PARAMETER_VALUES_MEMBER)
            .as_array()
            .map(|values| StaticValue {
                values: values
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect(),
            }),
    }
}

/// `body` as the tree it declares, or `None` when it is no single YAML document.
///
/// The nesting bounds are checked FIRST, on the bytes ([`guard_template_nesting`]), so a
/// body that would drive the loader into a deep recursion is refused before the loader is
/// entered rather than descended with. Every scalar is kept as the TEXT it was written as:
/// the template is CHECKED here, not interpreted, and the one value this reading asks
/// about — a resource's `Type` — is a string in every template. An ALIAS is refused rather
/// than expanded, because an alias is how a few hundred bytes of YAML turn into gigabytes,
/// and nothing in a conformance pack template needs one.
/// A node a YAML template tags with an intrinsic function's SHORT form is read as the
/// LONG form the same template written as JSON spells ([`long_form`]), so a declaration
/// reads the same whichever of the two forms its author wrote: `!Ref Name` is the node
/// `{"Ref": "Name"}`, which is what [`resolve_parameter_references`] resolves.
fn read_template(body: &str) -> Option<::serde_json::Value> {
    use ::serde_json::{Map, Value};

    enum Frame {
        Sequence {
            /// The intrinsic function this node's own tag names, where it names one.
            tag: Option<&'static str>,
            items: Vec<Value>,
        },
        Mapping {
            /// See [`Frame::Sequence`].
            tag: Option<&'static str>,
            entries: Map<String, Value>,
            pending_key: Option<String>,
        },
    }

    #[derive(Default)]
    struct Reader {
        root: Option<Value>,
        open: Vec<Frame>,
        nodes: usize,
        refused: bool,
    }

    impl Reader {
        /// Place one finished node: as the root, as the next item of the open sequence,
        /// or as the key or the value of the open mapping.
        fn place(&mut self, node: Value) {
            self.nodes += 1;
            if self.nodes > MAX_TEMPLATE_NODES {
                self.refused = true;
                return;
            }
            let Some(frame) = self.open.last_mut() else {
                // A second document in the same stream: a template is one document.
                if self.root.is_some() {
                    self.refused = true;
                }
                self.root = Some(node);
                return;
            };
            match frame {
                Frame::Sequence { items, .. } => items.push(node),
                Frame::Mapping {
                    entries,
                    pending_key,
                    ..
                } => match pending_key.take() {
                    // A mapping key is a scalar; a template keys no section by a list or
                    // a map, and this reader invents no spelling for one.
                    None => match node {
                        Value::String(key) => *pending_key = Some(key),
                        _ => self.refused = true,
                    },
                    Some(key) => {
                        entries.insert(key, node);
                    }
                },
            }
        }

        fn open_frame(&mut self, frame: Frame) {
            if self.open.len() >= MAX_TEMPLATE_DEPTH {
                self.refused = true;
                return;
            }
            self.open.push(frame);
        }

        fn close_frame(&mut self) {
            let Some(frame) = self.open.pop() else {
                self.refused = true;
                return;
            };
            let (tag, node) = match frame {
                Frame::Sequence { tag, items } => (tag, Value::Array(items)),
                Frame::Mapping { tag, entries, .. } => (tag, Value::Object(entries)),
            };
            let node = match tag {
                Some(name) => long_form(name, node),
                None => node,
            };
            self.place(node);
        }
    }

    impl ::yaml_rust2::parser::EventReceiver for Reader {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            use ::yaml_rust2::parser::Event;
            if self.refused {
                return;
            }
            match event {
                Event::Scalar(value, _style, _anchor, tag) => {
                    let node = Value::String(value);
                    let node = match short_form_tag(tag.as_ref()) {
                        Some(name) => long_form(name, node),
                        None => node,
                    };
                    self.place(node);
                }
                Event::SequenceStart(_anchor, tag) => self.open_frame(Frame::Sequence {
                    tag: short_form_tag(tag.as_ref()),
                    items: Vec::new(),
                }),
                Event::MappingStart(_anchor, tag) => self.open_frame(Frame::Mapping {
                    tag: short_form_tag(tag.as_ref()),
                    entries: Map::new(),
                    pending_key: None,
                }),
                Event::SequenceEnd | Event::MappingEnd => self.close_frame(),
                Event::Alias(_) => self.refused = true,
                _ => {}
            }
        }
    }

    let mut reader = Reader::default();
    guard_template_nesting(body)?;
    ::yaml_rust2::parser::Parser::new_from_str(body)
        .load(&mut reader, true)
        .ok()?;
    if reader.refused || !reader.open.is_empty() {
        return None;
    }
    reader.root
}

/// Every intrinsic function a YAML template may write in its SHORT form, as the name that
/// form tags a node with.
///
/// Source: the CloudFormation "Intrinsic function reference" and "Condition functions",
/// which give each function's name and its short form; a conformance pack template is a
/// CloudFormation template, so it is written in that language. The sibling reading of
/// CloudFormation's own templates names the same set
/// (`services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs`).
const SHORT_FORM_FUNCTIONS: [&str; 20] = [
    "And",
    "Base64",
    "Cidr",
    "Condition",
    "Equals",
    "FindInMap",
    "GetAZs",
    "GetAtt",
    "If",
    "ImportValue",
    "Join",
    "Length",
    "Not",
    "Or",
    "Ref",
    "Select",
    "Split",
    "Sub",
    "ToJsonString",
    "Transform",
];

/// The intrinsic function a YAML tag names, when it names one.
///
/// A short form is a LOCAL tag (handle `!`) whose suffix is the function's own name:
/// `!Ref`, `!Sub`, `!GetAtt`, … A standard YAML tag (`!!str`, whose handle is `!!`) names
/// no function, and neither does a local tag whose suffix is not one of
/// [`SHORT_FORM_FUNCTIONS`] — such a node is read as the scalar or collection it is.
fn short_form_tag(tag: Option<&::yaml_rust2::parser::Tag>) -> Option<&'static str> {
    let tag = tag?;
    if tag.handle != "!" {
        return None;
    }
    SHORT_FORM_FUNCTIONS
        .into_iter()
        .find(|name| *name == tag.suffix)
}

/// One short-form intrinsic as the LONG form a template written as JSON spells: `!Ref x`
/// is `{"Ref": "x"}`, `!Condition c` is `{"Condition": "c"}`, and every other function is
/// `{"Fn::<Name>": …}`.
fn long_form(name: &str, node: ::serde_json::Value) -> ::serde_json::Value {
    let key = match name {
        REF_FUNCTION | CONDITION_FUNCTION => name.to_string(),
        other => format!("{FUNCTION_PREFIX}{other}"),
    };
    let mut wrapper = ::serde_json::Map::new();
    wrapper.insert(key, node);
    ::serde_json::Value::Object(wrapper)
}

/// The section of a template that declares the parameters the deployment supplies values
/// for, and the member of one declared parameter that gives the value to use when the
/// deployment supplies none. Both are CloudFormation's own spelling.
const TEMPLATE_PARAMETERS_SECTION: &str = "Parameters";
/// See [`TEMPLATE_PARAMETERS_SECTION`].
const TEMPLATE_PARAMETER_DEFAULT_MEMBER: &str = "Default";

/// The intrinsic function that stands for the value of what it names ("Ref returns the
/// value of the specified parameter or resource"), the one that names a condition, and the
/// prefix every other intrinsic function's long-form name carries.
const REF_FUNCTION: &str = "Ref";
/// See [`REF_FUNCTION`].
const CONDITION_FUNCTION: &str = "Condition";
/// See [`REF_FUNCTION`].
const FUNCTION_PREFIX: &str = "Fn::";
/// The intrinsic function that substitutes values into a string, which is the other place
/// a template names a parameter.
const SUB_FUNCTION: &str = "Fn::Sub";

/// The value each of the deployed template's parameters stands for: the value the request
/// gave it ("input parameters in the form of key-value pairs for the conformance pack",
/// `ConformancePackInputParameter`), or — for a parameter the request named no value for —
/// the `Default` the template itself declares, which is what a stack is deployed with when
/// a parameter is not supplied.
///
/// A parameter the request names that the template does not declare is kept all the same:
/// what a reference resolves to is the value given for the name it names, and this reading
/// does not make the template's parameter section the gatekeeper of that.
fn resolved_template_parameters(
    template: &::serde_json::Value,
    supplied: &[ConformancePackInputParameter],
) -> ::std::collections::BTreeMap<String, String> {
    let mut parameters: ::std::collections::BTreeMap<String, String> = template
        .get(TEMPLATE_PARAMETERS_SECTION)
        .and_then(::serde_json::Value::as_object)
        .map(|declared| {
            declared
                .iter()
                .filter_map(|(name, parameter)| {
                    Some((
                        name.clone(),
                        parameter
                            .get(TEMPLATE_PARAMETER_DEFAULT_MEMBER)?
                            .as_str()?
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    for parameter in supplied {
        parameters.insert(
            parameter.parameter_name.clone(),
            parameter.parameter_value.clone(),
        );
    }
    parameters
}

/// Replace every reference to a template parameter in `node` by the value that parameter
/// stands for ([`resolved_template_parameters`]), in place and everywhere in the tree.
///
/// A reference is the intrinsic node CloudFormation writes it as — `{"Ref": "<name>"}`, or
/// an `Fn::Sub` string naming parameters as `${<name>}` — so a template is resolved
/// wherever it names a parameter rather than only in the members this deployment happens
/// to read. A reference nothing resolves (a parameter the request gave no value for and
/// the template declares no default for, a pseudo parameter, a reference to a resource) is
/// LEFT AS IT IS: the declaration then carries no value for that member, which is what
/// keeps a deployment from inventing one.
///
/// Both of the ways this could grow with the request are bounded. The recursion walks the
/// tree the reader built, which nests at most [`MAX_TEMPLATE_DEPTH`] levels and carries at
/// most [`MAX_TEMPLATE_NODES`] nodes; and what the pass SUBSTITUTES across the whole tree
/// is capped at [`MAX_SUBSTITUTED_CHARS`], so the product of the references a body carries
/// and the length of the values they stand for cannot grow the resolved tree without end.
fn resolve_parameter_references(
    node: &mut ::serde_json::Value,
    parameters: &::std::collections::BTreeMap<String, String>,
) {
    let mut budget = MAX_SUBSTITUTED_CHARS;
    resolve_references_within(node, parameters, &mut budget);
}

/// The most characters ONE resolution pass may substitute into the template it resolves,
/// across every reference in that template.
///
/// A HOST bound on the amplification a request can ask for. Neither modelled length bounds
/// the resolved tree on its own: a `TemplateBody` is at most
/// [`TEMPLATE_BODY_MAX_CHARS`] characters, but each of the references it carries stands for
/// a parameter value of up to [`PARAMETER_VALUE_MAX_CHARS`] of its own, so it is their
/// PRODUCT that has to be capped. This sits at sixteen whole template bodies — orders of
/// magnitude above what a conformance pack template substitutes, which is a handful of
/// resource names and ARNs — and a reference the remaining budget would not cover is left
/// unresolved, so the member carries no value exactly as one naming a parameter nothing
/// resolves does.
const MAX_SUBSTITUTED_CHARS: usize = 16 * TEMPLATE_BODY_MAX_CHARS;

/// The longest `TemplateBody` this operation takes (`@length(1, 51200)`) and the longest
/// value a `ConformancePackInputParameter` carries for a parameter
/// (`ConformancePackInputParameter$ParameterValue`, `@length(0, 4096)`: "values can have a
/// maximum length of 4096 characters"). Both are the model's, and the boundary validator
/// enforces them before this handler runs; they are named here because
/// [`MAX_SUBSTITUTED_CHARS`] is a bound on what the two together can produce.
const TEMPLATE_BODY_MAX_CHARS: usize = 51_200;
/// See [`TEMPLATE_BODY_MAX_CHARS`].
const PARAMETER_VALUE_MAX_CHARS: usize = 4_096;

/// Resolve the references in `node`, spending from `budget` for every character substituted
/// ([`resolve_parameter_references`]).
fn resolve_references_within(
    node: &mut ::serde_json::Value,
    parameters: &::std::collections::BTreeMap<String, String>,
    budget: &mut usize,
) {
    match node {
        ::serde_json::Value::Object(members) => {
            if let Some(resolved) = resolved_reference(members, parameters, *budget) {
                *budget = budget.saturating_sub(resolved.len());
                *node = ::serde_json::Value::String(resolved);
                return;
            }
            for member in members.values_mut() {
                resolve_references_within(member, parameters, budget);
            }
        }
        ::serde_json::Value::Array(items) => {
            for item in items {
                resolve_references_within(item, parameters, budget);
            }
        }
        _ => {}
    }
}

/// The text one intrinsic node stands for, or `None` for a node that is not a reference to
/// a parameter this deployment has a value for — or whose value is longer than `budget`,
/// the characters this pass may still substitute.
///
/// An intrinsic function is a single-member object whose name is the function's, so a
/// mapping carrying anything beside it is a declaration rather than a reference.
fn resolved_reference(
    members: &::serde_json::Map<String, ::serde_json::Value>,
    parameters: &::std::collections::BTreeMap<String, String>,
    budget: usize,
) -> Option<String> {
    if members.len() != 1 {
        return None;
    }
    let (function, argument) = members.iter().next()?;
    let argument = argument.as_str()?;
    match function.as_str() {
        REF_FUNCTION => parameters
            .get(argument)
            .filter(|value| value.len() <= budget.min(PARAMETER_VALUE_MAX_CHARS))
            .cloned(),
        SUB_FUNCTION => substituted(argument, parameters, budget),
        _ => None,
    }
}

/// `text` with every `${<name>}` replaced by the value the parameter `<name>` stands for,
/// or `None` when it names one this deployment has no value for — the string is then left
/// unsubstituted rather than part-filled.
///
/// `${!Literal}` is how a template writes a literal `${Literal}`, so it substitutes
/// nothing and the text keeps the braces.
///
/// `budget` is the characters this resolution pass may still substitute
/// ([`MAX_SUBSTITUTED_CHARS`]): a text that would grow past it is left unsubstituted, so
/// one `Fn::Sub` cannot turn a short request into an unbounded string.
fn substituted(
    text: &str,
    parameters: &::std::collections::BTreeMap<String, String>,
    budget: usize,
) -> Option<String> {
    let mut substituted = String::new();
    let mut rest = text;
    while let Some(opener) = rest.find("${") {
        substituted.push_str(&rest[..opener]);
        let named = &rest[opener + 2..];
        // An opener with no closer is no substitution at all: the text is not the
        // substitution template it would have to be, so nothing is substituted into it.
        let closer = named.find('}')?;
        let name = &named[..closer];
        rest = &named[closer + 1..];
        match name.strip_prefix('!') {
            Some(literal) => substituted.push_str(&format!("${{{literal}}}")),
            None => substituted.push_str(parameters.get(name)?),
        }
        // Checked as the text grows, so a body naming one parameter a great many times
        // stops at the bound rather than being built first and judged after.
        if substituted.len() > budget {
            return None;
        }
    }
    substituted.push_str(rest);
    (substituted.len() <= budget).then_some(substituted)
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutConformancePackError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.configservice#PutConformancePack")]
#[derive(Debug)]
pub enum PutConformancePackErrorModeled {
    ConformancePackTemplateValidationException(ConformancePackTemplateValidationException),
    InsufficientPermissionsException(InsufficientPermissionsException),
    InvalidParameterValueException(InvalidParameterValueException),
    MaxNumberOfConformancePacksExceededException(MaxNumberOfConformancePacksExceededException),
    ResourceInUseException(ResourceInUseException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_conformance_pack::PutConformancePackError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutConformancePackError = crate::scaffolded::Scaffolded<PutConformancePackErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutConformancePackErrorModeled> for PutConformancePackError {
    fn from(modeled: PutConformancePackErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutConformancePack` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutConformancePack<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutConformancePack<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutConformancePack<Eng>
{
    type Input = PutConformancePackInput;
    type Output = PutConformancePackOutput;
    type Error = PutConformancePackError;

    const NAME: &'static str = "PutConformancePack";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_CONFORMANCE_PACK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_CONFORMANCE_PACK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            crate::scaffolded::Scaffolded::validation(crate::state::STATE_UNAVAILABLE)
        })?;

        // "For managed conformance packs, you must specify Source. Do not specify
        // TemplateS3Uri, TemplateBody, TemplateSSMDocumentDetails, DeliveryS3Bucket, or
        // DeliveryS3KeyPrefix": a `Source` is what makes the request a MANAGED
        // deployment — "if you specify a source, Config creates a managed conformance
        // pack" — and a managed pack is deployed from the collection that source names
        // rather than from a template of the account's own, so a request naming both
        // describes two deployments. Each member is optional on its own, so the request
        // walk cannot refuse the COMBINATION: this operation's own
        // `InvalidParameterValueException` ("One or more of the specified parameters are
        // not valid") is what refuses it, naming the members the request sent.
        let template_or_delivery = [
            ("TemplateS3Uri", input.template_s3_uri.is_some()),
            ("TemplateBody", input.template_body.is_some()),
            (
                "TemplateSSMDocumentDetails",
                input.template_ssm_document_details.is_some(),
            ),
            ("DeliveryS3Bucket", input.delivery_s3_bucket.is_some()),
            ("DeliveryS3KeyPrefix", input.delivery_s3_key_prefix.is_some()),
        ];
        if input.source.is_some() {
            let specified: Vec<&str> = template_or_delivery
                .iter()
                .filter(|(_, present)| *present)
                .map(|(member, _)| *member)
                .collect();
            if !specified.is_empty() {
                return Err(invalid_parameter(format!(
                    "When you specify Source, you cannot specify {}, and {} {} specified",
                    template_or_delivery
                        .iter()
                        .map(|(member, _)| *member)
                        .collect::<Vec<&str>>()
                        .join(", "),
                    specified.join(", "),
                    if specified.len() == 1 { "was" } else { "were" },
                )));
            }

            // What a `Source` names is a collection of Config rules Amazon Web Services
            // created and maintains, which this emulator holds none of, so the pack its
            // deployment would create has no rules to create: the scaffold's 501 is
            // answered for this leg rather than a pack reported deployed whose rules
            // exist nowhere. See the module doc.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "PutConformancePack with a managed conformance pack Source",
            ));
        }

        // "This field is optional and is supported only for managed conformance packs.
        // If you specify a scope, you must also specify Source": a scope defines which
        // resources trigger an evaluation for the rules of the MANAGED pack a source
        // names, so it stands for nothing on its own.
        if input.scope.is_some() {
            return Err(invalid_parameter(
                crate::state::CONFORMANCE_PACK_SCOPE_NEEDS_SOURCE_MESSAGE.to_string(),
            ));
        }

        // "For custom conformance packs, you must specify only one of TemplateS3Uri,
        // TemplateBody, TemplateSSMDocumentDetails." Each is optional on its own, so
        // the request walk cannot refuse a request that carries none or several: the
        // rule is about the COMBINATION, which this operation's own
        // `InvalidParameterValueException` is what refuses.
        let sources = [
            ("TemplateBody", input.template_body.is_some()),
            ("TemplateS3Uri", input.template_s3_uri.is_some()),
            (
                "TemplateSSMDocumentDetails",
                input.template_ssm_document_details.is_some(),
            ),
        ];
        let specified: Vec<&str> = sources
            .iter()
            .filter(|(_, present)| *present)
            .map(|(member, _)| *member)
            .collect();
        if specified.len() != 1 {
            return Err(invalid_parameter(format!(
                "You must specify only one of the following parameters: {}, and {} were \
                 specified",
                sources
                    .iter()
                    .map(|(member, _)| *member)
                    .collect::<Vec<&str>>()
                    .join(", "),
                if specified.is_empty() {
                    "none".to_string()
                } else {
                    specified.join(", ")
                },
            )));
        }

        // A template body this call carries is READ here; a template Config would have
        // to FETCH is not, so the two legs that name one answer the scaffold's 501
        // rather than deploying a pack from a template nothing read. See the module doc.
        if input.template_s3_uri.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "PutConformancePack with a TemplateS3Uri",
            ));
        }
        if input.template_ssm_document_details.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "PutConformancePack with a TemplateSSMDocumentDetails",
            ));
        }

        let template_body = input.template_body;
        // The values the template is deployed WITH: the request's own input parameters,
        // read before the template is, because a reference to one resolves as the
        // template's declarations are read.
        let input_parameters = input.conformance_pack_input_parameters.unwrap_or_default();
        // A template that cannot be deployed is refused BEFORE anything is stored, so
        // the refusal leaves no pack behind for this name; what the template DECLARES is
        // read in the same pass, because it is what the deployment creates.
        let declared = match template_body.as_deref() {
            Some(body) => {
                read_conformance_pack_template(body, &input_parameters).map_err(|message| {
                    PutConformancePackError::from(
                        PutConformancePackErrorModeled::ConformancePackTemplateValidationException(
                            ConformancePackTemplateValidationException {
                                message: Some(message),
                            },
                        ),
                    )
                })?
            }
            // The two legs that carry no body answered 501 above.
            None => Vec::new(),
        };

        let name = input.conformance_pack_name;
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();
        let completes_at = now + crate::state::CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER;

        // A deployment can only CREATE a resource this account could create directly, so
        // every resource the template declares is judged by the preconditions the
        // operation that creates that kind of resource enforces. For a Config rule those
        // are `PutConfigRule`'s own ([`crate::state::unusable_config_rule`]), and for a
        // remediation action `PutRemediationConfigurations`' own
        // ([`crate::state::unusable_remediation`]), asked of Systems Manager before the
        // store is locked because the target is another service's resource. A template
        // declaring one the account could not create deploys NOTHING and the pack settles
        // in the modelled `CREATE_FAILED` carrying this reason, rather than reporting a
        // deployment that created a resource no create would have accepted.
        let mut deployment_failure: Option<String> = None;
        for resource in &declared {
            let DeclaredResource::Rule(rule) = resource else {
                continue;
            };
            // Judged under the name the TEMPLATE declared the rule with, which is what the
            // reason has to name: the name Config gives a pack's rule is minted below, and
            // the template's author cannot fix a name they never wrote.
            if let Some(detail) =
                crate::state::unusable_config_rule(&rule.declared_name, rule.scope.as_ref())
            {
                deployment_failure =
                    Some(format!("The conformance pack could not be deployed: {detail}."));
                break;
            }
        }
        for resource in &declared {
            // A deployment that cannot be performed is reported by the FIRST resource that
            // cannot be created, so nothing further is asked of another service for it.
            if deployment_failure.is_some() {
                break;
            }
            let DeclaredResource::Remediation(remediation) = resource else {
                continue;
            };
            // Judged under the rule name the TEMPLATE declared the action for, which is
            // what the reason has to name: the name Config gives that rule is minted
            // below, and the template's author cannot fix a name they never wrote.
            let mut declared_for = remediation.requested.clone();
            declared_for.config_rule_name = remediation.rule_declared_name.clone();
            if let Some(detail) =
                crate::state::unusable_remediation(ctx.callback(), &region, &declared_for).await
            {
                deployment_failure =
                    Some(format!("The conformance pack could not be deployed: {detail}."));
                break;
            }
        }

        let mut store = state.lock();

        // "For PutConformancePack and PutOrganizationConformancePack, a conformance pack
        // creation, update, and deletion is in progress. Try your request again later."
        // (`ResourceInUseException`): the deployment this operation starts is still
        // running when the call answers, so a further call for a pack whose deployment is
        // in flight is refused rather than restarting work in progress. A pack whose
        // deployment has finished — complete, or failed — is redeployed as usual. The
        // refusal NAMES the pack whose workflow holds it, built from the request, so a
        // caller deploying several packs can tell which one is busy
        // ([`crate::state::conformance_pack_workflow_in_progress_message`]).
        if store
            .conformance_packs
            .get(&name)
            .is_some_and(|pack| {
                pack.state_at(now) == crate::state::CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS
            })
        {
            return Err(PutConformancePackErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    message: Some(crate::state::conformance_pack_workflow_in_progress_message(
                        &name,
                    )),
                },
            )
            .into());
        }

        // "Subsequent requests won't create a duplicate resource if one was already
        // created": a pack this account already holds is REDEPLOYED from the template
        // this call carries, keeping the identity (the id, the ARN and the stack) it was
        // created with and the tags it was created with ("Tags are added at creation and
        // cannot be updated with this operation"). Only a NEW pack has to fit under this
        // Region's conformance pack quota.
        let existing = store.conformance_packs.get(&name).cloned();

        // "You cannot convert an existing conformance pack between a managed conformance
        // pack and a custom conformance pack": which kind a pack is was fixed when it
        // was created, and this call deploys a CUSTOM pack from a template of the
        // account's own — a request naming a `Source` answered above — so a name an
        // existing MANAGED pack holds is refused and that pack is left exactly as it is.
        if existing
            .as_ref()
            .is_some_and(|pack| pack.pack_type == crate::state::CONFORMANCE_PACK_TYPE_MANAGED)
        {
            return Err(invalid_parameter(
                crate::state::CONFORMANCE_PACK_KIND_IS_FIXED_MESSAGE.to_string(),
            ));
        }

        let (id, arn, deployed_stack_arn, pack_type, tags, previously_deployed) = match existing {
            Some(pack) => (
                pack.id,
                pack.arn,
                pack.stack_arn,
                pack.pack_type,
                pack.tags,
                pack.deployed_rule_names,
            ),
            None => {
                if store.conformance_packs.len() >= crate::state::CONFORMANCE_PACKS_QUOTA {
                    return Err(
                        PutConformancePackErrorModeled::MaxNumberOfConformancePacksExceededException(
                            MaxNumberOfConformancePacksExceededException {
                                message: Some(format!(
                                    "this account already has the maximum of {} conformance \
                                     packs in this Region",
                                    crate::state::CONFORMANCE_PACKS_QUOTA,
                                )),
                            },
                        )
                        .into(),
                    );
                }
                // The id and the ARN are Config's to assign, never read off the request.
                // The pack's STACK is what its deployment creates, so it is minted with
                // the deployment below rather than here.
                let id = crate::state::mint_conformance_pack_id();
                (
                    id.clone(),
                    crate::state::conformance_pack_arn(&region, &name, &id),
                    None,
                    // This call is the account deploying a pack of its own, which is the
                    // provenance the pack reports from here on.
                    crate::state::CONFORMANCE_PACK_TYPE_CUSTOM.to_string(),
                    // The tags a create carries are this resource's tag set, reached by
                    // the tagging operations through the store's one ARN-to-tags path.
                    crate::state::tag_set(input.tags.as_deref().unwrap_or_default()),
                    Vec::new(),
                )
            }
        };

        // The deployment itself: the Config rules the template declares are CREATED in
        // this account, with the names Config gives a pack's rules, and the remediation
        // actions declared for them beside them. A pack whose rules would not fit under
        // this Region's Config rules quota cannot be deployed, which is a deployment
        // FAILURE the pack reports (`CREATE_FAILED` with the reason) rather than a
        // completion of work nothing performed — and such a deployment changes NOTHING,
        // so a failed redeploy leaves the pack's previous resources standing.
        let rules: Vec<&DeclaredRule> = declared
            .iter()
            .filter_map(|resource| match resource {
                DeclaredResource::Rule(rule) => Some(rule),
                DeclaredResource::Remediation(_) => None,
            })
            .collect();
        // The rules THIS pack already deployed are the ones a redeploy replaces, so they
        // do not count against the quota the newly declared ones have to fit under.
        let held_by_this_pack = previously_deployed
            .iter()
            .filter(|rule_name| store.config_rules.contains_key(*rule_name))
            .count();
        let held_by_others = store.config_rules.len().saturating_sub(held_by_this_pack);
        let mut deployed_rule_names = Vec::new();
        let mut failure_reason = deployment_failure;
        if failure_reason.is_none()
            && held_by_others + rules.len() > crate::state::CONFIG_RULES_QUOTA
        {
            failure_reason = Some(format!(
                "The conformance pack could not be deployed: creating the {} Config rule(s) it \
                 declares would take this account past the limit of {} Config rules in this \
                 Region.",
                rules.len(),
                crate::state::CONFIG_RULES_QUOTA,
            ));
        }
        if failure_reason.is_some() {
            // Nothing was created, so what the pack's previous deployment created is
            // still what it holds.
            deployed_rule_names = previously_deployed;
        } else {
            // The resources a previous deployment of THIS pack created are the ones it
            // replaces: the redeploy drops them before creating what the new template
            // declares, and touches nothing else in the account's rules.
            store.remove_conformance_pack_resources(&previously_deployed);
            for rule in rules {
                let rule_name = crate::state::conformance_pack_rule_name(&rule.declared_name, &id);
                let rule_id = crate::state::mint_config_rule_id();
                let rule_arn = crate::state::config_rule_arn(&region, &rule_id);
                let mut entry = crate::state::ConfigRuleEntry {
                    name: rule_name.clone(),
                    arn: rule_arn,
                    id: rule_id,
                    description: rule.description.clone(),
                    scope: rule.scope.clone(),
                    source: rule.source.clone(),
                    input_parameters: rule.input_parameters.clone(),
                    maximum_execution_frequency: rule.maximum_execution_frequency.clone(),
                    state: crate::state::CONFIG_RULE_STATE_ACTIVE.to_string(),
                    evaluation_modes: crate::state::default_evaluation_modes(),
                    rule_evaluation_visibility: Some(
                        crate::state::RULE_EVALUATION_VISIBILITY_EXTERNAL.to_string(),
                    ),
                    // The pack deployed this rule through Config's own
                    // service-linked role, which is what `CreatedBy` reports.
                    created_by: Some(crate::state::CONFORMANCE_PACK_SERVICE_PRINCIPAL.to_string()),
                    created_at: now,
                    // A pack's rule carries no tags of the caller's: the tags the
                    // request names are the PACK's.
                    tags: crate::state::TagSet::new(),
                    evaluation_results: ::std::collections::BTreeMap::new(),
                    // Config evaluates a rule as soon as it is created, with no call
                    // from the caller; the pass completes a span later.
                    evaluation: crate::state::RuleEvaluationEntry::invoked_at(now),
                };
                // The pass that evaluation is carries a `ResultToken` where the rule the
                // template declared is one whose verdicts are DELIVERED back to Config —
                // a Custom Lambda rule — exactly as the pass a `PutConfigRule` write
                // invokes does, so such a rule's own evaluator is invoked for it too.
                entry.mint_evaluation_result_token(now);
                store.config_rules.insert(rule_name.clone(), entry);
                deployed_rule_names.push(rule_name);
            }

            for resource in &declared {
                let DeclaredResource::Remediation(remediation) = resource else {
                    continue;
                };
                // The remediation configuration is keyed by the rule it remediates, as
                // the deployment named that rule.
                let rule_name = crate::state::conformance_pack_rule_name(
                    &remediation.rule_declared_name,
                    &id,
                );
                let remediation_id = crate::state::mint_remediation_configuration_id();
                let remediation_arn = crate::state::remediation_configuration_arn(
                    &region,
                    &rule_name,
                    &remediation_id,
                );
                let mut requested = remediation.requested.clone();
                requested.config_rule_name = rule_name.clone();
                store.remediation_configurations.insert(
                    rule_name,
                    crate::state::RemediationConfigurationEntry::new(
                        remediation_id,
                        remediation_arn,
                        &requested,
                    ),
                );
            }
        }

        // The pack's CloudFormation stack is what the deployment CREATES: a pack that
        // has one keeps it, a deployment that is performed for a pack that has none is
        // given one, and a deployment that could not be performed created no stack at all
        // — so a pack whose only deployment failed has none to report. The stack's own id
        // is CloudFormation's, which is why it is DRAWN rather than derived from the pack.
        let stack_arn = match deployed_stack_arn {
            Some(stack_arn) => Some(stack_arn),
            None if failure_reason.is_some() => None,
            None => Some(crate::state::conformance_pack_stack_arn(
                &region,
                &name,
                &id,
                &crate::state::stack_id(),
            )),
        };

        store.conformance_packs.insert(
            name.clone(),
            crate::state::ConformancePackEntry {
                name,
                id,
                arn: arn.clone(),
                stack_arn,
                pack_type,
                template_body,
                delivery_s3_bucket: input.delivery_s3_bucket,
                delivery_s3_key_prefix: input.delivery_s3_key_prefix,
                input_parameters,
                deployed_rule_names,
                failure_reason,
                last_update_requested_at: now,
                deployment_completes_at: completes_at,
                tags,
                // The managed pack's own members: this call deploys a pack from a
                // template the account wrote, which carries neither.
                source: None,
                scope: None,
            },
        );
        drop(store);

        // Config evaluates a rule it has just created, with no call from the caller, so
        // the rules this deployment created are evaluated by the in-process pump the way
        // `PutConfigRule`'s are — which is what gives the pack's own compliance surface
        // results to report.
        crate::state::drive_recorder_clock(ctx, &self.engine, &region);

        Ok(PutConformancePackOutput {
            conformance_pack_arn: Some(arn),
        })
    }
}

/// Refuse the request with this operation's own modelled `InvalidParameterValueException`
/// (HTTP 400), which is what Config answers a request whose parameters cannot hold
/// together. The message is BUILT FROM THE REQUEST.
fn invalid_parameter(message: String) -> PutConformancePackError {
    PutConformancePackErrorModeled::InvalidParameterValueException(InvalidParameterValueException {
        message: Some(message),
    })
    .into()
}
