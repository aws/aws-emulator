// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the data and storage services: Amazon Data Firehose,
//! Amazon Kinesis, Amazon CloudWatch, Amazon ECR Public, Amazon OpenSearch Service and
//! OpenSearch Serverless, Amazon S3 and Amazon EMR. One file of
//! [`super`]'s per-type arms, dispatched to by its `Provisioner`.

use super::*;

/// The `ReplicationConfiguration` an `AWS::S3::Bucket` declares, which is a concern of its
/// own: the property is the whole of Amazon S3's replication request shape, so its reading
/// lives beside this file rather than inside it.
mod bucket_replication;

use bucket_replication::declared_replication_configuration;

/// Amazon Data Firehose's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: firehose`, service shape `Firehose_20150804`), which speaks
/// `awsJson1_1`.
pub(super) const FIREHOSE_SIGNING_NAME: &str = "firehose";
pub(super) const FIREHOSE_TARGET_PREFIX: &str = "Firehose_20150804";

/// The error code Firehose answers for a stream it does not have.
pub(super) const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Create the stream an `AWS::KinesisFirehose::DeliveryStream` declares.
///
/// The type's properties are the `CreateDeliveryStream` request's own members — the
/// destination configurations, the source configuration, `DeliveryStreamType`,
/// `DeliveryStreamEncryptionConfigurationInput` and `Tags` are named identically on both
/// sides — so the resolved properties ARE the request, with the one member CloudFormation
/// decides (the stream's name, which is this resource's physical id) set to it.
///
/// `Tags` is the one member the request does not carry verbatim: the stream carries the
/// tags CloudFormation gives it beside the declared ones ([`tags_of`]), as the `[{Key,
/// Value}]` list Firehose's own `Tag` shape spells — less the `aws:cloudformation:*` ones,
/// which `CreateDeliveryStream` refuses to a caller like the other reserved-namespace APIs
/// ([`Provisioner::applicable_tags`]).
pub(super) async fn create_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert(
        "DeliveryStreamName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.CreateDeliveryStream"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("DeliveryStreamARN").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created::attributes(attributes))
}

/// Delete the stream a stack created. A stream Firehose no longer has
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
pub(super) async fn delete_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "DeliveryStreamName": physical_resource_id });
    match call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.DeleteDeliveryStream"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        // The stream is already in the state the delete asks for. Judged on the error
        // CODE, never on the message, which carries the request's own resource name.
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The `DeliveryStreamStatus` a stream takes data in: "After the Firehose stream is created,
/// its status is ACTIVE and it now accepts data" (`CreateDeliveryStream`).
pub(super) const DELIVERY_STREAM_ACTIVE: &str = "ACTIVE";

/// The `DeliveryStreamStatus` a created stream starts in: "The initial status of the Firehose
/// stream is CREATING" (`CreateDeliveryStream`).
pub(super) const DELIVERY_STREAM_CREATING: &str = "CREATING";

/// Whether Amazon Data Firehose has finished bringing up the stream a stack created.
///
/// `CreateDeliveryStream` "is an asynchronous operation that immediately returns. The initial
/// status of the Firehose stream is CREATING. After the Firehose stream is created, its status
/// is ACTIVE and it now accepts data. If the Firehose stream creation fails, the status
/// transitions to CREATING_FAILED" — so `ACTIVE` is the state the resource's creation is
/// finished in, `CREATING` the state it is still coming up in, and every other status is one
/// no wait will bring a stream out of: a creation that failed, which "doesn't change", and a
/// stream being deleted. A stream Firehose no longer holds is in the same position, since
/// nothing is going to create it.
pub(super) async fn delivery_stream_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(description) = describe_delivery_stream(ctx, region, physical_resource_id).await?
    else {
        return Ok(Readiness::Failed(format!(
            "Amazon Data Firehose holds no stream {physical_resource_id}"
        )));
    };
    let status = description
        .get("DeliveryStreamStatus")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match status {
        DELIVERY_STREAM_ACTIVE => Ok(Readiness::Ready),
        DELIVERY_STREAM_CREATING => Ok(Readiness::Pending(DELIVERY_STREAM_CREATING.to_string())),
        other => {
            // `DeliveryStreamDescription$FailureDescription` carries "a message providing
            // details about the error that caused the failure" where Firehose reported one,
            // and the status speaks for itself otherwise.
            let details = description
                .get("FailureDescription")
                .and_then(|failure| failure.get("Details"))
                .and_then(Value::as_str);
            Ok(Readiness::Failed(match details {
                Some(details) => format!(
                    "Amazon Data Firehose reports stream {physical_resource_id} as \
                     {other}: {details}"
                ),
                None => format!(
                    "Amazon Data Firehose reports stream {physical_resource_id} as {other}"
                ),
            }))
        }
    }
}

/// The `DeliveryStreamStatus` a stream whose deletion Amazon Data Firehose has accepted is
/// reported in: "When an API request to DeleteDeliveryStream succeeds, the Firehose stream is
/// marked for deletion, and it goes into the DELETING state" (`DeleteDeliveryStream`).
pub(super) const DELIVERY_STREAM_DELETING: &str = "DELETING";

/// Whether Amazon Data Firehose has finished removing the stream a stack deleted.
///
/// "Removal of a Firehose stream that is in the DELETING state is a low priority operation
/// for the service. A stream may remain in the DELETING state for several minutes"
/// (`DeleteDeliveryStream`), so a stream it still reports is one the removal has not reached
/// yet, and the deletion is finished when that service answers that it holds no such stream.
/// A stream reported in any OTHER status is one no delete of this stack's is going to remove
/// — a client of that service created it again under the same name — so there is nothing left
/// to wait for either.
pub(super) async fn delivery_stream_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    let Some(description) = describe_delivery_stream(ctx, region, physical_resource_id).await?
    else {
        return Ok(Removal::Gone);
    };
    let status = description
        .get("DeliveryStreamStatus")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status == DELIVERY_STREAM_DELETING {
        return Ok(Removal::Pending(status.to_string()));
    }
    Ok(Removal::Gone)
}

/// The stream's configuration as Amazon Data Firehose reports it, for drift detection.
/// A stream it no longer has (`ResourceNotFoundException`) was deleted.
pub(super) async fn describe_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "DeliveryStreamName": physical_resource_id });
    match call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.DescribeDeliveryStream"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "DeliveryStreamDescription"))),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon Kinesis Data Streams: AWS::Kinesis::Stream
// ---------------------------------------------------------------------------

/// Amazon Kinesis Data Streams' signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: kinesis`, service shape `Kinesis_20131202`), which speaks `awsJson1_1`.
pub(super) const KINESIS_SIGNING_NAME: &str = "kinesis";
pub(super) const KINESIS_TARGET_PREFIX: &str = "Kinesis_20131202";

/// The error code Amazon Kinesis Data Streams answers for a stream it holds none of.
pub(super) const KINESIS_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `StreamStatus` a stream serves its callers in: "ACTIVE - The stream exists and is
/// ready for read and write operations or deletion. You should perform read and write
/// operations only on an ACTIVE stream" (`StreamDescriptionSummary$StreamStatus`).
pub(super) const KINESIS_STREAM_ACTIVE: &str = "ACTIVE";

/// The `StreamStatus` a created stream starts in ("CREATING - The stream is being created")
/// and the one an accepted change leaves it in ("UPDATING"): the two that service brings a
/// stream out of by itself.
pub(super) const KINESIS_STREAM_CREATING: &str = "CREATING";
pub(super) const KINESIS_STREAM_UPDATING: &str = "UPDATING";

/// The `StreamStatus` of a stream whose deletion that service has accepted: "DELETING - The
/// stream is being deleted", which is the state a delete of the resource asks for.
pub(super) const KINESIS_STREAM_DELETING: &str = "DELETING";

/// The retention window a stream carries until something changes it: "The default value is
/// 24" (`CreateStream`'s own documented default retention period), which is the window the
/// create this arm makes leaves the stream with. So a declaration stating exactly it asks
/// for no change at all.
pub(super) const KINESIS_DEFAULT_RETENTION_HOURS: i32 = 24;

/// Every property of `AWS::Kinesis::Stream` this arm carries to Amazon Kinesis Data Streams
/// (AWS's resource reference for the type): the name CloudFormation decides, the two
/// `CreateStream` takes as its own members, the tag set the create carries, and the two a
/// SECOND call applies to the stream that create made.
///
/// A property outside this set reaches no call, so a declaration stating one is REFUSED
/// rather than deployed as a stream configured differently from its declaration.
pub(super) const KINESIS_STREAM_PROPERTIES: &[&str] = &[
    "Name",
    "ShardCount",
    "StreamModeDetails",
    "Tags",
    KINESIS_RETENTION_PROPERTY,
    KINESIS_STREAM_ENCRYPTION_PROPERTY,
];

/// The `AWS::Kinesis::Stream` properties `CreateStream` takes under the SAME member name, as
/// `(property, member)` pairs: the capacity mode, which `CreateStreamInput$StreamModeDetails`
/// spells identically, with the one member (`StreamMode`) its structure carries.
///
/// `ShardCount` is not here although that call takes it too: the member is modelled an
/// integer and a template may write a whole number as a quoted scalar, so it goes through
/// [`whole_number`]. `Name` is not here because it is the one value CloudFormation decides,
/// `Tags` because the create takes a tag MAP while the type publishes a tag LIST, and the
/// last two because they are no members of that call at all.
pub(super) const KINESIS_STREAM_CREATE_MEMBERS: &[(&str, &str)] =
    &[("StreamModeDetails", "StreamModeDetails")];

/// The property that asks for server-side encryption on the stream: the `EncryptionType` and
/// the `KeyId` `StartStreamEncryption` takes under those same member names.
pub(super) const KINESIS_STREAM_ENCRYPTION_PROPERTY: &str = "StreamEncryption";

/// The property that asks for a retention window other than the default one.
pub(super) const KINESIS_RETENTION_PROPERTY: &str = "RetentionPeriodHours";

/// One property of the type that carries a WHOLE NUMBER, in either spelling a template gives
/// one ([`whole_number`]); `None` for a declaration that does not state it at all.
///
/// A property the declaration DOES state and that is no whole number is REFUSED rather than
/// left out of the call: the members these two reach are modelled integers, so sending
/// nothing for one would deploy a stream configured differently from its declaration.
pub(super) fn kinesis_whole_number(
    properties: &Map<String, Value>,
    property: &str,
) -> Result<Option<i32>, Refusal> {
    if !properties.contains_key(property) {
        return Ok(None);
    }
    whole_number(properties.get(property))
        .map(Some)
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{KINESIS_STREAM}'s {property} is no whole number"),
        })
}

/// The `StreamDescriptionSummary` members that service reports under this type's own
/// property names, as `(property, member)` pairs, for drift detection.
///
/// `ShardCount` is read off `OpenShardCount` — "The number of open shards in the stream",
/// which is what a declared count provisions. `Tags` is left out for the reason it is
/// everywhere else here (the tags a resource carries change independently of its
/// declaration), and [`KINESIS_STREAM_ENCRYPTION_PROPERTY`] because the summary reports its
/// two members FLAT rather than as the structure the property is, and a stream encrypted
/// under an alias reports the key that service resolved the alias to — so comparing either
/// would need a mapping this module would have to invent, and a property the actual document
/// does not carry is not evidence of anything ([`super::drift`]).
pub(super) const KINESIS_STREAM_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Name", "StreamName"),
    ("RetentionPeriodHours", "RetentionPeriodHours"),
    ("ShardCount", "OpenShardCount"),
    ("StreamModeDetails", "StreamModeDetails"),
];

/// How often this arm asks Amazon Kinesis Data Streams whether a stream has come out of the
/// transient status a call left it in, and how many times.
///
/// The product is an upper bound on the wait, not a duration anything is owed: that service
/// owes the stream `ACTIVE` a moment after the create ("Upon receiving a CreateStream
/// request, Kinesis Data Streams immediately returns and sets the stream status to CREATING.
/// After the stream is created, Kinesis Data Streams sets the stream status to ACTIVE"), the
/// check is far shorter than that moment, and the whole wait is a small multiple of it —
/// enough that a loaded machine still gets there, and bounded so one stack operation cannot
/// hold its request open.
pub(super) const KINESIS_STREAM_CHECK_INTERVAL: ::std::time::Duration =
    ::std::time::Duration::from_millis(25);
pub(super) const KINESIS_STREAM_CHECKS: usize = 80;

/// Create the data stream an `AWS::Kinesis::Stream` declares — so a client that deployed the
/// stack and then calls `kinesis DescribeStreamSummary`, `ListTagsForResource` or `PutRecord`
/// with the name or the ARN the stack publishes reaches the stream rather than a
/// `ResourceNotFoundException`.
///
/// "Ref returns the stream name", which is this resource's physical id and the identifier
/// every Kinesis operation addresses a stream by, so the stream is created under it and the
/// id the stack already computed stays in force. `Fn::GetAtt Arn` is re-pointed at the ARN
/// that service keyed the stream by, read back off its own summary.
///
/// The stream carries the tags the declaration asks for plus the stack's own ([`tags_of`]),
/// as `CreateStream`'s own tag MAP — the one member of that call this arm does not take
/// verbatim off the declaration, the type publishing its `Tags` as a list.
///
/// The type's other two properties are no members of that create: a retention window and
/// server-side encryption are applied to the stream it made, which is what
/// [`configure_kinesis_stream`] does. The resource the template declares is the stream AND
/// that configuration, so a configuration that service refuses fails the resource, and the
/// stream this arm already created is removed before that refusal propagates (the stack
/// records no physical id for a resource whose create failed, so nothing else would ever
/// delete it).
pub(super) async fn create_kinesis_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if let Some(unmapped) = properties
        .keys()
        .find(|property| !KINESIS_STREAM_PROPERTIES.contains(&property.as_str()))
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not carry the {unmapped} property of an {KINESIS_STREAM} \
                 to Amazon Kinesis Data Streams"
            ),
        });
    }

    // Both whole-number properties are read BEFORE anything is created, so a declaration
    // this arm cannot carry leaves no stream behind.
    let shards = kinesis_whole_number(&properties, "ShardCount")?;
    let retention = kinesis_whole_number(&properties, KINESIS_RETENTION_PROPERTY)?
        .filter(|hours| *hours != KINESIS_DEFAULT_RETENTION_HOURS);

    let name = resource.physical_resource_id.clone();
    let mut request = Map::new();
    request.insert("StreamName".to_string(), Value::String(name.clone()));
    copy_members(&properties, KINESIS_STREAM_CREATE_MEMBERS, &mut request);
    if let Some(shards) = shards {
        request.insert("ShardCount".to_string(), Value::Number(shards.into()));
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("Tags".to_string(), Value::Object(tags));
    }
    call_kinesis(ctx, region, "CreateStream", &Value::Object(request)).await?;

    if let Err(refusal) =
        configure_kinesis_stream(ctx, region, &name, &properties, retention).await
    {
        delete_kinesis_stream(ctx, region, &name)
            .await
            .map_err(|failure| Refusal {
                code: refusal.code.clone(),
                message: format!("{refusal}; the stream it created was left behind: {failure}"),
            })?;
        return Err(refusal);
    }

    // The ARN is the one that service keyed the stream by, read off its own summary rather
    // than computed here: a create whose stream that service then reports no ARN for has
    // published nothing `Fn::GetAtt Arn` could resolve to, so it fails rather than leaving
    // the resource holding a value the owning service does not know.
    let arn = describe_kinesis_stream_summary(ctx, region, &name)
        .await?
        .as_ref()
        .and_then(|summary| summary.get("StreamARN"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "the created stream carries no StreamARN".to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn);
    Ok(Created::attributes(attributes))
}

/// Apply the two properties `CreateStream` takes no member for to the stream it just created:
/// the retention window, which `IncreaseStreamRetentionPeriod` and
/// `DecreaseStreamRetentionPeriod` change in either direction from the default the create
/// left ([`KINESIS_DEFAULT_RETENTION_HOURS`]), and the server-side encryption
/// `StartStreamEncryption` turns on.
///
/// Both calls publish the same precondition — "For successful operation, the resource must be
/// in the ACTIVE state" — and the create leaves the stream `CREATING`, so this waits for that
/// service to report the stream out of that status ([`awaited_kinesis_stream_status`]) before
/// either is made. A declaration asking for neither property waits for nothing: the create
/// already said everything there was to say about such a stream.
pub(super) async fn configure_kinesis_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Map<String, Value>,
    retention: Option<i32>,
) -> Result<(), Refusal> {
    let encryption = properties.get(KINESIS_STREAM_ENCRYPTION_PROPERTY);
    if retention.is_none() && encryption.is_none() {
        return Ok(());
    }
    awaited_kinesis_stream_status(ctx, region, name).await?;

    if let Some(hours) = retention {
        // "If you choose a longer stream retention period, this operation increases the time
        // period during which records that have not yet expired are accessible", and the
        // sibling call is the one that shortens it: which of the two carries a declared
        // window is decided by the window the create left.
        let operation = if hours > KINESIS_DEFAULT_RETENTION_HOURS {
            "IncreaseStreamRetentionPeriod"
        } else {
            "DecreaseStreamRetentionPeriod"
        };
        call_kinesis(
            ctx,
            region,
            operation,
            &serde_json::json!({ "StreamName": name, "RetentionPeriodHours": hours }),
        )
        .await?;
    }

    if let Some(encryption) = encryption {
        let (Some(encryption_type), Some(key_id)) = (
            encryption.get("EncryptionType").and_then(scalar_text),
            encryption.get("KeyId").and_then(scalar_text),
        ) else {
            // Both members are required by the type, so a declaration carrying neither
            // describes no encryption that could be turned on.
            return Err(Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "{KINESIS_STREAM}'s {KINESIS_STREAM_ENCRYPTION_PROPERTY} declares no \
                     EncryptionType and KeyId"
                ),
            });
        };
        call_kinesis(
            ctx,
            region,
            "StartStreamEncryption",
            &serde_json::json!({
                "StreamName": name,
                "EncryptionType": encryption_type,
                "KeyId": key_id,
            }),
        )
        .await?;
    }
    Ok(())
}

