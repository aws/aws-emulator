// SPDX-License-Identifier: CC0-1.0

//! The ACCESS-CONTROL settings an `AWS::S3::Bucket` declares — its Object Ownership and its
//! Block Public Access configuration — as the Amazon S3 requests that put them on the bucket.
//!
//! Neither is a member of `CreateBucket`'s payload, so each is installed on the created
//! bucket by the ONE call Amazon S3 publishes for it: `OwnershipControls` is a
//! `PutBucketOwnershipControls` ("creates or modifies `OwnershipControls` for an Amazon S3
//! bucket") and `PublicAccessBlockConfiguration` is a `PutPublicAccessBlock` ("creates or
//! modifies the `PublicAccessBlock` configuration for an Amazon S3 bucket").
//!
//! These two decide every later ACL and policy call on the bucket rather than merely being
//! reported: "Access control lists (ACLs) are disabled and no longer affect permissions" under
//! `BucketOwnerEnforced`, so a `PutObjectAcl` on such a bucket is refused outright, while a
//! bucket whose Object Ownership is `ObjectWriter` or `BucketOwnerPreferred` has ACLs ENABLED
//! and judges the same call against its Block Public Access settings instead ("PUT Bucket ACL
//! and PUT Object ACL calls fail if the specified ACL is public",
//! `PublicAccessBlockConfiguration$BlockPublicAcls`). A declaration whose settings never reach
//! those calls leaves the bucket on the settings a bucket is CREATED with — which are not the
//! ones the template asked for — so every authorization decision afterwards is taken against
//! the wrong configuration.
//!
//! The template's spelling is the API's: `OwnershipControls.Rules[].ObjectOwnership` is the
//! API's own container and member, and the four Block Public Access flags
//! (`BlockPublicAcls`, `BlockPublicPolicy`, `IgnorePublicAcls`, `RestrictPublicBuckets`) are
//! named identically on both sides. Each value travels as the template wrote it, so Amazon
//! S3's own deserializer and handler are what judge it.
//!
//! A declaration missing a member the API REQUIRES is REFUSED rather than sent incomplete:
//! the stack must not complete with a bucket whose access control the deployment never set.
//! Only `OwnershipControls` has such members — its `Rules` and each rule's `ObjectOwnership`
//! — while `PutPublicAccessBlock` requires the container alone, so a declaration of it that
//! states no switch is sent as the configuration it is rather than refused.

use super::*;

/// The property an `AWS::S3::Bucket` declares its Object Ownership under.
pub(super) const OWNERSHIP_CONTROLS: &str = "OwnershipControls";

/// The property an `AWS::S3::Bucket` declares its Block Public Access settings under.
pub(super) const PUBLIC_ACCESS_BLOCK_CONFIGURATION: &str = "PublicAccessBlockConfiguration";

/// The four settings a `PublicAccessBlockConfiguration` carries, spelled the same in the
/// template and in the API (`smithy_query(query="shape",
/// shape="PublicAccessBlockConfiguration")` publishes exactly these members).
const PUBLIC_ACCESS_BLOCK_FLAGS: [&str; 4] = [
    "BlockPublicAcls",
    "BlockPublicPolicy",
    "IgnorePublicAcls",
    "RestrictPublicBuckets",
];

