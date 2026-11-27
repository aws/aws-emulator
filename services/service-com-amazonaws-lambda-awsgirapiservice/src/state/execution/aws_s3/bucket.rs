// SPDX-License-Identifier: CC0-1.0

//! The BUCKET-WIDE half of the Amazon S3 seam a running handler calls S3 through: a
//! bucket's own configuration subresources, and the operations that act on the objects it
//! holds as a SET.
//!
//! A handler that administers storage rather than storing through it calls these: it brings a
//! bucket into being and tears it down again, probes
//! whether a bucket is there and usable at all, lists the account's buckets, reads the Region
//! a bucket resides in, reads and
//! replaces the notification container that decides where a bucket's object events are
//! published, reads, replaces and REMOVES the bucket's tag set, its resource policy and its
//! default server-side encryption, reads and replaces its versioning state, lists the
//! versions a bucket holds and removes a set of them in one call. Every one of them is bound exactly as the object calls
//! beside it are ([`super`]): the request is built, signed and parsed by the S3 client this
//! crate holds, over the in-process callback, so the answer is the emulated S3 service's
//! own — that operation's output members for a success, and S3's own code, message and
//! status for a refusal.
//!
//! The input of each is read out of the document the handler's client method was given, with
//! the nested containers these operations carry (`CreateBucketConfiguration`,
//! `NotificationConfiguration`, `Tagging`, `Delete`) walked member by member as the model lays
//! them out. A member this binding does
//! not carry stops the call rather than travelling dropped, which is the rule the whole seam
//! holds to.

use ::serde_json::{Map, Value};

use ::aws_sdk_s3::types::{
    BucketCannedAcl, BucketInfo, BucketLocationConstraint, BucketType, BucketVersioningStatus,
    ChecksumAlgorithm, CreateBucketConfiguration, DataRedundancy, Delete, Event,
    EventBridgeConfiguration, FilterRule,
    FilterRuleName, LambdaFunctionConfiguration, LocationInfo, LocationType, MfaDelete,
    MfaDeleteStatus,
    NotificationConfiguration, NotificationConfigurationFilter, ObjectIdentifier, ObjectOwnership,
    Owner,
    QueueConfiguration, S3KeyFilter, ServerSideEncryption, ServerSideEncryptionByDefault,
    ServerSideEncryptionConfiguration, ServerSideEncryptionRule, Tag, TopicConfiguration,
    VersioningConfiguration,
};
use ::aws_sdk_s3::Client;

use super::super::aws_api::{Answer, Refusal};
use super::{
    insert_flag, insert_number, insert_text, insert_timestamp, not_made, read_tagging, refused,
    refused_without_a_document, tag_set_document, Input, Observed,
};

/// `CreateBucket`: the bucket itself, brought into being by this call.
///
/// "Creates a new S3 bucket… By creating the bucket, you become the bucket owner"
/// (`CreateBucket`'s own documentation): the first call a handler that PROVISIONS storage makes,
/// which is why it is bound beside the probe and the listing below. Where the bucket is placed
/// is the `CreateBucketConfiguration` payload's to say — the Region a general purpose bucket
/// resides in (`CreateBucketConfiguration$LocationConstraint`), the zone and the kind a
/// directory bucket is placed in (`$Location`, `$Bucket`) and the tag set it comes into being
/// with (`$Tags`) — so that container is walked member by member and sent as the operation
/// binds it.
///
/// A name the account already holds, or another account holds, is S3's own refusal to answer
/// (`BucketAlreadyOwnedByYou`, `BucketAlreadyExists`), reported as every other refusal of this
/// seam is: the service's own code, message and status.
pub(super) async fn create_bucket(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "CreateBucketConfiguration",
        "ACL",
        "GrantFullControl",
        "GrantRead",
        "GrantReadACP",
        "GrantWrite",
        "GrantWriteACP",
        "ObjectLockEnabledForBucket",
        "ObjectOwnership",
    ])?;
    let configuration = match input.structure("CreateBucketConfiguration")? {
        Some(carried) => Some(create_bucket_configuration(&carried)?),
        None => None,
    };
    let sent = client
        .create_bucket()
        .bucket(input.required_text("Bucket")?)
        .set_create_bucket_configuration(configuration)
        .set_acl(
            input
                .text("ACL")?
                .map(|value| BucketCannedAcl::from(value.as_str())),
        )
        .set_grant_full_control(input.text("GrantFullControl")?)
        .set_grant_read(input.text("GrantRead")?)
        .set_grant_read_acp(input.text("GrantReadACP")?)
        .set_grant_write(input.text("GrantWrite")?)
        .set_grant_write_acp(input.text("GrantWriteACP")?)
        .set_object_lock_enabled_for_bucket(input.boolean("ObjectLockEnabledForBucket")?)
        .set_object_ownership(
            input
                .text("ObjectOwnership")?
                .map(|value| ObjectOwnership::from(value.as_str())),
        )
        .send()
        .await;
    let created = match sent {
        Ok(created) => created,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Location", created.location());
    insert_text(&mut document, "BucketArn", created.bucket_arn());
    observed.answer(document, Vec::new())
}

