// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOYMENT validation a stack operation's resources are held to: the property
//! values a resource type's own contract admits.
//!
//! ## The rule this module implements
//!
//! A stack operation runs "pre-deployment validations" unless the request turns them off
//! (`DisableValidation`, "set to true to disable pre-deployment validations in changeset or
//! stack operations", default `false`), and what those validations are FOR is catching a
//! declaration CloudFormation already knows cannot be deployed: the operation then fails
//! before the resource is provisioned, so no resource of the stack is created, changed or
//! asked for at all, and no resource-level event is recorded for it. A request that disables
//! them deploys the declaration as written and the same problem surfaces where the resource
//! is really created — as that resource's own failure, carrying the owning service's
//! refusal.
//!
//! ## What this stage can decide, and what it deliberately cannot
//!
//! CloudFormation validates a declaration against the resource TYPE'S SCHEMA, which its
//! registry holds for every type. This emulator holds no such schema: the one it derives for
//! a published type carries that type's RETURN VALUES and states no writable property, no
//! type and no value set at all ([`super::resource_schema`] says so in full), and the service
//! that owns a type is reachable only by CREATING the resource — there is no read that asks
//! it whether a declaration would be accepted. So this stage is NOT that schema check and
//! does not stand in for it: it decides the questions a type's published contract makes
//! decidable without asking anybody —
//!
//! * a property the contract marks REQUIRED that the declaration does not carry at all, over
//!   the rows this emulator holds ([`REQUIRED_PROPERTIES`]), which is the
//!   `Required property [<member>] not found` the stage reports
//!   ([`missing_property_reason`]);
//! * that SAME question asked of a structure NESTED inside a property the declaration states
//!   — a member the nested structure's own contract marks required that the structure does
//!   not carry — over the rows this emulator holds
//!   ([`nested_required_members::NESTED_REQUIRED_MEMBERS`]), which is the same sentence
//!   reported at the pointer of the structure that lacks the member;
//! * a member the contract states NO property for, over the types whose property set this
//!   emulator holds in full ([`declared_properties`]), which is the
//!   `Unsupported property [<member>]` the stage reports
//!   ([`declared_properties::unsupported_property_reason`]);
//! * a property whose contract states the TYPE of its value, carrying a scalar that spells no
//!   value of that type, over the rows this emulator holds
//!   ([`typed_properties::TYPED_PROPERTIES`]), which is the
//!   `Property validation failure: [Value of property {/<member>} does not match type
//!   {<type>}]` the stage reports ([`typed_properties::type_failure_reason`]);
//! * a property whose contract states a STRUCTURE for its value — a list, an object — carrying
//!   a value that is not one, over the rows this emulator holds
//!   ([`structured_properties::STRUCTURED_PROPERTIES`]), which is the
//!   `Property [<member>] expected type: <expected>, found: <actual>` the stage reports
//!   ([`structured_properties::type_mismatch_reason`]);
//! * a property whose contract publishes the values it admits, carrying a value that is not
//!   one of them, over the rows this emulator holds ([`CONSTRAINED_PROPERTIES`]);
//! * a property whose type's REGISTRY SCHEMA states the values it admits as an `enum`,
//!   carrying one it does not state — in a list-valued property, in any element of it — over
//!   the rows this emulator holds ([`enum_properties::ENUM_PROPERTIES`]), which is the
//!   schema validator's own `<value> is not a valid enum value. …`
//!   ([`enum_properties::enum_failure_reason`]);
//! * a property whose contract publishes the PATTERN its values match, carrying a value
//!   outside it, over the rows this emulator holds
//!   ([`patterned_properties::PATTERNED_PROPERTIES`]), which is the
//!   `Property value [<value>] does not match pattern: <pattern>` the stage reports
//!   ([`patterned_properties::pattern_failure_reason`]);
//! * a property whose type's REGISTRY SCHEMA states the closed RANGE its numbers lie in,
//!   carrying a number outside it, over the rows this emulator holds
//!   ([`ranged_properties::RANGED_PROPERTIES`]), which is the schema validator's own
//!   `#/<member>: <value> is not less or equal to <maximum>`
//!   ([`ranged_properties::maximum_failure_reason`]) and its counterpart for the other end of
//!   the range ([`ranged_properties::minimum_failure_reason`]).
//!
//! A resource is judged in that order, because a property that is not there has no value to
//! hold to a set, a member the type does not have has no set to hold one to and a value of the
//! wrong type is no value of a set or a spelling whatever it reads like, and the resources in
//! the order the deployment would have created them — so the failure reported is the one the
//! operation would have run into first.
//!
//! How much of a JUDGED VALUE a failure says is one rule for the whole stage
//! ([`reported_value`]): the questions whose answer names the value — the set its property
//! publishes, the spelling it admits and the range its numbers lie in — report the declaration's own value for a type whose
//! property document is the template's own text, and report it redacted for a type that carries
//! credentials among its properties ([`CREDENTIAL_PROPERTIES`]), whose document carries
//! material the template does not state in the clear.
//!
//! What the STACK reports for a failure of this stage is not one of those reasons but the
//! summary over them ([`validation_failure_summary`]): the count of errors, and that the
//! detail of each is on the operation's own events, which is where each failure is recorded.
//!
//! That makes the stage deliberately PARTIAL, and fail-open: a property no row covers, a type
//! with no row at all, and a value an expression produced that is not text are each left to
//! the service that owns the resource, which is the authority on everything this is not. So
//! it can only ever anticipate a refusal that service would have made, never invent one — and
//! a declaration it passes is still provisioned and still judged there. It becomes the real
//! schema check when this registry holds real schemas, or when an owning service serves a
//! would-this-be-accepted read; until then the rows are what it knows.
//!
//! A declaration that reaches provisioning missing a property its type requires — one of a
//! type this stage holds no row for, or any one at all when the request disabled this stage —
//! is reported by the PROVIDER's own model validation instead, in that validator's own
//! sentence and on the resource's own failed event, which is a stage of provisioning and lives
//! with it.
//!
//! The document a resource is judged against is the DEPLOYED one — the property document the
//! owning service would be asked to create the resource with — so a property whose value this
//! service could not evaluate at all counts as absent, because the create would carry it
//! absent too ([`super::template::resolve_properties`] drops such a property rather than
//! guessing a value for it).
//!
//! ## Where the value sets come from
//!
//! [`CONSTRAINED_PROPERTIES`] is a catalogue: one row per (resource type, property) whose
//! admitted values the type's own contract publishes, read off the "AWS resource and
//! property types reference" of the CloudFormation User Guide for that type and the API
//! member the property is passed to. It is deliberately a SET of rows rather than a schema
//! language: what this stage needs is the question "is this value one the property admits",
//! and a row answers exactly that.
//!
//! The set a row carries is the one the OWNING API publishes for the member the property
//! becomes, which is the authority on whether a create carrying it can succeed: a value
//! outside it is one the owning service would refuse, so the validation refuses it first —
//! and a value inside it is never refused here.
//!
//! ## Where the required properties come from
//!
//! [`REQUIRED_PROPERTIES`] is the same kind of catalogue for the other question: one row per
//! resource type, carrying the properties that reference publishes with "Required: Yes" for
//! it. Those are the members the type's owning API declares `@required` on the call that
//! creates the resource, LESS the name, which is required of the API and not of the template
//! — CloudFormation generates a name for a declaration that states none
//! ([`super::template::physical_name`]), so a template may always leave it out.

