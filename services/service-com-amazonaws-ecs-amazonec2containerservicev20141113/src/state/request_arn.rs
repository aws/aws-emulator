// SPDX-License-Identifier: CC0-1.0

//! How this service reads an ARN a REQUEST supplies, and what it refuses one for.
//!
//! An ARN is a WHOLE identifier: it carries the partition, the service, the Region, the
//! ACCOUNT and the resource. Most of this service's reference members accept either form
//! — "The short name or full Amazon Resource Name (ARN) of the cluster" — and reducing an
//! ARN to its trailing name segment ([`super::cluster_name_from_reference`],
//! [`super::task_arn_from_reference`]) throws the rest away. So the REGION and the ACCOUNT
//! segments are compared against the Region the request addressed and the account this
//! emulator serves BEFORE that reduction — Amazon ECS is regional ("Amazon ECS clusters
//! are Region specific"), so an endpoint resolves its own Region's resources and its own
//! account's, and nothing else. Otherwise another Region's or another account's ARN
//! resolves to this endpoint's resource of the same name, and "this ARN is not one of
//! mine" — a programming or credentials error a client must stop on — becomes
//! indistinguishable from "this resource does not exist", which a client retries or treats
//! as already-deleted.
//!
//! The reading is TOTAL: [`RequestArn::parse`] walks the colon-separated fields through an
//! iterator and answers `None` for anything that is not an ARN, so no caller-supplied
//! value reaches an index, a slice or an unwrap. A value that is not an ARN carries no
//! account to disagree with, and neither does an ARN whose account field is empty, so both
//! are left to resolve as the names they are.

use super::{is_valid_name, ARN_NAMESPACE};

/// How many colon-separated fields an ARN is spelled in. The RESOURCE is the last one, so
/// it keeps any colon of its own (a task-definition revision, an IAM path).
const ARN_FIELD_COUNT: usize = 6;

/// The literal first field of every ARN.
const ARN_SCHEME: &str = "arn";

/// The resource-type segment a CLUSTER's ARN carries, as [`super::cluster_arn`] spells it.
const CLUSTER_RESOURCE_TYPE: &str = "cluster";

/// The fields of an ARN a request supplied, borrowed from the request's own value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestArn<'a> {
    /// The partition (`aws`, `aws-cn`, …).
    pub partition: &'a str,
    /// The service namespace the resource belongs to (`ecs` for this service's own).
    pub service: &'a str,
    /// The Region, empty for a global service's ARN.
    pub region: &'a str,
    /// The account that owns the resource, empty where the resource has no owner.
    pub account: &'a str,
    /// The resource, colons and all.
    pub resource: &'a str,
}

impl<'a> RequestArn<'a> {
    /// The fields of `reference` when it is spelled as an ARN, and `None` when it is not
    /// one at all — a bare resource name, which most reference members also accept.
    ///
    /// Every field is taken from the iterator with `?`, so a value with too few fields
    /// answers `None` rather than reaching an index.
    pub fn parse(reference: &'a str) -> Option<Self> {
        let mut fields = reference.splitn(ARN_FIELD_COUNT, ':');
        let scheme = fields.next()?;
        let partition = fields.next()?;
        let service = fields.next()?;
        let region = fields.next()?;
        let account = fields.next()?;
        let resource = fields.next()?;
        (scheme == ARN_SCHEME).then_some(Self {
            partition,
            service,
            region,
            account,
            resource,
        })
    }

    /// Whether this ARN names a resource of `resource_type` of THIS service, and the
    /// resource's own name when it does.
    fn named_resource(&self, resource_type: &str) -> Option<&'a str> {
        if self.service != ARN_NAMESPACE {
            return None;
        }
        let (segment, name) = self.resource.split_once('/')?;
        (segment == resource_type).then_some(name)
    }
}

/// Whether `reference` is an ARN whose ACCOUNT segment is not the account this emulator
/// serves ([`::engine::emulator_account_id`], the account every ARN it mints carries and
/// the one every caller it serves holds).
pub fn names_another_account(reference: &str) -> bool {
    RequestArn::parse(reference).is_some_and(|arn| {
        !arn.account.is_empty() && arn.account != ::engine::emulator_account_id()
    })
}

/// `InvalidParameterException$message` for a request whose resource IDENTIFIER is an ARN
/// of another account — a member documented as taking "the short name or full Amazon
/// Resource Name (ARN)" of the resource, given an ARN whose account is not the caller's.
///
/// Fixed text, carrying nothing from the request: the condition is about WHOSE resource
/// the ARN names, which the caller can see in the value it sent.
pub const ACCOUNT_IDS_MISMATCH_MESSAGE: &str = "AccountIDs mismatch";

