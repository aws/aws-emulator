// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon Kinesis Video Streams: the video stream an
//! `AWS::KinesisVideo::Stream` declares and the WebRTC signaling channel an
//! `AWS::KinesisVideo::SignalingChannel` declares. One file of [`super`]'s per-type arms,
//! dispatched to by its `Provisioner`.
//!
//! Both resources are named by the template: `CreateStream` takes a `StreamName` that "is an
//! identifier for the stream, and must be unique for each account and region" and
//! `CreateSignalingChannel` a `ChannelName` in the same position, and that name is the handle
//! every later call of that service takes for the resource — so it stays the physical id the
//! naming rules computed, which is what each type's `Ref` returns.
//!
//! The ARN is the one value either type publishes, and it is that service's OWN: its resource
//! path carries the instant the record was made (`ResourceARN`'s pattern ends
//! `…:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`), which no naming rule can compute — so it is read off
//! the create's own answer, and off that service's own report of the resource everywhere else.
//!
//! Neither create leaves the resource usable: a created stream and a created channel are both
//! `CREATING`, and a client can neither write media to a stream nor signal over a channel until
//! that service reports it `ACTIVE` — so each resource's creation is held open on that service's
//! own read of it ([`kinesis_video_readiness`]). A stream's teardown is asynchronous the same
//! way ("this method marks the stream for deletion"), so its deletion is finished once that
//! service no longer holds the stream ([`kinesis_video_removal`]).

use super::*;

/// Which of the two Kinesis Video Streams resources an arm is serving: the arms differ only in
/// the operation names, the members those operations spell the resource's name, ARN and record
/// with, and the tagging surface each resource's tags live on — so the one pair of arms serves
/// both types and the catalogue still lists each type separately
/// ([`super::provisioner_for`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resource {
    /// `AWS::KinesisVideo::Stream`: the video stream `CreateStream` makes.
    Stream,
    /// `AWS::KinesisVideo::SignalingChannel`: the WebRTC signaling channel
    /// `CreateSignalingChannel` makes.
    SignalingChannel,
}

/// Amazon Kinesis Video Streams' signing name, from its model (`sigv4Name: kinesisvideo`),
/// which speaks `restJson1` — so its calls ride on [`call_rest_json_named`], each on the URI
/// its own operation's `@http` binding declares, naming the operation so a refusal is reported
/// the way that service's own SDK raises it.
const KINESISVIDEO_SIGNING_NAME: &str = "kinesisvideo";

/// The error code that service answers for a stream or a channel it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of every read, update and delete it
/// publishes).
const KINESISVIDEO_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `Status` a stream or a channel is usable in: the terminal state of a create, which is
/// where a client can write media to a stream and signal over a channel.
const KINESISVIDEO_ACTIVE: &str = "ACTIVE";

/// The `Status` values a stream or a channel is still SETTLING in: `CREATING` is where a create
/// starts ("`CreateStream` is an asynchronous operation") and `UPDATING` where an accepted
/// reconfiguration runs; in neither has the change the resource waits on landed. `DELETING` is
/// not one — a teardown is what [`kinesis_video_removal`] waits on — so a resource reported in
/// it is one nothing is going to bring up.
const KINESISVIDEO_SETTLING: &[&str] = &["CREATING", "UPDATING"];

/// The `Status` a stream is reported in once it has been MARKED for deletion — "this method
/// marks the stream for deletion" — which is the state a teardown already under way is waited
/// out in ([`kinesis_video_removal`]).
const KINESISVIDEO_DELETING: &str = "DELETING";

/// The attribute either type publishes: the ARN that service minted for the resource.
const ARN_ATTRIBUTE: &str = "Arn";

