// SPDX-License-Identifier: CC0-1.0

//! Whether the AWS KMS key an Amazon S3 object is SEALED under lets a job's execution role
//! open it — the second half of the ONE question every read a job's work makes has to settle.
//!
//! # The rule this module implements
//!
//! An object stored with server-side encryption under a KMS key is readable only THROUGH that
//! key: its plaintext comes of opening the object's data key, so a read that serves such an
//! object's bytes performs `kms:Decrypt` on the key it was stored under — "If the object is
//! encrypted with SSE-KMS, you must also have the kms:GenerateDataKey and kms:Decrypt
//! permissions in IAM identity-based policies and KMS key policies for the KMS key"
//! (`HeadObject`'s own account of the permissions such a read needs).
//!
//! Every Amazon S3 call a job's work makes is made UNDER the job's `RoleArn` ("an IAM role that
//! Amazon SageMaker can assume to perform tasks on your behalf"), so that `kms:Decrypt` is
//! performed as that role: a role the authorities in force on the key withhold the action from
//! cannot bring the object down, and the leg of the job's work that read it FAILS the job — the
//! terminal `Failed` status with the `ClientError:` reason naming that leg — rather than
//! completing over data it never saw. It is the same reading [`super::s3_access`] takes of the
//! object itself, over the action a read performs on the KEY instead of on the object, which is
//! why the two are asked together ([`object_read_refusal`]).
//!
//! # The decision is KMS's OWN
//!
//! Nothing about the key is decided here. The key record, its policy and the grants issued on
//! it are KMS's, written by KMS's own handlers on this same shared engine, so the verdict comes
//! from the service that holds them ([`kms::authorization::authorize_use`], taken for the
//! execution role as the principal, which is how every service in this tree that uses a key AS
//! a role decides it) and the sentence a refused read carries is the one KMS words for it. The
//! use ARRIVES through Amazon S3 serving the object, which is what a key policy scoped by
//! `kms:ViaService` is written against, and carries the encryption context that object's data
//! key is bound to ([`SealedUnder::encryption_context`]).
//!
//! Only what was ANSWERED refuses anything: an object whose encryption Amazon S3 did not report,
//! a reference KMS resolves no key of this Region for, an Amazon Web Services managed key (whose
//! policy AWS manages for the service that owns it) and an emulator with no KMS mounted beside
//! this service all leave the read alone. What the KEY's state admits is Amazon S3's own answer
//! to the read rather than a question about the role, so it is not asked here either.
//!
//! LOCKING: this takes KMS's store lock and IAM's, never both at once and neither across an
//! await — the role's documents are read before the key's store is reached, and no store of
//! THIS service is held while either is.

use super::*;

use ::service_com_amazonaws_kms_trentservice::state as kms;

/// The KMS operation a read of an object sealed under a key performs on that key, as KMS names
/// one (its API name, without the `kms:` namespace a policy spells it with).
pub const DECRYPT_OPERATION: &str = "Decrypt";