/// Wait until Amazon Kinesis Data Streams reports the stream out of the transient status a
/// call left it in, and answer the status it reports then; `None` for a stream that service
/// no longer holds.
///
/// A wait that runs out answers the last status read: every call that follows one publishes
/// its own precondition on the status, so what a stream still coming up owes its caller is
/// that service's own refusal rather than one invented here.
pub(super) async fn awaited_kinesis_stream_status(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<String>, Refusal> {
    let mut reported = String::new();
    for _ in 0..KINESIS_STREAM_CHECKS {
        let Some(summary) = describe_kinesis_stream_summary(ctx, region, name).await? else {
            return Ok(None);
        };
        reported = kinesis_stream_status(&summary);
        if reported != KINESIS_STREAM_CREATING && reported != KINESIS_STREAM_UPDATING {
            return Ok(Some(reported));
        }
        ::tokio::time::sleep(KINESIS_STREAM_CHECK_INTERVAL).await;
    }
    Ok(Some(reported))
}

/// Whether Amazon Kinesis Data Streams has finished bringing up the stream a stack created.
///
/// `ACTIVE` is the status a stream serves reads and writes in, `CREATING` the one its create
/// left it in and `UPDATING` the one a change this arm made leaves it in — both of which that
/// service brings the stream out of by itself. Every other status is one no wait will bring a
/// stream out of: a stream being deleted, and one that service no longer holds at all, since
/// nothing is going to create it.
pub(super) async fn kinesis_stream_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(summary) = describe_kinesis_stream_summary(ctx, region, physical_resource_id).await?
    else {
        return Ok(Readiness::Failed(format!(
            "Amazon Kinesis Data Streams holds no stream {physical_resource_id}"
        )));
    };
    let status = kinesis_stream_status(&summary);
    match status.as_str() {
        KINESIS_STREAM_ACTIVE => Ok(Readiness::Ready),
        KINESIS_STREAM_CREATING | KINESIS_STREAM_UPDATING => {
            Ok(Readiness::Pending(status.clone()))
        }
        other => Ok(Readiness::Failed(format!(
            "Amazon Kinesis Data Streams reports stream {physical_resource_id} as {other}"
        ))),
    }
}

/// Whether Amazon Kinesis Data Streams has finished removing the stream a stack deleted.
///
/// "DeleteStream is an asynchronous operation … After a DeleteStream request, the specified
/// stream is in the DELETING state until Kinesis Data Streams completes the deletion", so a
/// stream it reports `DELETING` is one the removal has not reached yet, and the deletion is
/// finished when that service answers that it holds no such stream. A stream reported in any
/// OTHER status is one no delete of this stack's is going to remove — a client of that
/// service created it again under the same name — so there is nothing left to wait for
/// either.
pub(super) async fn kinesis_stream_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    let Some(summary) = describe_kinesis_stream_summary(ctx, region, physical_resource_id).await?
    else {
        return Ok(Removal::Gone);
    };
    let status = kinesis_stream_status(&summary);
    if status == KINESIS_STREAM_DELETING {
        return Ok(Removal::Pending(status));
    }
    Ok(Removal::Gone)
}

/// Delete the stream a stack created, addressed by its NAME — this resource's physical id,
/// which is what `Ref` returns for the type.
///
/// "If the stream is in the ACTIVE state, you can delete it", and the delete is refused for a
/// stream in any other state ("For successful operation, the resource must be in the ACTIVE
/// state"), so a stream that service still reports coming up is waited for exactly as the
/// create's own configuration calls wait for it — which is what lets the rollback of a create
/// that failed take down the stream it had just made. A stream it no longer holds
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries the
/// request's own name), and one whose deletion it has already accepted, are already in the
/// state the delete asks for.
pub(super) async fn delete_kinesis_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match awaited_kinesis_stream_status(ctx, region, physical_resource_id).await? {
        None => return Ok(()),
        Some(status) if status == KINESIS_STREAM_DELETING => return Ok(()),
        Some(_) => {}
    }
    let request = serde_json::json!({ "StreamName": physical_resource_id });
    match call_kinesis(ctx, region, "DeleteStream", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == KINESIS_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Bring the tags of the stream a resource record names to the ones this deployment gives it,
/// through Amazon Kinesis Data Streams' own `AddTagsToStream` and `RemoveTagsFromStream`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this
/// deployment no longer gives the resource and is removed, and one in it with another value
/// is re-valued. Where the two sets already agree nothing is called at all, which is what
/// makes this safe to ask on every deployment.
///
/// A stream that service no longer holds has no tags to reconcile.
pub(super) async fn retag_kinesis_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let name = resource.physical_resource_id.clone();
    if describe_kinesis_stream_summary(ctx, region, &name)
        .await?
        .is_none()
    {
        return Ok(());
    }
    let desired = tags_of(declared, propagated);
    // "You can assign up to 50 tags to a Kinesis resource", which is the `Limit` of one tag
    // read, so the first page carries every tag a stream can hold.
    let listed = call_kinesis(
        ctx,
        region,
        "ListTagsForStream",
        &serde_json::json!({ "StreamName": &name }),
    )
    .await?;
    // That service answers its tag read as the same `[{Key, Value}]` list a declaration's
    // `Tags` property is written as, so one reader serves both sides.
    let (added, removed) = tag_delta(&declared_tags(&listed), &desired);
    if !added.is_empty() {
        call_kinesis(
            ctx,
            region,
            "AddTagsToStream",
            &serde_json::json!({
                "StreamName": &name,
                "Tags": Value::Object(tags_as_map(&added)),
            }),
        )
        .await?;
    }
    if !removed.is_empty() {
        call_kinesis(
            ctx,
            region,
            "RemoveTagsFromStream",
            &serde_json::json!({
                "StreamName": &name,
                "TagKeys": tag_keys_as_list(&removed),
            }),
        )
        .await?;
    }
    Ok(())
}

/// The stream's configuration as Amazon Kinesis Data Streams reports it, for drift
/// detection, under this type's own property names
/// ([`KINESIS_STREAM_REPORTED_PROPERTIES`]). A stream it no longer holds was deleted.
pub(super) async fn describe_kinesis_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(
        describe_kinesis_stream_summary(ctx, region, physical_resource_id)
            .await?
            .map(|summary| {
                reported_as_properties(&summary, KINESIS_STREAM_REPORTED_PROPERTIES)
            }),
    )
}

/// The `StreamDescriptionSummary` that service reports for a stream now, or `None` for one it
/// no longer holds (`ResourceNotFoundException`, judged on the CODE alone).
pub(super) async fn describe_kinesis_stream_summary(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "StreamName": name });
    match call_kinesis(ctx, region, "DescribeStreamSummary", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "StreamDescriptionSummary"))),
        Err(refusal) if refusal.code == KINESIS_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `StreamStatus` one reported summary carries, or the empty text for an answer that
/// carries none.
pub(super) fn kinesis_stream_status(summary: &Value) -> String {
    summary
        .get("StreamStatus")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Call one Amazon Kinesis Data Streams operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_kinesis(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        KINESIS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{KINESIS_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon CloudWatch: AWS::CloudWatch::MetricStream
// ---------------------------------------------------------------------------

/// Amazon CloudWatch's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: monitoring`, service shape `GraniteServiceVersion20100801`), whose
/// protocol list starts with `awsJson1_0` — the wire this call is written on, as the
/// service's own suites drive it.
pub(in crate::state) const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";
pub(in crate::state) const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// Create — or update — the metric stream an `AWS::CloudWatch::MetricStream` declares.
///
/// The type's properties ARE `PutMetricStream`'s own members: `FirehoseArn`, `RoleArn`,
/// `OutputFormat`, `IncludeFilters`, `ExcludeFilters`, `StatisticsConfigurations`,
/// `IncludeLinkedAccountsMetrics` and `Tags` are named identically on both sides (AWS's
/// resource reference for the type against `PutMetricStreamInput`), so the resolved
/// property document IS the request, with the one member CloudFormation decides — the
/// stream's name, which is this resource's physical id — set on the way out.
///
/// The same call serves an UPDATE of the stream: "Creates or updates a metric stream …
/// When you use PutMetricStream to create a new metric stream, the stream is created in
/// the running state. If you use it to update an existing stream, the state of the stream
/// is not changed" (`PutMetricStream`). That is why [`put_metric_stream_configuration`] is
/// also the type's [`Provisioner::update`]: a deployment that changes a declared filter
/// reaches CloudWatch as the configuration the template now declares, under the same name.
pub(super) async fn put_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    // The stream carries the tags the declaration asks for plus the ones CloudFormation
    // gives the resource ([`tags_of`]), as the `[{Key, Value}]` list CloudWatch's own `Tag`
    // shape spells. Only on the CREATE: "If you're using PutMetricStream to update an
    // existing stream, you can't change the tags of the existing stream"
    // (`PutMetricStream`), which is why the update path below sends the declaration as it
    // stands.
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        properties.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    put_metric_stream_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &Value::Object(properties),
    )
    .await
}

/// `PutMetricStream` with `properties` as the request and `name` as the one member
/// CloudFormation decides — the call behind both this type's create and its update, so the
/// two cannot send a different request for the same declaration. `properties` is the
/// configuration to put: the resource's own on a create, and the document the deployment
/// DECLARES on an update.
pub(super) async fn put_metric_stream_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let mut request = match properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert("Name".to_string(), Value::String(name.to_string()));
    let reported = call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.PutMetricStream"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("Arn").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created::attributes(attributes))
}

/// Delete the metric stream a stack created: "Permanently deletes the metric stream that
/// you specify."
///
/// `DeleteMetricStream` models no not-found error at all (`InternalServiceFault`,
/// `InvalidParameterValueException` and `MissingRequiredParameterException` are its whole
/// error list), unlike `GetMetricStream`, so a stream CloudWatch no longer holds is not
/// refused and there is no not-found code to treat as success here.
pub(super) async fn delete_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.DeleteMetricStream"),
        &request,
    )
    .await
    .map(|_| ())
}

/// The metric stream's configuration as CloudWatch reports it, for drift detection: the
/// `GetMetricStream` answer, whose members are the type's own property names. A stream
/// CloudWatch no longer holds (`ResourceNotFoundException`) was deleted.
pub(super) async fn describe_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.GetMetricStream"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon CloudWatch: AWS::CloudWatch::Dashboard
// ---------------------------------------------------------------------------

/// The code CloudWatch answers for a dashboard it does not hold
/// (`DashboardNotFoundError`, the 404 `GetDashboard` models: "The specified dashboard does
/// not exist.").
pub(super) const DASHBOARD_NOT_FOUND: &str = "DashboardNotFoundError";

/// Create the dashboard an `AWS::CloudWatch::Dashboard` declares.
///
/// The type's properties are `PutDashboard`'s own members, spelled identically:
/// `DashboardName`, which CloudFormation decides and which is this resource's physical id,
/// `DashboardBody`, and `Tags`.
///
/// The dashboard carries the tags its own declaration asks for plus the ones CloudFormation
/// gives the resource ([`tags_of`]), as the `[{Key, Value}]` list CloudWatch's own `Tag`
/// shape spells. Only on the CREATE: "You can use this parameter only when creating a new
/// dashboard. If you specify Tags when updating an existing dashboard, the tag updates are
/// ignored." (`PutDashboardInput$Tags`), which is why a later deployment's tag change
/// travels in [`retag_dashboard`] instead.
pub(super) async fn put_dashboard(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        properties.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    put_dashboard_body(
        ctx,
        region,
        &resource.physical_resource_id,
        &Value::Object(properties),
    )
    .await
}

/// `PutDashboard` with the `DashboardBody` `properties` declares and `name` as the one
/// member CloudFormation decides — the call behind both this type's create and its update,
/// so the two cannot send a different request for the same declaration.
///
/// The type declares `DashboardBody` as the dashboard document while the request takes the
/// JSON TEXT of one, so an inline object is rendered and a string passes through — the same
/// spelling rule a policy document reaches IAM under ([`document_text`]). A declaration
/// that carries no body sends none, and CloudWatch refuses the call for the required
/// member it is missing rather than this module inventing an empty dashboard.
///
/// The answer's `DashboardValidationMessages` is no return value of the type, and the type
/// publishes no `Fn::GetAtt` attribute at all ("Ref returns the dashboard name"), so
/// nothing of the answer is recorded on the resource.
///
/// `Tags` is carried through as the caller put it on `properties`: the whole tag set on the
/// CREATE ([`put_dashboard`]), and the declaration's own list on an update, where CloudWatch
/// ignores it — an update's tag change travels in [`retag_dashboard`].
pub(super) async fn put_dashboard_body(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert("DashboardName".to_string(), Value::String(name.to_string()));
    if let Some(body) = properties.get("DashboardBody").and_then(document_text) {
        request.insert("DashboardBody".to_string(), Value::String(body));
    }
    if let Some(tags) = properties.get("Tags") {
        request.insert("Tags".to_string(), tags.clone());
    }
    call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.PutDashboard"),
        &Value::Object(request),
    )
    .await?;
    Ok(Created::default())
}

/// Bring the tags of the dashboard a resource record names to the ones this deployment gives
/// it, through CloudWatch's own `TagResource` and `UntagResource`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this
/// deployment no longer gives the resource and is removed, and a key in it with another
/// value is re-valued ("adds or updates the specified tags", `TagResource`). Where the two
/// sets already agree nothing is called at all, which is what makes this safe to ask on
/// every deployment.
///
/// The dashboard's own ARN, as CloudWatch reports it, is what its tagging calls address it
/// by: a dashboard is global, so its ARN carries no Region and is not one this module
/// reconstructs. A dashboard CloudWatch no longer holds has no tags to reconcile.
pub(super) async fn retag_dashboard(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let desired = tags_of(declared, propagated);
    let Some(arn) = describe_dashboard(ctx, region, &resource.physical_resource_id)
        .await?
        .as_ref()
        .and_then(|dashboard| dashboard.get("DashboardArn"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        return Ok(());
    };
    let held = declared_tags(
        // CloudWatch answers its tag read as the same `[{Key, Value}]` list a declaration's
        // `Tags` property is written as, so one reader serves both sides.
        &call_json_rpc(
            ctx,
            CLOUDWATCH_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{CLOUDWATCH_TARGET_PREFIX}.ListTagsForResource"),
            &serde_json::json!({ "ResourceARN": &arn }),
        )
        .await?,
    );
    let (added, removed) = tag_delta(&held, &desired);
    if !added.is_empty() {
        call_json_rpc(
            ctx,
            CLOUDWATCH_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{CLOUDWATCH_TARGET_PREFIX}.TagResource"),
            &serde_json::json!({
                "ResourceARN": &arn,
                "Tags": tags_as_list(&added, UPPER_CASED_TAG),
            }),
        )
        .await?;
    }
    if !removed.is_empty() {
        call_json_rpc(
            ctx,
            CLOUDWATCH_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{CLOUDWATCH_TARGET_PREFIX}.UntagResource"),
            &serde_json::json!({
                "ResourceARN": &arn,
                "TagKeys": tag_keys_as_list(&removed),
            }),
        )
        .await?;
    }
    Ok(())
}

/// Delete the dashboard a stack created: "Deletes all dashboards that you specify."
///
/// `DeleteDashboards` models no not-found error at all
/// (`ConflictException`, `InternalServiceFault` and `InvalidParameterValueException` are its
/// whole error list), unlike `GetDashboard`, so a dashboard CloudWatch no longer holds is
/// not refused and there is no not-found code to treat as success here.
pub(super) async fn delete_dashboard(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "DashboardNames": [physical_resource_id] });
    call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.DeleteDashboards"),
        &request,
    )
    .await
    .map(|_| ())
}

/// The dashboard's configuration as CloudWatch reports it, for drift detection: the
/// `GetDashboard` answer, whose `DashboardName` and `DashboardBody` are the type's own
/// property names. A dashboard CloudWatch no longer holds (`DashboardNotFoundError`) was
/// deleted.
pub(super) async fn describe_dashboard(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "DashboardName": physical_resource_id });
    match call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.GetDashboard"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(answer)),
        // Judged on the error CODE, never on the message, which carries the request's own
        // dashboard name.
        Err(refusal) if refusal.code == DASHBOARD_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon ECR Public: AWS::ECR::PublicRepository
// ---------------------------------------------------------------------------

/// Amazon ECR Public's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: ecr-public`, service shape `SpencerFrontendService`), which speaks
/// `awsJson1_1`.
pub(super) const ECR_PUBLIC_SIGNING_NAME: &str = "ecr-public";
pub(super) const ECR_PUBLIC_TARGET_PREFIX: &str = "SpencerFrontendService";

/// The error code ECR Public answers for a repository it does not hold
/// (`RepositoryNotFoundException`: "The specified repository can't be found").
pub(super) const REPOSITORY_NOT_FOUND: &str = "RepositoryNotFoundException";

/// What `AWS::ECR::PublicRepository`'s `RepositoryCatalogData` property declares, as
/// `CreateRepository`'s `catalogData` member spells it.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member against
/// the `RepositoryCatalogDataInput` shape. `LogoImageBlob` is base64 on both sides (the
/// property is "the base64-encoded repository logo payload", and `awsJson1_1` carries a
/// blob as base64), so it goes through as the template spells it.
pub(super) const CATALOG_DATA_MEMBERS: &[(&str, &str)] = &[
    ("RepositoryDescription", "description"),
    ("AboutText", "aboutText"),
    ("UsageText", "usageText"),
    ("LogoImageBlob", "logoImageBlob"),
    ("Architectures", "architectures"),
    ("OperatingSystems", "operatingSystems"),
];

/// Create the repository an `AWS::ECR::PublicRepository` declares.
///
/// The repository's own ARN is read back off the answer: it is the resource's real
/// identity, so it replaces the one the naming rules computed.
pub(super) async fn create_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let mut request = Map::new();
    // The repository's name is this resource's physical id — "Ref returns the repository
    // name" — already resolved against the stack's parameters.
    request.insert(
        "repositoryName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    if let Some(Value::Object(declared)) = properties.get("RepositoryCatalogData") {
        let mut catalog_data = Map::new();
        for (property, member) in CATALOG_DATA_MEMBERS {
            if let Some(value) = declared.get(*property) {
                catalog_data.insert((*member).to_string(), value.clone());
            }
        }
        request.insert("catalogData".to_string(), Value::Object(catalog_data));
    }
    // `Tags` is a list of `{Key, Value}` on both sides: ECR Public's own `Tag` shape
    // spells its members `Key` and `Value` too, so the declared list goes through as
    // declared, carrying the tags CloudFormation gives the repository beside them.
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.CreateRepository"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("repository")
        .and_then(|repository| repository.get("repositoryArn"))
        .and_then(Value::as_str)
    {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    // The repository was created under the name CloudFormation decided.
    Ok(Created::attributes(attributes))
}

/// Delete the repository a stack created. A repository ECR Public no longer holds
/// (`RepositoryNotFoundException`) is already in the state the delete asks for; one that
/// still holds images is NOT deleted (`RepositoryNotEmptyException`), and that refusal is
/// what the stack reports.
pub(super) async fn delete_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "repositoryName": physical_resource_id });
    match call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.DeleteRepository"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The repository's configuration as Amazon ECR Public reports it, for drift detection. A
/// repository it no longer holds (`RepositoryNotFoundException`, and an answer carrying no
/// repository at all) was deleted.
pub(super) async fn describe_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "repositoryNames": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.DescribeRepositories"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "repositories")),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon ECR: AWS::ECR::Repository
// ---------------------------------------------------------------------------