/// The `AWS::KinesisVideo::Stream` properties `CreateStream` takes under its own member name,
/// as `(property, member)` pairs.
///
/// Source: the type's own resource reference against `CreateStreamInput`
/// (`smithy_query(query="operation", operation="CreateStream")` on that model), which spells
/// each member exactly as the type spells the property.
///
/// `Name` is not here because the stream's name is its physical resource id, which the create
/// below sends as that operation's own `StreamName`; `Tags` is not because the tag set the
/// resource must end up carrying is more than the declaration's own ([`tags_of`]).
const STREAM_MEMBERS: &[(&str, &str)] = &[
    ("DataRetentionInHours", "DataRetentionInHours"),
    ("DeviceName", "DeviceName"),
    ("KmsKeyId", "KmsKeyId"),
    ("MediaType", "MediaType"),
];

/// Every `AWS::KinesisVideo::Stream` property this arm carries onto the owning API, so a
/// declaration stating one outside the set is REFUSED rather than reported as created without
/// it ([`refuse_unmapped_properties`]).
const STREAM_MAPPED_PROPERTIES: &[&str] = &[
    "DataRetentionInHours",
    "DeviceName",
    "KmsKeyId",
    "MediaType",
    "Name",
    "Tags",
];

/// Every `AWS::KinesisVideo::SignalingChannel` property this arm carries onto the owning API,
/// for the same reason: the channel's type, the retention window of its undelivered messages,
/// its name and its tags.
const CHANNEL_MAPPED_PROPERTIES: &[&str] = &["MessageTtlSeconds", "Name", "Tags", "Type"];

/// The property a stream's retention window is declared in, and the member `CreateStream` takes
/// it as (`DataRetentionInHours`, `@range min 0`): a NUMBER on the wire, so it is read in the
/// one spelling the member takes ([`declared_number`]) before the request is sent.
const DATA_RETENTION_PROPERTY: &str = "DataRetentionInHours";

/// The property a channel's own type is declared in (`Type`), and the member
/// `CreateSignalingChannel` takes it as (`ChannelType`), which that model spells with the
/// resource's noun.
const CHANNEL_TYPE_PROPERTY: &str = "Type";
const CHANNEL_TYPE_MEMBER: &str = "ChannelType";

/// The property a channel's message retention window is declared in, and the structure and
/// member that call carries it inside
/// (`SingleMasterConfiguration$MessageTtlSeconds`, `@range 5..120`) — a NUMBER on the wire,
/// read the same way the stream's retention is.
const MESSAGE_TTL_PROPERTY: &str = "MessageTtlSeconds";
const SINGLE_MASTER_CONFIGURATION_MEMBER: &str = "SingleMasterConfiguration";
const MESSAGE_TTL_MEMBER: &str = "MessageTtlSeconds";

/// The members of a stream's declaration that only `UpdateStream` could carry to a stream that
/// already exists, which this emulator does not serve: that operation is still a scaffold stub
/// in the owning crate, so a declaration changing one of them is reported as not applied rather
/// than as an update that dropped it ([`update_kinesis_video_resource`]).
const STREAM_UNCHANGEABLE_PROPERTIES: &[&str] = &["DeviceName", "MediaType"];

/// The members `DescribeStream` reports a stream's declaration back under, as
/// `(property, member)` pairs, for drift detection — the scalars of `StreamInfo`
/// (`smithy_query(query="shape", shape="StreamInfo")`) this type declares as properties.
const STREAM_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("DataRetentionInHours", "DataRetentionInHours"),
    ("DeviceName", "DeviceName"),
    ("KmsKeyId", "KmsKeyId"),
    ("MediaType", "MediaType"),
    ("Name", "StreamName"),
];

/// The members `DescribeSignalingChannel` reports a channel's declaration back under, from
/// `ChannelInfo` — its own name and type. Its `MessageTtlSeconds` is nested inside
/// `SingleMasterConfiguration` rather than reported as a scalar of the record, which is the
/// position every non-scalar property is in ([`super::drift`]).
const CHANNEL_REPORTED_PROPERTIES: &[(&str, &str)] =
    &[("Name", "ChannelName"), ("Type", "ChannelType")];