/// `InvalidParameterException$message` for a member whose whole value IS an ARN (a tagging
/// `resourceArn`) naming another account's resource: the ARN itself is what the member was
/// given and what is wrong with it.
pub const INVALID_ARN_MESSAGE: &str = "The ARN provided is invalid.";

/// `message` for an identifier that is an ARN of a Region other than the one the request
/// was addressed to. Fixed text: the ARN's own Region segment is what is wrong with it.
pub const INVALID_REGION_IN_ARN_MESSAGE: &str = "Invalid Region in ARN";

/// Whether `reference` is an ARN whose REGION segment is not `request_region`, the Region
/// the request was addressed to ([`super::request_region`]). An ARN with an EMPTY Region
/// names a global resource, so it carries no Region to disagree with — as an empty account
/// segment carries no account.
pub fn names_another_region(reference: &str, request_region: &str) -> bool {
    RequestArn::parse(reference)
        .is_some_and(|arn| !arn.region.is_empty() && arn.region != request_region)
}

/// Which segment of an ARN a request supplied is not this endpoint's, so an operation that
/// answers the two conditions under DIFFERENT error codes reads which one it is from here
/// rather than ordering the two checks itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignArnSegment {
    /// The REGION segment names another Region.
    Region,
    /// The ACCOUNT segment names another account's resource.
    Account,
}

impl ForeignArnSegment {
    /// The `message` this condition is refused with.
    pub fn message(self) -> &'static str {
        match self {
            Self::Region => INVALID_REGION_IN_ARN_MESSAGE,
            Self::Account => ACCOUNT_IDS_MISMATCH_MESSAGE,
        }
    }
}

/// The segment of the identifiers a request carries that is not this endpoint's, and `None`
/// when every one of them is one it could resolve. The REGION is decided first, because it
/// decides which endpoint owns the resource at all; both are decided before any lookup, so
/// neither answer depends on what this store holds. That order lives HERE and nowhere else.
pub fn foreign_arn_segment<'a>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    references: impl IntoIterator<Item = &'a str>,
) -> Option<ForeignArnSegment> {
    let region = super::request_region(ctx);
    references.into_iter().find_map(|reference| {
        if names_another_region(reference, &region) {
            Some(ForeignArnSegment::Region)
        } else if names_another_account(reference) {
            Some(ForeignArnSegment::Account)
        } else {
            None
        }
    })
}

/// The sentence that segment earns, for the operations that answer both conditions with
/// one error code.
pub fn request_arn_violation<'a>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    references: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    foreign_arn_segment(ctx, references).map(ForeignArnSegment::message)
}

/// The same check for a tagging `resourceArn`: another account's earns the sentence about
/// the ARN ([`INVALID_ARN_MESSAGE`]), because that member accepts nothing but an ARN, and
/// another Region's earns the sentence every member's ARN earns.
pub fn request_arn_resource_violation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    resource_arn: &str,
) -> Option<&'static str> {
    match foreign_arn_segment(ctx, Some(resource_arn))? {
        ForeignArnSegment::Region => Some(INVALID_REGION_IN_ARN_MESSAGE),
        ForeignArnSegment::Account => Some(INVALID_ARN_MESSAGE),
    }
}

/// How a refusal names the `taskDefinition` member a foreign ARN was supplied in.
pub const TASK_DEFINITION_ARN_SUBJECT: &str = "Invalid task definition";

/// The same check for a TASK-DEFINITION reference — a member documented as taking "the
/// family and revision (family:revision) or full Amazon Resource Name (ARN) of the task
/// definition": the sentence names the MEMBER the identifier was supplied in and then the
/// segment of that ARN which is not this endpoint's, so a caller is told which of the
/// several identifiers its request carried is the foreign one rather than only that one of
/// them was.
pub fn task_definition_arn_violation<'a>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    references: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    foreign_arn_segment(ctx, references)
        .map(|segment| format!("{TASK_DEFINITION_ARN_SUBJECT}: {}", segment.message()))
}

/// Whether `reference` is the full ARN of a CLUSTER of this service —
/// `arn:<partition>:ecs:<region>:<account>:cluster/<name>` with a name of the character
/// set a cluster name is spelled in — rather than a bare cluster name or an ARN of some
/// other resource.
pub fn is_cluster_arn(reference: &str) -> bool {
    RequestArn::parse(reference)
        .and_then(|arn| arn.named_resource(CLUSTER_RESOURCE_TYPE))
        .is_some_and(is_valid_name)
}