/// Amazon ECR's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: ecr`, service shape `AmazonEC2ContainerRegistry_V20150921`), which
/// speaks `awsJson1_1`.
pub(super) const ECR_SIGNING_NAME: &str = "ecr";
pub(super) const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// What an `AWS::ECR::Repository`'s declaration states, as `CreateRepository`'s own
/// members spell it: the property name against the member it is sent under.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member
/// against `CreateRepositoryRequest` (`smithy_query(query="operation",
/// operation="CreateRepository")`). The nested documents go through as the template
/// spells them, because each one's members are spelled the same on both sides but for
/// their first letter — `ImageScanningConfiguration.ScanOnPush` against
/// `imageScanningConfiguration.scanOnPush`, `EncryptionConfiguration.EncryptionType` /
/// `.KmsKey` against `encryptionConfiguration.encryptionType` / `.kmsKey` — so they are
/// mapped member by member rather than passed through
/// ([`REPOSITORY_DOCUMENT_MEMBERS`]).
const REPOSITORY_SCALAR_MEMBERS: &[(&str, &str)] = &[("ImageTagMutability", "imageTagMutability")];

/// One nested document of an `AWS::ECR::Repository`: the property it is declared under,
/// the member `CreateRepository` takes it as, and the property-to-member mapping of its
/// own members.
type RepositoryDocument = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

/// The nested documents of an `AWS::ECR::Repository`, as the member each is sent under
/// and the property-to-member mapping inside it. See [`REPOSITORY_SCALAR_MEMBERS`].
const REPOSITORY_DOCUMENT_MEMBERS: &[RepositoryDocument] = &[
    (
        "ImageScanningConfiguration",
        "imageScanningConfiguration",
        &[("ScanOnPush", "scanOnPush")],
    ),
    (
        "EncryptionConfiguration",
        "encryptionConfiguration",
        &[("EncryptionType", "encryptionType"), ("KmsKey", "kmsKey")],
    ),
];

/// Create the repository an `AWS::ECR::Repository` declares, so a client that pushes to
/// or reads the registry the stack declared finds it in Amazon ECR itself.
///
/// The repository's ARN and URI are read back off the answer: they are the registry's
/// real identity for this repository, so they replace the ones the naming rules computed.
/// A declared `RepositoryPolicyText` is the repository's resource-based policy, which
/// that service sets on the repository this call just created.
pub(super) async fn create_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let mut request = Map::new();
    // The repository's name is this resource's physical id — "Ref returns the repository
    // name" — already resolved against the stack's parameters.
    request.insert(
        "repositoryName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    for (property, member) in REPOSITORY_SCALAR_MEMBERS {
        if let Some(value) = properties.get(*property) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    for (property, member, mapping) in REPOSITORY_DOCUMENT_MEMBERS {
        let Some(Value::Object(declared)) = properties.get(*property) else {
            continue;
        };
        let mut document = Map::new();
        for (inner_property, inner_member) in *mapping {
            if let Some(value) = declared.get(*inner_property) {
                document.insert((*inner_member).to_string(), value.clone());
            }
        }
        request.insert((*member).to_string(), Value::Object(document));
    }
    // `Tags` is a list of `{Key, Value}` on both sides: ECR's own `Tag` shape spells its
    // members `Key` and `Value` too, so the declared list goes through as declared,
    // carrying the tags CloudFormation gives the repository beside them.
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        ECR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_TARGET_PREFIX}.CreateRepository"),
        &Value::Object(request),
    )
    .await?;
    if let Some(policy) = repository_policy_text(&properties) {
        call_json_rpc(
            ctx,
            ECR_SIGNING_NAME,
            region,
            AWS_JSON_1_1,
            &format!("{ECR_TARGET_PREFIX}.SetRepositoryPolicy"),
            &serde_json::json!({
                "repositoryName": resource.physical_resource_id,
                "policyText": policy,
            }),
        )
        .await?;
    }
    let repository = reported.get("repository");
    let mut attributes = BTreeMap::new();
    for (member, attribute) in [("repositoryArn", "Arn"), ("repositoryUri", "RepositoryUri")] {
        if let Some(value) = repository
            .and_then(|repository| repository.get(member))
            .and_then(Value::as_str)
        {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    // The repository was created under the name CloudFormation decided.
    Ok(Created::attributes(attributes))
}

/// The repository policy an `AWS::ECR::Repository` declares, as the policy TEXT
/// `SetRepositoryPolicy` takes: the property is a policy document, which a template may
/// write either as JSON text or as the document itself.
fn repository_policy_text(properties: &Map<String, Value>) -> Option<String> {
    match properties.get("RepositoryPolicyText")? {
        Value::String(text) => Some(text.clone()),
        Value::Null => None,
        document => Some(document.to_string()),
    }
}

/// Delete the repository a stack created. A repository ECR no longer holds
/// (`RepositoryNotFoundException`) is already in the state the delete asks for; one that
/// still holds images is NOT deleted unless the declaration asked for that
/// (`EmptyOnDelete`: "if true, deleting the repository force deletes the contents of the
/// repository"), and ECR's own refusal is what the stack reports.
pub(super) async fn delete_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let force = resource
        .properties
        .get("EmptyOnDelete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let request = serde_json::json!({
        "repositoryName": resource.physical_resource_id,
        "force": force,
    });
    match call_json_rpc(
        ctx,
        ECR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_TARGET_PREFIX}.DeleteRepository"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The repository's configuration as Amazon ECR reports it, for drift detection. A
/// repository it no longer holds (`RepositoryNotFoundException`, and an answer carrying
/// no repository at all) was deleted.
pub(super) async fn describe_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "repositoryNames": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        ECR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_TARGET_PREFIX}.DescribeRepositories"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "repositories")),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// OpenSearch Serverless: AWS::OpenSearchServerless::Collection / ::SecurityPolicy
// ---------------------------------------------------------------------------

/// OpenSearch Serverless's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: aoss`, service shape `OpenSearchServerless`).
pub(super) const AOSS_SIGNING_NAME: &str = "aoss";
pub(super) const AOSS_TARGET_PREFIX: &str = "OpenSearchServerless";

/// Create the policy an `AWS::OpenSearchServerless::SecurityPolicy` declares.
///
/// The resource declares `Name`, `Type`, `Policy` and `Description` against an API that
/// takes `name`, `type`, `policy` and `description`, so those four are mapped here. `Ref`
/// on the resource returns the policy's NAME, which is the physical id the deployment
/// already computed from the `Name` property, so the service assigns nothing this record
/// has to take over.
pub(super) async fn create_security_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    request.insert("type".to_string(), Value::String(declared_type(resource)?));
    request.insert(
        "policy".to_string(),
        Value::String(policy_document(resource)?),
    );
    if let Some(description) = property(resource, "Description") {
        request.insert("description".to_string(), Value::String(description));
    }
    call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.CreateSecurityPolicy"),
        &Value::Object(request),
    )
    .await?;
    // `AWS::OpenSearchServerless::SecurityPolicy` publishes no `Fn::GetAtt` attributes, so
    // an attribute read against it yields no value rather than a guessed one.
    Ok(Created::default())
}

/// Delete the policy a stack created, by the `(type, name)` pair that identifies one. A
/// policy the service no longer has is already in the state the delete asks for.
pub(super) async fn delete_security_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let kind = declared_type(resource)?;
    let request = serde_json::json!({
        "type": kind,
        "name": resource.physical_resource_id,
    });
    match call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.DeleteSecurityPolicy"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Create the collection an `AWS::OpenSearchServerless::Collection` declares.
///
/// `Ref` on this resource returns the collection's **Id** — the server-minted
/// `CollectionId`, which is what `BatchGetCollection` and `CreateIndex` address a
/// collection by — so the create's own answer replaces the computed physical id, and
/// `Fn::GetAtt` reads `Id`, `Arn`, `CollectionEndpoint` and `DashboardEndpoint` off what
/// the owning service reports rather than off anything rebuilt here. The resource's
/// `Name`, `Type`, `Description`, `StandbyReplicas` and `Tags` are mapped onto the API's
/// own members, and its `tags` carry the stack's propagated tags beside the declared ones
/// ([`tags_of`]).
pub(super) async fn create_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    for (declared, member) in [
        ("Type", "type"),
        ("Description", "description"),
        ("StandbyReplicas", "standbyReplicas"),
    ] {
        if let Some(value) = property(resource, declared) {
            request.insert(member.to_string(), Value::String(value));
        }
    }
    let tags: Vec<Value> = tags_of(&resource.properties, propagated)
        .into_iter()
        .map(|(key, value)| serde_json::json!({ "key": key, "value": value }))
        .collect();
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Array(tags));
    }
    let reported = call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.CreateCollection"),
        &Value::Object(request),
    )
    .await?;
    let detail = reported
        .get("createCollectionDetail")
        .cloned()
        .unwrap_or(Value::Null);
    let id = detail
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "the created collection carries no id".to_string(),
        })?
        .to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.clone());
    if let Some(arn) = detail.get("arn").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    // The endpoints are derived from the collection's id BY THE OWNING SERVICE, so they
    // are read back from it rather than rebuilt here. A read that does not answer them
    // leaves those attributes unset rather than guessed.
    let read = call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.BatchGetCollection"),
        &serde_json::json!({ "ids": [id] }),
    )
    .await
    .ok();
    let detail = read
        .as_ref()
        .and_then(|answer| answer.get("collectionDetails"))
        .and_then(Value::as_array)
        .and_then(|details| details.first());
    if let Some(detail) = detail {
        for (member, attribute) in [
            ("collectionEndpoint", "CollectionEndpoint"),
            ("dashboardEndpoint", "DashboardEndpoint"),
        ] {
            if let Some(endpoint) = detail.get(member).and_then(Value::as_str) {
                attributes.insert(attribute.to_string(), endpoint.to_string());
            }
        }
    }
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// Delete the collection a stack created, by the id the create reported. A collection the
/// service no longer has is already in the state the delete asks for.
pub(super) async fn delete_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "id": physical_resource_id });
    match call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.DeleteCollection"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon OpenSearch Service: AWS::OpenSearchService::Domain
// ---------------------------------------------------------------------------

/// Amazon OpenSearch Service's signing name, from its model (`sigv4Name: es`). It speaks
/// `restJson1`, so its calls ride on [`call_rest_json`] with the method and URI each
/// operation's own `http` binding declares.
pub(super) const OPENSEARCH_SIGNING_NAME: &str = "es";

/// Where that service binds its domain operations: `CreateDomain` is `POST` on this URI and
/// `DescribeDomain`/`DeleteDomain` are `GET`/`DELETE` on `{DomainName}` under it
/// (`smithy_query(query="operation", operation="CreateDomain,DescribeDomain,DeleteDomain")`).
pub(super) const OPENSEARCH_DOMAIN_URI: &str = "/2021-01-01/opensearch/domain";

/// The properties of an `AWS::OpenSearchService::Domain` that `CreateDomain` does NOT take
/// verbatim:
///
/// * `DomainName` is the one value CloudFormation decides — this resource's physical id,
///   which is what `Ref` returns — so it is set on the way out rather than copied;
/// * `Tags` is the create's own `TagList`, and carries the tags CloudFormation gives the
///   resource beside the declared ones ([`tags_of`]);
/// * `SkipShardMigrationWait` is published for the type's update path and is a member of no
///   call of that service at all, so nothing is sent for it.
///
/// Every other property of the type IS a `CreateDomain` member spelled identically —
/// `AccessPolicies`, `AdvancedOptions`, `AdvancedSecurityOptions`, `ClusterConfig`,
/// `CognitoOptions`, `DomainEndpointOptions`, `EBSOptions`, `EncryptionAtRestOptions`,
/// `EngineVersion`, `IPAddressType`, `IdentityCenterOptions`, `LogPublishingOptions`,
/// `NodeToNodeEncryptionOptions`, `OffPeakWindowOptions`, `SnapshotOptions`,
/// `SoftwareUpdateOptions` and `VPCOptions` — so the evaluated property document IS the
/// request, each property carried as the TYPE its member is modelled with
/// ([`DOMAIN_DOCUMENT_PROPERTIES`]).
pub(super) const DOMAIN_NON_CREATE_PROPERTIES: &[&str] = &["DomainName", "Tags", "SkipShardMigrationWait"];

/// The attribute `AWS::OpenSearchService::Domain` publishes the hosts of a domain reached
/// inside a VPC under: a MAP, keyed as the owning service keys `DomainStatus$Endpoints`, whose
/// entries `Fn::GetAtt` reads one at a time through `DomainEndpoints.<key>`.
pub(super) const DOMAIN_ENDPOINTS_ATTRIBUTE: &str = "DomainEndpoints";

/// The keys that map carries the two hosts of a domain reached inside a VPC under:
/// `(the IPv4 host, the dual-stack host)`.
///
/// Both are the owning service's own, from the member's documentation
/// (`smithy_query(query="doc",shape="DomainStatus",member="Endpoints")`): "the key-value pair
/// that exists if the OpenSearch Service domain uses VPC endpoints … IPv4 IP addresses —
/// `'vpc','vpc-endpoint-….es.amazonaws.com'` … Dual stack IP addresses —
/// `'vpcv2':'vpc-endpoint-….on.aws'`". They are the addresses such a domain IS reached on, so
/// they are what its endpoint attributes report ([`domain_created`]).
pub(super) const DOMAIN_VPC_ENDPOINT_KEYS: (&str, &str) = ("vpc", "vpcv2");

/// The attribute that type publishes the DUAL-STACK endpoint of a domain under, and the
/// `DomainStatus` member the owning service reports it as: "if `IPAddressType` [is] set to
/// `dualstack`, a version 2 domain endpoint is provisioned. This endpoint functions like a
/// normal endpoint, except that it works with both IPv4 and IPv6 IP addresses"
/// (`smithy_query(query="doc",shape="DomainStatus",member="EndpointV2")` of that service's
/// model) — so a domain that service provisions no version 2 endpoint for publishes no such
/// host, and the attribute stays unset rather than carrying the IPv4 one.
pub(super) const DOMAIN_ENDPOINT_V2: (&str, &str) = ("DomainEndpointV2", "EndpointV2");

/// The attributes that type publishes as NESTED members of a container the owning service
/// reports about the domain, each read through the dotted path `Fn::GetAtt` names it by
/// (`ServiceSoftwareOptions.CurrentVersion`), exactly as a `DomainEndpoints.<key>` entry is.
///
/// `(the container the service reports, the members of it the type publishes)`. The member
/// names are that container's own, from the owning service's model
/// (`smithy_query(query="shape",shape="ServiceSoftwareOptions,AdvancedSecurityOptions")`):
/// the service-software record every full descriptor of a domain carries, and the one
/// fine-grained-access-control member of the type's published set. A member the service does
/// not report leaves its attribute unset.
pub(super) const DOMAIN_NESTED_ATTRIBUTES: &[(&str, &[&str])] = &[
    (
        "ServiceSoftwareOptions",
        &[
            "AutomatedUpdateDate",
            "Cancellable",
            "CurrentVersion",
            "Description",
            "NewVersion",
            "OptionalDeployment",
            "UpdateAvailable",
            "UpdateStatus",
        ],
    ),
    ("AdvancedSecurityOptions", &["AnonymousAuthDisableDate"]),
];

/// The properties of an `AWS::OpenSearchService::Domain` whose `CreateDomain` member is
/// modelled a STRING carrying a JSON document, so the property's value is sent as that
/// document's TEXT and not as the JSON the template spelled it with
/// ([`render_document_members`]).
///
/// `AccessPolicies` is the one such property of this type: it targets
/// `com.amazonaws.opensearch#PolicyDocument`, a string of 0..102400 characters
/// (`smithy_query(query="operation", operation="CreateDomain")`), while a template declares
/// an access policy the way every policy-document property is declared — as the policy
/// object itself, with its intrinsics resolved by the time this runs. Every other member
/// this arm forwards is a structure, a map or a list on both sides, or a string the template
/// also writes as a string, so none of them is rendered.
pub(super) const DOMAIN_DOCUMENT_PROPERTIES: &[&str] = &["AccessPolicies"];

/// Create the domain an `AWS::OpenSearchService::Domain` declares.
///
/// VALUES are untouched: the engine version, the cluster configuration and the storage the
/// template declared reach that service as declared, and ITS validation is what judges them
/// — an instance type the requested engine release does not offer is its own
/// `InvalidTypeException`, and that refusal is what fails this resource and rolls the stack
/// back, rather than a domain recorded as created on a cluster nothing could provision. What
/// a property is SPELLED as is the one thing that changes: a property whose member is
/// modelled a string carrying a JSON document is sent as that document's text
/// ([`DOMAIN_DOCUMENT_PROPERTIES`]), so the request carries the same value under the type
/// the member declares.
///
/// "Ref returns the domain name", so the physical resource id the stack reports IS the
/// domain a client addresses and the service assigns no identity this record has to take
/// over.
pub(super) async fn create_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    for property in DOMAIN_NON_CREATE_PROPERTIES {
        request.remove(*property);
    }
    render_document_members(&mut request, DOMAIN_DOCUMENT_PROPERTIES);
    request.insert(
        "DomainName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("TagList".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_rest_json(
        ctx,
        OPENSEARCH_SIGNING_NAME,
        region,
        "POST",
        OPENSEARCH_DOMAIN_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(domain_created(
        &resource.resource_type,
        &reported_member(&reported, "DomainStatus"),
    ))
}

