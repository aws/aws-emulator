// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon S3 Tables: the `AWS::S3Tables::TableBucket` a
//! stack creates there. One file of [`super`]'s per-type arms, dispatched to by its
//! `Provisioner`.
//!
//! This is the THIRD bucket variant, beside Amazon S3's own bucket
//! ([`super::data_and_storage`]) and the Amazon S3 Vectors vector bucket
//! ([`super::network_and_delivery`]), and the three differ in exactly one thing that matters
//! to a client reading one out of a stack: the identifier their owning service accepts. S3
//! takes a bucket NAME, while S3 Tables and S3 Vectors each key their bucket by its own ARN
//! — which is why this type's `Ref` and physical resource id are that ARN
//! ([`super::template::return_values`]) and why every call below addresses the bucket with
//! it.

use super::*;

/// Amazon S3 Tables' signing name, from its model (`sigv4Name: s3tables`, service shape
/// `S3TableBuckets`), which speaks `restJson1` — so its calls ride on [`call_rest_json`],
/// each on the URI its own operation's `@http` binding declares.
pub(super) const S3TABLES_SIGNING_NAME: &str = "s3tables";

/// The URI `CreateTableBucket` is bound to (`PUT /buckets`), from that model's `@http`
/// trait.
pub(super) const S3TABLES_BUCKETS_URI: &str = "/buckets";

/// The error code S3 Tables answers for a table bucket it does not hold
/// (`NotFoundException`, the modelled 404 of its reads and its deletes).
pub(super) const S3TABLES_NOT_FOUND: &str = "NotFoundException";

/// Every property `AWS::S3Tables::TableBucket` publishes that this arm carries to Amazon S3
/// Tables: the bucket's name, which is `CreateTableBucket`'s own `name` member, the
/// encryption settings that call's `encryptionConfiguration` takes, and the tag list its
/// `tags` map takes.
///
/// A declaration naming anything else — the `UnreferencedFileRemoval` maintenance settings
/// among them, which are no member of that create but a
/// `PutTableBucketMaintenanceConfiguration` of their own — is REFUSED
/// ([`refuse_unmapped_properties`]) rather than deployed with the property dropped, so a
/// stack never reports a table bucket configured differently from its declaration.
pub(super) const S3TABLES_TABLE_BUCKET_PROPERTIES: &[&str] =
    &["TableBucketName", "EncryptionConfiguration", "Tags"];

/// The members of an `AWS::S3Tables::TableBucket`'s `EncryptionConfiguration` as
/// `CreateTableBucket` spells them: that type publishes the algorithm and the key with the
/// upper-case initial and acronym CloudFormation spells members with, while the API's own
/// `EncryptionConfiguration` has `sseAlgorithm` and `kmsKeyArn`
/// (`smithy_query(query="shape", shape="EncryptionConfiguration")` on that service's model).
pub(super) const S3TABLES_ENCRYPTION_MEMBERS: &[(&str, &str)] = &[
    ("SSEAlgorithm", "sseAlgorithm"),
    ("KMSKeyArn", "kmsKeyArn"),
];

/// Create the table bucket an `AWS::S3Tables::TableBucket` declares, so a client that
/// deployed the stack and then calls `s3tables GetTableBucket` with the ARN the stack
/// publishes reads the bucket back rather than a refusal.
///
/// `Ref` of this type returns the bucket's ARN, which is this resource's physical id, and
/// such an ARN is a total function of the Region and the bucket's NAME — so the bucket is
/// created under the name CloudFormation decided ([`table_bucket_name`]) and the ARN S3
/// Tables answers with is the one the stack already computed. It is reported back as the
/// resource's `TableBucketARN` attribute all the same, so the handle the stack publishes is
/// the one that service really minted.
///
/// The type's remaining properties are that create's own members: the
/// `EncryptionConfiguration`, whose two members S3 Tables spells in lower camel case
/// ([`S3TABLES_ENCRYPTION_MEMBERS`]), and the tags — the ones the declaration asks for plus
/// the ones CloudFormation gives the resource ([`tags_of`]), as the MAP that request takes,
/// so `s3tables ListTagsForResource` on the ARN the stack publishes answers with the
/// stack's own tags.
pub(super) async fn create_table_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, S3TABLES_TABLE_BUCKET_PROPERTIES)?;
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(table_bucket_name(resource)),
    );
    if let Some(configuration) = resource.properties.get("EncryptionConfiguration") {
        request.insert(
            "encryptionConfiguration".to_string(),
            mapped_members(configuration, S3TABLES_ENCRYPTION_MEMBERS),
        );
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json_named(
        ctx,
        S3TABLES_SIGNING_NAME,
        region,
        "CreateTableBucket",
        "PUT",
        S3TABLES_BUCKETS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = answered
        .get("arn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon S3 Tables answered CreateTableBucket without an arn".to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert(
        S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE.to_string(),
        arn.to_string(),
    );
    Ok(Created::attributes(attributes))
}

/// Delete the table bucket a stack created, addressed by its ARN — this resource's physical
/// id, which is what `Ref` returns for the type. A bucket S3 Tables no longer holds
/// (`NotFoundException`, judged on the CODE and never on a message that carries the
/// request's own name) is already in the state the delete asks for; one that still holds
/// namespaces is NOT deleted (`ConflictException`), and that refusal is what the stack
/// reports.
pub(super) async fn delete_table_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        S3TABLES_SIGNING_NAME,
        region,
        "DELETE",
        &table_bucket_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == S3TABLES_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The table bucket's configuration as Amazon S3 Tables reports it, for drift detection:
/// `GetTableBucket`'s own record, read by the ARN that is this resource's physical id. One
/// it no longer holds (`NotFoundException`) was deleted.
///
/// The one property its read reports under the name this type declares is the bucket's
/// `TableBucketName` — that read answers the bucket's `name` — and a property the read does
/// not report is not evidence of anything ([`super::drift`]).
pub(super) async fn describe_table_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        S3TABLES_SIGNING_NAME,
        region,
        "GET",
        &table_bucket_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            &[("TableBucketName", "name")],
        ))),
        Err(refusal) if refusal.code == S3TABLES_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI an operation on ONE table bucket is bound to (`/buckets/{tableBucketARN}`), with
/// the ARN written as the single path segment that `@httpLabel` is: its `:` and `/` travel
/// percent-encoded ([`percent_encoded`]), or the request would address a different route.
fn table_bucket_uri(table_bucket_arn: &str) -> String {
    format!(
        "{S3TABLES_BUCKETS_URI}/{}",
        percent_encoded(table_bucket_arn)
    )
}

/// The NAME of the table bucket a resource record describes: the segment its own ARN
/// carries, which is this type's physical id and how S3 Tables keys a table bucket.
///
/// A record whose physical id is NOT such an ARN is left naming itself, so what the stack
/// reports is S3 Tables' own refusal of the name the declaration really asked for rather
/// than one invented here.
pub(super) fn table_bucket_name(resource: &StoredResource) -> String {
    arn_bucket_of(&resource.physical_resource_id, S3TABLES_BUCKET_SEGMENT)
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}