impl Resource {
    /// Every property of the type this arm carries onto the owning API.
    fn mapped_properties(self) -> &'static [&'static str] {
        match self {
            Self::Stream => STREAM_MAPPED_PROPERTIES,
            Self::SignalingChannel => CHANNEL_MAPPED_PROPERTIES,
        }
    }

    /// The operation that makes the resource, and the URI its `@http` binding binds it to.
    fn create(self) -> (&'static str, &'static str) {
        match self {
            Self::Stream => ("CreateStream", "/createStream"),
            Self::SignalingChannel => ("CreateSignalingChannel", "/createSignalingChannel"),
        }
    }

    /// The operation that reads one resource, and its URI.
    fn describe(self) -> (&'static str, &'static str) {
        match self {
            Self::Stream => ("DescribeStream", "/describeStream"),
            Self::SignalingChannel => {
                ("DescribeSignalingChannel", "/describeSignalingChannel")
            }
        }
    }

    /// The operation that removes one resource, and its URI.
    fn delete(self) -> (&'static str, &'static str) {
        match self {
            Self::Stream => ("DeleteStream", "/deleteStream"),
            Self::SignalingChannel => ("DeleteSignalingChannel", "/deleteSignalingChannel"),
        }
    }

    /// The member every call of that service spells the resource's NAME with, which is this
    /// resource's physical id.
    fn name_member(self) -> &'static str {
        match self {
            Self::Stream => "StreamName",
            Self::SignalingChannel => "ChannelName",
        }
    }

    /// The member it spells the resource's ARN with, on the create's answer and on every call
    /// that takes one.
    fn arn_member(self) -> &'static str {
        match self {
            Self::Stream => "StreamARN",
            Self::SignalingChannel => "ChannelARN",
        }
    }

    /// The member the read answers the resource's RECORD in (`StreamInfo`, `ChannelInfo`).
    fn record_member(self) -> &'static str {
        match self {
            Self::Stream => "StreamInfo",
            Self::SignalingChannel => "ChannelInfo",
        }
    }

    /// The member that record reports the resource's STATUS in: a stream's is `Status` and a
    /// channel's `ChannelStatus`, both of the one `Status` value set.
    fn status_member(self) -> &'static str {
        match self {
            Self::Stream => "Status",
            Self::SignalingChannel => "ChannelStatus",
        }
    }

    /// The members this type reports back under its own property names, for drift detection.
    fn reported_properties(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Stream => STREAM_REPORTED_PROPERTIES,
            Self::SignalingChannel => CHANNEL_REPORTED_PROPERTIES,
        }
    }

    /// What the resource is called in a sentence this module words about it.
    fn noun(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::SignalingChannel => "signaling channel",
        }
    }
}