/// What a domain create answered, as the resource's published attributes: the identifiers
/// the OWNING service decided, never ones rebuilt here.
///
/// `DomainStatus$ARN` is the domain's ARN, under this service's own `es` namespace, so it is
/// what the resource's `Arn`/`<Type>Arn` attributes report
/// ([`super::template::arn_attributes`]); `DomainStatus$DomainId` is "the unique identifier
/// for the domain" — the account and the name — which is the `Id` this type publishes, so it
/// re-points the `Id`/`<Type>Id` pair ([`super::template::identity_attributes`]); and
/// `DomainStatus$Endpoint` is "the domain-specific endpoint used to submit index, search, and
/// data upload requests", which is this type's `DomainEndpoint`. A member the answer does not
/// carry leaves its attribute unset rather than guessed.
///
/// An ENDPOINT attribute is the address a client submits its requests to, so it carries the
/// address the owning service published for the domain however that domain is REACHED: a
/// domain provisioned inside a VPC has no public host at all and is addressed on the host
/// that service publishes in `DomainStatus$Endpoints` under `vpc`, with its dual-stack
/// address under `vpcv2` ([`DOMAIN_VPC_ENDPOINT_KEYS`]), so for such a domain those are the
/// `DomainEndpoint` and the `DomainEndpointV2` it publishes. An attribute of a domain that
/// is addressed nowhere yet — one still being provisioned — stays unset until that service
/// reports an address for it, which is what [`domain_ready_attributes`] reads.
///
/// The REST of the type's published set is read off that same answer, so every attribute it
/// publishes carries the value that service holds rather than nothing: the dual-stack host
/// ([`DOMAIN_ENDPOINT_V2`]) and the members the type publishes as nested paths through a
/// container the answer carries ([`DOMAIN_NESTED_ATTRIBUTES`]) — the domain's service-software
/// record and its anonymous-auth migration date. Each is recorded under the dotted name
/// `Fn::GetAtt` reads it by, exactly as a `DomainEndpoints.<key>` entry is, and a member the
/// service does not report is left absent rather than invented.
///
/// A domain reached inside a VPC publishes its hosts in `DomainStatus$Endpoints` too,
/// "the key-value pair that exists if the OpenSearch Service domain uses VPC endpoints",
/// which is this type's [`DOMAIN_ENDPOINTS_ATTRIBUTE`] return value. `Fn::GetAtt` reads one
/// entry of a map-valued attribute through a dotted path, so each host that service reported
/// is recorded under `DomainEndpoints.<key>` — the address a client reaches such a domain on,
/// which no naming rule here could compute, and the only one it has. The map is whatever that
/// service reported rather than a key list restated here, so it is the one open-ended part of
/// the attribute set and is recorded LAST, under the same ceiling as every other attribute of
/// a resource ([`super::template::MAX_ATTRIBUTES_PER_RESOURCE`]) — the fixed members above are
/// the identity of the resource and are never the ones a ceiling drops.
pub(super) fn domain_created(resource_type: &str, status: &Value) -> Created {
    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    if let Some(arn) = status.get("ARN").and_then(Value::as_str) {
        attributes.extend(super::template::arn_attributes(resource_type, arn));
    }
    if let Some(domain_id) = status.get("DomainId").and_then(Value::as_str) {
        attributes.extend(super::template::identity_attributes(
            resource_type,
            domain_id,
        ));
    }
    // The hosts the owning service published for the domain, keyed as it keys them: the
    // addresses a domain reached inside a VPC is addressed at, which are the ones its endpoint
    // attributes report where it is reached that way.
    let published_hosts = reported_endpoints(status);
    let (vpc_key, vpc_dual_stack_key) = DOMAIN_VPC_ENDPOINT_KEYS;
    // The address the domain is reached on is part of its identity, like the ARN and the id
    // above, so it is recorded unconditionally: the attribute ceiling below bounds the
    // open-ended parts of the set, never these.
    if let Some(endpoint) = status
        .get("Endpoint")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| published_hosts.get(vpc_key).cloned())
    {
        attributes.insert("DomainEndpoint".to_string(), endpoint);
    }
    let (dual_stack_attribute, dual_stack_member) = DOMAIN_ENDPOINT_V2;
    if let Some(endpoint) = status
        .get(dual_stack_member)
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| published_hosts.get(vpc_dual_stack_key).cloned())
    {
        attributes.insert(dual_stack_attribute.to_string(), endpoint);
    }
    for (container, members) in DOMAIN_NESTED_ATTRIBUTES {
        let Some(reported) = status.get(*container) else {
            continue;
        };
        for member in *members {
            if attributes.len() >= super::template::MAX_ATTRIBUTES_PER_RESOURCE {
                break;
            }
            if let Some(text) = reported.get(*member).and_then(scalar_text) {
                attributes.insert(format!("{container}.{member}"), text);
            }
        }
    }
    for (key, host) in &published_hosts {
        if attributes.len() >= super::template::MAX_ATTRIBUTES_PER_RESOURCE {
            break;
        }
        attributes.insert(
            format!("{DOMAIN_ENDPOINTS_ATTRIBUTE}.{key}"),
            host.clone(),
        );
    }
    Created::attributes(attributes)
}

/// The hosts `DomainStatus$Endpoints` reports for a domain, keyed as the owning service keys
/// them — empty for a domain that publishes no such map, which is every domain reached on a
/// public endpoint.
fn reported_endpoints(status: &Value) -> BTreeMap<String, String> {
    status
        .get("Endpoints")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, host)| host.as_str().map(|host| (key.clone(), host.to_string())))
        .collect()
}

/// The properties of an `AWS::OpenSearchService::Domain` that `UpdateDomainConfig` takes, so
/// a declaration that states one of them differently is applied to the domain the stack has
/// rather than replacing it.
///
/// Every member of that call which is also a property of this type
/// (`smithy_query(query="operation", operation="UpdateDomainConfig")`), and no other: the
/// engine release a domain runs is NOT a member of it — Amazon OpenSearch Service moves a
/// domain between releases through `UpgradeDomain` — and the name is the resource's identity
/// ([`DOMAIN_NON_CREATE_PROPERTIES`]), so neither reaches this call.
///
/// The list is that CALL's membership, which is why it is wider than the properties named
/// elsewhere in this file: `DOMAIN_NON_CREATE_PROPERTIES` enumerates only the properties the
/// CREATE does not take verbatim, and every other property of the type is a member of both
/// calls under the same name. A property this type does not publish at all compares absent
/// on both sides below and is never sent.
pub(super) const DOMAIN_UPDATE_PROPERTIES: &[&str] = &[
    "AccessPolicies",
    "AIMLOptions",
    "AdvancedOptions",
    "AdvancedSecurityOptions",
    "AutoTuneOptions",
    "AutomatedSnapshotPauseOptions",
    "ClusterConfig",
    "CognitoOptions",
    "DeploymentStrategyOptions",
    "DomainEndpointOptions",
    "EBSOptions",
    "EncryptionAtRestOptions",
    "IPAddressType",
    "IdentityCenterOptions",
    "LogPublishingOptions",
    "NodeToNodeEncryptionOptions",
    "OffPeakWindowOptions",
    "SnapshotOptions",
    "SoftwareUpdateOptions",
    "VPCOptions",
];

/// Whether a changed declaration may state `property` differently and still be applied to
/// the domain the stack has: the properties `UpdateDomainConfig` carries
/// ([`DOMAIN_UPDATE_PROPERTIES`]), plus the three that call does not take because they are
/// not domain CONFIGURATION at all ([`DOMAIN_NON_CREATE_PROPERTIES`]) — the name, which is
/// the resource's identity, so a revision stating another one replaces the resource and
/// never reaches this call; `Tags`, which CloudFormation reconciles through the owning
/// service's own tagging API ([`Provisioner::retag`]); and `SkipShardMigrationWait`, which
/// steers how an update is PERFORMED rather than being state the domain holds.
fn domain_applies_in_place(property: &str) -> bool {
    DOMAIN_UPDATE_PROPERTIES.contains(&property)
        || DOMAIN_NON_CREATE_PROPERTIES.contains(&property)
}

/// Apply a CHANGED `AWS::OpenSearchService::Domain` declaration to the domain the stack
/// created, through that service's own `UpdateDomainConfig` — the call that "modifies the
/// cluster configuration of the specified Amazon OpenSearch Service domain" — so a changed
/// property re-points the domain a client is already using instead of replacing it: the
/// domain keeps its name, its id, its ARN and the address it is reached at.
///
/// Only the properties that call TAKES travel ([`DOMAIN_UPDATE_PROPERTIES`]), and only the
/// ones this deployment states differently from the last: a property neither side changed is
/// not re-sent. Each is carried as the TYPE its member is modelled with, through the same
/// rendering the create uses ([`DOMAIN_DOCUMENT_PROPERTIES`]), so an access policy the
/// template declares as an object reaches that service as the document's text.
///
/// `Ok(None)` is a revision this call cannot make, so nothing is asked of that service and
/// the deployment reports the change as not applied rather than as a success that dropped
/// part of it. That is decided over EVERY property the two revisions state differently
/// ([`changed_properties`], the reading `update_managed_policy` decides the same question
/// by): one outside [`domain_applies_in_place`] — the engine release, which moves through
/// `UpgradeDomain`, or a property the type publishes for no member of this call — makes the
/// whole revision unapplyable, whatever else changed beside it; and so does a revision that
/// STOPS declaring one of the properties this call carries, since the call takes a new value
/// for a member and not the removal of one. The attributes come from the domain itself
/// afterwards ([`domain_ready_attributes`]), never rebuilt here.
pub(super) async fn update_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let changed = changed_properties(&resource.properties, declared);
    if changed
        .iter()
        .any(|property| !domain_applies_in_place(property))
    {
        return Ok(None);
    }
    let mut request = Map::new();
    for property in &changed {
        if !DOMAIN_UPDATE_PROPERTIES.contains(&property.as_str()) {
            // A property this call does not carry, and that nothing here has to send: the
            // tags travel through the tagging API and the two others are not configuration.
            continue;
        }
        match declared.get(property) {
            Some(value) => {
                request.insert(property.clone(), value.clone());
            }
            // The revision no longer declares a property the domain was deployed with:
            // this call carries no removal, so the whole revision is reported as not
            // applied rather than as one that silently kept the old value.
            None => return Ok(None),
        }
    }
    if request.is_empty() {
        // The declaration changed nothing this call carries: either nothing at all among
        // its members, or only what it cannot change.
        return Ok(None);
    }
    render_document_members(&mut request, DOMAIN_DOCUMENT_PROPERTIES);
    let uri = format!(
        "{OPENSEARCH_DOMAIN_URI}/{}/config",
        percent_encoded(&resource.physical_resource_id)
    );
    call_rest_json(
        ctx,
        OPENSEARCH_SIGNING_NAME,
        region,
        "POST",
        &uri,
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::attributes(
        domain_ready_attributes(
            ctx,
            region,
            &resource.resource_type,
            &resource.physical_resource_id,
        )
        .await?,
    )))
}

/// The published attributes of a domain Amazon OpenSearch Service now reports as settled:
/// the whole set [`domain_created`] reads off a `DomainStatus`, which for a domain that is UP
/// carries the addresses a client reaches it on as well as the identifiers the create
/// already answered. A domain that service no longer holds publishes nothing.
pub(super) async fn domain_ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_type: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let Some(status) = read_domain(ctx, region, physical_resource_id).await? else {
        return Ok(BTreeMap::new());
    };
    Ok(domain_created(resource_type, &status).attributes)
}

