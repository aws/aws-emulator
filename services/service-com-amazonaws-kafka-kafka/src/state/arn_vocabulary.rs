// SPDX-License-Identifier: CC0-1.0

//! The ARNs this service READS, and the refusals a value it cannot read earns.
//!
//! One concern, in one place: the single parse of a caller-supplied ARN
//! ([`arn_resource_of`]), the predicate for each kind of resource this service
//! mints an ARN for, the names the WIRE gives the members those references (and
//! the handful of plain values beside them) travel in — which is what every
//! refusal of this service reports as the parameter at fault — and the
//! `BadRequestException` envelopes a value that is not an ARN of the kind its
//! member carries is refused with. Keeping them together is what makes one value
//! a bad request to every operation that reads it rather than a bad request to
//! one and a miss to the next, and keeps one member from being named two ways by
//! two refusals.

use super::*;

/// The resource type and id path of an MSK resource ARN —
/// `arn:<partition>:kafka:<region>:<account>:<type>/<id-path>` — or `None` when
/// `value` is not one.
///
/// Built on [`kafka_arn_resource`], the one ARN parse this module has, so an
/// operation that takes ANY resource ARN (the tagging operations) draws the
/// "this is not an ARN of mine" line exactly where the `is_*_arn` predicates
/// draw it.
pub fn kafka_resource_arn_parts(value: &str) -> Option<(&str, &str)> {
    let resource = kafka_arn_resource(value)?;
    let (resource_type, id_path) = resource.split_once('/')?;
    (!resource_type.is_empty() && !id_path.is_empty()).then_some((resource_type, id_path))
}

/// The resource type `value` names, for an operation that takes any MSK resource
/// ARN — provided it is a type this service TAGS.
///
/// Three things fail here, and they are told apart by WHY: a value that is not an
/// ARN at all ([`TagTargetRejected::NotAnArn`]), and — as one answer
/// ([`TagTargetRejected::NotATaggableResource`]) — a well-formed ARN that is not
/// one of this service's, one whose type this service mints in a shape `value`
/// does not have, and one naming a kind of resource the tagging surface does not
/// serve. The last is the rule's point: a resource of this service carries tags
/// only where the API gives it a `Tags` member ([`TAGGABLE_RESOURCE_TYPES`]), and
/// a configuration or a cluster operation has none — so such an ARN is a
/// `ResourceArn` the service cannot act on rather than a resource that might
/// appear later. The shape check goes through the same `is_*_arn` predicate the
/// readers of that resource type use, so one value is never a bad request to one
/// operation and a miss to another.
pub(super) fn resource_type_of_arn(value: &str) -> Result<&str, TagTargetRejected> {
    if !is_arn(value) {
        return Err(TagTargetRejected::NotAnArn);
    }
    let Some((resource_type, _)) = kafka_resource_arn_parts(value) else {
        return Err(TagTargetRejected::NotATaggableResource);
    };
    let shaped = match resource_type {
        CLUSTER_RESOURCE_TYPE => is_cluster_arn(value),
        CHANNEL_RESOURCE_TYPE => is_channel_arn(value),
        REPLICATOR_RESOURCE_TYPE => is_replicator_arn(value),
        VPC_CONNECTION_RESOURCE_TYPE => is_vpc_connection_arn(value),
        // Every other resource type this service mints, taggable by no API of
        // it.
        _ => return Err(TagTargetRejected::NotATaggableResource),
    };
    if shaped {
        Ok(resource_type)
    } else {
        Err(TagTargetRejected::NotATaggableResource)
    }
}