use ::serde_json::Value;

use super::StoredResource;

/// Which properties a type HAS at all, which is the other question its schema makes
/// decidable here.
pub mod declared_properties;

/// Which members a NESTED property structure requires, which is the required-property
/// question asked of the structures inside a declaration rather than of the document itself.
pub mod nested_required_members;

/// Which SPELLINGS a property admits, which is the other question its schema makes decidable
/// about a value.
pub mod patterned_properties;

/// How LONG a property's value may be, which is the question its schema's `minLength` and
/// `maxLength` make decidable about that value.
pub mod bounded_properties;

/// Which TYPE a property's value has to have, which is the first question its schema makes
/// decidable about that value.
pub mod typed_properties;

/// Which STRUCTURE a property's value has to have — a list, an object — which is that same
/// question for a property whose schema states no scalar type.
pub mod structured_properties;

/// The values a type's REGISTRY SCHEMA states for a property, and the sentence that schema's
/// own validator reports a value outside them with — including a LIST-valued property, whose
/// every element is held to the set.
pub mod enum_properties;

/// The closed RANGE a type's schema states for a property's number, and the sentence that
/// schema's own validator reports a number outside it with.
pub mod ranged_properties;

/// One resource type that carries CREDENTIALS among its properties: the members a declaration
/// supplies and no read of the owning service ever answers, because that service's own model
/// marks them `@sensitive`.
#[derive(Debug, Clone, Copy)]
pub struct CredentialProperties {
    /// The resource type, as a template declares it.
    pub resource_type: &'static str,
    /// Its properties the owning API holds write-only, in the order it publishes them.
    pub credentials: &'static [&'static str],
}

/// The types this stage knows to carry credentials among their properties.
///
/// Source for each row: the `@sensitive` members of the operation the type's declaration is
/// carried onto, which the type's registry schema keeps write-only for the same reason — a
/// value a client writes and nothing reads back.
///
/// What the rows decide is how much of a judged value a FAILURE of this stage may say
/// ([`reported_value`]): the property document of a resource of such a type carries material
/// the author did not put in the template in the clear (a credential resolved for the
/// deployment), so a failure on any property of it names the property and the condition and
/// reports the value itself as [`REDACTED_VALUE`], while a type whose whole document is the
/// template's own text has its value quoted back.
pub const CREDENTIAL_PROPERTIES: &[CredentialProperties] = &[
    // `AWS::BackupGateway::Hypervisor` carries the hypervisor's own sign-in: `Username` and
    // `Password` are `@sensitive` members of `ImportHypervisorConfiguration` and of
    // `UpdateHypervisor`, and no read of Backup Gateway reports either back
    // ([`super::provisioner::backup_gateway_hypervisors`] leaves both out of its drift read
    // for that reason).
    CredentialProperties {
        resource_type: crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
        credentials: &["Password", "Username"],
    },
];

/// What a failure of this stage reports in place of a value it may not disclose.
pub const REDACTED_VALUE: &str = "<REDACTED>";

/// The value one failure of this stage reports for a declaration of `resource_type`: the
/// judged value itself, or [`REDACTED_VALUE`] for a type whose properties carry credentials
/// ([`CREDENTIAL_PROPERTIES`]).
///
/// The ONE spelling of that rule for this stage, so the questions whose answer quotes a
/// value — the set a property publishes ([`enum_properties`]), the spelling it admits
/// ([`patterned_properties`]) and the range its numbers lie in ([`ranged_properties`]) —
/// cannot disagree about which values may be said out loud.
pub fn reported_value<'a>(resource_type: &str, judged: &'a str) -> &'a str {
    if carries_credentials(resource_type) {
        return REDACTED_VALUE;
    }
    judged
}

/// Whether `resource_type` carries credentials among its properties
/// ([`CREDENTIAL_PROPERTIES`]).
fn carries_credentials(resource_type: &str) -> bool {
    CREDENTIAL_PROPERTIES
        .iter()
        .any(|row| row.resource_type == resource_type)
}

/// One property whose admitted values its type's contract publishes.
#[derive(Debug, Clone, Copy)]
pub struct ConstrainedProperty {
    /// The resource type the property belongs to, as a template declares it.
    pub resource_type: &'static str,
    /// The property's name, as a template spells it.
    pub property: &'static str,
    /// The values the property admits, in the order the contract publishes them.
    pub admitted: &'static [&'static str],
}

impl ConstrainedProperty {
    /// Whether this row's property ADMITS the value `declared` states for it.
    ///
    /// The one reading of a published value set, so the pre-deployment stage here and the
    /// resource provider's own model validation ([`super::provisioning`]'s
    /// `model_validation`) decide membership identically and can differ only in what they do
    /// about it — one refuses the stack operation, the other fails the resource.
    ///
    /// A value that is not TEXT is admitted: it is no value of a published set either way (an
    /// expression this service left unevaluated, a structure the author wrote where the
    /// schema states a string), and what it fails is a constraint neither stage judges, so it
    /// is left to the service that owns the resource.
    pub fn admits(&self, declared: &Value) -> bool {
        declared
            .as_str()
            .is_none_or(|declared| self.admitted.contains(&declared))
    }
}