/// The `CreateBucketConfiguration` payload a create carries, read off the input the handler
/// passed.
///
/// Every member of the container is optional (`smithy_query(query="shape",
/// shape="CreateBucketConfiguration")`: `LocationConstraint`, `Location`, `Bucket`, `Tags`), so
/// a container carrying none of one leaves that member off the call rather than sending a value
/// the handler did not ask for; a member this binding does not carry stops the call, as
/// everywhere else on this seam.
fn create_bucket_configuration(carried: &Input) -> Result<CreateBucketConfiguration, Refusal> {
    carried.only(&["LocationConstraint", "Location", "Bucket", "Tags"])?;
    let location = match carried.structure("Location")? {
        Some(held) => {
            held.only(&["Type", "Name"])?;
            Some(
                LocationInfo::builder()
                    .set_type(
                        held.text("Type")?
                            .map(|value| LocationType::from(value.as_str())),
                    )
                    .set_name(held.text("Name")?)
                    .build(),
            )
        }
        None => None,
    };
    let kind = match carried.structure("Bucket")? {
        Some(held) => {
            held.only(&["DataRedundancy", "Type"])?;
            Some(
                BucketInfo::builder()
                    .set_data_redundancy(
                        held.text("DataRedundancy")?
                            .map(|value| DataRedundancy::from(value.as_str())),
                    )
                    .set_type(
                        held.text("Type")?
                            .map(|value| BucketType::from(value.as_str())),
                    )
                    .build(),
            )
        }
        None => None,
    };
    let mut tags = Vec::new();
    for tag in carried.structures("Tags")? {
        tag.only(&["Key", "Value"])?;
        tags.push(
            Tag::builder()
                .key(tag.required_text("Key")?)
                .value(tag.required_text("Value")?)
                .build()
                .map_err(not_made)?,
        );
    }
    Ok(CreateBucketConfiguration::builder()
        .set_location_constraint(
            carried
                .text("LocationConstraint")?
                .map(|value| BucketLocationConstraint::from(value.as_str())),
        )
        .set_location(location)
        .set_bucket(kind)
        .set_tags((!tags.is_empty()).then_some(tags))
        .build())
}

