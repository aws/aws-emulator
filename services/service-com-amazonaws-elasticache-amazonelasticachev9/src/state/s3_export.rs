// SPDX-License-Identifier: CC0-1.0

//! The ONE seam this crate exports a serverless cache snapshot into Amazon S3
//! through.
//!
//! `ExportServerlessCacheSnapshot` "provides the functionality to export the
//! serverless cache snapshot data to Amazon S3", and what lands there is an RDB
//! file: the model's own example of an exported snapshot is the object
//! `arn:aws:s3:::my_bucket/snapshot1.rdb`, "a Valkey or Redis OSS RDB snapshot
//! file stored in Amazon S3"
//! (`CreateCacheClusterMessage$SnapshotArns`). So the export is an object WRITE,
//! and it is performed where objects live: Amazon S3 is the service that owns
//! buckets and objects, so this module asks it rather than recording a claim
//! here that the object store would not answer for.
//!
//! Amazon S3 is reached the way the rest of this crate reaches another emulated
//! service ([`super::kms_key`], [`super::ec2_client`]): the AWS SDK driven over
//! the operation callback's in-process transport
//! ([`smithy_server_runtime::OperationContext::callback`]), through the ONE
//! construction of that client this tree keeps
//! ([`::service_common::s3_client`]), so no socket is opened, no host name is
//! resolved and nothing leaves this process. The destination is this emulator's
//! OWN object store and nothing else: the only caller-supplied values that reach
//! the client are the bucket NAME and the signing Region, and the Region is
//! narrowed to a Region LABEL ([`::service_common::arn::region_label`]) before it
//! can reach the endpoint URL's authority. The credentials are the callback's,
//! which sign that in-process hop and never appear in a response.
//!
//! An export is refused rather than reported over nothing: a bucket the object
//! store will not take the object into, and a bucket it holds in another Region
//! ("The Amazon S3 bucket must also be in same region as the snapshot",
//! `ExportServerlessCacheSnapshotRequest$S3BucketName`), both end as
//! [`ExportRefusal`] and never as a success — a caller told its snapshot was
//! exported must find the object in the bucket it named.

/// The Region a bucket lives in when `GetBucketLocation` reports no constraint
/// for it: "Buckets in Region us-east-1 have a LocationConstraint of null"
/// (`GetBucketLocationOutput$LocationConstraint`), which is why that Region is
/// not one of the values `BucketLocationConstraint` carries.
const UNCONSTRAINED_BUCKET_REGION: &str = "us-east-1";

/// The magic every RDB file opens with, and the file-format version that follows
/// it: `REDIS` then four version digits.
const RDB_MAGIC: &str = "REDIS";

/// The RDB file-format version an exported snapshot carries. Version 11 is the
/// one the Valkey and Redis OSS 7 engines this service offers
/// ([`super::engine_versions`]) write their snapshot files in.
const RDB_VERSION: &str = "0011";

/// The RDB opcode that ends the key space: everything after it is the file's
/// trailing checksum.
const RDB_OPCODE_EOF: u8 = 0xFF;

/// The trailing checksum of an RDB file written with checksums off, which is
/// eight zero bytes — the encoding that tells a loader there is no checksum to
/// verify rather than offering one that was never computed.
const RDB_CHECKSUM_DISABLED: [u8; 8] = [0; 8];

/// Why ElastiCache cannot export a snapshot into the bucket a request named.
///
/// Both kinds are grounded in what the object store ANSWERED, and both lift
/// into `InvalidParameterValueException` ("the value for a parameter is
/// invalid") at the operation, which is the only bad-parameter failure the
/// export models.
#[derive(Debug, Clone)]
pub enum ExportRefusal {
    /// The object store holds the bucket in the Region named here, which is not
    /// the one the snapshot lives in.
    CrossRegion(String),
    /// The object store would not take the export into the bucket: it holds no
    /// such bucket, it refused the write, or there is no object store beside
    /// this service to write into at all.
    NotWritable,
}

/// The S3 key the export of the snapshot called `snapshot_name` is written
/// under: the snapshot's own (stored, lowercased) name, as the RDB file an
/// exported snapshot is ("Example of an Amazon S3 ARN:
/// `arn:aws:s3:::my_bucket/snapshot1.rdb`").
#[must_use]
pub fn exported_object_key(snapshot_name: &str) -> String {
    format!("{snapshot_name}.rdb")
}