/// Whether Amazon OpenSearch Service has finished bringing up the domain a stack created.
///
/// `DomainStatus$Created` is "true if domain creation is complete" and
/// `DomainStatus$Processing` is "true if OpenSearch Service is processing configuration
/// changes. False if the configuration is active" — so a domain is ready when that service
/// reports it created and no longer processing, and until then the state it is still in is
/// `DomainStatus$DomainProcessingStatus`, "the status of any changes that are currently in
/// progress for the domain". A domain that service no longer holds is not one any wait will
/// bring up.
pub(super) async fn domain_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(status) = read_domain(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon OpenSearch Service holds no domain {physical_resource_id}"
        )));
    };
    let created = status
        .get("Created")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let processing = status
        .get("Processing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if created && !processing {
        return Ok(Readiness::Ready);
    }
    Ok(Readiness::Pending(
        status
            .get("DomainProcessingStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    ))
}

/// Delete the domain a stack created, by the name it was created under. A domain that
/// service no longer holds (`ResourceNotFoundException`) is already in the state the delete
/// asks for — judged on the error CODE, never on a message, which carries the request's own
/// domain name. The name goes into the URI percent-encoded, as a non-greedy `@httpLabel` is
/// sent.
pub(super) async fn delete_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!(
        "{OPENSEARCH_DOMAIN_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, OPENSEARCH_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The domain's configuration as Amazon OpenSearch Service reports it, for drift detection:
/// the `DomainStatus` its own read answers with. A domain it no longer holds was deleted.
///
/// The record's members are that service's own, which this type's properties are spelled
/// identically to ([`DOMAIN_NON_CREATE_PROPERTIES`]), so a declared property takes part in
/// the comparison rather than leaving the domain reported as merely existing.
pub(super) async fn describe_opensearch_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    read_domain(ctx, region, physical_resource_id).await
}

/// The `DomainStatus` Amazon OpenSearch Service reports for one domain, or `None` for a
/// domain it no longer holds. The ONE read behind the readiness answer and the drift one, so
/// neither can be asked of a different operation than the other.
pub(super) async fn read_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!(
        "{OPENSEARCH_DOMAIN_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, OPENSEARCH_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "DomainStatus"))),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon S3: AWS::S3::Bucket
// ---------------------------------------------------------------------------

/// The error code Amazon S3 answers for a bucket it does not hold.
pub(super) const NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// Create the bucket an `AWS::S3::Bucket` declares, and apply the
/// `VersioningConfiguration`, the `NotificationConfiguration`, the
/// `AnalyticsConfigurations`, the `InventoryConfigurations`, the
/// `IntelligentTieringConfigurations`, the `ReplicationConfiguration` and the tags it
/// carries.
///
/// "Ref returns the bucket name" for this type, so the physical resource id the stack
/// reports IS the bucket a client addresses, and it is the bucket created here.
/// `CreateBucketConfiguration$LocationConstraint` is what decides a bucket's Region and
/// "buckets in Region us-east-1 have a LocationConstraint of null", so the stack's own
/// Region is sent as that constraint for every other Region and as nothing for that one.
///
/// A bucket's sub-resource configurations are not members of `CreateBucket` at all: each is
/// applied by the one call Amazon S3 publishes for it, made here on the created bucket, so a
/// read of that sub-resource straight after the stack completes reports what the template
/// declared. `VersioningConfiguration` is a `PutBucketVersioning`, the whole
/// `NotificationConfiguration` is a `PutBucketNotificationConfiguration`
/// ([`declared_notification_configuration`]), and each entry of `AnalyticsConfigurations` is
/// one `PutBucketAnalyticsConfiguration` ([`declared_analytics_configurations`]) — the call
/// Amazon S3 publishes for ONE analytics configuration, since the property is a list keyed
/// by `Id` and the API takes one id per call. `InventoryConfigurations` is the same story
/// with `PutBucketInventoryConfiguration` ([`declared_inventory_configurations`]), and
/// `IntelligentTieringConfigurations` with `PutBucketIntelligentTieringConfiguration`
/// ([`declared_intelligent_tiering_configurations`]).
///
/// `CreateBucket` takes no tag set at all, so the bucket's tags are a `PutBucketTagging` of
/// its own — "sets the tags for a bucket", the one call S3 publishes for them — carrying
/// the tags the declaration asks for plus the ones CloudFormation gives the resource
/// ([`tags_of`]). It is sent only where there are tags to set: an empty tag set is no
/// tagging rather than a call that would clear one.
pub(super) async fn create_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    use ::aws_sdk_s3::types::{BucketLocationConstraint, CreateBucketConfiguration};

    let client = s3_client(ctx, region);
    let bucket = &resource.physical_resource_id;
    let configuration = (region != super::DEFAULT_REGION).then(|| {
        CreateBucketConfiguration::builder()
            .location_constraint(BucketLocationConstraint::from(region))
            .build()
    });
    if let Err(error) = client
        .create_bucket()
        .bucket(bucket)
        .set_create_bucket_configuration(configuration)
        .send()
        .await
    {
        // Whatever Amazon S3 answers short of success is a bucket this deployment did not
        // create, including a name that is already in use: "for legacy compatibility, if
        // you re-create an existing bucket that you already own in the North Virginia
        // Region, Amazon S3 returns 200 OK" is a SUCCESS S3 itself answers where it holds
        // — nothing here has to stand in for it — and everywhere else a name in use is a
        // refusal, which fails the resource like any other.
        return Err(s3_refusal_of(&error));
    }
    // The bucket EXISTS from here on, so a follow-up call Amazon S3 refuses must not leave
    // it behind: the resource the create could not finish is taken away again
    // ([`discard_created_bucket`]) and the refusal is what the deployment reports, so the
    // rollback of the failed stack leaves no bucket of its own standing.
    if let Err(refusal) = configure_bucket(&client, resource, propagated).await {
        return Err(discard_created_bucket(&client, bucket, refusal).await);
    }
    Ok(Created::default())
}

/// Take away a bucket this create had already made, for a create one of its own follow-up
/// calls refused.
///
/// `CreateBucket` is only the first of the calls an `AWS::S3::Bucket` declaration takes to
/// install: each sub-resource configuration is a call of its own on the bucket that now
/// exists, and a refusal of one of those is a resource the provider created and could not
/// finish. CloudFormation fails such a resource and rolls the stack back, and the rollback
/// asks each owning service to delete what the create MADE — so a bucket nothing recorded
/// as created has to be removed here, by the provider that created it, or it outlives the
/// stack that made it.
///
/// The reported refusal is the one Amazon S3 answered; a delete it will not carry out is
/// said beside it, because the bucket is then really still there and a client reading the
/// failed stack has to be able to tell.
async fn discard_created_bucket(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    refusal: Refusal,
) -> Refusal {
    match client.delete_bucket().bucket(bucket).send().await {
        Ok(_) => refusal,
        Err(cleanup) => Refusal {
            code: refusal.code,
            message: format!(
                "{}; the bucket it created could not be deleted again: {}",
                refusal.message,
                s3_refusal_of(&cleanup)
            ),
        },
    }
}

/// Apply every sub-resource configuration an `AWS::S3::Bucket` declaration carries to the
/// bucket [`create_bucket`] has just created, each by the one call Amazon S3 publishes for
/// it.
///
/// Separated from the create itself so that ONE answer covers all of them: whatever Amazon
/// S3 refuses here leaves a bucket that exists and is not configured as declared, which is
/// the case [`create_bucket`] undoes.
async fn configure_bucket(
    client: &::aws_sdk_s3::Client,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    use ::aws_sdk_s3::types::{BucketVersioningStatus, Tag, Tagging, VersioningConfiguration};

    let bucket = &resource.physical_resource_id;
    // "VersioningConfiguration — enables multiple versions of all objects in this bucket",
    // whose one member is `Status`. The value is the template's: S3's own deserializer is
    // what judges it against `BucketVersioningStatus`.
    if let Some(status) = resource
        .properties
        .get("VersioningConfiguration")
        .and_then(|configuration| configuration.get("Status"))
        .and_then(Value::as_str)
    {
        client
            .put_bucket_versioning()
            .bucket(bucket)
            .versioning_configuration(
                VersioningConfiguration::builder()
                    .status(BucketVersioningStatus::from(status))
                    .build(),
            )
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "NotificationConfiguration — configuration that defines how Amazon S3 handles bucket
    // notifications" (this type's own property): the declared destination configurations are
    // applied to the created bucket through `PutBucketNotificationConfiguration`, the one
    // call Amazon S3 publishes for them.
    if let Some(configuration) = declared_notification_configuration(resource)? {
        client
            .put_bucket_notification_configuration()
            .bucket(bucket)
            .notification_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "AnalyticsConfigurations — specifies the configuration and any analyses for the
    // analytics filter of an Amazon S3 bucket" (this type's own property): the property is a
    // LIST keyed by `Id` and `PutBucketAnalyticsConfiguration` sets the configuration "for
    // the bucket (specified by the analytics configuration ID)", so each declared entry is
    // installed by its own call under the id it names.
    for (id, configuration) in declared_analytics_configurations(resource)? {
        client
            .put_bucket_analytics_configuration()
            .bucket(bucket)
            .id(id)
            .analytics_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "InventoryConfigurations — specifies the inventory configuration for an Amazon S3
    // bucket" (this type's own property): the property is a LIST keyed by `Id` and
    // `PutBucketInventoryConfiguration` "adds an S3 Inventory configuration (identified by
    // the inventory ID) to the bucket", so each declared entry is installed by its own call
    // under the id it names.
    for (id, configuration) in declared_inventory_configurations(resource)? {
        client
            .put_bucket_inventory_configuration()
            .bucket(bucket)
            .id(id)
            .inventory_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "IntelligentTieringConfigurations — defines how Amazon S3 handles Intelligent-Tiering
    // storage" (this type's own property): the property is a LIST keyed by `Id` and
    // `PutBucketIntelligentTieringConfiguration` "puts a S3 Intelligent-Tiering
    // configuration to the specified bucket" one id at a time, so each declared entry is
    // installed by its own call under the id it names.
    for (id, configuration) in declared_intelligent_tiering_configurations(resource)? {
        client
            .put_bucket_intelligent_tiering_configuration()
            .bucket(bucket)
            .id(id)
            .intelligent_tiering_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "MetricsConfigurations — specifies a metrics configuration" for the CloudWatch request
    // metrics of a subset of the bucket's objects (this type's own property): the property is
    // a LIST keyed by `Id` and `PutBucketMetricsConfiguration` sets the configuration
    // "specified by the metrics configuration ID", so each declared entry is installed by its
    // own call under the id it names.
    for (id, configuration) in declared_metrics_configurations(resource)? {
        client
            .put_bucket_metrics_configuration()
            .bucket(bucket)
            .id(id)
            .metrics_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "LifecycleConfiguration — specifies the lifecycle configuration for objects in an
    // Amazon S3 bucket" (this type's own property): the whole configuration is ONE
    // `PutBucketLifecycleConfiguration`, the call Amazon S3 publishes for it, since the API
    // takes the rule set as a single container.
    if let Some(lifecycle) = declared_lifecycle_configuration(resource)? {
        client
            .put_bucket_lifecycle_configuration()
            .bucket(bucket)
            .lifecycle_configuration(lifecycle.configuration)
            .set_transition_default_minimum_object_size(lifecycle.minimum_object_size)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "BucketEncryption — specifies default encryption for a bucket using server-side
    // encryption with different key options" (this type's own property): the whole
    // configuration is ONE `PutBucketEncryption`, the call Amazon S3 publishes for it, so a
    // bucket a template declares an SSE-KMS or SSE-S3 default for behaves exactly like one
    // configured through that call — including for the writes that inherit it.
    if let Some(configuration) = declared_bucket_encryption(resource)? {
        client
            .put_bucket_encryption()
            .bucket(bucket)
            .server_side_encryption_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "ReplicationConfiguration — configuration for replicating objects in an S3 bucket"
    // (this type's own property): the whole configuration is ONE `PutBucketReplication`, the
    // call Amazon S3 publishes for it, so a bucket a template declares replication for really
    // replicates the writes it takes — under the role the configuration names, which S3
    // assumes to make each copy.
    if let Some(configuration) = declared_replication_configuration(resource)? {
        client
            .put_bucket_replication()
            .bucket(bucket)
            .replication_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // The bucket's whole tag set, in the one call Amazon S3 takes it through.
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        let mut tag_set = Vec::with_capacity(tags.len());
        for (key, value) in &tags {
            tag_set.push(
                Tag::builder()
                    .key(key)
                    .value(value)
                    .build()
                    .map_err(|error| Refusal {
                        code: "InternalFailure".to_string(),
                        message: format!("the tag {key} cannot be sent to Amazon S3: {error}"),
                    })?,
            );
        }
        let tagging = Tagging::builder()
            .set_tag_set(Some(tag_set))
            .build()
            .map_err(|error| Refusal {
                code: "InternalFailure".to_string(),
                message: format!("the tag set for {bucket} cannot be sent: {error}"),
            })?;
        client
            .put_bucket_tagging()
            .bucket(bucket)
            .tagging(tagging)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    Ok(())
}

/// The property an `AWS::S3::Bucket` declares its event notifications under.
pub(super) const NOTIFICATION_CONFIGURATION: &str = "NotificationConfiguration";

/// One declared destination configuration, translated out of the template's spelling.
///
/// Every one of the API's three destination configurations carries the same three things —
/// a destination ARN, the events the destination is notified about and an optional key-name
/// filter — so the reading is shared and only the shape it is built into differs.
pub(super) struct DeclaredNotification {
    /// The destination's ARN, as the template's `Function`, `Queue` or `Topic` resolved to.
    pub(super) arn: String,
    /// The events the destination is notified about. The template states ONE `Event` per
    /// configuration where the API takes an `Events` LIST, so this carries that one event.
    pub(super) events: Vec<::aws_sdk_s3::types::Event>,
    /// The key-name filter the configuration's `Filter` states, or `None` when it states
    /// none (and so every key notifies).
    pub(super) filter: Option<::aws_sdk_s3::types::NotificationConfigurationFilter>,
}

/// The configurations one destination kind of a declared `NotificationConfiguration` lists.
///
/// `member` is the property that lists them (`LambdaConfigurations`, `QueueConfigurations`
/// or `TopicConfigurations`) and `destination` the property inside one that names the
/// destination (`Function`, `Queue` or `Topic`) — the members `AWS::S3::Bucket`'s
/// `NotificationConfiguration` publishes. A configuration missing the destination or the
/// event its type REQUIRES is refused rather than sent incomplete or dropped: the stack must
/// not complete with a notification it could not install.
pub(super) fn declared_notifications(
    declared: &Value,
    member: &str,
    destination: &str,
) -> Result<Vec<DeclaredNotification>, Refusal> {
    let mut notifications = Vec::new();
    let listed = declared
        .get(member)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for configuration in listed {
        let arn = configuration
            .get(destination)
            .and_then(scalar_text)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "a {NOTIFICATION_CONFIGURATION}.{member} entry of {S3_BUCKET} declares no \
                     {destination}"
                ),
            })?;
        let event = configuration
            .get("Event")
            .and_then(scalar_text)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "a {NOTIFICATION_CONFIGURATION}.{member} entry of {S3_BUCKET} declares no \
                     Event"
                ),
            })?;
        notifications.push(DeclaredNotification {
            arn,
            events: vec![::aws_sdk_s3::types::Event::from(event.as_str())],
            filter: notification_filter(configuration),
        });
    }
    Ok(notifications)
}

/// The key-name filter one declared configuration's `Filter` states, as Amazon S3 takes it.
///
/// The template nests the rules as `Filter.S3Key.Rules` where the API nests them as
/// `Filter.Key.FilterRules`, with the same two members on each rule (`Name` and `Value`), so
/// the rules are carried through one for one and in the declared order. A configuration
/// stating no filter gets none, which is the configuration that matches every key.
pub(super) fn notification_filter(
    configuration: &Value,
) -> Option<::aws_sdk_s3::types::NotificationConfigurationFilter> {
    use ::aws_sdk_s3::types::{
        FilterRule, FilterRuleName, NotificationConfigurationFilter, S3KeyFilter,
    };

    let rules = configuration
        .get("Filter")
        .and_then(|filter| filter.get("S3Key"))
        .and_then(|key| key.get("Rules"))
        .and_then(Value::as_array)?;
    let filter_rules: Vec<FilterRule> = rules
        .iter()
        .map(|rule| {
            let mut builder = FilterRule::builder();
            // Both members are optional in the API's own shape, so each is sent only where
            // the rule states it: Amazon S3 is what judges a rule naming neither.
            if let Some(name) = rule.get("Name").and_then(scalar_text) {
                builder = builder.name(FilterRuleName::from(name.as_str()));
            }
            if let Some(value) = rule.get("Value").and_then(scalar_text) {
                builder = builder.value(value);
            }
            builder.build()
        })
        .collect();
    Some(
        NotificationConfigurationFilter::builder()
            .key(
                S3KeyFilter::builder()
                    .set_filter_rules(Some(filter_rules))
                    .build(),
            )
            .build(),
    )
}

/// The whole `NotificationConfiguration` a bucket's declaration states, as the
/// `PutBucketNotificationConfiguration` request Amazon S3 takes — `None` for a declaration
/// that states no notifications at all, which is the state a bucket is created in.
///
/// The template's spelling differs from the API's in three ways, and this is the one place
/// they are reconciled: the destination configurations are listed under
/// `LambdaConfigurations`/`QueueConfigurations`/`TopicConfigurations` against the API's
/// `LambdaFunctionConfigurations`/`QueueConfigurations`/`TopicConfigurations`, each states
/// ONE `Event` where the API takes an `Events` list, and each names its destination with
/// `Function`/`Queue`/`Topic` against the API's `LambdaFunctionArn`/`QueueArn`/`TopicArn`.
/// Nothing else is renamed: a `Filter`'s rules travel through [`notification_filter`], and
/// an `Id` is not a member of this type's configurations at all — "if you don't provide one,
/// Amazon S3 will assign an ID", and S3 is what does so here.
///
/// `EventBridgeConfiguration` is the fourth member, and the one whose shapes disagree: the
/// template states `EventBridgeEnabled`, while the API's shape has no members and its
/// PRESENCE is what "enables delivery of events to Amazon EventBridge" — so it is sent for a
/// declaration that enables it and left out otherwise.
pub(super) fn declared_notification_configuration(
    resource: &StoredResource,
) -> Result<Option<::aws_sdk_s3::types::NotificationConfiguration>, Refusal> {
    use ::aws_sdk_s3::types::{
        EventBridgeConfiguration, LambdaFunctionConfiguration, NotificationConfiguration,
        QueueConfiguration, TopicConfiguration,
    };

    let Some(declared) = resource.properties.get(NOTIFICATION_CONFIGURATION) else {
        return Ok(None);
    };
    let incomplete = |member: &str, error: &dyn ::std::fmt::Display| Refusal {
        code: UNIDENTIFIED_FAILURE.to_string(),
        message: format!(
            "a {NOTIFICATION_CONFIGURATION}.{member} entry of {S3_BUCKET} cannot be sent to \
             Amazon S3: {error}"
        ),
    };

    let mut lambda_configurations = Vec::new();
    for notification in declared_notifications(declared, "LambdaConfigurations", "Function")? {
        lambda_configurations.push(
            LambdaFunctionConfiguration::builder()
                .lambda_function_arn(notification.arn)
                .set_events(Some(notification.events))
                .set_filter(notification.filter)
                .build()
                .map_err(|error| incomplete("LambdaConfigurations", &error))?,
        );
    }
    let mut queue_configurations = Vec::new();
    for notification in declared_notifications(declared, "QueueConfigurations", "Queue")? {
        queue_configurations.push(
            QueueConfiguration::builder()
                .queue_arn(notification.arn)
                .set_events(Some(notification.events))
                .set_filter(notification.filter)
                .build()
                .map_err(|error| incomplete("QueueConfigurations", &error))?,
        );
    }
    let mut topic_configurations = Vec::new();
    for notification in declared_notifications(declared, "TopicConfigurations", "Topic")? {
        topic_configurations.push(
            TopicConfiguration::builder()
                .topic_arn(notification.arn)
                .set_events(Some(notification.events))
                .set_filter(notification.filter)
                .build()
                .map_err(|error| incomplete("TopicConfigurations", &error))?,
        );
    }
    let event_bridge_enabled = declared
        .get("EventBridgeConfiguration")
        .and_then(|configuration| configuration.get("EventBridgeEnabled"))
        .and_then(scalar_text)
        .is_some_and(|enabled| enabled.eq_ignore_ascii_case("true"));

    if lambda_configurations.is_empty()
        && queue_configurations.is_empty()
        && topic_configurations.is_empty()
        && !event_bridge_enabled
    {
        return Ok(None);
    }
    Ok(Some(
        NotificationConfiguration::builder()
            .set_lambda_function_configurations(
                (!lambda_configurations.is_empty()).then_some(lambda_configurations),
            )
            .set_queue_configurations(
                (!queue_configurations.is_empty()).then_some(queue_configurations),
            )
            .set_topic_configurations(
                (!topic_configurations.is_empty()).then_some(topic_configurations),
            )
            .set_event_bridge_configuration(
                event_bridge_enabled.then(|| EventBridgeConfiguration::builder().build()),
            )
            .build(),
    ))
}

/// The property an `AWS::S3::Bucket` declares its storage class analyses under.
pub(super) const ANALYTICS_CONFIGURATIONS: &str = "AnalyticsConfigurations";

/// Every analytics configuration a bucket's declaration states, each paired with the `Id`
/// that identifies it — the `(Id, AnalyticsConfiguration)` pair one
/// `PutBucketAnalyticsConfiguration` takes. A declaration stating none yields none, which is
/// the state a bucket is created in.
///
/// The template's spelling differs from the API's in two ways, and this is the one place they
/// are reconciled. The object scope travels as the two FLAT properties `Prefix` and
/// `TagFilters` where the API takes the single `Filter` union ([`analytics_filter`]), and the
/// export destination travels FLAT as `{ BucketArn, BucketAccountId, Format, Prefix }` where
/// the API nests it one level deeper under `Destination.S3BucketDestination` with `BucketArn`
/// spelled `Bucket` ([`declared_storage_class_analysis`]). An entry declaring no `Id` is
/// refused rather than installed under an id of this module's invention: the stack must not
/// complete with an analysis a client cannot address.
pub(super) fn declared_analytics_configurations(
    resource: &StoredResource,
) -> Result<Vec<(String, ::aws_sdk_s3::types::AnalyticsConfiguration)>, Refusal> {
    use ::aws_sdk_s3::types::AnalyticsConfiguration;

    let listed = resource
        .properties
        .get(ANALYTICS_CONFIGURATIONS)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut configurations = Vec::with_capacity(listed.len());
    for declared in listed {
        let id = declared
            .get("Id")
            .and_then(scalar_text)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "an {ANALYTICS_CONFIGURATIONS} entry of {S3_BUCKET} declares no Id"
                ),
            })?;
        let configuration = AnalyticsConfiguration::builder()
            .id(&id)
            .set_filter(analytics_filter(declared, &id)?)
            .storage_class_analysis(declared_storage_class_analysis(declared, &id)?)
            .build()
            .map_err(|error| analytics_not_sendable(&id, &error))?;
        configurations.push((id, configuration));
    }
    Ok(configurations)
}

/// The refusal for an entry of one of a bucket's id-keyed configuration LISTS that the SDK
/// will not build into the request shape: the property and the entry are both named, so the
/// failing declaration can be found. One wording for every such list, since what went wrong
/// is the same in each — the entry, not the list it is in.
pub(super) fn configuration_not_sendable(
    property: &str,
    id: &str,
    error: &dyn ::std::fmt::Display,
) -> Refusal {
    Refusal {
        code: UNIDENTIFIED_FAILURE.to_string(),
        message: format!(
            "the {property} entry {id} of {S3_BUCKET} cannot be sent to Amazon S3: {error}"
        ),
    }
}

/// The refusal for a declared analytics configuration the SDK will not build into the
/// request shape: the entry is named, so the failing declaration can be found.
pub(super) fn analytics_not_sendable(id: &str, error: &dyn ::std::fmt::Display) -> Refusal {
    configuration_not_sendable(ANALYTICS_CONFIGURATIONS, id, error)
}

/// The `AnalyticsFilter` one declared configuration's object scope amounts to, or `None` for
/// a configuration that narrows nothing — "if no filter is provided, all objects will be
/// considered in any analysis" (`AnalyticsFilter`).
///
/// The template narrows with two flat properties, a `Prefix` and a `TagFilters` list of
/// `{ Key, Value }`, while "a filter must have exactly one prefix, one tag, or one
/// conjunction (AnalyticsAndOperator)" and that conjunction "must have at least two
/// predicates" (`AnalyticsFilter` and `AnalyticsFilter$And`). So a scope naming ONE predicate
/// is that predicate's own member, and a scope naming several is the conjunction of them —
/// the prefix and each tag counting as one predicate each.
pub(super) fn analytics_filter(
    declared: &Value,
    id: &str,
) -> Result<Option<::aws_sdk_s3::types::AnalyticsFilter>, Refusal> {
    use ::aws_sdk_s3::types::{AnalyticsAndOperator, AnalyticsFilter};

    let prefix = declared_text(declared, "Prefix");
    let tags = declared_tag_filters(declared, ANALYTICS_CONFIGURATIONS, id)?;
    Ok(match (prefix, tags.len()) {
        (None, 0) => None,
        (Some(prefix), 0) => Some(AnalyticsFilter::Prefix(prefix)),
        (None, 1) => tags.into_iter().next().map(AnalyticsFilter::Tag),
        (prefix, _) => Some(AnalyticsFilter::And(
            AnalyticsAndOperator::builder()
                .set_prefix(prefix)
                .set_tags(Some(tags))
                .build(),
        )),
    })
}

/// The tags one declared entry of `property` narrows its scope by, as the API's own `Tag`
/// list, in the declared order. `TagFilters` is how EVERY id-keyed configuration list of this
/// type spells the tag half of an object scope, so one reading serves them all.
///
/// A filter stating neither half of the tag is refused rather than sent as a tag on nothing:
/// `Tag` REQUIRES both `Key` and `Value`, so an incomplete one narrows by a predicate the
/// service cannot evaluate.
pub(super) fn declared_tag_filters(
    declared: &Value,
    property: &str,
    id: &str,
) -> Result<Vec<::aws_sdk_s3::types::Tag>, Refusal> {
    use ::aws_sdk_s3::types::Tag;

    let listed = declared
        .get("TagFilters")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut tags = Vec::with_capacity(listed.len());
    for filter in listed {
        let missing = |member: &str| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "a TagFilters entry of the {property} entry {id} of {S3_BUCKET} declares no \
                 {member}"
            ),
        };
        let key = declared_text(filter, "Key").ok_or_else(|| missing("Key"))?;
        let value = filter
            .get("Value")
            .and_then(scalar_text)
            .ok_or_else(|| missing("Value"))?;
        tags.push(
            Tag::builder()
                .key(key)
                .value(value)
                .build()
                .map_err(|error| configuration_not_sendable(property, id, &error))?,
        );
    }
    Ok(tags)
}