/// The properties this stage knows a published value set for.
///
/// Source for each row: the resource type's entry in the "AWS resource and property types
/// reference" of the CloudFormation User Guide, whose property documentation publishes the
/// allowed values, and the API member that property is passed to
/// ([`super::provisioner`] is where each type's mapping onto that member lives).
pub const CONSTRAINED_PROPERTIES: &[ConstrainedProperty] = &[
    // `AWS::SSM::Parameter`'s `Type` is `PutParameter$Type`, "the type of parameter", whose
    // published values are the two a TEMPLATE may ask for: "Allowed values: String |
    // StringList", and "CloudFormation doesn't support creating a SecureString parameter
    // type" — so the encrypted type Systems Manager's own API admits is not one this resource
    // type publishes, however a parameter of it is held in Parameter Store itself.
    ConstrainedProperty {
        resource_type: "AWS::SSM::Parameter",
        property: "Type",
        admitted: &["String", "StringList"],
    },
    // ...and its `Tier` is `PutParameter$Tier`, "the parameter tier to assign to a
    // parameter".
    ConstrainedProperty {
        resource_type: "AWS::SSM::Parameter",
        property: "Tier",
        admitted: &["Standard", "Advanced", "Intelligent-Tiering"],
    },
    // `AWS::DynamoDB::Table`'s `BillingMode` is `CreateTable$BillingMode`, "controls how
    // you are charged for read and write throughput".
    ConstrainedProperty {
        resource_type: "AWS::DynamoDB::Table",
        property: "BillingMode",
        admitted: &["PROVISIONED", "PAY_PER_REQUEST"],
    },
    // ...and its `TableClass` is `CreateTable$TableClass`, "the table class of the new
    // table".
    ConstrainedProperty {
        resource_type: "AWS::DynamoDB::Table",
        property: "TableClass",
        admitted: &["STANDARD", "STANDARD_INFREQUENT_ACCESS"],
    },
    // `AWS::S3::Bucket`'s `AccessControl` is the canned ACL `CreateBucket$ACL` takes, "a
    // canned access control list (ACL) that grants predefined permissions to the bucket".
    ConstrainedProperty {
        resource_type: "AWS::S3::Bucket",
        property: "AccessControl",
        admitted: &[
            "AuthenticatedRead",
            "AwsExecRead",
            "BucketOwnerFullControl",
            "BucketOwnerRead",
            "LogDeliveryWrite",
            "Private",
            "PublicRead",
            "PublicReadWrite",
        ],
    },
    // `AWS::StepFunctions::StateMachine`'s `StateMachineType` is
    // `CreateStateMachine$type`, "determines whether a Standard or an Express workflow is
    // created".
    ConstrainedProperty {
        resource_type: "AWS::StepFunctions::StateMachine",
        property: "StateMachineType",
        admitted: &["STANDARD", "EXPRESS"],
    },
    // `AWS::Lambda::Function`'s `PackageType` is `CreateFunction$PackageType`, "the type of
    // deployment package".
    ConstrainedProperty {
        resource_type: "AWS::Lambda::Function",
        property: "PackageType",
        admitted: &["Zip", "Image"],
    },
    // `AWS::EC2::VPC`'s `InstanceTenancy` is `CreateVpc$InstanceTenancy`, "the tenancy
    // options for instances launched into the VPC".
    ConstrainedProperty {
        resource_type: "AWS::EC2::VPC",
        property: "InstanceTenancy",
        admitted: &["default", "dedicated", "host"],
    },
];

/// One resource type's REQUIRED properties: the ones a declaration of that type cannot be
/// deployed without.
#[derive(Debug, Clone, Copy)]
pub struct RequiredProperties {
    /// The resource type the row is about, as a template declares it.
    pub resource_type: &'static str,
    /// The properties its contract marks required, in the order the contract publishes them.
    pub required: &'static [&'static str],
}