/// Create the stream or the signaling channel the declaration asks for, in Amazon Kinesis Video
/// Streams — so a client that deployed the stack reads it back through that service's own
/// `DescribeStream`/`DescribeSignalingChannel`, finds it in `ListStreams`/`ListSignalingChannels`
/// and reaches the ARN the stack's outputs publish, rather than a `ResourceNotFoundException`
/// for a resource nothing created.
///
/// The declaration IS the request, under the name that is this resource's physical id, with the
/// declared tags beside the ones CloudFormation gives the resource ([`tags_of`]) in the shape
/// each create takes them: a stream's `Tags` is the `ResourceTags` MAP of key to value, a
/// channel's the `TagOnCreateList` of `{Key, Value}` structures.
///
/// The ARN that service minted is the one value the type publishes, so it is read off its own
/// answer; the resource is left `CREATING`, and [`kinesis_video_readiness`] is what its
/// creation is held open on.
pub(super) async fn create_kinesis_video_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, kind.mapped_properties())?;
    let declared = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        kind.name_member().to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    match kind {
        Resource::Stream => {
            copy_members(&declared, STREAM_MEMBERS, &mut request);
            carried_as_a_number(&mut request, DATA_RETENTION_PROPERTY);
            if !tags.is_empty() {
                request.insert("Tags".to_string(), Value::Object(tags_as_map(&tags)));
            }
        }
        Resource::SignalingChannel => {
            if let Some(channel_type) = declared.get(CHANNEL_TYPE_PROPERTY) {
                request.insert(CHANNEL_TYPE_MEMBER.to_string(), channel_type.clone());
            }
            if let Some(configuration) = single_master_configuration(&resource.properties) {
                request.insert(
                    SINGLE_MASTER_CONFIGURATION_MEMBER.to_string(),
                    configuration,
                );
            }
            if !tags.is_empty() {
                request.insert(
                    "Tags".to_string(),
                    tags_as_list(&tags, UPPER_CASED_TAG),
                );
            }
        }
    }
    let (operation, uri) = kind.create();
    let answered = call_rest_json_named(
        ctx,
        KINESISVIDEO_SIGNING_NAME,
        region,
        operation,
        "POST",
        uri,
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = reported_text(&answered, kind.arn_member()).unwrap_or_default();
    if arn.is_empty() {
        // There would be nothing for the stack to publish and no handle the delete could
        // address, so this is a failed create rather than a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: format!(
                "the Kinesis Video Streams control plane answered a created {} without an ARN",
                kind.noun()
            ),
        });
    }
    Ok(Created::attributes(
        [(ARN_ATTRIBUTE.to_string(), arn)].into_iter().collect(),
    ))
}

/// Whether Amazon Kinesis Video Streams has finished bringing up the resource a stack created.
///
/// A created stream and a created channel are both `CREATING` — neither serves a client in that
/// state — so [`KINESISVIDEO_ACTIVE`] is where the creation is finished,
/// [`KINESISVIDEO_SETTLING`] are the states it is still running in, and every other status is
/// one no wait brings the resource out of (one being torn down out of this stack's reach). A
/// resource that service no longer holds is in the same position, nothing being left to create
/// it.
pub(super) async fn kinesis_video_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(record) = read_kinesis_video_resource(ctx, region, kind, physical_resource_id).await?
    else {
        return Ok(Readiness::Failed(format!(
            "Amazon Kinesis Video Streams holds no {} {physical_resource_id}",
            kind.noun()
        )));
    };
    let status = reported_text(&record, kind.status_member()).unwrap_or_default();
    if status == KINESISVIDEO_ACTIVE {
        return Ok(Readiness::Ready);
    }
    if KINESISVIDEO_SETTLING.contains(&status.as_str()) {
        return Ok(Readiness::Pending(status));
    }
    Ok(Readiness::Failed(format!(
        "Amazon Kinesis Video Streams reports {} {physical_resource_id} as {status}",
        kind.noun()
    )))
}

/// Whether that service has finished removing the resource a stack deleted.
///
/// `DeleteStream` "marks the stream for deletion" and leaves it `DELETING`, so the stream is
/// still there when the delete answers and the deletion is finished once that service no longer
/// holds it. A channel is gone as its own delete answers, which this reports the same way.
pub(super) async fn kinesis_video_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match read_kinesis_video_resource(ctx, region, kind, physical_resource_id).await? {
        None => Ok(Removal::Gone),
        Some(record) => Ok(Removal::Pending(
            reported_text(&record, kind.status_member()).unwrap_or_default(),
        )),
    }
}