/// Whether `value` is an ARN AT ALL — the six colon-separated fields every AWS
/// ARN carries, `arn:<partition>:<service>:<region>:<account>:<resource>`, with a
/// partition, a service namespace and a resource named.
///
/// The region and the account fields may be EMPTY, because AWS publishes ARNs
/// that leave them so (a global service's resource, a bucket), while a value
/// naming no partition, no service or no resource references nothing at all. Says
/// nothing about WHOSE resource the ARN is or whether it exists: that is what the
/// predicates over this service's own ARNs are for, and [`arn_resource_of`] is
/// stricter for exactly that reason — every ARN THIS service mints names a region
/// and an account. Both read the fields through [`arn_fields`], so the two cannot
/// disagree about where an ARN's fields are.
pub fn is_arn(value: &str) -> bool {
    arn_fields(value).is_some_and(|arn| {
        !arn.partition.is_empty() && !arn.vendor.is_empty() && !arn.resource.is_empty()
    })
}

/// The resource types of this service that carry tags — the ones whose API gives
/// the resource a `Tags` member (a cluster, a channel, a replicator and a VPC
/// connection), which is what makes them the targets the tagging operations
/// serve. A configuration and a cluster operation are not among them: no shape
/// of theirs models tags.
pub const TAGGABLE_RESOURCE_TYPES: [&str; 4] = [
    CLUSTER_RESOURCE_TYPE,
    CHANNEL_RESOURCE_TYPE,
    REPLICATOR_RESOURCE_TYPE,
    VPC_CONNECTION_RESOURCE_TYPE,
];

/// Whether `value` is an ARN of a resource of THIS service at all — any resource
/// type it names, in any partition, region and account.
///
/// The gate of the cluster READ surface, which resolves the reference it is handed
/// rather than parsing it as the one kind it reports: an operation that answers
/// `NotFoundException` for a cluster reference that resolves to nothing treats
/// every MSK resource ARN as such a reference, so an ARN of another resource type
/// is a cluster that does not exist rather than a parameter the caller mistyped,
/// and only a value that is no MSK resource ARN at all earns [`malformed_arn`].
/// The surfaces that CHANGE a cluster keep the stricter [`is_cluster_arn`]: what
/// each answers for an ARN of another type is the operation's own, and the two
/// classes are recorded separately.
pub fn is_kafka_resource_arn(value: &str) -> bool {
    kafka_resource_arn_parts(value).is_some()
}

/// Whether `value` is shaped like an MSK cluster ARN —
/// `arn:<partition>:kafka:<region>:<account>:cluster/<name>/<id>`.
///
/// A reader that models both errors answers `BadRequestException` for a value
/// that is not an ARN of this kind, and reserves `NotFoundException` for a
/// well-formed ARN no live cluster holds: that is the distinction the real
/// service draws between a request it cannot parse and a resource it cannot find.
pub fn is_cluster_arn(value: &str) -> bool {
    is_kafka_arn_for(value, "cluster")
}

/// The input member the cluster-scoped operations carry their cluster's ARN in,
/// named as the API documents it on the wire — the camelCase spelling the
/// request itself uses, not the Smithy structure member's identifier. What those
/// operations' refusals report as the parameter at fault, beside
/// [`REPLICATOR_ARN_MEMBER`].
pub const CLUSTER_ARN_MEMBER: &str = "clusterArn";

/// The member `CreateVpcConnection` carries the cluster's ARN in, under the same
/// rule: that operation names the cluster it connects to `TargetClusterArn`, so
/// its refusal reports that member's wire name rather than the one every
/// cluster-scoped operation binds.
pub const TARGET_CLUSTER_ARN_MEMBER: &str = "targetClusterArn";

/// The member the readers of one cluster OPERATION carry their target's ARN in,
/// named the same way.
pub const CLUSTER_OPERATION_ARN_MEMBER: &str = "clusterOperationArn";

/// The member an operation acting on one client VPC CONNECTION of a cluster
/// carries that connection's ARN in, under the same rule: the wire name of
/// `RejectClientVpcConnectionRequest$VpcConnectionArn`. The operations that bind
/// the connection's ARN as a path label of their own (`DescribeVpcConnection`,
/// `DeleteVpcConnection`, whose label is spelled `Arn`) name the connection by
/// this same name, which is the one the API documents it under.
pub const VPC_CONNECTION_ARN_MEMBER: &str = "vpcConnectionArn";