/// The types this stage knows the REQUIRED properties of.
///
/// Source for each row: the resource type's entry in the "AWS resource and property types
/// reference" of the CloudFormation User Guide — every property it publishes with
/// "Required: Yes", in the order that reference lists a type's properties (alphabetically)
/// — which are the `@required` members of the call the owning API creates such a resource
/// with, less the name CloudFormation itself may generate (see the module docs). That order
/// is the order a declaration missing SEVERAL of them is reported in, and reading it off the
/// reference is what keeps that order the contract's rather than this catalogue's.
pub const REQUIRED_PROPERTIES: &[RequiredProperties] = &[
    // `AWS::Pipes::Pipe`: `CreatePipe` declares `Source`, `Target` and `RoleArn` required
    // beside the `Name` CloudFormation generates, so a pipe cannot be created without the
    // source it reads, the target it delivers to and the role it acts as.
    RequiredProperties {
        resource_type: "AWS::Pipes::Pipe",
        required: &["RoleArn", "Source", "Target"],
    },
    // `AWS::Events::EventBusPolicy`: the resource IS one grant on a bus's resource policy,
    // addressed by the `StatementId` that identifies it — "each StatementId must be unique",
    // and `RemovePermission` revokes the statement that id names — so a declaration stating
    // none describes no grant that could be installed or ever removed. The bus itself is not
    // required: a declaration naming none is about the account's default bus.
    RequiredProperties {
        resource_type: crate::state::template::EVENTS_EVENT_BUS_POLICY,
        required: &["StatementId"],
    },
    // `AWS::SSM::Parameter`: `PutParameter` declares `Name`, `Value` and (for a new
    // parameter) `Type` required; the name is CloudFormation's to generate, the other two
    // are the template's.
    RequiredProperties {
        resource_type: "AWS::SSM::Parameter",
        required: &["Type", "Value"],
    },
    // `AWS::SSM::Document`: `CreateDocument` declares `Content` required beside the `Name`
    // CloudFormation generates — a document is the content it carries — so a declaration
    // stating none declares no document.
    RequiredProperties {
        resource_type: crate::state::provisioner::ssm_documents::SSM_DOCUMENT,
        required: &["Content"],
    },
    // `AWS::SNS::Subscription`: `Subscribe` declares `TopicArn` and `Protocol` required —
    // there is no subscription without the topic it is on and the protocol it is delivered
    // over.
    RequiredProperties {
        resource_type: "AWS::SNS::Subscription",
        required: &["Protocol", "TopicArn"],
    },
    // `AWS::SQS::QueuePolicy` and `AWS::SNS::TopicPolicy`: each resource IS the policy the
    // declaration states, on the queues or topics it names, so both parts are required.
    RequiredProperties {
        resource_type: "AWS::SQS::QueuePolicy",
        required: &["PolicyDocument", "Queues"],
    },
    RequiredProperties {
        resource_type: "AWS::SNS::TopicPolicy",
        required: &["PolicyDocument", "Topics"],
    },
    // `AWS::S3::BucketPolicy`: the same case for the bucket whose policy it is
    // (`PutBucketPolicy` takes the bucket and the document).
    RequiredProperties {
        resource_type: "AWS::S3::BucketPolicy",
        required: &["Bucket", "PolicyDocument"],
    },
    // `AWS::Scheduler::Schedule`: `CreateSchedule` declares `FlexibleTimeWindow`,
    // `ScheduleExpression` and `Target` required beside the name CloudFormation generates.
    RequiredProperties {
        resource_type: "AWS::Scheduler::Schedule",
        required: &["FlexibleTimeWindow", "ScheduleExpression", "Target"],
    },
    // `AWS::EC2::SecurityGroup`: `CreateSecurityGroup` declares `GroupDescription`
    // required ("you must provide a description"), the group's own name being
    // CloudFormation's to generate.
    RequiredProperties {
        resource_type: "AWS::EC2::SecurityGroup",
        required: &["GroupDescription"],
    },
    // `AWS::EC2::Subnet`: a subnet exists IN a VPC, which `CreateSubnet` declares
    // required.
    RequiredProperties {
        resource_type: "AWS::EC2::Subnet",
        required: &["VpcId"],
    },
    // `AWS::Logs::LogStream`: a stream exists IN a log group, which `CreateLogStream`
    // declares required beside the stream name CloudFormation generates.
    RequiredProperties {
        resource_type: "AWS::Logs::LogStream",
        required: &["LogGroupName"],
    },
    // `AWS::MediaConvert::CertificateAssociation`: the resource IS the certificate the
    // account's MediaConvert service trusts, and `AssociateCertificate` declares that `Arn`
    // required — a declaration stating none describes no association at all.
    RequiredProperties {
        resource_type: crate::state::template::MEDIACONVERT_CERTIFICATE_ASSOCIATION,
        required: &["Arn"],
    },
    // `AWS::ECR::PullTimeUpdateExclusion`: the resource IS the IAM principal the private
    // registry excludes from pull-time recording, and `RegisterPullTimeUpdateExclusion`
    // declares that `principalArn` required — a declaration stating none describes no
    // exclusion at all.
    RequiredProperties {
        resource_type: crate::state::template::ECR_PULL_TIME_UPDATE_EXCLUSION,
        required: &["PrincipalArn"],
    },
    // `AWS::SSMQuickSetup::ConfigurationManager`: a configuration manager IS the deployment
    // of its definitions — `CreateConfigurationManager` declares `ConfigurationDefinitions`
    // required, and the name it also takes is the one CloudFormation may generate — so a
    // declaration stating no definition describes nothing to deploy.
    RequiredProperties {
        resource_type: crate::state::template::SSM_QUICKSETUP_CONFIGURATION_MANAGER,
        required: &["ConfigurationDefinitions"],
    },
    // `AWS::AIOps::InvestigationGroup`: an investigation group gathers its investigation data
    // AS a role — `CreateInvestigationGroup` declares `roleArn` required beside the `name` that
    // call also takes, which is the one CloudFormation may generate — so a declaration stating
    // no role describes a group that could investigate nothing.
    RequiredProperties {
        resource_type: crate::state::template::AIOPS_INVESTIGATION_GROUP,
        required: &["RoleArn"],
    },
];

/// One group of properties a resource type's schema admits AT MOST ONE of: the `oneOf` that
/// schema states over them.
#[derive(Debug, Clone, Copy)]
pub struct ExclusiveProperties {
    /// The resource type the group belongs to, as a template declares it.
    pub resource_type: &'static str,
    /// The properties that exclude each other, in the order the contract publishes them.
    pub exclusive: &'static [&'static str],
}

/// The types this stage knows a MUTUALLY EXCLUSIVE group of properties of.
///
/// Source for each row: the resource type's entry in the "AWS resource and property types
/// reference" of the CloudFormation User Guide, which publishes the exclusion in words
/// beside the properties it is about, and which the type's registry schema states as a
/// `oneOf` over them.
///
/// The two Amazon Route 53 record types are the rows here: each publishes `HostedZoneId` and
/// `HostedZoneName` with "Specify either `HostedZoneName` or `HostedZoneId`, but not both" —
/// a record set is written in ONE zone, named either by the id Route 53 knows it by or by its
/// DNS name, and a declaration stating both names no one zone to write it in.
pub const EXCLUSIVE_PROPERTIES: &[ExclusiveProperties] = &[
    ExclusiveProperties {
        resource_type: "AWS::Route53::RecordSet",
        exclusive: &["HostedZoneId", "HostedZoneName"],
    },
    ExclusiveProperties {
        resource_type: "AWS::Route53::RecordSetGroup",
        exclusive: &["HostedZoneId", "HostedZoneName"],
    },
];

/// The name this service reports for the pre-deployment validation that holds a
/// declaration's properties to what its type's contract requires and admits.
///
/// `OperationEvent$ValidationName` is "the name of the validation that was performed", and a
/// validation of this service is named for WHAT it validated, in the form the other one this
/// service performs is named in ([`super::early_validation::NAME_CONFLICT_VALIDATION`], the
/// existence check's own name).
pub const PROPERTY_VALIDATION: &str = "PROPERTY_VALIDATION";

/// What a failed pre-deployment property validation is: the resource it failed on, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyFailure {
    /// The logical id of the resource whose declaration failed the validation.
    pub logical_id: String,
    /// That resource's declared type.
    pub resource_type: String,
    /// Why it failed: which property is missing ([`missing_property_reason`]), which member
    /// the type does not have
    /// ([`declared_properties::unsupported_property_reason`]), or which property carries a
    /// value its type does not admit ([`failure_reason`]).
    pub reason: String,
    /// `OperationEvent$ValidationPath`: "the path within the resource where the validation
    /// was applied" — the element of the submitted template this validation ran on
    /// ([`properties_path`], or the property itself where that is what was judged,
    /// [`declared_properties::property_path`]).
    pub path: String,
}