/// Apply a changed declaration to the resource the stack created, through the calls that
/// service publishes for one that already exists.
///
/// A stream's retention window is `UpdateDataRetention`, which takes the DIFFERENCE from the
/// window in force as an increase or a decrease ("the retention period … in hours", with
/// `Operation` naming which way), so the change is computed against what that service reports
/// rather than against the record this stack keeps. A channel's message retention window is
/// `UpdateSignalingChannel`, which takes the whole `SingleMasterConfiguration`. Both take the
/// `CurrentVersion` that service keeps for the resource, so the version is read off its own
/// report of it immediately before the call.
///
/// `Ok(None)` is a change those calls cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it: a stream's `DeviceName` or `MediaType`, which only
/// `UpdateStream` carries and that operation is not served here.
pub(super) async fn update_kinesis_video_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if kind == Resource::Stream
        && STREAM_UNCHANGEABLE_PROPERTIES
            .iter()
            .any(|property| resource.properties.get(property) != declared.get(property))
    {
        return Ok(None);
    }
    let Some(record) =
        read_kinesis_video_resource(ctx, region, kind, &resource.physical_resource_id).await?
    else {
        // That service no longer holds the resource, so there is nothing a change could be
        // applied to and the deployment reports it as not applied.
        return Ok(None);
    };
    let arn = reported_text(&record, kind.arn_member()).unwrap_or_default();
    let version = reported_text(&record, "Version").unwrap_or_default();
    match kind {
        Resource::Stream => {
            let reported = record
                .get(DATA_RETENTION_PROPERTY)
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let wanted = declared
                .get(DATA_RETENTION_PROPERTY)
                .and_then(declared_number)
                .and_then(|value| value.as_i64())
                .unwrap_or_default();
            if wanted != reported {
                let mut request = Map::new();
                request.insert(
                    kind.name_member().to_string(),
                    Value::String(resource.physical_resource_id.clone()),
                );
                request.insert("CurrentVersion".to_string(), Value::String(version));
                request.insert(
                    "Operation".to_string(),
                    Value::String(
                        match wanted > reported {
                            true => "INCREASE_DATA_RETENTION",
                            false => "DECREASE_DATA_RETENTION",
                        }
                        .to_string(),
                    ),
                );
                request.insert(
                    "DataRetentionChangeInHours".to_string(),
                    Value::Number(wanted.abs_diff(reported).into()),
                );
                call_rest_json_named(
                    ctx,
                    KINESISVIDEO_SIGNING_NAME,
                    region,
                    "UpdateDataRetention",
                    "POST",
                    "/updateDataRetention",
                    Some(&Value::Object(request)),
                )
                .await?;
            }
        }
        Resource::SignalingChannel => {
            let Some(configuration) = single_master_configuration(declared) else {
                // A declaration that no longer states the window asks for the default that
                // service applies to a channel made without one, and `UpdateSignalingChannel`
                // has no member that states "the default": its `SingleMasterConfiguration` is
                // the window itself. So a declaration that DROPPED a window it used to state is
                // reported as not applied rather than as a success that left the channel on the
                // window an earlier deployment gave it; one that never stated a window asks for
                // nothing this call carries, and the channel is already on that default.
                return match single_master_configuration(&resource.properties) {
                    Some(_) => Ok(None),
                    None => Ok(Some(Created::attributes(reported_arn(&arn)))),
                };
            };
            let mut request = Map::new();
            request.insert(kind.arn_member().to_string(), Value::String(arn.clone()));
            request.insert("CurrentVersion".to_string(), Value::String(version));
            request.insert(
                SINGLE_MASTER_CONFIGURATION_MEMBER.to_string(),
                configuration,
            );
            call_rest_json_named(
                ctx,
                KINESISVIDEO_SIGNING_NAME,
                region,
                "UpdateSignalingChannel",
                "POST",
                "/updateSignalingChannel",
                Some(&Value::Object(request)),
            )
            .await?;
        }
    }
    Ok(Some(Created::attributes(reported_arn(&arn))))
}