/// The member the channel-scoped operations carry a channel's ARN in, named the
/// same way: the API documents the label `ChannelArn` as `channelArn`, which is
/// the spelling a refusal reports whether the value travelled in the URI or in
/// the body.
pub const CHANNEL_ARN_MEMBER: &str = "channelArn";

/// The member the Topic Management operations carry a topic's name in, named the
/// same way (`topicName`, the wire spelling of `TopicName` whether the request
/// binds it as a path label or sends it in the body).
pub const TOPIC_NAME_MEMBER: &str = "topicName";

/// The cluster-ARN-SPECIFIC refusal wording, which the SCRAM-secret surface and
/// the client-VPC-connection listing answer a cluster ARN they cannot parse with
/// instead of the generic [`BAD_REQUEST_MESSAGE`] (the apostrophe is U+2019, as
/// the service writes it).
///
/// Fixed text end to end, like the generic one: the offending value is not part
/// of it. Which of the two an operation answers is the operation's, so the two
/// are named separately here rather than evened out — a caller of one surface is
/// told what that surface tells callers.
pub const CLUSTER_ARN_NOT_VALID_MESSAGE: &str =
    "The specified cluster ARN isn\u{2019}t valid. Specify a valid cluster ARN, and try again.";

/// The refusal an operation answers a value that is not shaped like the kind of
/// ARN the member naming it carries ([`is_cluster_arn`],
/// [`is_cluster_operation_arn`]): a `BadRequestException` (HTTP 400) whose
/// message is the service's generic parameter sentence
/// ([`BAD_REQUEST_MESSAGE`]) — the WHOLE text, with no part taken from the
/// request, so two calls carrying different unusable values are told the same
/// thing and the offending value is never echoed back — and which names the
/// member at fault in `invalidParameter`, by its wire name
/// ([`CLUSTER_ARN_MEMBER`], [`TARGET_CLUSTER_ARN_MEMBER`],
/// [`CLUSTER_OPERATION_ARN_MEMBER`]).
///
/// Built HERE rather than beside each shape check, so no handler composes a
/// sentence of its own around the ARN it was handed: one condition, one
/// envelope, whichever operation met it.
pub fn malformed_arn(member: &str) -> BadRequestException {
    parameter_refusal(member, BAD_REQUEST_MESSAGE)
}

/// The same refusal for the surfaces that answer the cluster-ARN-specific
/// wording ([`CLUSTER_ARN_NOT_VALID_MESSAGE`]): the member at fault is the
/// cluster's own ARN, which is what that sentence is about.
pub fn invalid_cluster_arn() -> BadRequestException {
    parameter_refusal(CLUSTER_ARN_MEMBER, CLUSTER_ARN_NOT_VALID_MESSAGE)
}

/// The sentence the REPLICATOR surface answers a value its member cannot carry
/// with: fixed text end to end, so the value the request sent is never echoed
/// back and two calls carrying different unusable values are told the same thing.
///
/// A sentence of the service's own, beside the generic [`BAD_REQUEST_MESSAGE`]
/// the cluster surface answers and the cluster-specific
/// [`CLUSTER_ARN_NOT_VALID_MESSAGE`]: which of them a caller gets is the
/// operation's, so they are named separately here rather than evened out.
pub const VALUE_NOT_VALID_MESSAGE: &str = "The provided value is not valid.";

/// The refusal every replicator operation answers a value that is not shaped like
/// a replicator ARN ([`is_replicator_arn`]) with: a `BadRequestException` (HTTP
/// 400) carrying [`VALUE_NOT_VALID_MESSAGE`] and naming
/// [`REPLICATOR_ARN_MEMBER`] in `invalidParameter`.
///
/// Built HERE, like [`malformed_arn`], so the readers, the delete and the
/// replication update cannot drift apart on how an unusable replicator reference
/// is reported: one condition, one envelope, whichever operation met it. A
/// well-formed ARN no live replicator holds is the DIFFERENT refusal those
/// operations answer with [`resource_not_found`].
pub fn invalid_replicator_arn() -> BadRequestException {
    parameter_refusal(REPLICATOR_ARN_MEMBER, VALUE_NOT_VALID_MESSAGE)
}