/// The `StorageClassAnalysis` one declared configuration states, as Amazon S3 takes it.
///
/// `StorageClassAnalysis` is REQUIRED on an `AnalyticsConfiguration` and its one member
/// `DataExport` is not, so a declaration stating no export — or no analysis block at all —
/// becomes the EMPTY container, which is the analysis that collects access patterns without
/// exporting them. A declared export is one level deeper on the API than in the template:
/// `Destination.S3BucketDestination` carries the template's flat destination with `BucketArn`
/// spelled `Bucket`, "the Amazon Resource Name (ARN) of the bucket to which data is
/// exported" (`AnalyticsS3BucketDestination$Bucket`), beside the optional `BucketAccountId`
/// — "the account ID that owns the destination S3 bucket" — and `Prefix`.
///
/// A declared export missing a member the API REQUIRES is refused rather than sent
/// incomplete or quietly dropped to the empty analysis: the stack must not complete with an
/// export it could not install.
pub(super) fn declared_storage_class_analysis(
    declared: &Value,
    id: &str,
) -> Result<::aws_sdk_s3::types::StorageClassAnalysis, Refusal> {
    use ::aws_sdk_s3::types::{
        AnalyticsExportDestination, AnalyticsS3BucketDestination, AnalyticsS3ExportFileFormat,
        StorageClassAnalysis, StorageClassAnalysisDataExport, StorageClassAnalysisSchemaVersion,
    };

    let Some(export) = declared
        .get("StorageClassAnalysis")
        .and_then(|analysis| analysis.get("DataExport"))
    else {
        return Ok(StorageClassAnalysis::builder().build());
    };
    let missing = |member: &str| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "the {ANALYTICS_CONFIGURATIONS} entry {id} of {S3_BUCKET} declares no \
             StorageClassAnalysis.DataExport.{member}"
        ),
    };
    let destination = export
        .get("Destination")
        .ok_or_else(|| missing("Destination"))?;
    let required_text = |member: &str| {
        destination
            .get(member)
            .and_then(scalar_text)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| missing(&format!("Destination.{member}")))
    };
    // The values are the template's: Amazon S3's own deserializer is what judges them
    // against `AnalyticsS3ExportFileFormat` and `StorageClassAnalysisSchemaVersion`.
    let bucket_arn = required_text("BucketArn")?;
    let format = required_text("Format")?;
    let schema_version = export
        .get("OutputSchemaVersion")
        .and_then(scalar_text)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| missing("OutputSchemaVersion"))?;
    let s3_bucket_destination = AnalyticsS3BucketDestination::builder()
        .format(AnalyticsS3ExportFileFormat::from(format.as_str()))
        .bucket(bucket_arn)
        .set_bucket_account_id(destination.get("BucketAccountId").and_then(scalar_text))
        .set_prefix(destination.get("Prefix").and_then(scalar_text))
        .build()
        .map_err(|error| analytics_not_sendable(id, &error))?;
    let data_export = StorageClassAnalysisDataExport::builder()
        .output_schema_version(StorageClassAnalysisSchemaVersion::from(
            schema_version.as_str(),
        ))
        .destination(
            AnalyticsExportDestination::builder()
                .s3_bucket_destination(s3_bucket_destination)
                .build(),
        )
        .build()
        .map_err(|error| analytics_not_sendable(id, &error))?;
    Ok(StorageClassAnalysis::builder()
        .data_export(data_export)
        .build())
}

/// The property an `AWS::S3::Bucket` declares its S3 Inventory reports under.
pub(super) const INVENTORY_CONFIGURATIONS: &str = "InventoryConfigurations";

/// Every inventory configuration a bucket's declaration states, each paired with the `Id`
/// that identifies it — the `(Id, InventoryConfiguration)` pair one
/// `PutBucketInventoryConfiguration` takes. A declaration stating none yields none, which is
/// the state a bucket is created in.
///
/// The template's spelling is FLAT where the API nests, and this is the one place the two are
/// reconciled. An entry states the object scope as a single `Prefix` where the API takes the
/// `Filter` whose one REQUIRED member is that prefix, and the schedule as a single
/// `ScheduleFrequency` where the API takes the `Schedule` whose one REQUIRED member is that
/// frequency; `Enabled` is the API's `IsEnabled`, read as a BOOLEAN in either spelling a
/// template gives one ([`declared_flag`]); and the destination travels one level deeper
/// through [`declared_inventory_destination`]. `IncludedObjectVersions` and `OptionalFields`
/// are spelled the same on both sides and pass through — the values are the template's,
/// since Amazon S3's own deserializer is what judges them against their enums.
///
/// An entry missing a member the API REQUIRES — the `Id` it is addressable by, the enabled
/// flag, the schedule or the object versions — is refused rather than installed under a value
/// of this module's invention: the stack must not complete with an inventory report a client
/// cannot address or one the service never agreed to.
pub(super) fn declared_inventory_configurations(
    resource: &StoredResource,
) -> Result<Vec<(String, ::aws_sdk_s3::types::InventoryConfiguration)>, Refusal> {
    use ::aws_sdk_s3::types::{
        InventoryConfiguration, InventoryFilter, InventoryFrequency,
        InventoryIncludedObjectVersions, InventoryOptionalField, InventorySchedule,
    };

    let listed = resource
        .properties
        .get(INVENTORY_CONFIGURATIONS)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut configurations = Vec::with_capacity(listed.len());
    for declared in listed {
        let id = declared
            .get("Id")
            .and_then(scalar_text)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "an {INVENTORY_CONFIGURATIONS} entry of {S3_BUCKET} declares no Id"
                ),
            })?;
        let missing = |member: &str| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "the {INVENTORY_CONFIGURATIONS} entry {id} of {S3_BUCKET} declares no {member}"
            ),
        };
        let enabled = declared
            .as_object()
            .and_then(|entry| declared_flag(entry, "Enabled"))
            .ok_or_else(|| missing("Enabled"))?;
        let frequency = declared_text(declared, "ScheduleFrequency")
            .ok_or_else(|| missing("ScheduleFrequency"))?;
        let versions = declared_text(declared, "IncludedObjectVersions")
            .ok_or_else(|| missing("IncludedObjectVersions"))?;
        // The entry's own `Prefix` is the object scope's, and `InventoryFilter`'s one member
        // is REQUIRED, so an entry stating no prefix carries no filter at all — which is the
        // report that covers every object in the bucket.
        let filter = match declared_text(declared, "Prefix") {
            Some(prefix) => Some(
                InventoryFilter::builder()
                    .prefix(prefix)
                    .build()
                    .map_err(|error| inventory_not_sendable(&id, &error))?,
            ),
            None => None,
        };
        let optional_fields: Option<Vec<InventoryOptionalField>> = declared
            .get("OptionalFields")
            .and_then(Value::as_array)
            // An EMPTY list asks for no optional field, which is what a declaration stating
            // none asks for: both send the member not at all, rather than an empty element.
            .filter(|fields| !fields.is_empty())
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(scalar_text)
                    .map(|field| InventoryOptionalField::from(field.as_str()))
                    .collect()
            });
        let schedule = InventorySchedule::builder()
            .frequency(InventoryFrequency::from(frequency.as_str()))
            .build()
            .map_err(|error| inventory_not_sendable(&id, &error))?;
        let configuration = InventoryConfiguration::builder()
            .id(&id)
            .is_enabled(enabled)
            .destination(declared_inventory_destination(declared, &id)?)
            .schedule(schedule)
            .included_object_versions(InventoryIncludedObjectVersions::from(versions.as_str()))
            .set_filter(filter)
            .set_optional_fields(optional_fields)
            .build()
            .map_err(|error| inventory_not_sendable(&id, &error))?;
        configurations.push((id, configuration));
    }
    Ok(configurations)
}

/// The refusal for a declared inventory configuration the SDK will not build into the request
/// shape: the entry is named, so the failing declaration can be found.
pub(super) fn inventory_not_sendable(id: &str, error: &dyn ::std::fmt::Display) -> Refusal {
    configuration_not_sendable(INVENTORY_CONFIGURATIONS, id, error)
}

/// The `InventoryDestination` one declared entry states, as Amazon S3 takes it.
///
/// The API nests the destination one level deeper than the template: `S3BucketDestination`
/// carries "the Amazon Resource Name (ARN) of the bucket where inventory results will be
/// published" (`InventoryS3BucketDestination$Bucket`) and the `Format` the report is written
/// in, both REQUIRED, beside the optional `AccountId` — "the account ID that owns the
/// destination S3 bucket" — and `Prefix`, which is "prepended to all inventory results".
///
/// The destination is the ONE nested object an entry states, and the entry's own `Prefix` is
/// the object scope's filter rather than this one's — so a destination prefix is declared
/// exactly where `Destination.Prefix` is, and one that is not declared is left OUT of the
/// request rather than sent as an empty one.
///
/// A declaration missing the destination, or either member it REQUIRES, is refused rather
/// than sent incomplete: the stack must not complete with an inventory report that is
/// published nowhere.
pub(super) fn declared_inventory_destination(
    declared: &Value,
    id: &str,
) -> Result<::aws_sdk_s3::types::InventoryDestination, Refusal> {
    use ::aws_sdk_s3::types::{InventoryDestination, InventoryFormat, InventoryS3BucketDestination};

    let missing = |member: &str| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "the {INVENTORY_CONFIGURATIONS} entry {id} of {S3_BUCKET} declares no {member}"
        ),
    };
    let destination = declared
        .get("Destination")
        .ok_or_else(|| missing("Destination"))?;
    // The values are the template's: Amazon S3's own deserializer is what judges the format
    // against `InventoryFormat`.
    let required_text = |member: &str| {
        declared_text(destination, member).ok_or_else(|| missing(&format!("Destination.{member}")))
    };
    let s3_bucket_destination = InventoryS3BucketDestination::builder()
        .bucket(required_text("BucketArn")?)
        .format(InventoryFormat::from(required_text("Format")?.as_str()))
        .set_account_id(declared_text(destination, "BucketAccountId"))
        .set_prefix(declared_text(destination, "Prefix"))
        .build()
        .map_err(|error| inventory_not_sendable(id, &error))?;
    Ok(InventoryDestination::builder()
        .s3_bucket_destination(s3_bucket_destination)
        .build())
}

/// The property an `AWS::S3::Bucket` declares its S3 Intelligent-Tiering configurations
/// under.
pub(super) const INTELLIGENT_TIERING_CONFIGURATIONS: &str = "IntelligentTieringConfigurations";

/// Every S3 Intelligent-Tiering configuration a bucket's declaration states, each paired with
/// the `Id` that identifies it — the `(Id, IntelligentTieringConfiguration)` pair one
/// `PutBucketIntelligentTieringConfiguration` takes. A declaration stating none yields none,
/// which is the state a bucket is created in.
///
/// The template's spelling is FLAT where the API nests, and this is the one place the two are
/// reconciled. An entry states the object scope as `Prefix` and `TagFilters` where the API
/// takes the single `Filter` container ([`intelligent_tiering_filter`]); `Status` and
/// `Tierings` are spelled the same on both sides and pass through, each `Tiering` carrying
/// the `Days` and `AccessTier` the API REQUIRES of it. The values are the template's: Amazon
/// S3's own deserializer and handler are what judge them against
/// `IntelligentTieringStatus`, `IntelligentTieringAccessTier` and the day range each tier
/// publishes.
///
/// An entry missing a member the API REQUIRES — the `Id` it is addressable by, the `Status`,
/// the `Tierings` the configuration transitions by, or either half of one tiering — is
/// refused rather than installed under a value of this module's invention: the stack must not
/// complete with a tiering policy a client cannot address or one the service never agreed to.
pub(super) fn declared_intelligent_tiering_configurations(
    resource: &StoredResource,
) -> Result<Vec<(String, ::aws_sdk_s3::types::IntelligentTieringConfiguration)>, Refusal> {
    use ::aws_sdk_s3::types::{
        IntelligentTieringAccessTier, IntelligentTieringConfiguration, IntelligentTieringStatus,
        Tiering,
    };

    let listed = resource
        .properties
        .get(INTELLIGENT_TIERING_CONFIGURATIONS)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut configurations = Vec::with_capacity(listed.len());
    for declared in listed {
        let id = declared_text(declared, "Id").ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "an {INTELLIGENT_TIERING_CONFIGURATIONS} entry of {S3_BUCKET} declares no Id"
            ),
        })?;
        let missing = |member: &str| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "the {INTELLIGENT_TIERING_CONFIGURATIONS} entry {id} of {S3_BUCKET} declares no \
                 {member}"
            ),
        };
        let status = declared_text(declared, "Status").ok_or_else(|| missing("Status"))?;
        let declared_tierings = declared
            .get("Tierings")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if declared_tierings.is_empty() {
            return Err(missing("Tierings"));
        }
        let mut tierings = Vec::with_capacity(declared_tierings.len());
        for tiering in declared_tierings {
            let days =
                whole_number(tiering.get("Days")).ok_or_else(|| missing("Tierings.Days"))?;
            let access_tier = declared_text(tiering, "AccessTier")
                .ok_or_else(|| missing("Tierings.AccessTier"))?;
            tierings.push(
                Tiering::builder()
                    .days(days)
                    .access_tier(IntelligentTieringAccessTier::from(access_tier.as_str()))
                    .build()
                    .map_err(|error| {
                        configuration_not_sendable(
                            INTELLIGENT_TIERING_CONFIGURATIONS,
                            &id,
                            &error,
                        )
                    })?,
            );
        }
        let configuration = IntelligentTieringConfiguration::builder()
            .id(&id)
            .status(IntelligentTieringStatus::from(status.as_str()))
            .set_filter(intelligent_tiering_filter(declared, &id)?)
            .set_tierings(Some(tierings))
            .build()
            .map_err(|error| {
                configuration_not_sendable(INTELLIGENT_TIERING_CONFIGURATIONS, &id, &error)
            })?;
        configurations.push((id, configuration));
    }
    Ok(configurations)
}

/// The `IntelligentTieringFilter` one declared configuration's object scope amounts to, or
/// `None` for a configuration that narrows nothing — the filter "is used to identify objects
/// that the S3 Intelligent-Tiering configuration applies to", so a configuration stating no
/// scope applies to every object.
///
/// The template narrows with the two flat properties `Prefix` and `TagFilters`, while the
/// API's container carries a `Prefix`, a single `Tag` or an `And` conjunction whose "operator
/// must have at least two predicates"
/// (`smithy_query(query="shape",shape="IntelligentTieringFilter")`). So a scope naming ONE
/// predicate is that predicate's own member, and a scope naming several is the conjunction of
/// them — the prefix and each tag counting as one predicate each, exactly as
/// [`analytics_filter`] folds the same flat pair for its own list.
pub(super) fn intelligent_tiering_filter(
    declared: &Value,
    id: &str,
) -> Result<Option<::aws_sdk_s3::types::IntelligentTieringFilter>, Refusal> {
    use ::aws_sdk_s3::types::{IntelligentTieringAndOperator, IntelligentTieringFilter};

    let prefix = declared_text(declared, "Prefix");
    let tags = declared_tag_filters(declared, INTELLIGENT_TIERING_CONFIGURATIONS, id)?;
    Ok(match (prefix, tags.len()) {
        (None, 0) => None,
        (Some(prefix), 0) => Some(IntelligentTieringFilter::builder().prefix(prefix).build()),
        (None, 1) => tags
            .into_iter()
            .next()
            .map(|tag| IntelligentTieringFilter::builder().tag(tag).build()),
        (prefix, _) => Some(
            IntelligentTieringFilter::builder()
                .and(
                    IntelligentTieringAndOperator::builder()
                        .set_prefix(prefix)
                        // Every scope that reaches the conjunction carries at least one tag:
                        // a prefix is this filter's only other predicate, and a prefix alone
                        // is the single-predicate arm above.
                        .set_tags(Some(tags))
                        .build(),
                )
                .build(),
        ),
    })
}

/// The property an `AWS::S3::Bucket` declares its CloudWatch request metrics under.
pub(super) const METRICS_CONFIGURATIONS: &str = "MetricsConfigurations";

/// Every metrics configuration a bucket's declaration states, each paired with the `Id` that
/// identifies it — the `(Id, MetricsConfiguration)` pair one `PutBucketMetricsConfiguration`
/// takes. A declaration stating none yields none, which is the state a bucket is created in.
///
/// The object scope is what the template spells FLAT: an `AccessPointArn`, a `Prefix` and a
/// `TagFilters` list, where the API takes the single `Filter` union
/// ([`metrics_filter`]). An entry declaring no `Id` is refused rather than installed under an
/// id of this module's invention: the stack must not complete with a metrics configuration a
/// client cannot address.
pub(super) fn declared_metrics_configurations(
    resource: &StoredResource,
) -> Result<Vec<(String, ::aws_sdk_s3::types::MetricsConfiguration)>, Refusal> {
    use ::aws_sdk_s3::types::MetricsConfiguration;

    let listed = resource
        .properties
        .get(METRICS_CONFIGURATIONS)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut configurations = Vec::with_capacity(listed.len());
    for declared in listed {
        let id = declared_text(declared, "Id").ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("a {METRICS_CONFIGURATIONS} entry of {S3_BUCKET} declares no Id"),
        })?;
        let configuration = MetricsConfiguration::builder()
            .id(&id)
            .set_filter(metrics_filter(declared, &id)?)
            .build()
            .map_err(|error| configuration_not_sendable(METRICS_CONFIGURATIONS, &id, &error))?;
        configurations.push((id, configuration));
    }
    Ok(configurations)
}

/// The `MetricsFilter` one declared configuration's object scope amounts to, or `None` for a
/// configuration that narrows nothing — the filter is what makes the configuration cover "a
/// subset of objects", so a configuration stating no scope covers the whole bucket.
///
/// The template narrows with the three flat properties `AccessPointArn`, `Prefix` and
/// `TagFilters`, while "a filter must have exactly one prefix, one tag, one access point ARN,
/// or one conjunction (MetricsAndOperator)" whose operator "must have at least two
/// predicates" (`smithy_query(query="shape",shape="MetricsFilter")`). So a scope naming ONE
/// predicate is that predicate's own member, and a scope naming several is the conjunction of
/// them — the access point ARN, the prefix and each tag counting as one predicate each,
/// exactly as [`analytics_filter`] folds the same flat spelling for its own list.
pub(super) fn metrics_filter(
    declared: &Value,
    id: &str,
) -> Result<Option<::aws_sdk_s3::types::MetricsFilter>, Refusal> {
    use ::aws_sdk_s3::types::{MetricsAndOperator, MetricsFilter};

    let access_point_arn = declared_text(declared, "AccessPointArn");
    let prefix = declared_text(declared, "Prefix");
    let tags = declared_tag_filters(declared, METRICS_CONFIGURATIONS, id)?;
    let predicates = usize::from(access_point_arn.is_some()) + usize::from(prefix.is_some())
        + tags.len();
    Ok(match predicates {
        0 => None,
        1 => match (access_point_arn, prefix, tags.into_iter().next()) {
            (Some(arn), _, _) => Some(MetricsFilter::AccessPointArn(arn)),
            (_, Some(prefix), _) => Some(MetricsFilter::Prefix(prefix)),
            (_, _, tag) => tag.map(MetricsFilter::Tag),
        },
        _ => Some(MetricsFilter::And(
            MetricsAndOperator::builder()
                .set_access_point_arn(access_point_arn)
                .set_prefix(prefix)
                // A conjunction of the prefix and the access point ARN alone carries no tag
                // at all, which is not the same as an empty tag list.
                .set_tags((!tags.is_empty()).then_some(tags))
                .build(),
        )),
    })
}