/// Bring the tags of the resource a record names to the ones this deployment gives it, through
/// the tagging calls that service publishes for it.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this deployment
/// no longer gives the resource and is removed, and one in it with another value is re-valued.
/// Where the two already agree nothing is called at all, which is what makes this safe to ask
/// on every deployment.
///
/// A STREAM has a tagging surface of its own, addressed by the stream's name
/// (`TagStream`/`UntagStream`/`ListTagsForStream`), while a CHANNEL is tagged through that
/// service's generic `TagResource`/`UntagResource`/`ListTagsForResource` by the ARN it minted —
/// which is read off its own report of the channel. Both answer a tag read as the `ResourceTags`
/// map of key to value. A resource that service no longer holds has no tags to reconcile.
pub(super) async fn retag_kinesis_video_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    // The one member every tagging call of that surface addresses the resource by: a stream's
    // own name, or the ARN that service minted for the channel.
    let addressing: Map<String, Value> = match kind {
        Resource::Stream => [(
            kind.name_member().to_string(),
            Value::String(resource.physical_resource_id.clone()),
        )]
        .into_iter()
        .collect(),
        Resource::SignalingChannel => {
            let Some(arn) =
                read_kinesis_video_resource(ctx, region, kind, &resource.physical_resource_id)
                    .await?
                    .as_ref()
                    .and_then(|record| reported_text(record, kind.arn_member()))
            else {
                return Ok(());
            };
            [("ResourceARN".to_string(), Value::String(arn))]
                .into_iter()
                .collect()
        }
    };
    let (read, tag, untag) = match kind {
        Resource::Stream => (
            ("ListTagsForStream", "/listTagsForStream"),
            ("TagStream", "/tagStream"),
            ("UntagStream", "/untagStream"),
        ),
        Resource::SignalingChannel => (
            ("ListTagsForResource", "/ListTagsForResource"),
            ("TagResource", "/TagResource"),
            ("UntagResource", "/UntagResource"),
        ),
    };
    let answered = match call_rest_json_named(
        ctx,
        KINESISVIDEO_SIGNING_NAME,
        region,
        read.0,
        "POST",
        read.1,
        Some(&Value::Object(addressing.clone())),
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == KINESISVIDEO_RESOURCE_NOT_FOUND => return Ok(()),
        Err(refusal) => return Err(refusal),
    };
    let held = reported_tag_map(&answered, "Tags");
    let desired = tags_of(declared, propagated);
    let (added, removed) = tag_delta(&held, &desired);
    if !added.is_empty() {
        let mut request = addressing.clone();
        // That service takes a stream's tags as the `ResourceTags` map and a channel's as the
        // `TagList` of `{Key, Value}` structures, which is the one difference between the two
        // tagging calls.
        request.insert(
            "Tags".to_string(),
            match kind {
                Resource::Stream => Value::Object(tags_as_map(&added)),
                Resource::SignalingChannel => tags_as_list(&added, UPPER_CASED_TAG),
            },
        );
        call_rest_json_named(
            ctx,
            KINESISVIDEO_SIGNING_NAME,
            region,
            tag.0,
            "POST",
            tag.1,
            Some(&Value::Object(request)),
        )
        .await?;
    }
    if !removed.is_empty() {
        let mut request = addressing;
        request.insert("TagKeyList".to_string(), tag_keys_as_list(&removed));
        call_rest_json_named(
            ctx,
            KINESISVIDEO_SIGNING_NAME,
            region,
            untag.0,
            "POST",
            untag.1,
            Some(&Value::Object(request)),
        )
        .await?;
    }
    Ok(())
}