/// The bytes of a serverless cache snapshot's exported data set.
///
/// An RDB file of an EMPTY key space: the magic, the file-format version, the
/// end-of-key-space opcode and the no-checksum trailer, with no key between
/// them. A snapshot does hold the source cache's data set
/// ([`super::ServerlessCacheSnapshotRecord::data`], whose size it reports as
/// `BytesUsedForCache`), but this emulator writes no RDB ENCODING of a key, so
/// the file states an empty key space rather than one whose framing claims keys
/// the bytes do not carry.
#[must_use]
pub fn exported_data_set() -> Vec<u8> {
    let mut file = Vec::with_capacity(
        RDB_MAGIC.len() + RDB_VERSION.len() + 1 + RDB_CHECKSUM_DISABLED.len(),
    );
    file.extend_from_slice(RDB_MAGIC.as_bytes());
    file.extend_from_slice(RDB_VERSION.as_bytes());
    file.push(RDB_OPCODE_EOF);
    file.extend_from_slice(&RDB_CHECKSUM_DISABLED);
    file
}

/// Export the snapshot called `snapshot_name` into `bucket`, by writing its data
/// set there as the object [`exported_object_key`] names.
///
/// `region` is the request's own signing Region, which is where the snapshot
/// lives and therefore the Region the bucket has to be in. Answers `Ok` only
/// when the object store took the object, so the operation reports an export
/// exactly when there is one to find.
pub async fn export(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    snapshot_name: &str,
) -> Result<(), ExportRefusal> {
    // A Region that is not a Region LABEL would re-point the authority of the
    // endpoint URL below, so nothing is asked and nothing is written for it.
    let Some(region) = ::service_common::arn::region_label(region) else {
        return Err(ExportRefusal::NotWritable);
    };
    let client = s3_client(ctx, region);

    // WHERE the bucket lives is a question the object store ANSWERS, and its
    // answer also settles whether there is a bucket at all: a bucket it does not
    // hold takes no export.
    match client.get_bucket_location().bucket(bucket).send().await {
        Ok(answer) => {
            let bucket_region = answer
                .location_constraint()
                .map(|constraint| constraint.as_str().to_string())
                .filter(|constraint| !constraint.is_empty())
                .unwrap_or_else(|| UNCONSTRAINED_BUCKET_REGION.to_string());
            if bucket_region != region {
                return Err(ExportRefusal::CrossRegion(bucket_region));
            }
        }
        Err(error) => {
            // The caller's own value is rendered in its escaped `Debug` form, so
            // a bucket name carrying a newline cannot forge a log line.
            ::tracing::debug!(
                bucket = ?bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Amazon S3 did not report a bucket for this export's destination"
            );
            return Err(ExportRefusal::NotWritable);
        }
    }

    client
        .put_object()
        .bucket(bucket)
        .key(exported_object_key(snapshot_name))
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            exported_data_set(),
        ))
        .send()
        .await
        .map_err(|error| {
            ::tracing::debug!(
                bucket = ?bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Amazon S3 did not take the exported serverless cache snapshot"
            );
            ExportRefusal::NotWritable
        })?;
    Ok(())
}

/// What `InvalidParameterValueException` carries for a bucket ElastiCache cannot
/// write the export into: ElastiCache states the permission the destination
/// bucket has to grant it, naming the bucket the request named.
#[must_use]
pub fn bucket_not_writable_message(bucket: &str) -> String {
    format!("ElastiCache has not been granted WRITE permissions {bucket} on the S3 Bucket.")
}

/// What it carries for a bucket that lives outside the snapshot's Region: the
/// bucket, the Region the object store holds it in, the Region the snapshot is
/// in, and then the rule itself — "The Amazon S3 bucket must also be in same
/// region as the snapshot" (`ExportServerlessCacheSnapshotRequest$S3BucketName`).
#[must_use]
pub fn cross_region_bucket_message(
    bucket: &str,
    bucket_region: &str,
    snapshot_region: &str,
) -> String {
    format!(
        "The S3 bucket {bucket} is in region {bucket_region}, which is not the region of the \
         snapshot ({snapshot_region}). The S3 bucket must be in the same region as the snapshot."
    )
}

/// An Amazon S3 client over the in-process callback into this same emulator,
/// built by the ONE construction of it this tree keeps
/// ([`::service_common::s3_client`]) rather than a copy configured here.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    ::service_common::s3_client::s3_client(ctx.callback(), region)
}