/// The refusal the CLUSTER delete, whose version pre-condition is OPT-IN,
/// answers a cluster reference it cannot read with — and which input that
/// refusal blames.
///
/// The cluster delete takes the cluster's version on the optional
/// `currentVersion` query parameter, so a request that sends one asks for the
/// delete to go through only while the cluster is still at that version. A
/// reference the service cannot read at all resolves no cluster, so that
/// pre-condition can never be satisfied: the input that refusal names is the
/// version it could not check ([`CURRENT_VERSION_MEMBER`]). A request that sent
/// no version states no such pre-condition, so the same refusal names the member
/// that carried the unreadable reference ([`CLUSTER_ARN_MEMBER`]).
///
/// The attribution is the SURFACE's, not this condition's: the replicator
/// delete, whose version is optional in the same way, blames the path-bound ARN
/// it could not read whether or not the request quoted a version
/// ([`invalid_replicator_arn`]), so it does not come through here.
///
/// The sentence is this surface's own ([`BAD_REQUEST_MESSAGE`]), fixed end to
/// end: only which input is blamed depends on the request, and neither the
/// reference nor the version reaches the message.
pub fn unreadable_cluster_reference(current_version: Option<&str>) -> BadRequestException {
    match current_version {
        Some(_) => parameter_refusal(CURRENT_VERSION_MEMBER, BAD_REQUEST_MESSAGE),
        None => parameter_refusal(CLUSTER_ARN_MEMBER, BAD_REQUEST_MESSAGE),
    }
}

/// The sentence an operation that models NO `NotFoundException` answers a
/// well-formed resource ARN this endpoint's region holds nothing for with.
///
/// The SAME fixed, request-independent sentence [`malformed_arn`] carries
/// ([`BAD_REQUEST_MESSAGE`]): both are the one thing this service says about a
/// parameter it cannot act on, and neither takes any part of its text from the
/// request — not the value, not the region the endpoint serves. Operations that
/// DO model `NotFoundException` answer [`RESOURCE_NOT_FOUND_MESSAGE`] instead,
/// which is that error's own sentence.
///
/// Named here, beside the shape refusal, so no handler composes a sentence of
/// its own around the reference it was handed.
pub const UNRESOLVED_REFERENCE_MESSAGE: &str = BAD_REQUEST_MESSAGE;

/// The member every paginated listing of this service carries its resume token
/// in, as the request spells it on the wire (`NextToken`'s `httpQuery` name), so
/// a refusal names the query parameter the caller actually sent.
pub const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The refusal every paginated listing answers a `nextToken` it cannot read
/// with: a `BadRequestException` (HTTP 400) naming [`NEXT_TOKEN_MEMBER`] and
/// carrying the service's GENERIC parameter sentence
/// ([`BAD_REQUEST_MESSAGE`]) — the service has no sentence of its own for a
/// resume token, so an unusable one is told the same thing as any other
/// parameter it cannot read, and the token itself never reaches the message.
///
/// Built HERE, like [`malformed_arn`], so no listing composes its own wording
/// for a token: one condition, one envelope, whichever listing met it. A token
/// this service DID mint but that names an entry the caller's registry no longer
/// holds is not this refusal — it resumes an empty page.
pub fn invalid_page_token() -> BadRequestException {
    parameter_refusal(NEXT_TOKEN_MEMBER, BAD_REQUEST_MESSAGE)
}

/// One `BadRequestException` naming a request member and carrying one of the
/// service's fixed sentences, so the two refusals above cannot drift apart on
/// how a parameter is named or on the fact that the message carries nothing of
/// the request.
pub(crate) fn parameter_refusal(member: &str, message: &str) -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(member.to_string()),
        message: Some(message.to_string()),
    }
}