/// Delete the stream or the channel a stack created, addressed by the ARN that service minted
/// for it — the one identifier either delete takes, which is read off its own report of the
/// resource rather than rebuilt here.
///
/// A resource that service no longer holds is already in the state the delete asks for, and a
/// stream it has already MARKED for deletion is on its way out of the store: that mark is what
/// its `ResourceInUseException` names, so a resource reported `DELETING` is left to finish
/// rather than refused a second time ([`kinesis_video_removal`] is what waits for it).
pub(super) async fn delete_kinesis_video_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let Some(record) = read_kinesis_video_resource(ctx, region, kind, physical_resource_id).await?
    else {
        return Ok(());
    };
    if reported_text(&record, kind.status_member()).as_deref() == Some(KINESISVIDEO_DELETING) {
        return Ok(());
    }
    let Some(arn) = reported_text(&record, kind.arn_member()).filter(|arn| !arn.is_empty()) else {
        // That service holds the resource but reports no ARN for it, and the ARN is the one
        // identifier either delete takes — so there is nothing this could address, and reporting
        // the delete as issued would leave the deletion waiting on a call nobody made.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: format!(
                "the Kinesis Video Streams control plane reports the {} \
                 {physical_resource_id} without an ARN",
                kind.noun()
            ),
        });
    };
    let (operation, uri) = kind.delete();
    let request: Map<String, Value> = [(kind.arn_member().to_string(), Value::String(arn))]
        .into_iter()
        .collect();
    match call_rest_json_named(
        ctx,
        KINESISVIDEO_SIGNING_NAME,
        region,
        operation,
        "POST",
        uri,
        Some(&Value::Object(request)),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == KINESISVIDEO_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The resource's configuration as that service reports it, for drift detection: its own read
/// of the stream or the channel, by the name the stack published. One it no longer holds was
/// deleted.
pub(super) async fn describe_kinesis_video_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(
        read_kinesis_video_resource(ctx, region, kind, physical_resource_id)
            .await?
            .map(|record| reported_as_properties(&record, kind.reported_properties())),
    )
}

/// The record that service holds under `name`, or `None` when it holds none
/// (`ResourceNotFoundException`, judged on the CODE and never on a message, which carries the
/// request's own identifier).
///
/// Either read takes the NAME and the ARN as an exclusive choice, so exactly the name is sent:
/// it is this resource's physical id and the handle the stack published.
async fn read_kinesis_video_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Resource,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    let (operation, uri) = kind.describe();
    let request: Map<String, Value> = [(
        kind.name_member().to_string(),
        Value::String(name.to_string()),
    )]
    .into_iter()
    .collect();
    match call_rest_json_named(
        ctx,
        KINESISVIDEO_SIGNING_NAME,
        region,
        operation,
        "POST",
        uri,
        Some(&Value::Object(request)),
    )
    .await
    {
        Ok(answered) => Ok(answered.get(kind.record_member()).cloned()),
        Err(refusal) if refusal.code == KINESISVIDEO_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The one attribute either type publishes, as the resource's attribute map: the ARN that
/// service minted. An empty ARN contributes nothing, so the resource keeps the value the
/// naming rules computed rather than taking an empty one.
fn reported_arn(arn: &str) -> BTreeMap<String, String> {
    match arn.is_empty() {
        true => BTreeMap::new(),
        false => [(ARN_ATTRIBUTE.to_string(), arn.to_string())]
            .into_iter()
            .collect(),
    }
}

/// The `SingleMasterConfiguration` a channel's declaration asks for: the retention window of
/// its undelivered messages, which the type publishes as a flat `MessageTtlSeconds` property
/// and that service takes inside this structure. `None` for a declaration that states no
/// window, which leaves the member off the request — the create then applies that service's own
/// default for it.
fn single_master_configuration(properties: &Value) -> Option<Value> {
    let declared = properties.get(MESSAGE_TTL_PROPERTY)?;
    let carried = declared_number(declared).unwrap_or_else(|| declared.clone());
    Some(Value::Object(
        [(MESSAGE_TTL_MEMBER.to_string(), carried)]
            .into_iter()
            .collect(),
    ))
}

/// Put `member` of `request` into the spelling a NUMBER is carried in ([`declared_number`]).
///
/// A value already in that spelling is left exactly as it came, and so is one that is neither
/// spelling of a number: the owning operation's own validation is what refuses a value that is
/// no number at all, rather than this dropping it.
fn carried_as_a_number(request: &mut Map<String, Value>, member: &str) {
    let Some(converted) = request.get(member).and_then(declared_number) else {
        return;
    };
    request.insert(member.to_string(), converted);
}