/// The path a property validation is applied at: the `Properties` of the resource the
/// template declares under `logical_id`.
///
/// A resource's properties are what this stage judges, and a template's resources live
/// under `Resources` keyed by their logical id with their property document under
/// `Properties` — so the element judged is that one, named as a path into the submitted
/// template rather than taken from the request.
pub fn properties_path(logical_id: &str) -> String {
    format!(
        "{}/{PROPERTIES_SECTION}",
        super::early_validation::resource_path(logical_id)
    )
}

/// The member of a template's resource declaration that carries its property document,
/// which the path above steps through.
const PROPERTIES_SECTION: &str = "Properties";

/// Every value a declaration carries AT a member chain, each with the path it sits at under
/// the resource's property document — the chain's own members, with the INDEX of an element
/// where the chain stepped through a LIST.
///
/// The ONE walk of this stage that steps a property document down a chain of members over
/// every element of every list it passes through, so the questions asked of a nested value —
/// the spelling it is held to ([`patterned_properties`]), the length bounds it is held to
/// ([`bounded_properties`]) and the members the structure it is must carry
/// ([`nested_required_members`]) — cannot disagree about where a judged value sits, nor about
/// how many of them one declaration is judged on.
///
/// The values come out in declaration order, which is the order the stage reports the first
/// failure from, and at most [`MAX_JUDGED_VALUES`] of them. A member the declaration does not
/// carry and a chain that runs into a value no member can be read off contribute nothing:
/// neither is a value the chain names.
///
/// What the walk may SPEND is bounded too ([`MAX_WALKED_VALUES`]): a chain stepping through a
/// list at more than one of its steps visits the PRODUCT of those lists' lengths, every one of
/// which the submitted template states, and a chain whose last member is absent everywhere
/// produces no value to count while visiting them all. So the budget is charged per value
/// visited rather than per value produced, and a walk that spends it answers with what it found
/// so far — which is the fail-open reading a value this stage cannot reach gets anyway.
pub(super) fn judged_at<'a>(
    document: &'a Value,
    chain: &[&str],
) -> Vec<(Vec<String>, &'a Value)> {
    let mut found = Vec::new();
    let mut budget = MAX_WALKED_VALUES;
    collect(document, chain, &mut Vec::new(), &mut found, &mut budget);
    found
}

/// The walk behind [`judged_at`]: one step of the chain per call, over each element where the
/// declaration states a LIST at that step.
///
/// The recursion is as deep as the chain is long and no deeper — a step reads at most one list
/// level, so a list nested directly inside a list yields no value rather than another frame,
/// and nothing a caller's template states decides how far this descends. The walk also STOPS
/// at [`MAX_JUDGED_VALUES`] values produced and at [`MAX_WALKED_VALUES`] values visited, so
/// neither does anything a template states decide how many values come out of it, nor how much
/// work one request buys.
fn collect<'a>(
    document: &'a Value,
    chain: &[&str],
    at: &mut Vec<String>,
    found: &mut Vec<(Vec<String>, &'a Value)>,
    budget: &mut usize,
) {
    for (index, element) in elements_of(document) {
        if found.len() >= MAX_JUDGED_VALUES || *budget == 0 {
            return;
        }
        *budget -= 1;
        if let Some(index) = index {
            at.push(index.to_string());
        }
        match chain.split_first() {
            None => found.push((at.clone(), element)),
            Some((member, rest)) => {
                if let Some(nested) = element.get(member) {
                    at.push((*member).to_string());
                    collect(nested, rest, at, found, budget);
                    at.pop();
                }
            }
        }
        if index.is_some() {
            at.pop();
        }
    }
}

/// The values one step of a chain is taken over: the elements of a LIST the declaration states
/// there, each with its own index, and otherwise the value itself with no index at all.
///
/// At most [`MAX_WALKED_VALUES`] of them, which is all one walk may visit in total, so the list
/// a step is taken over costs a bounded amount to lay out however long the template states it.
fn elements_of(document: &Value) -> Vec<(Option<usize>, &Value)> {
    match document {
        Value::Array(elements) => elements
            .iter()
            .enumerate()
            .take(MAX_WALKED_VALUES)
            .map(|(index, element)| (Some(index), element))
            .collect(),
        value => vec![(None, value)],
    }
}

/// The most values ONE declaration may have judged against ONE row of a catalogue of this
/// stage.
///
/// A list-valued property is judged element by element, and how many elements it carries is
/// the template's to say — so the number of values one operation can ask to have judged is
/// bounded here rather than left to the document. A value past it is left ADMITTED, which is
/// the treatment every value this stage cannot decide gets: it is still judged where the
/// resource is really created, by the service that owns it.
pub(super) const MAX_JUDGED_VALUES: usize = 128;

/// The most values ONE walk of a member chain may VISIT, whether or not they are values the
/// chain names.
///
/// A chain that steps through a list at more than one of its steps reaches the PRODUCT of those
/// lists' lengths, and a chain whose last member is absent reaches none of its values while
/// visiting all of those — so the walk is charged for every value it looks at, not only for the
/// ones it produces, and what one request can make this stage spend is a constant rather than
/// something the submitted template chooses. Chosen far above the structures any template of a
/// real estate states under one property (a rule's targets, a task's tags) and small enough
/// that the worst case this bound permits stays a fraction of a millisecond of work. A value
/// past it is left ADMITTED, as every value this stage cannot decide about is.
///
/// The budget is ONE WALK's, and a question that holds a declaration to several rows walks each
/// of them, so what one declaration of a type with `n` rows may have visited is `n` times this
/// — a product of this constant and a catalogue's own length, and of nothing a template states.
pub(super) const MAX_WALKED_VALUES: usize = 4_096;

/// The path ONE MEMBER of a declaration's property document sits at: the chain of members
/// leading to it, under the `Properties` of the resource the template declares.
///
/// `OperationEvent$ValidationPath` is "the path within the resource where the validation was
/// applied", and a validation applied to one member names that member rather than the document
/// as a whole — so the path steps through every member above it, which is the JSON pointer of
/// the judged element in the submitted template
/// (`/Resources/<LogicalId>/Properties/<Property>[/<Member>…]`). The ONE spelling of that rule
/// for this stage: the questions that judge a member of the document itself
/// ([`declared_properties::property_path`]) and one nested inside a structured property
/// ([`enum_properties`]) both build their path here.
pub fn member_path(logical_id: &str, property: &[&str]) -> String {
    format!(
        "{}/{}",
        properties_path(logical_id),
        property.join("/")
    )
}