/// Apply the ACCESS-CONTROL settings `resource` declares to the bucket named `bucket`, each
/// through the one call Amazon S3 publishes for it. A declaration that states neither makes no
/// call at all.
///
/// One function for the pair, AWAITED THROUGH A BOX by the create
/// ([`super::configure_bucket`]): a bucket's create runs inside a chain of in-process service
/// calls — a stack-declared notification configuration has Amazon S3 invoke a function whose
/// own SDK calls come back through this same router — and every step of that chain holds its
/// awaited futures in the frame that polls it, so the depth of the chain is what bounds how
/// large one step's future may be. Keeping these two calls in a future of their own, behind a
/// pointer, leaves the create's own frame the size it was.
pub(super) async fn apply_access_controls(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    // "OwnershipControls — configuration that defines how Amazon S3 handles Object Ownership
    // rules" (this type's own property): the whole container is ONE
    // `PutBucketOwnershipControls`, so a bucket a template declares `ObjectWriter` or
    // `BucketOwnerPreferred` for really has ACLs ENABLED afterwards — which is what every
    // later ACL call on it is judged by.
    if let Some(controls) = declared_ownership_controls(resource)? {
        client
            .put_bucket_ownership_controls()
            .bucket(bucket)
            .ownership_controls(controls)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    // "PublicAccessBlockConfiguration — configuration that defines how Amazon S3 handles
    // public access" (this type's own property): the whole configuration is ONE
    // `PutPublicAccessBlock`, so the guardrails a template declares are the ones in force on
    // the bucket — including for the public ACL and the public policy they refuse.
    if let Some(configuration) = declared_public_access_block(resource)? {
        client
            .put_public_access_block()
            .bucket(bucket)
            .public_access_block_configuration(configuration)
            .send()
            .await
            .map_err(|error| s3_refusal_of(&error))?;
    }
    Ok(())
}

/// The whole `OwnershipControls` a bucket's declaration states, as the
/// `PutBucketOwnershipControls` request Amazon S3 takes — `None` for a declaration that
/// carries no such property, which leaves the bucket on the Object Ownership every bucket is
/// created with.
///
/// `OwnershipControls$Rules` and `OwnershipControlsRule$ObjectOwnership` are both REQUIRED by
/// the API, so a declaration stating the container with no rule, or a rule naming no
/// ownership, is refused rather than sent incomplete.
pub(super) fn declared_ownership_controls(
    resource: &StoredResource,
) -> Result<Option<::aws_sdk_s3::types::OwnershipControls>, Refusal> {
    use ::aws_sdk_s3::types::{ObjectOwnership, OwnershipControls, OwnershipControlsRule};

    let Some(declared) = resource.properties.get(OWNERSHIP_CONTROLS) else {
        return Ok(None);
    };
    let listed = declared
        .get("Rules")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if listed.is_empty() {
        return Err(missing("Rules"));
    }
    let mut rules = Vec::with_capacity(listed.len());
    for rule in listed {
        let ownership =
            declared_text(rule, "ObjectOwnership").ok_or_else(|| missing("Rules.ObjectOwnership"))?;
        rules.push(
            OwnershipControlsRule::builder()
                .object_ownership(ObjectOwnership::from(ownership.as_str()))
                .build()
                .map_err(|error| not_sendable(OWNERSHIP_CONTROLS, &error))?,
        );
    }
    Ok(Some(
        OwnershipControls::builder()
            .set_rules(Some(rules))
            .build()
            .map_err(|error| not_sendable(OWNERSHIP_CONTROLS, &error))?,
    ))
}

/// The whole `PublicAccessBlockConfiguration` a bucket's declaration states, as the
/// `PutPublicAccessBlock` request Amazon S3 takes — `None` for a declaration that carries no
/// such property, which leaves the bucket on the Block Public Access settings every bucket is
/// created with.
///
/// Every flag is OPTIONAL on both sides and the API requires only the CONTAINER, so a
/// declaration that states some of them sends exactly those and one that states none sends the
/// empty configuration: "when Amazon S3 evaluates the `PublicAccessBlock` configuration for a
/// bucket … the most restrictive combination" is what applies, and a setting the template
/// leaves out is one the deployment does not state. Nothing here is refused for a missing
/// member, because there is no member of it Amazon S3 requires.
pub(super) fn declared_public_access_block(
    resource: &StoredResource,
) -> Result<Option<::aws_sdk_s3::types::PublicAccessBlockConfiguration>, Refusal> {
    use ::aws_sdk_s3::types::PublicAccessBlockConfiguration;

    let Some(declared) = resource.properties.get(PUBLIC_ACCESS_BLOCK_CONFIGURATION) else {
        return Ok(None);
    };
    // A declaration that is no container at all states no setting this arm can read; Amazon
    // S3's own deserializer is what judges the shape a template gave it, so the empty
    // configuration is what is sent for it rather than a refusal of this arm's own.
    let empty = Map::new();
    let stated = declared.as_object().unwrap_or(&empty);
    let flag = |name: &str| declared_flag(stated, name);
    Ok(Some(
        PublicAccessBlockConfiguration::builder()
            .set_block_public_acls(flag(PUBLIC_ACCESS_BLOCK_FLAGS[0]))
            .set_block_public_policy(flag(PUBLIC_ACCESS_BLOCK_FLAGS[1]))
            .set_ignore_public_acls(flag(PUBLIC_ACCESS_BLOCK_FLAGS[2]))
            .set_restrict_public_buckets(flag(PUBLIC_ACCESS_BLOCK_FLAGS[3]))
            .build(),
    ))
}

/// The refusal for an `OwnershipControls` declaration missing a member Amazon S3 REQUIRES.
fn missing(member: &str) -> Refusal {
    Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("the {OWNERSHIP_CONTROLS} of {S3_BUCKET} declares no {member}"),
    }
}

/// The refusal for a declared setting the SDK will not build into the request shape, worded
/// the way every other sub-resource of this type words one.
fn not_sendable(property: &str, error: &dyn ::std::fmt::Display) -> Refusal {
    Refusal {
        code: UNIDENTIFIED_FAILURE.to_string(),
        message: format!("the {property} of {S3_BUCKET} cannot be sent to Amazon S3: {error}"),
    }
}