/// Whether `value` is shaped like an MSK replicator ARN —
/// `arn:<partition>:kafka:<region>:<account>:replicator/<name>/<id>`.
///
/// Same distinction, same reason, as [`is_cluster_arn`]: a value that is not an
/// ARN of this kind is a `BadRequestException`, and a well-formed ARN no live
/// replicator holds is a `NotFoundException`.
pub fn is_replicator_arn(value: &str) -> bool {
    replicator_arn_identity(value).is_some()
}

/// The replicator NAME and the IDENTIFIER the replicator ARN `value` carries, or
/// `None` for a value that is not a replicator ARN at all.
///
/// Read through [`kafka_arn_resource`], this module's one ARN split, and the
/// resource type the service mints such an ARN with
/// ([`REPLICATOR_RESOURCE_TYPE`]) — so the predicate above and every reader of
/// what a replicator ARN NAMES draw the same line, both around the fields in
/// front of the resource and around its segments. What the two segments SPELL is
/// not read beyond their being there: a well-formed ARN this endpoint holds no
/// replicator for is absent, whatever its name and identifier look like
/// (`super::replicator_resolves_to_nothing`).
pub(super) fn replicator_arn_identity(value: &str) -> Option<(&str, &str)> {
    let resource = kafka_arn_resource(value)?;
    let mut segments = resource.split('/');
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some(REPLICATOR_RESOURCE_TYPE), Some(name), Some(id), None)
            if !name.is_empty() && !id.is_empty() =>
        {
            Some((name, id))
        }
        _ => None,
    }
}

/// Whether `value` is an AWS Secrets Manager secret ARN —
/// `arn:<partition>:secretsmanager:<region>:<account>:secret:<name>`, the form
/// `SecretArnList` carries ("List of AWS Secrets Manager secret ARNs") and the
/// same requirement the shared ARN vocabulary states for such a member: the
/// vendor field is `secretsmanager` and the resource names a `secret`.
///
/// Read through [`arn_resource_of`], the one ARN split of this module, so this
/// predicate and the ones over this service's own ARNs cannot disagree about the
/// fields in front of the resource. The same distinction between the two client
/// errors as [`is_cluster_arn`]: a value that is not a secret ARN at all is a
/// request the service cannot act on (`BadRequestException`), not a resource it
/// cannot find.
pub fn is_secrets_manager_secret_arn(value: &str) -> bool {
    let Some(resource) = arn_resource_of(value, "secretsmanager") else {
        return false;
    };
    // A secret's resource field is `secret:<name>`; a name may itself carry ':'
    // and '/', so only the first separator ends the type and the rest is the
    // name, which must not be empty — an ARN naming no secret names none.
    resource
        .strip_prefix("secret:")
        .is_some_and(|name| !name.is_empty())
}

/// The sentence Amazon MSK answers a `SecretArnList` entry that is not an AWS
/// Secrets Manager secret ARN at all with: fixed text end to end, with the
/// offending value NOT echoed back, like this service's other parameter
/// refusals. Shared by the two SCRAM batch operations, so one condition is
/// stated one way whichever of them met it.
pub const SECRET_ARN_INVALID_MESSAGE: &str = "The provided secret ARN is invalid.";

/// Whether `value`'s resource part is `<kind>/<name>/<id>` — the shape a
/// cluster, a configuration and a replicator ARN all take, so the predicates
/// above cannot disagree about what a well-formed ARN of their kind is.
pub(super) fn is_kafka_arn_for(value: &str, kind: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some(found), Some(name), Some(id), None)
            if found == kind && !name.is_empty() && !id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK VPC-connection ARN —
/// `arn:<partition>:kafka:<region>:<account>:vpc-connection/<cluster-owner-account>/<cluster-name>/<id>`,
/// the form [`VpcConnection::requested`] mints and the one the real service
/// puts on a multi-VPC connection.
///
/// Same split of the two client errors as [`is_cluster_arn`]: a value of any
/// other shape is a request the service cannot parse, not a resource it cannot
/// find.
pub fn is_vpc_connection_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some("vpc-connection"), Some(owner), Some(cluster), Some(id), None)
            if !owner.is_empty() && !cluster.is_empty() && !id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK cluster-operation ARN —