/// Every error this stage reports for `declared`: the errors of the FIRST resource whose
/// property document its type's contract refuses — what the operation reports instead of
/// deploying anything; empty when every resource passes what this stage can judge.
///
/// The resources are walked in the order the deployment would have created them, so the
/// declaration reported on is the one the operation would have run into first
/// ([`resource_failures`] is what each one is judged by, and in which order).
///
/// A schema validation reports ONE error per violation it found, which is why this answers a
/// LIST: a declaration missing several required members is as many errors as there are members
/// missing, each recorded as its own validation result and counted by the summary the stack
/// carries ([`validation_failure_summary`]).
///
/// How many it reports is bounded by [`MAX_REPORTED_FAILURES`], so a document cannot decide
/// how many validation results one operation records.
pub fn failures(declared: &[StoredResource]) -> Vec<PropertyFailure> {
    let mut failures: Vec<PropertyFailure> = declared
        .iter()
        .map(resource_failures)
        .find(|failures| !failures.is_empty())
        .unwrap_or_default();
    failures.truncate(MAX_REPORTED_FAILURES);
    failures
}

/// The most errors ONE operation is refused with.
///
/// A declaration states how many structures this stage judges, and each of them can be missing
/// every member its contract requires — so how many validation results an operation records is
/// bounded here rather than by the template, as the values judged against one row of a
/// catalogue are ([`MAX_JUDGED_VALUES`]). The operation is refused either way: what the bound
/// decides is how much of the diagnosis one request buys, and the errors kept are the first
/// ones, which are the ones a template author fixes first.
pub const MAX_REPORTED_FAILURES: usize = 128;

/// The FIRST error of [`failures`], for a caller that only asks whether this stage refuses the
/// declaration and on what.
pub fn failure(declared: &[StoredResource]) -> Option<PropertyFailure> {
    failures(declared).into_iter().next()
}

/// Every error one resource's property document is refused with; empty for a declaration that
/// passes what this stage can judge.
///
/// A resource is judged in the order its type's schema states its conditions: that the
/// document carries every property the type REQUIRES (a property that is not there has no
/// value to hold to a set) and that every structure it states inside one carries the members
/// that structure requires ([`required_members`] asks both), then that every member it
/// does carry is one the type HAS
/// ([`declared_properties`]), then the `oneOf` over those members, and last each value
/// against what its property states about a value — the STRUCTURE it states
/// ([`structured_properties`]) and the scalar TYPE it states
/// ([`typed_properties`]), then the set it publishes (the one this emulator knows from the
/// type's property reference, [`CONSTRAINED_PROPERTIES`], and the one its registry schema
/// states as an `enum`, [`enum_properties`]), then the length bounds it states
/// ([`bounded_properties`]), the pattern it admits ([`patterned_properties`]) and the numeric
/// range it admits ([`ranged_properties`]), since a value of another type is no value of a
/// published set and a value outside such a set is not one of its values whatever it is spelled
/// like — while the spelling a text is held to and the bound a number is held to are questions
/// about values of two different types, so neither can stand in for the other, and a text
/// outside its LENGTH bounds is reported as that rather than as a spelling, which is the verdict
/// a schema validator reports for a value that is both too short and mis-spelled.
///
/// Only the PRESENCE question answers every error it found; the questions about a VALUE answer
/// the first value they refuse, as this emulator knows them one row at a time.
fn resource_failures(resource: &StoredResource) -> Vec<PropertyFailure> {
    let missing = required_members(resource);
    if !missing.is_empty() {
        return missing;
    }
    declared_properties::undeclared_property(resource)
        .or_else(|| matched_several_subschemas(resource))
        .or_else(|| structured_properties::value_of_another_structure(resource))
        .or_else(|| typed_properties::value_of_another_type(resource))
        .or_else(|| inadmissible_value(resource))
        .or_else(|| enum_properties::value_outside_its_enum(resource))
        .or_else(|| bounded_properties::value_outside_its_length(resource))
        .or_else(|| patterned_properties::value_outside_its_pattern(resource))
        .or_else(|| ranged_properties::value_outside_its_range(resource))
        .into_iter()
        .collect()
}

/// Every member a declaration is REQUIRED to carry and does not: the properties its type's
/// contract requires of the document itself ([`missing_required_properties`]), then the
/// members the structures it states inside those properties require
/// ([`nested_required_members`]).
///
/// One question, asked of the document and of every structure in it, so a declaration missing
/// two of them is reported as the two errors it is — at the pointer of whichever structure each
/// member is missing from. The document's own properties come first, because a property that is
/// not there states no structure for the other half of the question to judge.
fn required_members(resource: &StoredResource) -> Vec<PropertyFailure> {
    let mut missing = missing_required_properties(resource);
    missing.extend(nested_required_members::missing_nested_required_members(
        resource,
    ));
    missing
}

/// How a stack operation reports, ON THE STACK, that its resources did not pass this stage:
/// how many errors the stage found, and that the detail of each one is on the operation's own
/// events.
///
/// A failed schema validation reports ONE error per violation it found, so a declaration
/// missing SEVERAL required members is that many errors, and each error's detail — the
/// resource, the property and the path — is recorded as the validation result it is
/// ([`PropertyFailure`]), which `DescribeEvents` reports. So what the STACK carries is the
/// summary that counts them and says where to read them, rather than one of the reasons
/// standing in for all of them.
///
/// The one variable part is the count, which is how many failures this stage reported for the
/// operation — the length of `failures`, since [`failures`] reports every error of the FIRST
/// declaration the stage refuses and the operation is then refused on it.
///
/// The sentence itself is the one EVERY pre-deployment validation stage reports on the stack
/// ([`super::early_validation::validation_failure_summary`]), so this stage and the existence
/// check beside it cannot word the same refusal differently.
pub fn validation_failure_summary(failures: &[PropertyFailure]) -> String {
    super::early_validation::validation_failure_summary(failures.len())
}