/// `DeleteBucket`: the bucket itself, removed by this call.
///
/// "Deletes the S3 bucket. All objects (including all object versions and delete markers) in
/// the bucket must be deleted before the bucket itself can be deleted" (`DeleteBucket`'s own
/// documentation) — the last call a handler that tears its own storage down makes, which is why
/// it is bound beside the create above and the set removal below. A bucket that still holds
/// anything, or that does not exist, is S3's own refusal to answer, reported as such.
pub(super) async fn delete_bucket(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .delete_bucket()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        // The operation's output is `Unit`: the success IS the status, with no member to
        // report.
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `HeadBucket`: whether the bucket is there and whether this identity may use it.
///
/// "You can use this operation to determine if a bucket exists and if you have permission to
/// access it. The action returns a 200 OK HTTP status code if the bucket exists and you have
/// permission to access it" (`HeadBucket`'s own documentation) — the probe a handler makes
/// before it stores anything, which is why it is bound beside the HEAD of one object.
///
/// Its answer carries NO BODY either way — "if the bucket doesn't exist or you don't have
/// permission to access it, the HEAD request returns a generic 400 Bad Request, 403
/// Forbidden, or 404 Not Found HTTP status code. A message body isn't included, so you can't
/// determine the exception beyond these HTTP response codes" — so a refusal is reported the
/// way an SDK reports one with nothing to read: the STATUS as the code, with that status's
/// own reason as the message ([`super::refused_without_a_document`]), exactly as the HEAD of
/// an object is.
pub(super) async fn head_bucket(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .head_bucket()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused_without_a_document(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "BucketRegion", read.bucket_region());
    insert_text(&mut document, "BucketArn", read.bucket_arn());
    insert_text(&mut document, "BucketLocationName", read.bucket_location_name());
    insert_text(
        &mut document,
        "BucketLocationType",
        read.bucket_location_type()
            .map(::aws_sdk_s3::types::LocationType::as_str),
    );
    insert_flag(&mut document, "AccessPointAlias", read.access_point_alias());
    observed.answer(document, Vec::new())
}

/// `ListBuckets`: one page of the buckets the calling identity may list.
pub(super) async fn list_buckets(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["MaxBuckets", "ContinuationToken", "Prefix", "BucketRegion"])?;
    let sent = client
        .list_buckets()
        .set_max_buckets(input.number("MaxBuckets")?)
        .set_continuation_token(input.text("ContinuationToken")?)
        .set_prefix(input.text("Prefix")?)
        .set_bucket_region(input.text("BucketRegion")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    document.insert(
        "Buckets".to_string(),
        Value::Array(
            listed
                .buckets()
                .iter()
                .map(|bucket| {
                    let mut entry = Map::new();
                    insert_text(&mut entry, "Name", bucket.name());
                    insert_text(&mut entry, "BucketRegion", bucket.bucket_region());
                    insert_text(&mut entry, "BucketArn", bucket.bucket_arn());
                    insert_timestamp(&mut entry, "CreationDate", bucket.creation_date());
                    Value::Object(entry)
                })
                .collect(),
        ),
    );
    insert_owner(&mut document, listed.owner());
    insert_text(
        &mut document,
        "ContinuationToken",
        listed.continuation_token(),
    );
    insert_text(&mut document, "Prefix", listed.prefix());
    observed.answer(document, Vec::new())
}

/// `GetBucketLocation`: the Region the bucket resides in.
///
/// "Returns the Region the bucket resides in. You set the bucket's Region using the
/// LocationConstraint request parameter in a CreateBucket request" (`GetBucketLocation`'s own
/// documentation) — the read a handler resolves a bucket's Region with before it addresses a
/// client at it, which is why it is bound beside the probe and the listing above.
///
/// "Buckets in Region us-east-1 have a LocationConstraint of null"
/// (`GetBucketLocationOutput$LocationConstraint`), so the member is reported as the NULL an
/// answer carrying no constraint carries rather than left off the document: a handler reading
/// it for such a bucket reads the absence of a constraint, which is what that bucket's answer
/// says.
pub(super) async fn get_bucket_location(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_location()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let constraint = read
        .location_constraint()
        .map(BucketLocationConstraint::as_str)
        .filter(|constraint| !constraint.is_empty());
    let mut document = Map::new();
    document.insert(
        "LocationConstraint".to_string(),
        constraint.map_or(Value::Null, |constraint| {
            Value::String(constraint.to_string())
        }),
    );
    observed.answer(document, Vec::new())
}

/// `GetBucketNotificationConfiguration`: the notification container the bucket carries.
///
/// A bucket with no notifications configured carries an EMPTY container
/// (`PutBucketNotificationConfiguration`: "by default, your bucket has no event
/// notifications configured. That is, the notification configuration will be an empty
/// NotificationConfiguration"), so a list the answer holds none of is left off the document
/// rather than reported as an empty one.
pub(super) async fn get_bucket_notification_configuration(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_notification_configuration()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_configurations(
        &mut document,
        "TopicConfigurations",
        read.topic_configurations()
            .iter()
            .map(|configuration| {
                configuration_document(
                    configuration.id(),
                    "TopicArn",
                    configuration.topic_arn(),
                    configuration.events(),
                    configuration.filter(),
                )
            })
            .collect(),
    );
    insert_configurations(
        &mut document,
        "QueueConfigurations",
        read.queue_configurations()
            .iter()
            .map(|configuration| {
                configuration_document(
                    configuration.id(),
                    "QueueArn",
                    configuration.queue_arn(),
                    configuration.events(),
                    configuration.filter(),
                )
            })
            .collect(),
    );
    insert_configurations(
        &mut document,
        "LambdaFunctionConfigurations",
        read.lambda_function_configurations()
            .iter()
            .map(|configuration| {
                configuration_document(
                    configuration.id(),
                    "LambdaFunctionArn",
                    configuration.lambda_function_arn(),
                    configuration.events(),
                    configuration.filter(),
                )
            })
            .collect(),
    );
    // The shape carries no members at all, so its PRESENCE is the whole value
    // (`NotificationConfiguration$EventBridgeConfiguration`: "enables delivery of events to
    // Amazon EventBridge").
    if read.event_bridge_configuration().is_some() {
        document.insert(
            "EventBridgeConfiguration".to_string(),
            Value::Object(Map::new()),
        );
    }
    observed.answer(document, Vec::new())
}

/// `PutBucketNotificationConfiguration`: the whole container, replacing what the bucket
/// carried before.
pub(super) async fn put_bucket_notification_configuration(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "NotificationConfiguration",
        "SkipDestinationValidation",
        "ExpectedBucketOwner",
    ])?;
    let configuration = notification_configuration(
        &input.required_structure("NotificationConfiguration")?,
    )?;
    let sent = client
        .put_bucket_notification_configuration()
        .bucket(input.required_text("Bucket")?)
        .notification_configuration(configuration)
        .set_skip_destination_validation(input.boolean("SkipDestinationValidation")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        // The operation's output is `Unit`: the success IS the status, with no member to
        // report.
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `GetBucketTagging`: the bucket's own tag set.
pub(super) async fn get_bucket_tagging(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_tagging()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    // `TagSet` is a required output member, so it is reported whether or not the bucket
    // carries any tags (`GetBucketTaggingOutput$TagSet`).
    document.insert("TagSet".to_string(), tag_set_document(read.tag_set()));
    observed.answer(document, Vec::new())
}

/// `PutBucketTagging`: the whole tag set, replacing what the bucket carried before.
pub(super) async fn put_bucket_tagging(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "Tagging", "ExpectedBucketOwner"])?;
    let sent = client
        .put_bucket_tagging()
        .bucket(input.required_text("Bucket")?)
        .tagging(read_tagging(input)?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `DeleteBucketTagging`: the tag set removed from the bucket altogether.
///
/// The REMOVAL beside the read and the replacement above: "deletes the tags from the bucket"
/// (`DeleteBucketTagging`) leaves a bucket carrying no tag set at all, which is a different
/// state from one carrying an empty one, and a handler that cleans up after itself is how a
/// bucket reaches it.
pub(super) async fn delete_bucket_tagging(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .delete_bucket_tagging()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        // The operation's output is `Unit`: the success IS the status, with no member to
        // report.
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `GetBucketPolicy`: the resource policy in force on the bucket, as the document it is.
///
/// "Returns the policy of a specified bucket" (`GetBucketPolicy`): the member is the policy
/// DOCUMENT's own text (`GetBucketPolicyOutput$Policy`, bound to the whole payload), so what
/// a handler reads back is the text it would parse, not a container around it. A bucket
/// carrying no policy is S3's own refusal, reported as every other refusal of this seam is.
pub(super) async fn get_bucket_policy(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_policy()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Policy", read.policy());
    observed.answer(document, Vec::new())
}

/// `PutBucketPolicy`: the whole resource policy, replacing whatever the bucket carried.
pub(super) async fn put_bucket_policy(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Policy",
        "ChecksumAlgorithm",
        "ContentMD5",
        "ConfirmRemoveSelfBucketAccess",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .put_bucket_policy()
        .bucket(input.required_text("Bucket")?)
        // The policy is the operation's required payload (`PutBucketPolicyRequest$Policy`),
        // so a call carrying none stops here rather than replacing a policy with nothing.
        .policy(input.required_text("Policy")?)
        .set_checksum_algorithm(
            input
                .text("ChecksumAlgorithm")?
                .map(|value| ChecksumAlgorithm::from(value.as_str())),
        )
        .set_content_md5(input.text("ContentMD5")?)
        .set_confirm_remove_self_bucket_access(input.boolean("ConfirmRemoveSelfBucketAccess")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `DeleteBucketPolicy`: the policy removed, leaving the bucket with none.
pub(super) async fn delete_bucket_policy(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .delete_bucket_policy()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `GetBucketVersioning`: the versioning state the bucket is in.
///
/// "If the versioning state has never been set on a bucket, it has no versioning state"
/// (`GetBucketVersioning`), so a bucket nothing has set reports neither member rather than
/// a state it is not in.
pub(super) async fn get_bucket_versioning(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_versioning()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(
        &mut document,
        "Status",
        read.status().map(BucketVersioningStatus::as_str),
    );
    insert_text(
        &mut document,
        "MFADelete",
        read.mfa_delete().map(MfaDeleteStatus::as_str),
    );
    observed.answer(document, Vec::new())
}

/// `PutBucketVersioning`: the versioning state the bucket is put into.
pub(super) async fn put_bucket_versioning(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "VersioningConfiguration",
        "MFA",
        "ChecksumAlgorithm",
        "ContentMD5",
        "ExpectedBucketOwner",
    ])?;
    // `VersioningConfiguration` is the operation's required payload
    // (`smithy_query(query="shape",shape="VersioningConfiguration")`: `Status`, `MFADelete`).
    let configuration = input.required_structure("VersioningConfiguration")?;
    configuration.only(&["Status", "MFADelete"])?;
    let sent = client
        .put_bucket_versioning()
        .bucket(input.required_text("Bucket")?)
        .versioning_configuration(
            VersioningConfiguration::builder()
                .set_status(
                    configuration
                        .text("Status")?
                        .map(|value| BucketVersioningStatus::from(value.as_str())),
                )
                .set_mfa_delete(
                    configuration
                        .text("MFADelete")?
                        .map(|value| MfaDelete::from(value.as_str())),
                )
                .build(),
        )
        .set_mfa(input.text("MFA")?)
        .set_checksum_algorithm(
            input
                .text("ChecksumAlgorithm")?
                .map(|value| ChecksumAlgorithm::from(value.as_str())),
        )
        .set_content_md5(input.text("ContentMD5")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        // The operation's output is `Unit`: the success IS the status, with no member to
        // report.
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `GetBucketEncryption`: the default server-side encryption the bucket applies.
pub(super) async fn get_bucket_encryption(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_encryption()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let Some(configuration) = read.server_side_encryption_configuration() else {
        return observed.answer(Map::new(), Vec::new());
    };
    let rules: Vec<Value> = configuration
        .rules()
        .iter()
        .map(|rule| {
            let mut entry = Map::new();
            if let Some(by_default) = rule.apply_server_side_encryption_by_default() {
                let mut carried = Map::new();
                carried.insert(
                    "SSEAlgorithm".to_string(),
                    Value::String(by_default.sse_algorithm().as_str().to_string()),
                );
                insert_text(
                    &mut carried,
                    "KMSMasterKeyID",
                    by_default.kms_master_key_id(),
                );
                entry.insert(
                    "ApplyServerSideEncryptionByDefault".to_string(),
                    Value::Object(carried),
                );
            }
            if let Some(bucket_key) = rule.bucket_key_enabled() {
                entry.insert("BucketKeyEnabled".to_string(), Value::Bool(bucket_key));
            }
            Value::Object(entry)
        })
        .collect();
    let mut carried = Map::new();
    // `Rules` is a required member of the container, so it is reported whether or not it
    // holds anything (`ServerSideEncryptionConfiguration$Rules`).
    carried.insert("Rules".to_string(), Value::Array(rules));
    let mut document = Map::new();
    document.insert(
        "ServerSideEncryptionConfiguration".to_string(),
        Value::Object(carried),
    );
    observed.answer(document, Vec::new())
}

/// `PutBucketEncryption`: the whole default-encryption container, replacing what the bucket
/// carried before.
pub(super) async fn put_bucket_encryption(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "ServerSideEncryptionConfiguration",
        "ContentMD5",
        "ChecksumAlgorithm",
        "ExpectedBucketOwner",
    ])?;
    let configuration = input.required_structure("ServerSideEncryptionConfiguration")?;
    configuration.only(&["Rules"])?;
    let mut rules = Vec::new();
    // `Rules` is required on the container
    // (`smithy_query(query="shape",shape="ServerSideEncryptionConfiguration")`), so a
    // container carrying none of it stops the call rather than replacing the bucket's
    // encryption with nothing.
    for rule in configuration.required_structures("Rules")? {
        rule.only(&["ApplyServerSideEncryptionByDefault", "BucketKeyEnabled"])?;
        let by_default = match rule.structure("ApplyServerSideEncryptionByDefault")? {
            Some(carried) => {
                carried.only(&["SSEAlgorithm", "KMSMasterKeyID"])?;
                Some(
                    ServerSideEncryptionByDefault::builder()
                        .sse_algorithm(ServerSideEncryption::from(
                            carried.required_text("SSEAlgorithm")?.as_str(),
                        ))
                        .set_kms_master_key_id(carried.text("KMSMasterKeyID")?)
                        .build()
                        .map_err(not_made)?,
                )
            }
            None => None,
        };
        rules.push(
            ServerSideEncryptionRule::builder()
                .set_apply_server_side_encryption_by_default(by_default)
                .set_bucket_key_enabled(rule.boolean("BucketKeyEnabled")?)
                .build(),
        );
    }
    let sent = client
        .put_bucket_encryption()
        .bucket(input.required_text("Bucket")?)
        .server_side_encryption_configuration(
            ServerSideEncryptionConfiguration::builder()
                .set_rules(Some(rules))
                .build()
                .map_err(not_made)?,
        )
        .set_content_md5(input.text("ContentMD5")?)
        .set_checksum_algorithm(
            input
                .text("ChecksumAlgorithm")?
                .map(|value| ChecksumAlgorithm::from(value.as_str())),
        )
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `DeleteBucketEncryption`: the default-encryption container removed, leaving the bucket
/// with the encryption S3 applies to one that states none.
pub(super) async fn delete_bucket_encryption(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .delete_bucket_encryption()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `ListObjectVersions`: one page of every version and delete marker the bucket holds.
pub(super) async fn list_object_versions(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Prefix",
        "Delimiter",
        "MaxKeys",
        "KeyMarker",
        "VersionIdMarker",
        "EncodingType",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .list_object_versions()
        .bucket(input.required_text("Bucket")?)
        .set_prefix(input.text("Prefix")?)
        .set_delimiter(input.text("Delimiter")?)
        .set_max_keys(input.number("MaxKeys")?)
        .set_key_marker(input.text("KeyMarker")?)
        .set_version_id_marker(input.text("VersionIdMarker")?)
        .set_encoding_type(
            input
                .text("EncodingType")?
                .map(|value| ::aws_sdk_s3::types::EncodingType::from(value.as_str())),
        )
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Name", listed.name());
    insert_text(&mut document, "Prefix", listed.prefix());
    insert_text(&mut document, "Delimiter", listed.delimiter());
    insert_text(&mut document, "KeyMarker", listed.key_marker());
    insert_text(&mut document, "VersionIdMarker", listed.version_id_marker());
    insert_text(&mut document, "NextKeyMarker", listed.next_key_marker());
    insert_text(
        &mut document,
        "NextVersionIdMarker",
        listed.next_version_id_marker(),
    );
    insert_text(
        &mut document,
        "EncodingType",
        listed
            .encoding_type()
            .map(::aws_sdk_s3::types::EncodingType::as_str),
    );
    insert_number(&mut document, "MaxKeys", listed.max_keys().map(i64::from));
    if let Some(truncated) = listed.is_truncated() {
        document.insert("IsTruncated".to_string(), Value::Bool(truncated));
    }
    let versions: Vec<Value> = listed
        .versions()
        .iter()
        .map(|version| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", version.key());
            insert_text(&mut entry, "VersionId", version.version_id());
            insert_text(&mut entry, "ETag", version.e_tag());
            insert_number(&mut entry, "Size", version.size());
            insert_timestamp(&mut entry, "LastModified", version.last_modified());
            insert_text(
                &mut entry,
                "StorageClass",
                version
                    .storage_class()
                    .map(::aws_sdk_s3::types::ObjectVersionStorageClass::as_str),
            );
            if let Some(latest) = version.is_latest() {
                entry.insert("IsLatest".to_string(), Value::Bool(latest));
            }
            insert_owner(&mut entry, version.owner());
            Value::Object(entry)
        })
        .collect();
    if !versions.is_empty() {
        document.insert("Versions".to_string(), Value::Array(versions));
    }
    let markers: Vec<Value> = listed
        .delete_markers()
        .iter()
        .map(|marker| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", marker.key());
            insert_text(&mut entry, "VersionId", marker.version_id());
            insert_timestamp(&mut entry, "LastModified", marker.last_modified());
            if let Some(latest) = marker.is_latest() {
                entry.insert("IsLatest".to_string(), Value::Bool(latest));
            }
            insert_owner(&mut entry, marker.owner());
            Value::Object(entry)
        })
        .collect();
    if !markers.is_empty() {
        document.insert("DeleteMarkers".to_string(), Value::Array(markers));
    }
    let prefixes: Vec<Value> = listed
        .common_prefixes()
        .iter()
        .map(|common| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Prefix", common.prefix());
            Value::Object(entry)
        })
        .collect();
    if !prefixes.is_empty() {
        document.insert("CommonPrefixes".to_string(), Value::Array(prefixes));
    }
    observed.answer(document, Vec::new())
}

/// `DeleteObjects`: the removal of a SET of objects, which answers what became of each one.
pub(super) async fn delete_objects(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Delete",
        "MFA",
        "BypassGovernanceRetention",
        "ExpectedBucketOwner",
    ])?;
    let removals = input.required_structure("Delete")?;
    removals.only(&["Objects", "Quiet"])?;
    let mut objects = Vec::new();
    // `Objects` is required on the container (`smithy_query(query="shape",shape="Delete")`),
    // so a container carrying none of it stops the call rather than sending a removal that
    // names nothing.
    for object in removals.required_structures("Objects")? {
        object.only(&["Key", "VersionId", "ETag"])?;
        objects.push(
            ObjectIdentifier::builder()
                .key(object.required_text("Key")?)
                .set_version_id(object.text("VersionId")?)
                .set_e_tag(object.text("ETag")?)
                .build()
                .map_err(not_made)?,
        );
    }
    let sent = client
        .delete_objects()
        .bucket(input.required_text("Bucket")?)
        .delete(
            Delete::builder()
                .set_objects(Some(objects))
                .set_quiet(removals.boolean("Quiet")?)
                .build()
                .map_err(not_made)?,
        )
        .set_mfa(input.text("MFA")?)
        .set_bypass_governance_retention(input.boolean("BypassGovernanceRetention")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let removed = match sent {
        Ok(removed) => removed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    let deleted: Vec<Value> = removed
        .deleted()
        .iter()
        .map(|object| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", object.key());
            insert_text(&mut entry, "VersionId", object.version_id());
            insert_text(
                &mut entry,
                "DeleteMarkerVersionId",
                object.delete_marker_version_id(),
            );
            if let Some(marker) = object.delete_marker() {
                entry.insert("DeleteMarker".to_string(), Value::Bool(marker));
            }
            Value::Object(entry)
        })
        .collect();
    if !deleted.is_empty() {
        document.insert("Deleted".to_string(), Value::Array(deleted));
    }
    // What the service could NOT remove, per object: the code and sentence it reported for
    // that one key, which is the half of this answer a caller checks.
    let errors: Vec<Value> = removed
        .errors()
        .iter()
        .map(|failure| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", failure.key());
            insert_text(&mut entry, "VersionId", failure.version_id());
            insert_text(&mut entry, "Code", failure.code());
            insert_text(&mut entry, "Message", failure.message());
            Value::Object(entry)
        })
        .collect();
    if !errors.is_empty() {
        document.insert("Errors".to_string(), Value::Array(errors));
    }
    observed.answer(document, Vec::new())
}

