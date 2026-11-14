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
//! does not stand in for it: it decides the two questions a type's published contract makes
//! decidable without asking anybody —
//!
//! * a property the contract marks REQUIRED that the declaration does not carry at all, over
//!   the rows this emulator holds ([`REQUIRED_PROPERTIES`]), which is the
//!   `Required property [<member>] not found` the stage reports
//!   ([`missing_property_reason`]);
//! * a property whose contract publishes the values it admits, carrying a value that is not
//!   one of them, over the rows this emulator holds ([`CONSTRAINED_PROPERTIES`]).
//!
//! A resource is judged in that order, because a property that is not there has no value to
//! hold to a set, and the resources in the order the deployment would have created them — so
//! the failure reported is the one the operation would have run into first.
//!
//! That makes the stage deliberately PARTIAL, and fail-open: a property no row covers, a type
//! with no row at all, and a value an expression produced that is not text are each left to
//! the service that owns the resource, which is the authority on everything this is not. So
//! it can only ever anticipate a refusal that service would have made, never invent one — and
//! a declaration it passes is still provisioned and still judged there. It becomes the real
//! schema check when this registry holds real schemas, or when an owning service serves a
//! would-this-be-accepted read; until then the rows are what it knows.
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

/// The properties this stage knows a published value set for.
///
/// Source for each row: the resource type's entry in the "AWS resource and property types
/// reference" of the CloudFormation User Guide, whose property documentation publishes the
/// allowed values, and the API member that property is passed to
/// ([`super::provisioner`] is where each type's mapping onto that member lives).
pub const CONSTRAINED_PROPERTIES: &[ConstrainedProperty] = &[
    // `AWS::SSM::Parameter`'s `Type` is `PutParameter$Type`, "the type of parameter",
    // whose published values are the parameter types Systems Manager holds.
    ConstrainedProperty {
        resource_type: "AWS::SSM::Parameter",
        property: "Type",
        admitted: &["String", "StringList", "SecureString"],
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
/// is observable only in which property a declaration missing SEVERAL of them is reported
/// for, and reading it off the reference is what keeps that choice the contract's rather
/// than this catalogue's.
pub const REQUIRED_PROPERTIES: &[RequiredProperties] = &[
    // `AWS::Pipes::Pipe`: `CreatePipe` declares `Source`, `Target` and `RoleArn` required
    // beside the `Name` CloudFormation generates, so a pipe cannot be created without the
    // source it reads, the target it delivers to and the role it acts as.
    RequiredProperties {
        resource_type: "AWS::Pipes::Pipe",
        required: &["RoleArn", "Source", "Target"],
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
    /// Why it failed: which property is missing ([`missing_property_reason`]), or which
    /// property carries a value its type does not admit ([`failure_reason`]).
    pub reason: String,
    /// `OperationEvent$ValidationPath`: "the path within the resource where the validation
    /// was applied" — the element of the submitted template this validation ran on
    /// ([`properties_path`]).
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

/// `Some(failure)` for the first resource of `declared` whose property document its type's
/// contract refuses — the failure the operation reports instead of deploying anything;
/// `None` when every resource passes what this stage can judge.
///
/// The resources are walked in the order the deployment would have created them, so the
/// failure reported is the one the operation would have run into first, and each resource is
/// judged for a MISSING required property before the values it does carry: a property that
/// is not there has no value to hold to a published set.
pub fn failure(declared: &[StoredResource]) -> Option<PropertyFailure> {
    declared.iter().find_map(|resource| {
        missing_required_property(resource)
            .or_else(|| matched_several_subschemas(resource))
            .or_else(|| inadmissible_value(resource))
    })
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

/// `Some(failure)` for a resource whose declaration does not carry a property its type's
/// contract requires ([`REQUIRED_PROPERTIES`]) — the first such property in the order the
/// contract publishes them; `None` for a type with no row, and for one carrying every
/// property its row names.
fn missing_required_property(resource: &StoredResource) -> Option<PropertyFailure> {
    REQUIRED_PROPERTIES
        .iter()
        .filter(|row| row.resource_type == resource.resource_type)
        .find_map(|row| {
            let missing = row
                .required
                .iter()
                .find(|property| resource.properties.get(*property).is_none())?;
            Some(PropertyFailure {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                reason: missing_property_reason(missing),
                path: properties_path(&resource.logical_id),
            })
        })
}

/// `Some(failure)` for a resource carrying a property value its type's contract does not
/// admit ([`CONSTRAINED_PROPERTIES`]); `None` when every value this stage can judge is
/// admitted.
///
/// A value that is not text (an expression this service left unevaluated) is no value of a
/// published set either way and is left to the owning service.
fn inadmissible_value(resource: &StoredResource) -> Option<PropertyFailure> {
    CONSTRAINED_PROPERTIES
        .iter()
        .filter(|constrained| constrained.resource_type == resource.resource_type)
        .find_map(|constrained| {
            let value = resource
                .properties
                .get(constrained.property)
                .and_then(Value::as_str)?;
            if constrained.admitted.contains(&value) {
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
            Some(
                failure_reason("First", "Type", &["String", "StringList", "SecureString"]).as_str()
            ),
            "case \"two failing resources\": expected the first one's reason, got {failed:?}"
        );
    }
}