/// The property an `AWS::S3::Bucket` declares its object lifecycle under.
pub(super) const LIFECYCLE_CONFIGURATION: &str = "LifecycleConfiguration";

/// The lifecycle configuration one `PutBucketLifecycleConfiguration` carries: the rule set
/// and the default minimum object size the transitions are measured against, which is a
/// member of the REQUEST rather than of the container.
pub(super) struct DeclaredLifecycle {
    pub(super) configuration: ::aws_sdk_s3::types::BucketLifecycleConfiguration,
    pub(super) minimum_object_size: Option<::aws_sdk_s3::types::TransitionDefaultMinimumObjectSize>,
}

/// The lifecycle configuration a bucket's declaration states, as the one
/// `PutBucketLifecycleConfiguration` request Amazon S3 takes — `None` for a declaration that
/// states none, which is the state a bucket is created in.
///
/// The template's spelling is FLAT where the API nests, and this is the one place the two are
/// reconciled. A rule states its object scope as `Prefix`, `TagFilters`,
/// `ObjectSizeGreaterThan` and `ObjectSizeLessThan` where the API takes the single `Filter`
/// container ([`lifecycle_rule_filter`]); its expiry as `ExpirationDate`,
/// `ExpirationInDays` and `ExpiredObjectDeleteMarker` where the API takes the one
/// `Expiration` container ([`lifecycle_expiration`]); and each transition with
/// `TransitionDate`/`TransitionInDays` where the API spells the same two `Date`/`Days`.
/// `Status`, `Id`, `AbortIncompleteMultipartUpload` and the noncurrent-version members are
/// spelled the same on both sides. The values are the template's: Amazon S3's own
/// deserializer and handler are what judge them against `ExpirationStatus`, the transition
/// storage classes and the published rules a rule set has to satisfy.
///
/// A rule missing the `Status` the API REQUIRES, or naming a date that is no instant, is
/// refused rather than sent incomplete: the stack must not complete with a lifecycle the
/// bucket never carried.
pub(super) fn declared_lifecycle_configuration(
    resource: &StoredResource,
) -> Result<Option<DeclaredLifecycle>, Refusal> {
    use ::aws_sdk_s3::types::{BucketLifecycleConfiguration, TransitionDefaultMinimumObjectSize};

    let Some(declared) = resource.properties.get(LIFECYCLE_CONFIGURATION) else {
        return Ok(None);
    };
    let listed = declared
        .get("Rules")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rules = Vec::with_capacity(listed.len());
    for (index, rule) in listed.iter().enumerate() {
        // Counted from 1, the way the declaration orders the rules, because `Id` is optional:
        // a rule that states none is still identified in a refusal.
        rules.push(declared_lifecycle_rule(rule, index + 1)?);
    }
    let configuration = BucketLifecycleConfiguration::builder()
        .set_rules(Some(rules))
        .build()
        .map_err(|error| lifecycle_not_sendable("the rule set", &error))?;
    Ok(Some(DeclaredLifecycle {
        configuration,
        minimum_object_size: declared_text(declared, "TransitionDefaultMinimumObjectSize")
            .map(|size| TransitionDefaultMinimumObjectSize::from(size.as_str())),
    }))
}

/// One declared lifecycle rule, as the API's own `LifecycleRule`.
pub(super) fn declared_lifecycle_rule(
    declared: &Value,
    position: usize,
) -> Result<::aws_sdk_s3::types::LifecycleRule, Refusal> {
    use ::aws_sdk_s3::types::{ExpirationStatus, LifecycleRule};

    let named = format!("rule {position}");
    let status = declared_text(declared, "Status").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "{named} of the {LIFECYCLE_CONFIGURATION} of {S3_BUCKET} declares no Status"
        ),
    })?;
    let transitions = declared_transitions(declared, &named)?;
    let noncurrent_transitions = declared_noncurrent_version_transitions(declared, &named)?;
    LifecycleRule::builder()
        .status(ExpirationStatus::from(status.as_str()))
        .set_id(declared_text(declared, "Id"))
        // The scope travels as a `Filter` whatever the rule narrows by, including nothing at
        // all: "if the Filter element is left empty, the lifecycle rule applies to all
        // objects in the bucket" (`LifecycleRuleFilter`), where the rule's own deprecated
        // `Prefix` member is the older spelling a configuration cannot mix with a `Filter`.
        .filter(lifecycle_rule_filter(declared, &named)?)
        .set_expiration(lifecycle_expiration(declared, &named)?)
        .set_transitions((!transitions.is_empty()).then_some(transitions))
        .set_noncurrent_version_transitions(
            (!noncurrent_transitions.is_empty()).then_some(noncurrent_transitions),
        )
        .set_noncurrent_version_expiration(declared_noncurrent_version_expiration(declared))
        .set_abort_incomplete_multipart_upload(declared_abort_incomplete_multipart_upload(
            declared,
        ))
        .build()
        .map_err(|error| lifecycle_not_sendable(&named, &error))
}

/// The refusal for a declared lifecycle the SDK will not build into the request shape: the
/// rule is named, so the failing declaration can be found.
pub(super) fn lifecycle_not_sendable(named: &str, error: &dyn ::std::fmt::Display) -> Refusal {
    Refusal {
        code: UNIDENTIFIED_FAILURE.to_string(),
        message: format!(
            "{named} of the {LIFECYCLE_CONFIGURATION} of {S3_BUCKET} cannot be sent to Amazon \
             S3: {error}"
        ),
    }
}

/// The `LifecycleRuleFilter` one declared rule's object scope amounts to.
///
/// The template narrows with four flat properties — a `Prefix`, a `TagFilters` list and the
/// two object-size bounds — while the API's container carries exactly one of them or an `And`
/// conjunction of several (`smithy_query(query="shape",shape="LifecycleRuleFilter")`). So a
/// scope naming ONE predicate is that predicate's own member, a scope naming several is the
/// conjunction of them, and a scope naming none is the EMPTY filter, which is the rule that
/// applies to every object in the bucket.
///
/// A scope narrowing by a TAG is the one exception, and it is a property of this translation
/// rather than of the API: a declared `TagFilters` list travels as the `And` conjunction
/// WHATEVER its length, and that conjunction states a `Prefix` — the declared one, or the
/// EMPTY prefix for a rule that declares none — so one declared tag reads back as
/// `And{Prefix, Tags:[the tag]}` rather than as the single `Tag` member. `Filter.Tag` is what
/// a configuration written straight through `PutBucketLifecycleConfiguration` carries for the
/// same single tag; the two forms narrow by the same objects, and a client walking rules a
/// stack declared branches on the conjunction. Recorded behaviour: the flat-to-nested
/// translation is not part of the API model.
pub(super) fn lifecycle_rule_filter(
    declared: &Value,
    named: &str,
) -> Result<::aws_sdk_s3::types::LifecycleRuleFilter, Refusal> {
    use ::aws_sdk_s3::types::{LifecycleRuleAndOperator, LifecycleRuleFilter};

    let prefix = declared_text(declared, "Prefix");
    let greater_than = declared_object_size(declared, "ObjectSizeGreaterThan");
    let less_than = declared_object_size(declared, "ObjectSizeLessThan");
    let tags = declared_tag_filters(declared, LIFECYCLE_CONFIGURATION, named)?;
    let tagged = !tags.is_empty();
    let predicates = usize::from(prefix.is_some())
        + usize::from(greater_than.is_some())
        + usize::from(less_than.is_some())
        + tags.len();
    if tagged || predicates > 1 {
        return Ok(LifecycleRuleFilter::builder()
            .and(
                LifecycleRuleAndOperator::builder()
                    // A tag-narrowed conjunction states a prefix either way, the empty one
                    // standing for "every key"; a conjunction of the two object-size bounds
                    // alone narrows no prefix at all, which is not the same as the empty one.
                    .set_prefix(prefix.or_else(|| tagged.then(String::new)))
                    // A conjunction of the two object-size bounds alone carries no tag at
                    // all, which is not the same as an empty tag list.
                    .set_tags(tagged.then_some(tags))
                    .set_object_size_greater_than(greater_than)
                    .set_object_size_less_than(less_than)
                    .build(),
            )
            .build());
    }
    Ok(LifecycleRuleFilter::builder()
        .set_prefix(prefix)
        .set_object_size_greater_than(greater_than)
        .set_object_size_less_than(less_than)
        .build())
}

/// The `Expiration` one declared rule asks for, or `None` for a rule that expires nothing.
///
/// The three members the template spells flat are the three the API's container carries, with
/// `ExpirationInDays` spelled `Days` and `ExpirationDate` spelled `Date`: whichever the rule
/// states travel through, and S3's own validation is what judges a rule stating more than one
/// of them.
pub(super) fn lifecycle_expiration(
    declared: &Value,
    named: &str,
) -> Result<Option<::aws_sdk_s3::types::LifecycleExpiration>, Refusal> {
    use ::aws_sdk_s3::types::LifecycleExpiration;

    let date = declared_instant(declared, "ExpirationDate", named)?;
    let days = whole_number(declared.get("ExpirationInDays"));
    let delete_marker = declared
        .as_object()
        .and_then(|rule| declared_flag(rule, "ExpiredObjectDeleteMarker"));
    if date.is_none() && days.is_none() && delete_marker.is_none() {
        return Ok(None);
    }
    Ok(Some(
        LifecycleExpiration::builder()
            .set_date(date)
            .set_days(days)
            .set_expired_object_delete_marker(delete_marker)
            .build(),
    ))
}

/// Every transition one declared rule asks for, in declared order.
///
/// The property is a `Transitions` LIST, beside the singular `Transition` the type still
/// publishes for one transition alone; a rule stating both asks for all of them, so both are
/// read. `TransitionInDays` and `TransitionDate` are the API's `Days` and `Date`.
pub(super) fn declared_transitions(
    declared: &Value,
    named: &str,
) -> Result<Vec<::aws_sdk_s3::types::Transition>, Refusal> {
    use ::aws_sdk_s3::types::{Transition, TransitionStorageClass};

    let mut transitions = Vec::new();
    for transition in declared_entries(declared, "Transitions", "Transition") {
        let storage_class = declared_text(transition, "StorageClass").ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "a Transition of {named} of the {LIFECYCLE_CONFIGURATION} of {S3_BUCKET} \
                 declares no StorageClass"
            ),
        })?;
        transitions.push(
            Transition::builder()
                .storage_class(TransitionStorageClass::from(storage_class.as_str()))
                .set_days(whole_number(transition.get("TransitionInDays")))
                .set_date(declared_instant(transition, "TransitionDate", named)?)
                .build(),
        );
    }
    Ok(transitions)
}

/// Every noncurrent-version transition one declared rule asks for, in declared order — the
/// list and the singular property alike, as [`declared_transitions`] reads the current-version
/// ones. `TransitionInDays` is the API's `NoncurrentDays`.
pub(super) fn declared_noncurrent_version_transitions(
    declared: &Value,
    named: &str,
) -> Result<Vec<::aws_sdk_s3::types::NoncurrentVersionTransition>, Refusal> {
    use ::aws_sdk_s3::types::{NoncurrentVersionTransition, TransitionStorageClass};

    let mut transitions = Vec::new();
    for transition in declared_entries(
        declared,
        "NoncurrentVersionTransitions",
        "NoncurrentVersionTransition",
    ) {
        let storage_class = declared_text(transition, "StorageClass").ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "a NoncurrentVersionTransition of {named} of the {LIFECYCLE_CONFIGURATION} of \
                 {S3_BUCKET} declares no StorageClass"
            ),
        })?;
        transitions.push(
            NoncurrentVersionTransition::builder()
                .storage_class(TransitionStorageClass::from(storage_class.as_str()))
                .set_noncurrent_days(whole_number(transition.get("TransitionInDays")))
                .set_newer_noncurrent_versions(whole_number(
                    transition.get("NewerNoncurrentVersions"),
                ))
                .build(),
        );
    }
    Ok(transitions)
}

/// The `NoncurrentVersionExpiration` one declared rule asks for, or `None` for a rule that
/// expires no noncurrent version.
///
/// The type publishes the container beside the older `NoncurrentVersionExpirationInDays`,
/// which states the same day count on its own, so either spelling yields the one member the
/// API's container REQUIRES.
pub(super) fn declared_noncurrent_version_expiration(
    declared: &Value,
) -> Option<::aws_sdk_s3::types::NoncurrentVersionExpiration> {
    use ::aws_sdk_s3::types::NoncurrentVersionExpiration;

    let container = declared.get("NoncurrentVersionExpiration");
    let days = container
        .and_then(|expiration| whole_number(expiration.get("NoncurrentDays")))
        .or_else(|| whole_number(declared.get("NoncurrentVersionExpirationInDays")))?;
    Some(
        NoncurrentVersionExpiration::builder()
            .noncurrent_days(days)
            .set_newer_noncurrent_versions(
                container.and_then(|expiration| {
                    whole_number(expiration.get("NewerNoncurrentVersions"))
                }),
            )
            .build(),
    )
}

/// The `AbortIncompleteMultipartUpload` one declared rule asks for, or `None` for a rule that
/// aborts none. Both sides spell the container and its one member the same way.
pub(super) fn declared_abort_incomplete_multipart_upload(
    declared: &Value,
) -> Option<::aws_sdk_s3::types::AbortIncompleteMultipartUpload> {
    use ::aws_sdk_s3::types::AbortIncompleteMultipartUpload;

    let days = whole_number(
        declared
            .get("AbortIncompleteMultipartUpload")?
            .get("DaysAfterInitiation"),
    )?;
    Some(
        AbortIncompleteMultipartUpload::builder()
            .days_after_initiation(days)
            .build(),
    )
}

/// The entries a declaration states under `list`, followed by the one it states under the
/// singular `single` — the shape a resource type takes where it publishes both spellings for
/// the same thing.
pub(super) fn declared_entries<'a>(declared: &'a Value, list: &str, single: &str) -> Vec<&'a Value> {
    let mut entries: Vec<&Value> = declared
        .get(list)
        .and_then(Value::as_array)
        .map(|listed| listed.iter().collect())
        .unwrap_or_default();
    entries.extend(declared.get(single));
    entries
}

/// One declared property as the instant an API timestamp member takes, or `None` where the
/// declaration states none. Text that is no ISO-8601 instant is a refusal rather than a
/// silently dropped action.
pub(super) fn declared_instant(
    declared: &Value,
    property: &str,
    named: &str,
) -> Result<Option<::aws_smithy_types::DateTime>, Refusal> {
    let Some(text) = declared_text(declared, property) else {
        return Ok(None);
    };
    ::aws_smithy_types::DateTime::from_str(&text, ::aws_smithy_types::date_time::Format::DateTime)
        .map(Some)
        .map_err(|error| Refusal {
            code: INVALID_PROPERTY_CODE.to_string(),
            message: format!(
                "the {property} of {named} of the {LIFECYCLE_CONFIGURATION} of {S3_BUCKET} is no \
                 instant: {error}"
            ),
        })
}

/// One declared property as the object size in BYTES an API long member takes, in either
/// spelling a template gives a whole number ([`whole_number`] reads the same two for the
/// members the API takes as an integer).
pub(super) fn declared_object_size(declared: &Value, property: &str) -> Option<i64> {
    match declared.get(property)? {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// One property of a declaration as the text an API member takes, or `None` where the
/// declaration states it as nothing, as the empty string or as a composite.
pub(super) fn declared_text(declared: &Value, property: &str) -> Option<String> {
    declared
        .get(property)
        .and_then(scalar_text)
        .filter(|text| !text.is_empty())
}

/// The property an `AWS::S3::Bucket` declares its DEFAULT encryption under.
pub(super) const BUCKET_ENCRYPTION: &str = "BucketEncryption";

/// The whole `BucketEncryption` a bucket's declaration states, as the
/// `PutBucketEncryption` request Amazon S3 takes — `None` for a declaration that states no
/// default encryption of its own, which leaves the bucket on the default every bucket has.
///
/// The template nests the rules one level differently from the API and renames nothing else:
/// `BucketEncryption.ServerSideEncryptionConfiguration` is the LIST of rules the API carries
/// as `ServerSideEncryptionConfiguration.Rules`, and each entry states the same two things a
/// `ServerSideEncryptionRule` does — a `ServerSideEncryptionByDefault` naming the
/// `SSEAlgorithm` and, for the SSE-KMS algorithms, the `KMSMasterKeyID`, beside the rule's own
/// `BucketKeyEnabled`. `SSEAlgorithm` is REQUIRED inside that container, so an entry stating
/// the container without it is refused rather than sent incomplete: the stack must not
/// complete reporting a default encryption Amazon S3 was never asked for.
pub(super) fn declared_bucket_encryption(
    resource: &StoredResource,
) -> Result<Option<::aws_sdk_s3::types::ServerSideEncryptionConfiguration>, Refusal> {
    use ::aws_sdk_s3::types::{
        ServerSideEncryption, ServerSideEncryptionByDefault, ServerSideEncryptionConfiguration,
        ServerSideEncryptionRule,
    };

    let Some(declared) = resource.properties.get(BUCKET_ENCRYPTION) else {
        return Ok(None);
    };
    let listed = declared
        .get("ServerSideEncryptionConfiguration")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rules = Vec::with_capacity(listed.len());
    for rule in listed {
        let by_default = match rule.get("ServerSideEncryptionByDefault") {
            Some(container) => {
                let algorithm = declared_text(container, "SSEAlgorithm").ok_or_else(|| Refusal {
                    code: MISSING_PROPERTY_CODE.to_string(),
                    message: format!(
                        "a {BUCKET_ENCRYPTION}.ServerSideEncryptionConfiguration entry of \
                         {S3_BUCKET} declares no SSEAlgorithm"
                    ),
                })?;
                Some(
                    ServerSideEncryptionByDefault::builder()
                        .sse_algorithm(ServerSideEncryption::from(algorithm.as_str()))
                        .set_kms_master_key_id(declared_text(container, "KMSMasterKeyID"))
                        .build()
                        .map_err(|error| bucket_encryption_not_sendable(&error))?,
                )
            }
            None => None,
        };
        rules.push(
            ServerSideEncryptionRule::builder()
                .set_apply_server_side_encryption_by_default(by_default)
                .set_bucket_key_enabled(
                    rule.as_object()
                        .and_then(|stated| declared_flag(stated, "BucketKeyEnabled")),
                )
                .build(),
        );
    }
    if rules.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        ServerSideEncryptionConfiguration::builder()
            .set_rules(Some(rules))
            .build()
            .map_err(|error| bucket_encryption_not_sendable(&error))?,
    ))
}

/// The refusal for a declared default encryption the SDK will not build into the request
/// shape, worded the way every other sub-resource of this type words one.
fn bucket_encryption_not_sendable(error: &dyn ::std::fmt::Display) -> Refusal {
    Refusal {
        code: UNIDENTIFIED_FAILURE.to_string(),
        message: format!(
            "the {BUCKET_ENCRYPTION} of {S3_BUCKET} cannot be sent to Amazon S3: {error}"
        ),
    }
}