/// `arn:<partition>:kafka:<region>:<account>:cluster-operation/<cluster
/// name>/<cluster id>/<operation id>`, the form
/// [`Cluster::operation_arn`] mints and `ClusterOperationArn` carries.
///
/// Same distinction as above: a value that is not an ARN of this kind is a bad
/// request, while a well-formed one the registry does not hold is a resource
/// that cannot be found.
pub fn is_cluster_operation_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (
            Some(CLUSTER_OPERATION_RESOURCE_TYPE),
            Some(name),
            Some(cluster_id),
            Some(operation_id),
            None
        ) if !name.is_empty() && !cluster_id.is_empty() && !operation_id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK channel ARN —
/// `arn:<partition>:kafka:<region>:<account>:channel/<cluster>/<cluster-id>/<name>`,
/// optionally followed by the channel's own id: a channel is a sub-resource of
/// the cluster it reads from, so its ARN names that cluster and then the channel.
///
/// The cluster name, the cluster id and the channel name are what identify a
/// channel — a channel name is unique within its cluster — and
/// [`Channel::streaming`] appends the channel's own id after them. BOTH forms are
/// channel ARNs here: the behavioural corpus records the real service answering
/// `NotFoundException`, not `BadRequestException`, for one that stops at the
/// channel name, so that trailing id is not part of what a caller must supply.
///
/// Same division of labour as [`is_cluster_arn`]: a value that is not an ARN of
/// this kind is a `BadRequestException`, and `NotFoundException` is reserved for
/// a well-formed ARN no live channel holds.
pub fn is_channel_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    if segments.next() != Some(CHANNEL_RESOURCE_TYPE) {
        return false;
    }
    let identity: Vec<&str> = segments.collect();
    matches!(identity.len(), 3 | 4) && identity.iter().all(|segment| !segment.is_empty())
}

/// Whether `value` is shaped like an MSK ASSESSMENT ARN —
/// `arn:<partition>:kafka:<region>:<account>:assessment/<cluster>/<cluster-id>/<id>`,
/// the form [`ComAmazonawsKafkaKafkaState::scanned_assessment`] mints: an
/// assessment is a sub-resource of the cluster it was run against, so its ARN
/// names that cluster and then the assessment.
///
/// Same division of labour as [`is_cluster_arn`]: a value that is not an ARN of
/// this kind is a `BadRequestException`, and `NotFoundException` is reserved for
/// a well-formed ARN no stored assessment holds.
pub fn is_assessment_arn(value: &str) -> bool {
    is_kafka_cluster_sub_resource_arn(value, ASSESSMENT_RESOURCE_TYPE)
}

/// Whether `value` is shaped like an MSK TOPIC REASSIGNMENT ARN —
/// `arn:<partition>:kafka:<region>:<account>:topic-reassignment/<cluster>/<cluster-id>/<id>`,
/// the form [`ComAmazonawsKafkaKafkaState::start_topic_reassignment`] mints.
/// Same shape, and the same division of labour, as [`is_assessment_arn`].
pub fn is_topic_reassignment_arn(value: &str) -> bool {
    is_kafka_cluster_sub_resource_arn(value, TOPIC_REASSIGNMENT_RESOURCE_TYPE)
}

/// Whether `value`'s resource part is `<kind>/<cluster name>/<cluster id>/<id>` —
/// the shape every ARN this service mints for a sub-resource OF ONE CLUSTER takes,
/// so the predicates over those kinds cannot disagree about what a well-formed one
/// is.
fn is_kafka_cluster_sub_resource_arn(value: &str, kind: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some(found), Some(cluster), Some(cluster_id), Some(id), None)
            if found == kind
                && !cluster.is_empty()
                && !cluster_id.is_empty()
                && !id.is_empty()
    )
}