/// The container a `PutBucketNotificationConfiguration` carries, read out of the document
/// the handler passed.
///
/// Every destination family the container holds is read the same way — an optional `Id`, the
/// ARN of the destination, the events it is published for, and an optional key-name filter —
/// because that is how the model shapes each of them (`TopicConfiguration`,
/// `QueueConfiguration`, `LambdaFunctionConfiguration`).
fn notification_configuration(document: &Input) -> Result<NotificationConfiguration, Refusal> {
    document.only(&[
        "TopicConfigurations",
        "QueueConfigurations",
        "LambdaFunctionConfigurations",
        "EventBridgeConfiguration",
    ])?;
    let mut topics = Vec::new();
    for entry in document.structures("TopicConfigurations")? {
        entry.only(&["Id", "TopicArn", "Events", "Filter"])?;
        topics.push(
            TopicConfiguration::builder()
                .set_id(entry.text("Id")?)
                .topic_arn(entry.required_text("TopicArn")?)
                .set_events(Some(events(&entry)?))
                .set_filter(key_filter(&entry)?)
                .build()
                .map_err(not_made)?,
        );
    }
    let mut queues = Vec::new();
    for entry in document.structures("QueueConfigurations")? {
        entry.only(&["Id", "QueueArn", "Events", "Filter"])?;
        queues.push(
            QueueConfiguration::builder()
                .set_id(entry.text("Id")?)
                .queue_arn(entry.required_text("QueueArn")?)
                .set_events(Some(events(&entry)?))
                .set_filter(key_filter(&entry)?)
                .build()
                .map_err(not_made)?,
        );
    }
    let mut functions = Vec::new();
    for entry in document.structures("LambdaFunctionConfigurations")? {
        entry.only(&["Id", "LambdaFunctionArn", "Events", "Filter"])?;
        functions.push(
            LambdaFunctionConfiguration::builder()
                .set_id(entry.text("Id")?)
                .lambda_function_arn(entry.required_text("LambdaFunctionArn")?)
                .set_events(Some(events(&entry)?))
                .set_filter(key_filter(&entry)?)
                .build()
                .map_err(not_made)?,
        );
    }
    // The EventBridge container carries no members, so what the request says is whether it
    // is there at all.
    let event_bridge = match document.structure("EventBridgeConfiguration")? {
        Some(carried) => {
            carried.only(&[])?;
            Some(EventBridgeConfiguration::builder().build())
        }
        None => None,
    };
    Ok(NotificationConfiguration::builder()
        .set_topic_configurations(Some(topics))
        .set_queue_configurations(Some(queues))
        .set_lambda_function_configurations(Some(functions))
        .set_event_bridge_configuration(event_bridge)
        .build())
}