/// `Some(failure)` for a resource carrying MORE THAN ONE of a group of properties its type's
/// schema admits at most one of ([`EXCLUSIVE_PROPERTIES`]); `None` for a type with no row,
/// and for one carrying at most one member of every group it has.
///
/// A `oneOf` is satisfied by exactly one of its subschemas, so a declaration carrying two of
/// the properties matches two of them — which is what the schema validator reports
/// ([`matched_subschemas_reason`]), and why the count is the one variable part of that
/// sentence.
fn matched_several_subschemas(resource: &StoredResource) -> Option<PropertyFailure> {
    EXCLUSIVE_PROPERTIES
        .iter()
        .filter(|row| row.resource_type == resource.resource_type)
        .find_map(|row| {
            let matched = row
                .exclusive
                .iter()
                .filter(|property| resource.properties.get(*property).is_some())
                .count();
            (matched > 1).then(|| PropertyFailure {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                reason: matched_subschemas_reason(matched),
                path: properties_path(&resource.logical_id),
            })
        })
}

/// How CloudFormation reports a declaration that satisfies more than one subschema of a
/// `oneOf` its type's schema states: the schema validator's own sentence, whose one variable
/// part is HOW MANY of them the document matched.
pub fn matched_subschemas_reason(matched: usize) -> String {
    format!("#: #: {matched} subschemas matched instead of one")
}

/// Every property a resource's declaration does not carry that its type's contract requires
/// ([`REQUIRED_PROPERTIES`]), in the order the contract publishes them; empty for a type with
/// no row, and for one carrying every property its row names.
fn missing_required_properties(resource: &StoredResource) -> Vec<PropertyFailure> {
    REQUIRED_PROPERTIES
        .iter()
        .filter(|row| row.resource_type == resource.resource_type)
        .flat_map(|row| row.required)
        .filter(|property| resource.properties.get(*property).is_none())
        .map(|missing| PropertyFailure {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            reason: missing_property_reason(missing),
            path: properties_path(&resource.logical_id),
        })
        .collect()
}

/// `Some(failure)` for a resource carrying a property value its type's contract does not
/// admit ([`CONSTRAINED_PROPERTIES`]); `None` when every value this stage can judge is
/// admitted.
///
/// Membership is [`ConstrainedProperty::admits`], which is also what the resource provider's
/// own model validation asks of the same rows — including its reading of a value that is not
/// text (an expression this service left unevaluated), which is left to the owning service.
fn inadmissible_value(resource: &StoredResource) -> Option<PropertyFailure> {
    CONSTRAINED_PROPERTIES
        .iter()
        .filter(|constrained| constrained.resource_type == resource.resource_type)
        .find_map(|constrained| {
            let declared = resource.properties.get(constrained.property)?;
            if constrained.admits(declared) {
                return None;
            }
            Some(PropertyFailure {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                reason: failure_reason(
                    &resource.logical_id,
                    constrained.property,
                    constrained.admitted,
                ),
                path: properties_path(&resource.logical_id),
            })
        })
}

/// How CloudFormation reports a property its type's schema requires that the declaration
/// does not carry: fixed text around the MEMBER that is missing, which is the one variable
/// part and comes from the request's own template.
pub fn missing_property_reason(property: &str) -> String {
    format!("Required property [{property}] not found")
}