/// Delete the bucket a stack created. A bucket S3 no longer has is already in the state
/// the delete asks for; one that still holds objects is NOT deleted, and that refusal is
/// what the stack reports.
pub(super) async fn delete_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match s3_client(ctx, region)
        .delete_bucket()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = s3_refusal_of(&error);
            if refusal.code == NO_SUCH_BUCKET {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The error code Amazon S3 answers for a bucket a `HeadBucket` does not find
/// (`HeadBucketOutput`'s one modelled error is `NotFound`).
pub(super) const BUCKET_NOT_FOUND: &str = "NotFound";

/// The bucket's configuration as Amazon S3 reports it, for drift detection.
///
/// A bucket S3 no longer holds was deleted. For one it still holds, the configuration
/// reported is the `VersioningConfiguration` the create applies, under the property name the
/// template spells it with, so a bucket whose versioning was changed outside the stack is
/// MODIFIED. A versioning read the owning service refuses leaves the bucket reported as
/// existing with no comparable property rather than failing the whole detection. A declared
/// property this read does not report is not compared at all ("only resource properties
/// explicitly defined in the stack template are checked for drift", against what the owning
/// service reports), so the bucket's notification, analytics and inventory configurations —
/// which the create installs but this read does not fetch — are outside this detection rather
/// than reported as changed.
pub(super) async fn describe_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let client = s3_client(ctx, region);
    if let Err(error) = client
        .head_bucket()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        let refusal = s3_refusal_of(&error);
        if refusal.code == BUCKET_NOT_FOUND || refusal.code == NO_SUCH_BUCKET {
            return Ok(None);
        }
        return Err(refusal);
    }
    let mut actual = Map::new();
    if let Ok(versioning) = client
        .get_bucket_versioning()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        if let Some(status) = versioning.status() {
            actual.insert(
                "VersioningConfiguration".to_string(),
                serde_json::json!({ "Status": status.as_str() }),
            );
        }
    }
    Ok(Some(Value::Object(actual)))
}

// ---------------------------------------------------------------------------
// Amazon S3: AWS::S3::BucketPolicy
// ---------------------------------------------------------------------------

/// `AWS::S3::BucketPolicy`: not a resource of Amazon S3's own but the POLICY of the bucket
/// it names — the sub-resource `PutBucketPolicy` writes, `GetBucketPolicy` reads and
/// `DeleteBucketPolicy` removes.
pub(super) const S3_BUCKET_POLICY: &str = "AWS::S3::BucketPolicy";

/// The property naming the bucket whose policy this resource IS, which is the bucket every
/// call of this arm addresses.
pub(super) const S3_BUCKET_POLICY_BUCKET_PROPERTY: &str = "Bucket";

/// The property carrying the policy itself. A template declares it as a JSON document
/// where `PutBucketPolicy$Policy` takes "the bucket policy as a JSON document", so what
/// reaches S3 is that document's TEXT ([`document_text`]).
pub(super) const S3_BUCKET_POLICY_DOCUMENT_PROPERTY: &str = "PolicyDocument";

/// Every property `AWS::S3::BucketPolicy` publishes, which is what this arm carries to
/// Amazon S3: the bucket and the document. A declaration naming anything else is refused
/// rather than deployed with the property dropped.
pub(super) const S3_BUCKET_POLICY_PROPERTIES: &[&str] = &[
    S3_BUCKET_POLICY_BUCKET_PROPERTY,
    S3_BUCKET_POLICY_DOCUMENT_PROPERTY,
];

/// The code Amazon S3 answers for a bucket that carries no policy
/// (`GetBucketPolicy`/`DeleteBucketPolicy` on a bucket with none).
pub(super) const NO_SUCH_BUCKET_POLICY: &str = "NoSuchBucketPolicy";

/// Attach the policy an `AWS::S3::BucketPolicy` declares to the bucket it names, so a
/// client that deployed the stack — or another service of this emulator reading that
/// bucket's policy, as AWS Config does before it will deliver into it — sees the document
/// the template declared rather than a bucket carrying no policy at all.
///
/// The resource is the bucket's policy, so its identity is the BUCKET: every call of this
/// arm addresses it by the declared `Bucket`, and the policy's own physical id is the one
/// CloudFormation's naming rules computed for the resource.
pub(super) async fn put_bucket_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    if let Some(unmapped) = unmapped_property(&properties, S3_BUCKET_POLICY_PROPERTIES) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not carry the {unmapped} property of an \
                 {S3_BUCKET_POLICY} to Amazon S3; the resource was not created."
            ),
        });
    }
    set_bucket_policy(ctx, region, &Value::Object(properties)).await
}

/// `PutBucketPolicy` with the bucket and the document `properties` declares — the call
/// behind both this type's create and its update: "applies an Amazon S3 bucket policy to an
/// Amazon S3 bucket", replacing any policy the bucket already carries, which is exactly
/// what a changed declaration asks for.
pub(super) async fn set_bucket_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let bucket = declared_bucket_of_policy(properties)?;
    let document = properties
        .get(S3_BUCKET_POLICY_DOCUMENT_PROPERTY)
        .and_then(document_text)
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "an {S3_BUCKET_POLICY} declares the {S3_BUCKET_POLICY_DOCUMENT_PROPERTY} \
                 the bucket is to carry."
            ),
        })?;
    s3_client(ctx, region)
        .put_bucket_policy()
        .bucket(&bucket)
        .policy(document)
        .send()
        .await
        .map_err(|error| s3_refusal_of(&error))?;
    Ok(Created::default())
}

/// Remove the policy a stack attached from the bucket it named. A bucket that carries no
/// policy, and a bucket S3 no longer holds, are both already in the state the delete asks
/// for.
pub(super) async fn delete_bucket_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let bucket = declared_bucket_of_policy(&resource.properties)?;
    match s3_client(ctx, region)
        .delete_bucket_policy()
        .bucket(&bucket)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = s3_refusal_of(&error);
            if refusal.code == NO_SUCH_BUCKET_POLICY || refusal.code == NO_SUCH_BUCKET {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The policy the bucket carries NOW, for drift detection: the document `GetBucketPolicy`
/// answers, reported under the property name the template spells it with. A bucket that
/// carries no policy, or that S3 no longer holds, is a resource that is gone.
pub(super) async fn describe_bucket_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let bucket = declared_bucket_of_policy(&resource.properties)?;
    let answered = match s3_client(ctx, region)
        .get_bucket_policy()
        .bucket(&bucket)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = s3_refusal_of(&error);
            if refusal.code == NO_SUCH_BUCKET_POLICY || refusal.code == NO_SUCH_BUCKET {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let document: Value = answered
        .policy
        .as_deref()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    Ok(Some(serde_json::json!({
        S3_BUCKET_POLICY_BUCKET_PROPERTY: bucket,
        S3_BUCKET_POLICY_DOCUMENT_PROPERTY: document,
    })))
}

/// The bucket a declared bucket policy belongs to, or the refusal a declaration that names
/// none earns: the policy IS that bucket's, so there is no resource to write without it.
fn declared_bucket_of_policy(properties: &Value) -> Result<String, Refusal> {
    declared_text(properties, S3_BUCKET_POLICY_BUCKET_PROPERTY)
        .filter(|bucket| !bucket.is_empty())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "an {S3_BUCKET_POLICY} declares the {S3_BUCKET_POLICY_BUCKET_PROPERTY} whose \
                 policy it is."
            ),
        })
}

// ---------------------------------------------------------------------------
// Amazon EMR: AWS::EMR::Cluster
// ---------------------------------------------------------------------------

/// Amazon EMR's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: elasticmapreduce`, service shape `ElasticMapReduce`).
pub(super) const EMR_SIGNING_NAME: &str = "elasticmapreduce";
pub(super) const EMR_TARGET_PREFIX: &str = "ElasticMapReduce";

/// The error code Amazon EMR answers for a cluster id it does not hold.
pub(super) const INVALID_REQUEST: &str = "InvalidRequestException";

/// The deepest nesting of `Configurations` inside `Configurations` this arm descends into.
///
/// The declared properties come from a caller-supplied template body, so the nesting is the
/// REQUESTER's choice and a recursive walk over it must be bounded by a constant rather
/// than by the input: exhausting the native stack aborts the process instead of unwinding.
/// Chosen, and deliberately shallow, because EMR's own documented nesting is one level of
/// application configuration inside another — a deeper chain is a template built to explode
/// the walk, and it is REFUSED rather than quietly truncated.
pub(super) const MAX_CONFIGURATION_NESTING: usize = 8;

/// `InstanceGroupConfig$InstanceRole` for the three groups an `AWS::EMR::Cluster` declares
/// one property at a time, and which `RunJobFlow` takes as ONE list whose entries carry the
/// role. The one shape difference between the template's `Instances` and the API's.
pub(super) const INSTANCE_GROUP_ROLES: &[(&str, &str)] = &[
    ("MasterInstanceGroup", "MASTER"),
    ("CoreInstanceGroup", "CORE"),
    ("TaskInstanceGroups", "TASK"),
];

/// The same fold for the FLEET form of the same three node types: the template declares
/// `MasterInstanceFleet`, `CoreInstanceFleet` and `TaskInstanceFleets` one property at a
/// time where `RunJobFlow` takes one `InstanceFleets` list whose entries carry
/// `InstanceFleetConfig$InstanceFleetType` ("The node type that the instance fleet hosts.
/// Valid values are MASTER, CORE, and TASK").
///
/// A cluster may declare groups or fleets but not both ("A valid JobFlowInstancesConfig
/// must contain either InstanceGroups or InstanceFleets. They cannot be used together"),
/// which Amazon EMR's own `RunJobFlow` refuses — so a template declaring both is carried to
/// EMR as declared and left to that refusal rather than judged here.
pub(super) const INSTANCE_FLEET_ROLES: &[(&str, &str)] = &[
    ("MasterInstanceFleet", "MASTER"),
    ("CoreInstanceFleet", "CORE"),
    ("TaskInstanceFleets", "TASK"),
];

/// Run the job flow an `AWS::EMR::Cluster` declares, and report the `j-` job-flow id EMR
/// ASSIGNED it.
///
/// Unlike a stream, a repository or a bucket, this identifier is not CloudFormation's to
/// decide: "Ref returns the cluster ID", which exists only once EMR has run the flow — so
/// it is answered as the resource's own physical id ([`Created::physical_resource_id`]),
/// which [`super::provisioning::reconcile`] adopts, and the `Id`/`ClusterId` attributes are
/// re-pointed at it ([`super::template::identity_attributes`]).
///
/// Most of the type's properties ARE `RunJobFlow`'s own members (`ReleaseLabel`,
/// `Applications`, `BootstrapActions`, `Tags`, the roles and the scalars), so they are
/// passed through verbatim. Two are shaped differently and are mapped: `Instances`, whose
/// per-role capacity properties the API takes as one `InstanceGroups` or `InstanceFleets`
/// list, and `Configurations`, whose property map the template spells
/// `ConfigurationProperties` where the API spells it `Properties`. `Name` is the one member
/// CloudFormation decides when the template declares none: EMR requires a cluster name, so
/// the generated resource name is used.
///
/// The cluster carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `RunJobFlow`'s own `Tags` list — the `[{Key, Value}]`
/// spelling Amazon EMR's `Tag` shape uses, which is the type's own spelling too.
pub(super) async fn run_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let declared = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let mut request = Map::new();
    for (member, value) in &declared {
        match member.as_str() {
            "Instances" => {
                request.insert("Instances".to_string(), instances(value)?);
            }
            "Configurations" => {
                request.insert("Configurations".to_string(), configurations(value, 0)?);
            }
            _ => {
                request.insert(member.clone(), value.clone());
            }
        }
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let name = declared
        .get("Name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| resource.physical_resource_id.clone());
    request.insert("Name".to_string(), Value::String(name));

    let reported = call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.RunJobFlow"),
        &Value::Object(request),
    )
    .await?;
    let Some(job_flow_id) = reported.get("JobFlowId").and_then(Value::as_str) else {
        // EMR answered a success carrying no job-flow id: there is no identifier to report,
        // and nothing here invents one.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon EMR reported no JobFlowId for the job flow it ran".to_string(),
        });
    };
    Ok(Created {
        // The job-flow id is the cluster's identity, not an attribute beside it: "Ref
        // returns the cluster ID".
        physical_resource_id: Some(job_flow_id.to_string()),
        attributes: BTreeMap::new(),
    })
}

/// The cluster's configuration as Amazon EMR reports it, for drift detection:
/// `DescribeCluster`'s own record, read by the `j-` job-flow id EMR minted.
///
/// A cluster id EMR does not hold is refused with its own `InvalidRequestException` — "the
/// cluster id is not valid or does not exist" — which is the resource having been deleted.
/// The record's members are EMR's own (`Name`, `ReleaseLabel`, `Applications`,
/// `Configurations`), so a declared property spelled identically on both sides takes part in
/// the comparison and one spelled differently leaves the cluster reported as existing rather
/// than as modified, exactly as [`super::drift`] documents for every arm here.
pub(super) async fn describe_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ClusterId": physical_resource_id });
    match call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.DescribeCluster"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("Cluster").cloned()),
        Err(refusal) if refusal.code == INVALID_REQUEST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Terminate the job flow a stack created.
///
/// `TerminateJobFlows` "shuts a list of clusters down" and answers nothing; a cluster EMR
/// does not have is refused with its own `InvalidRequestException`, which is the state the
/// delete asks for.
pub(super) async fn terminate_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "JobFlowIds": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.TerminateJobFlows"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_REQUEST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The `Instances` an `AWS::EMR::Cluster` declares, as the `JobFlowInstancesConfig`
/// `RunJobFlow` takes: every member passed through, and the per-role capacity properties
/// folded onto the one list the API takes for that form — the three GROUP properties onto
/// `InstanceGroups`, keyed by `InstanceRole`, and the three FLEET properties onto
/// `InstanceFleets`, keyed by `InstanceFleetType`. Nothing a template declares is dropped:
/// a cluster created with less capacity than its template asked for is not the cluster the
/// template described.
pub(super) fn instances(declared: &Value) -> Result<Value, Refusal> {
    let Value::Object(members) = declared else {
        return Ok(declared.clone());
    };
    let mut config = Map::new();
    let mut groups: Vec<Value> = Vec::new();
    let mut fleets: Vec<Value> = Vec::new();
    for (member, value) in members {
        if let Some((_, role)) = INSTANCE_GROUP_ROLES
            .iter()
            .find(|(property, _)| property == member)
        {
            fold_per_role(member, value, "InstanceRole", role, &mut groups)?;
            continue;
        }
        if let Some((_, role)) = INSTANCE_FLEET_ROLES
            .iter()
            .find(|(property, _)| property == member)
        {
            fold_per_role(member, value, "InstanceFleetType", role, &mut fleets)?;
            continue;
        }
        config.insert(member.clone(), value.clone());
    }
    if !groups.is_empty() {
        config.insert("InstanceGroups".to_string(), Value::Array(groups));
    }
    if !fleets.is_empty() {
        config.insert("InstanceFleets".to_string(), Value::Array(fleets));
    }
    Ok(Value::Object(config))
}

/// Fold one per-role capacity property onto the list the API takes, stamping the role onto
/// each entry under `role_member`.
///
/// The task-node property is a LIST (`TaskInstanceGroups`, `TaskInstanceFleets`) where the
/// master and core ones are single structures, so both spellings are accepted. An entry
/// that is not a structure at all is REFUSED rather than dropped: it is capacity the
/// template asked for and this arm cannot carry.
pub(super) fn fold_per_role(
    property: &str,
    declared: &Value,
    role_member: &str,
    role: &str,
    onto: &mut Vec<Value>,
) -> Result<(), Refusal> {
    let entries: Vec<&Value> = match declared {
        Value::Array(items) => items.iter().collect(),
        single => vec![single],
    };
    for entry in entries {
        let Value::Object(entry) = entry else {
            return Err(malformed_property(property));
        };
        let mut built = entry.clone();
        built.insert(role_member.to_string(), Value::String(role.to_string()));
        onto.push(Value::Object(built));
    }
    Ok(())
}

/// The `Configurations` an `AWS::EMR::Cluster` declares, as the `Configuration` list
/// `RunJobFlow` takes — the application configurations (`hive-site`, `spark-hive-site`, …)
/// a client reads back off the cluster.
///
/// One member is spelled differently on the two sides: the template's property map is
/// `ConfigurationProperties` where the API's is `Properties`. Nested `Configurations` are
/// mapped the same way, to the depth the template declares them; a chain of more than
/// [`MAX_CONFIGURATION_NESTING`] levels is REFUSED, because a cluster created with part of
/// the configuration its template declared is not the cluster the template described.
///
/// `depth` is how many levels are already above this one, so the outermost list is `0` and
/// the deepest list this carries is `MAX_CONFIGURATION_NESTING - 1`: exactly
/// [`MAX_CONFIGURATION_NESTING`] levels, which is what that constant names.
pub(super) fn configurations(declared: &Value, depth: usize) -> Result<Value, Refusal> {
    if depth >= MAX_CONFIGURATION_NESTING {
        return Err(Refusal {
            code: "ValidationException".to_string(),
            message: format!(
                "Configurations are nested deeper than the {MAX_CONFIGURATION_NESTING} levels \
                 this emulator carries, so the cluster was not created"
            ),
        });
    }
    let Value::Array(items) = declared else {
        return Ok(declared.clone());
    };
    let mut mapped = Vec::with_capacity(items.len());
    for configuration in items {
        let Value::Object(members) = configuration else {
            return Err(malformed_property("Configurations"));
        };
        let mut built = Map::new();
        for (member, value) in members {
            match member.as_str() {
                "ConfigurationProperties" => {
                    built.insert("Properties".to_string(), value.clone());
                }
                "Configurations" => {
                    built.insert(
                        "Configurations".to_string(),
                        configurations(value, depth + 1)?,
                    );
                }
                _ => {
                    built.insert(member.clone(), value.clone());
                }
            }
        }
        mapped.push(Value::Object(built));
    }
    Ok(Value::Array(mapped))
}

/// The refusal for a declared property this arm cannot map onto the owning API's member,
/// named so a caller can tell WHICH property it was. Refusing is what keeps a resource from
/// being created with less than the template declared.
pub(super) fn malformed_property(property: &str) -> Refusal {
    Refusal {
        code: "ValidationException".to_string(),
        message: format!("Property {property} is not shaped as the resource type declares it"),
    }
}

// ---------------------------------------------------------------------------
// IAM: AWS::IAM::Role
// ---------------------------------------------------------------------------

/// The error code IAM answers for an entity it does not hold.
pub(super) const NO_SUCH_ENTITY: &str = "NoSuchEntity";