/// The resource type an assessment's ARN carries.
pub const ASSESSMENT_RESOURCE_TYPE: &str = "assessment";

/// The resource type a topic reassignment operation's ARN carries.
pub const TOPIC_REASSIGNMENT_RESOURCE_TYPE: &str = "topic-reassignment";

/// The member the assessment operations carry an assessment's ARN in, named as
/// the API documents it on the wire — the camelCase spelling, like every other
/// member a refusal of this service names.
pub const ASSESSMENT_ARN_MEMBER: &str = "assessmentArn";

/// The member the topic reassignment operations carry an operation's ARN in,
/// named the same way.
pub const TOPIC_REASSIGNMENT_OPERATION_ARN_MEMBER: &str = "topicReassignmentOperationArn";

/// Whether `value` is an EC2 resource id of `kind` (`"vpc"`, `"subnet"`,
/// `"sg"`): the prefix, a hyphen, and then either 8 or 17 lowercase
/// hexadecimal digits — the two published EC2 resource-id forms, the original
/// 8-character one and the longer 17-character one.
///
/// A handler that takes EC2 ids refuses anything else as a bad request: the
/// emulator holds no VPC inventory to look the id up in, but a value that is
/// not an id at all is a request the service cannot act on either way.
pub fn is_ec2_resource_id(kind: &str, value: &str) -> bool {
    let Some(suffix) = value
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The resource part of `value` when it is an ARN of THIS service in some
/// partition, region and account — `arn:<partition>:kafka:<region>:<account>:`
/// followed by the resource — and `None` otherwise. Borrowed from `value` and
/// split by the caller, so a predicate on the hot path of every reader allocates
/// nothing. The one ARN parse the `is_*_arn` predicates share, so they cannot
/// disagree about the five fields in front of the resource.
pub(super) fn kafka_arn_resource(value: &str) -> Option<&str> {
    arn_resource_of(value, "kafka")
}

/// The resource part of `value` when it is an ARN of `vendor` in some partition,
/// region and account, and `None` otherwise.
///
/// The ONE reading of an ARN's fields for the predicates over this service's own
/// ARNs and for the one over the Secrets Manager secret ARNs a member of this API
/// carries ([`arn_fields`]), so they cannot disagree about the five fields in
/// front of the resource. Stricter than [`is_arn`]: a resource of a REGIONAL
/// service belongs to one region of one account, and every ARN this service mints
/// or reads as its own names both, so a value leaving either empty names no
/// resource here. The vendor comparison is exact because an ARN namespace is a
/// fixed lowercase token.
pub(super) fn arn_resource_of<'a>(value: &'a str, vendor: &str) -> Option<&'a str> {
    let arn = arn_fields(value)?;
    (arn.vendor == vendor
        && !arn.partition.is_empty()
        && !arn.region.is_empty()
        && !arn.account.is_empty())
    .then_some(arn.resource)
}

/// The six fields of `value` read as an ARN — `arn`, then the partition, the
/// service namespace, the region, the account and the whole resource — or `None`
/// when `value` does not have them.
///
/// The one ARN split of this module, borrowed from `value` so a predicate on the
/// hot path of every reader allocates nothing. `splitn` with the field count
/// leaves the whole resource — colons included — as the last field, so an ARN of
/// the `<type>:<id>` form splits exactly like one of the `<type>/<id>` form. What
/// a field is allowed to SAY is the caller's: this reports the fields and judges
/// none of them.
fn arn_fields(value: &str) -> Option<ArnFields<'_>> {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(vendor), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    Some(ArnFields {
        partition,
        vendor,
        region,
        account,
        resource,
    })
}

/// The fields [`arn_fields`] reads out of an ARN, borrowed from it.
struct ArnFields<'a> {
    partition: &'a str,
    vendor: &'a str,
    region: &'a str,
    account: &'a str,
    resource: &'a str,
}