/// How this stage reports a property whose value its type does not admit: the property it is
/// about, in the path form a resource's property is named in, the resource that declared it,
/// and the values the property admits.
///
/// The sentence is the one this crate already words a property-validation failure with — the
/// `Property validation failure: [Value for property {/<Property>} …]` a resource handler of
/// this service answers for a declaration it cannot carry out
/// ([`super::provisioning::nested_stack`]) — so one kind of failure reads the same whether it
/// was caught before provisioning or reported during it. Its variable parts are the
/// declaration's own: the property, the resource that declared it, and the set that property
/// publishes.
///
/// The value the declaration carried is deliberately NOT quoted back: it may be a value a
/// template author marked `NoEcho`, and what the author has to act on is the set the
/// property admits.
pub fn failure_reason(logical_id: &str, property: &str, admitted: &[&str]) -> String {
    format!(
        "Property validation failure: [Value for property {{/{property}}} of resource \
         [{logical_id}] must be one of [{}]]",
        admitted.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::collections::BTreeMap;

    /// One declared resource of `resource_type` carrying `properties`.
    fn declared(logical_id: &str, resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: logical_id.to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: format!("{logical_id}-physical"),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// RULE: a property value outside the set its type's contract publishes fails the
    /// validation, and one inside it does not — over rows of the catalogue the issue did
    /// not cite as well as the one it did.
    #[test]
    fn a_value_outside_the_published_set_fails_the_validation() {
        let rows: &[(&str, &str, &str, Value, bool)] = &[
            (
                "a parameter type the contract does not publish",
                "AWS::SSM::Parameter",
                "Type",
                ::serde_json::json!({"Type": "NotAType", "Value": "x"}),
                false,
            ),
            (
                "each parameter type it does",
                "AWS::SSM::Parameter",
                "Type",
                ::serde_json::json!({"Type": "StringList", "Value": "a,b"}),
                true,
            ),
            (
                // The encrypted parameter type Systems Manager's own API admits, which this
                // resource type does not publish.
                "a parameter type the owning API has but the type does not publish",
                "AWS::SSM::Parameter",
                "Type",
                ::serde_json::json!({"Type": "SecureString", "Value": "x"}),
                false,
            ),
            (
                "a tier outside the published set",
                "AWS::SSM::Parameter",
                "Tier",
                ::serde_json::json!({"Type": "String", "Value": "x", "Tier": "Platinum"}),
                false,
            ),
            (
                "a billing mode outside the published set",
                "AWS::DynamoDB::Table",
                "BillingMode",
                ::serde_json::json!({"BillingMode": "FREE"}),
                false,
            ),
            (
                "a billing mode inside it",
                "AWS::DynamoDB::Table",
                "BillingMode",
                ::serde_json::json!({"BillingMode": "PAY_PER_REQUEST"}),
                true,
            ),
            (
                "a canned ACL outside the published set",
                "AWS::S3::Bucket",
                "AccessControl",
                ::serde_json::json!({"AccessControl": "EverybodyRead"}),
                false,
            ),
            (
                "a workflow type outside the published set",
                "AWS::StepFunctions::StateMachine",
                "StateMachineType",
                ::serde_json::json!({"StateMachineType": "FAST"}),
                false,
            ),
            (
                "a tenancy outside the published set",
                "AWS::EC2::VPC",
                "InstanceTenancy",
                ::serde_json::json!({"InstanceTenancy": "shared"}),
                false,
            ),
            (
                "a property the declaration states as an unevaluated expression",
                "AWS::SSM::Parameter",
                "Type",
                ::serde_json::json!({"Type": {"Ref": "Kind"}, "Value": "x"}),
                true,
            ),
            (
                "a type this stage knows no value set for",
                "AWS::SNS::Topic",
                "DisplayName",
                ::serde_json::json!({"DisplayName": "anything at all"}),
                true,
            ),
        ];
        for (case, resource_type, property, properties, admitted) in rows {
            let resources = vec![declared("R", resource_type, properties.clone())];
            let failed = failure(&resources);
            assert_eq!(
                failed.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted:?}, got {failed:?}"
            );
            if let Some(failure) = failed {
                assert!(
                    failure.reason.contains(&format!("{{/{property}}}"))
                        && failure.reason.contains("[R]")
                        && failure.logical_id == "R",
                    "case {case:?}: expected a failure naming the property and the resource, \
                     got {failure:?}"
                );
            }
        }
    }

    /// RULE: a declaration that does not carry a property its type's contract requires fails
    /// the validation, naming that property; one that carries every required property does
    /// not. The rows span the catalogue — types the issue did not cite among them — and the
    /// forms a declaration can be missing a property in (no `Properties` at all, some of
    /// them, a property whose value is an expression).
    #[test]
    fn a_declaration_missing_a_required_property_fails_the_validation() {
        // (case, type, properties, the property the stage must name — `None` for a
        // declaration that carries every required one).
        let rows: &[(&str, &str, Value, Option<&str>)] = &[
            (
                "a pipe with no target",
                "AWS::Pipes::Pipe",
                ::serde_json::json!({
                    "Source": "arn:aws:sqs:us-east-1:000000000000:in",
                    "RoleArn": "arn:aws:iam::000000000000:role/pipe",
                }),
                Some("Target"),
            ),
            (
                "a pipe with every required property, the target an expression",
                "AWS::Pipes::Pipe",
                ::serde_json::json!({
                    "Source": "arn:aws:sqs:us-east-1:000000000000:in",
                    "Target": {"Fn::GetAtt": ["Out", "Arn"]},
                    "RoleArn": "arn:aws:iam::000000000000:role/pipe",
                }),
                None,
            ),
            (
                "a pipe with no properties at all reports the first required one",
                "AWS::Pipes::Pipe",
                Value::Null,
                Some("RoleArn"),
            ),
            (
                "a parameter with no value",
                "AWS::SSM::Parameter",
                ::serde_json::json!({"Type": "String"}),
                Some("Value"),
            ),
            (
                "a subscription with no protocol",
                "AWS::SNS::Subscription",
                ::serde_json::json!({"TopicArn": "arn:aws:sns:us-east-1:000000000000:t"}),
                Some("Protocol"),
            ),
            (
                "a queue policy with no queues",
                "AWS::SQS::QueuePolicy",
                ::serde_json::json!({"PolicyDocument": {"Version": "2012-10-17"}}),
                Some("Queues"),
            ),
            (
                "a schedule with no expression",
                "AWS::Scheduler::Schedule",
                ::serde_json::json!({
                    "FlexibleTimeWindow": {"Mode": "OFF"},
                    "Target": {"Arn": "arn:aws:sqs:us-east-1:000000000000:t"},
                }),
                Some("ScheduleExpression"),
            ),
            (
                "a subnet with no VPC",
                "AWS::EC2::Subnet",
                ::serde_json::json!({"CidrBlock": "10.0.0.0/24"}),
                Some("VpcId"),
            ),
            (
                "a type this stage knows no required property of",
                "AWS::SNS::Topic",
                Value::Null,
                None,
            ),
        ];
        for (case, resource_type, properties, missing) in rows {
            let resources = vec![declared("R", resource_type, properties.clone())];
            let failed = failure(&resources);
            assert_eq!(
                failed.as_ref().map(|failure| failure.reason.clone()),
                missing.map(missing_property_reason),
                "case {case:?}: expected the missing property {missing:?}, got {failed:?}"
            );
            if let Some(failure) = failed {
                assert_eq!(
                    failure.path, "/Resources/R/Properties",
                    "case {case:?}: expected the resource's own properties path, got {failure:?}"
                );
            }
        }
    }

    /// RULE: the failure reported is the FIRST one the deployment would have run into.
    #[test]
    fn the_first_failing_resource_is_the_one_reported() {
        let resources = vec![
            declared(
                "Fine",
                "AWS::DynamoDB::Table",
                ::serde_json::json!({"BillingMode": "PROVISIONED"}),
            ),
            declared(
                "First",
                "AWS::SSM::Parameter",
                ::serde_json::json!({"Type": "NotAType", "Value": "x"}),
            ),
            declared(
                "Second",
                "AWS::S3::Bucket",
                ::serde_json::json!({"AccessControl": "EverybodyRead"}),
            ),
        ];
        let failed = failure(&resources);
        assert_eq!(
            failed.as_ref().map(|failure| failure.reason.as_str()),
            Some(failure_reason("First", "Type", &["String", "StringList"]).as_str()),
            "case \"two failing resources\": expected the first one's reason, got {failed:?}"
        );
    }

    /// RULE: how much of a judged value a failure says is the TYPE's to decide — a type
    /// carrying credentials among its properties has its values reported redacted, and a type
    /// whose whole property document is the template's own text has them said as declared. The
    /// rows span both sides, every type the catalogue of credential-carrying types holds, and a
    /// type it holds no row for at all.
    #[test]
    fn a_judged_value_is_said_as_far_as_its_type_may_say_it() {
        for row in CREDENTIAL_PROPERTIES {
            assert_eq!(
                reported_value(row.resource_type, "a judged value"),
                REDACTED_VALUE,
                "case {:?}: expected a type carrying credentials to report no value, it reports \
                 {:?}",
                row.resource_type,
                reported_value(row.resource_type, "a judged value")
            );
        }
        for resource_type in [
            crate::state::template::CLOUDHSM_CLUSTER,
            crate::state::template::SSM_QUICKSETUP_CONFIGURATION_MANAGER,
            "AWS::CodePipeline::Pipeline",
            "AWS::SNS::Topic",
        ] {
            assert_eq!(
                reported_value(resource_type, "a judged value"),
                "a judged value",
                "case {resource_type:?}: expected the judged value itself, got {:?}",
                reported_value(resource_type, "a judged value")
            );
        }
    }
}