/// The events one destination is published for, as the model's own `Event` values.
///
/// `Events` is a REQUIRED member of each destination family
/// (`smithy_query(query="shape",shape="QueueConfiguration")`), so a destination carrying none
/// of it stops the call rather than travelling with that member emptied; which values S3
/// publishes for is S3's own to judge.
fn events(entry: &Input) -> Result<Vec<Event>, Refusal> {
    Ok(entry
        .required_text_list("Events")?
        .into_iter()
        .map(|value| Event::from(value.as_str()))
        .collect())
}

/// The key-name filter one destination carries, or `None` where it filters on nothing.
///
/// `NotificationConfigurationFilter` carries exactly one member, `Key`, whose own `S3Key`
/// holds the rules (`smithy_query(query="shape",shape="S3KeyFilter")`), and each rule is a
/// name out of `FilterRuleName` (`prefix`, `suffix`) with the value it matches on.
fn key_filter(entry: &Input) -> Result<Option<NotificationConfigurationFilter>, Refusal> {
    let Some(filter) = entry.structure("Filter")? else {
        return Ok(None);
    };
    filter.only(&["Key"])?;
    let Some(key) = filter.structure("Key")? else {
        return Ok(Some(NotificationConfigurationFilter::builder().build()));
    };
    key.only(&["FilterRules"])?;
    let mut rules = Vec::new();
    for rule in key.structures("FilterRules")? {
        rule.only(&["Name", "Value"])?;
        rules.push(
            FilterRule::builder()
                .set_name(
                    rule.text("Name")?
                        .map(|name| FilterRuleName::from(name.as_str())),
                )
                .set_value(rule.text("Value")?)
                .build(),
        );
    }
    Ok(Some(
        NotificationConfigurationFilter::builder()
            .key(S3KeyFilter::builder().set_filter_rules(Some(rules)).build())
            .build(),
    ))
}