/// The `ServerSideEncryption` values an object sealed under a KMS key carries — the two values
/// of Amazon S3's own enum that name a KMS key
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` of that service's model lists
/// `AES256`, `aws:fsx`, `aws:kms` and `aws:kms:dsse`; the first two name no KMS key).
const KMS_SSE_ALGORITHMS: &[&str] = &["aws:kms", "aws:kms:dsse"];

/// The entry Amazon S3 always binds an object's data key to, which is what a key policy, a
/// condition or a grant scoped to one bucket or one object is decided against: "Amazon S3 uses
/// the object or bucket Amazon Resource Name (ARN) as the encryption context pair"
/// (`PutObject`'s own account of the encryption context of an SSE-KMS write).
const ARN_CONTEXT_KEY: &str = "aws:s3:arn";

/// The endpoint vendor name the `kms:ViaService` value of a read Amazon S3 performs is built
/// from.
const S3_ENDPOINT_VENDOR: &str = "s3";

/// Why one object a job's work READS cannot be read as the execution role — the detail that
/// job's `FailureReason` carries for the leg that read it — and `None` when nothing in force
/// refuses the read.
///
/// The ONE question every read a job's work makes asks per object, in both halves an SSE-KMS
/// read has: the object itself, authorized against the bucket's own policy and the grant in
/// force on the role ([`object_read_is_unauthorized`]), and the KEY it is sealed under, whose
/// decrypt is decided by KMS ([`key_withholds_decrypt`]). Either answer is Amazon S3 refusing
/// the read, which is why both are reported in the envelope Amazon S3 answers a refused read in
/// ([`s3_refusal_detail`]) — the second one relaying the sentence KMS worded, as Amazon S3
/// relays it.
pub async fn object_read_refusal<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    location: &S3Location,
) -> Option<String> {
    if object_read_is_unauthorized(ctx, client, region, role_arn, partition, location).await {
        return Some(access_denied_detail());
    }
    // Asked of Amazon S3 BEFORE any store is reached, because it is the awaited half.
    let sealed = object_kms_seal(client, location).await?;
    let sentence = key_withholds_decrypt(engine, region, role_arn, partition, location, &sealed)?;
    Some(s3_refusal_detail(&sentence))
}

/// The sentence KMS words for the `kms:Decrypt` it REFUSES the execution role on the key the
/// object is sealed under, and `None` when nothing KMS answered refuses it (see the module
/// docs).
///
/// Synchronous, and holds no guard of this service's own: it reads IAM's records and then KMS's,
/// each the owning service's own store on this same engine.
#[must_use]
fn key_withholds_decrypt<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    region: &str,
    role_arn: &str,
    partition: &str,
    location: &S3Location,
    sealed: &SealedUnder,
) -> Option<String> {
    let state = kms::service_state(engine)?;
    // The role's own identity-based documents are read out of IAM's store HERE, before KMS's
    // store is locked below: no two of those locks are held at once.
    let caller = kms::authorization::role_caller(engine, role_arn);
    let store = state.lock();
    // A reference this Region's KMS resolves no key for says nothing about the role: what such
    // an object's read answers is Amazon S3's own, and it is not about permission.
    let kms::KeyRef::Found(key) = store.resolve(region, &sealed.key) else {
        return None;
    };
    // An Amazon Web Services managed key's policy is managed by AWS for the service that owns
    // the alias, so no identity needs an authority of its own on one.
    if key.key_manager == kms::KEY_MANAGER_AWS {
        return None;
    }
    kms::authorization::authorize_use(
        &caller,
        &store,
        region,
        key,
        // The decrypt arrives through Amazon S3, which performs it to serve the object's
        // bytes — the value a key policy scoped by `kms:ViaService` is written against.
        kms::authorization::RequestedUse::through_service(DECRYPT_OPERATION, &via_service(region)),
        Some(&sealed.encryption_context(partition, location)),
        // The read presents no grant token: the authority a grant gives this role is the
        // role's already (`authorize_use` weighs the grants it holds).
        None,
    )
    .err()
}

/// How an object Amazon S3 reported is SEALED: the KMS key reference the write recorded, and
/// whether that write used a bucket-level key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SealedUnder {
    /// The key the object's data key was made with, as Amazon S3 reports it ("the ID of the KMS
    /// key used to encrypt the object", `HeadObjectOutput$SSEKMSKeyId`).
    key: String,
    /// Whether the object "uses an S3 Bucket Key for server-side encryption with Amazon Web
    /// Services KMS" (`HeadObjectOutput$BucketKeyEnabled`), which decides what its data key is
    /// bound to ([`Self::encryption_context`]).
    bucket_key: bool,
}

impl SealedUnder {
    /// The encryption context this object's data key is bound to: the one entry Amazon S3 always
    /// binds ([`ARN_CONTEXT_KEY`]), naming the resource that was encrypted — the BUCKET for an
    /// object written with a bucket-level key, since such a key is bound to the bucket rather
    /// than to each object under it ("setting this header to true causes Amazon S3 to use an S3
    /// Bucket Key", `PutObjectRequest$BucketKeyEnabled`), and the object itself otherwise.
    ///
    /// Additional pairs a write supplied in `x-amz-server-side-encryption-context` are part of
    /// the context too, and no answer ABOUT the object reports them to a reader, so a context a
    /// write added is not carried here: an authority conditioned on one is decided on the entry
    /// Amazon S3 binds rather than on a value this read would be inventing.
    fn encryption_context(
        &self,
        partition: &str,
        location: &S3Location,
    ) -> HashMap<String, String> {
        let encrypted = if self.bucket_key {
            location.bucket_arn(partition)
        } else {
            location.object_arn(partition)
        };
        HashMap::from([(ARN_CONTEXT_KEY.to_string(), encrypted)])
    }
}

/// How the object `location` names is SEALED, as Amazon S3 reports the object's own encryption —
/// and `None` for an object that is not stored under a KMS key, and for one Amazon S3 reported
/// nothing about.
///
/// The encryption is read from the object itself ("the server-side encryption algorithm used
/// when you store this object", `HeadObjectOutput$ServerSideEncryption`), so the key a read is
/// authorized against is the key the write that stored it recorded rather than a key named by
/// any later request.
async fn object_kms_seal(
    client: &::aws_sdk_s3::Client,
    location: &S3Location,
) -> Option<SealedUnder> {
    let head = client
        .head_object()
        .bucket(&location.bucket)
        .key(&location.key)
        .send()
        .await
        .ok()?;
    if !KMS_SSE_ALGORITHMS.contains(&head.server_side_encryption()?.as_str()) {
        return None;
    }
    let key = head.ssekms_key_id().filter(|key| !key.is_empty())?;
    Some(SealedUnder {
        key: key.to_string(),
        bucket_key: head.bucket_key_enabled().unwrap_or(false),
    })
}

/// The value of the `kms:ViaService` condition key for the decrypt Amazon S3 performs to serve
/// an object's bytes: that service's own regional endpoint name.
fn via_service(region: &str) -> String {
    format!(
        "{S3_ENDPOINT_VENDOR}.{}.amazonaws.com",
        ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The location every row below is about.
    fn location() -> S3Location {
        S3Location {
            bucket: "secure".to_string(),
            key: "input/data.csv".to_string(),
        }
    }

    /// RULE: the encryption context an SSE-KMS read is decided under names the resource that was
    /// encrypted — the object itself, and the BUCKET for an object written with a bucket-level
    /// key — in the partition the execution role's own ARN names.
    ///
    /// The rows differ along whether the write used a bucket key (reported, and reported absent)
    /// and along the partition.
    #[test]
    fn the_context_names_the_resource_that_was_encrypted() {
        // (case, bucket key used, the partition, the ARN the context names)
        let rows: [(&str, bool, &str, &str); 3] = [
            (
                "an object's own data key",
                false,
                "aws",
                "arn:aws:s3:::secure/input/data.csv",
            ),
            ("a bucket-level key", true, "aws", "arn:aws:s3:::secure"),
            (
                "an object's own data key in another partition",
                false,
                "aws-cn",
                "arn:aws-cn:s3:::secure/input/data.csv",
            ),
        ];
        for (case, bucket_key, partition, expected) in rows {
            let sealed = SealedUnder {
                key: "arn:aws:kms:us-east-1:000000000000:key/abc".to_string(),
                bucket_key,
            };
            let context = sealed.encryption_context(partition, &location());
            let named = context.get(ARN_CONTEXT_KEY).cloned();
            assert_eq!(
                named.as_deref(),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
            assert_eq!(
                context.len(),
                1,
                "case {case:?}: the context carries the one entry Amazon S3 binds, got {context:?}"
            );
        }
    }

    /// RULE: the service a key policy sees this decrypt arriving through is Amazon S3's own
    /// regional endpoint, for the Region the read was made in — and this emulator's own Region
    /// for a value that names no Region label.
    #[test]
    fn the_decrypt_arrives_through_the_object_stores_endpoint() {
        for (case, region, expected) in [
            ("the signed Region", "eu-west-1", "s3.eu-west-1.amazonaws.com"),
            (
                "a value that is no Region label",
                "not a region",
                "s3.us-east-1.amazonaws.com",
            ),
        ] {
            let read = via_service(region);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