/// One destination of a notification container, as the answer reports it.
fn configuration_document(
    id: Option<&str>,
    arn_member: &str,
    arn: &str,
    events: &[Event],
    filter: Option<&NotificationConfigurationFilter>,
) -> Value {
    let mut entry = Map::new();
    insert_text(&mut entry, "Id", id);
    entry.insert(arn_member.to_string(), Value::String(arn.to_string()));
    entry.insert(
        "Events".to_string(),
        Value::Array(
            events
                .iter()
                .map(|event| Value::String(event.as_str().to_string()))
                .collect(),
        ),
    );
    if let Some(filter) = filter {
        entry.insert("Filter".to_string(), filter_document(filter));
    }
    Value::Object(entry)
}

/// The key-name filter one reported destination carries, as the answer reports it.
fn filter_document(filter: &NotificationConfigurationFilter) -> Value {
    let mut document = Map::new();
    if let Some(key) = filter.key() {
        let rules: Vec<Value> = key
            .filter_rules()
            .iter()
            .map(|rule| {
                let mut entry = Map::new();
                insert_text(&mut entry, "Name", rule.name().map(FilterRuleName::as_str));
                insert_text(&mut entry, "Value", rule.value());
                Value::Object(entry)
            })
            .collect();
        let mut carried = Map::new();
        carried.insert("FilterRules".to_string(), Value::Array(rules));
        document.insert("Key".to_string(), Value::Object(carried));
    }
    Value::Object(document)
}

/// Put a destination list on the answer's document, where the answer carried any.
fn insert_configurations(document: &mut Map<String, Value>, name: &str, entries: Vec<Value>) {
    if !entries.is_empty() {
        document.insert(name.to_string(), Value::Array(entries));
    }
}

/// Put the owner of a listed version on its entry, where the answer carried one.
fn insert_owner(entry: &mut Map<String, Value>, owner: Option<&Owner>) {
    let Some(owner) = owner else {
        return;
    };
    let mut carried = Map::new();
    insert_text(&mut carried, "ID", owner.id());
    insert_text(&mut carried, "DisplayName", owner.display_name());
    entry.insert("Owner".to_string(), Value::Object(carried));
}
