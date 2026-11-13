// SPDX-License-Identifier: CC0-1.0

//! How a request names a function's deployment package, and what package types admit.

use super::*;

/// Where the code of a function came from.
///
/// `zip_bytes` holds the deployment package's own bytes whenever this emulator has them:
/// the ones a caller uploaded inline, and the ones Amazon S3 handed over for a package
/// named by an S3 location ([`super::s3_source`]). They are what `code_size` and
/// `code_sha256` are computed from; a container image is a reference this emulator does not
/// pull, and an S3 object nothing could be shown about holds no bytes either, so nothing is
/// reported for one that was not computed.
#[derive(Debug, Clone, Default)]
pub struct CodeRecord {
    /// The deployment package bytes, when the caller uploaded them inline or Amazon S3
    /// handed them over for the location the package names.
    pub zip_bytes: Option<Vec<u8>>,
    /// The S3 bucket holding the deployment package, when the caller named one.
    pub s3_bucket: Option<String>,
    /// The S3 key of the deployment package.
    pub s3_key: Option<String>,
    /// The S3 object version of the deployment package.
    pub s3_object_version: Option<String>,
    /// How the deployment package is stored, when the caller said: `COPY` (the default,
    /// "Uploads a copy of your deployment package to Lambda") or `REFERENCE` ("Lambda
    /// references the deployment package from the specified Amazon S3 bucket"). See
    /// [`S3_OBJECT_STORAGE_MODE_COPY`] and [`S3_OBJECT_STORAGE_MODE_REFERENCE`].
    pub s3_object_storage_mode: Option<String>,
    /// The ARN of the customer managed KMS key the .zip deployment package is encrypted
    /// with, when the caller named one; `FunctionCodeLocation$SourceKMSKeyArn` reports
    /// it back.
    pub source_kms_key_arn: Option<String>,
    /// The container image URI, for an `Image` package.
    pub image_uri: Option<String>,
    /// The manifest digest the Amazon ECR registry holds for [`Self::image_uri`], as the
    /// call that stored this package resolved it (see [`super::image_source`]). It is what
    /// `FunctionCodeLocation$ResolvedImageUri` pins the repository to, so the function
    /// reports the digest the registry minted for the image it runs. `None` when no
    /// registry could be asked, which is evidence of nothing.
    pub resolved_image_digest: Option<String>,
}

/// The `CodeSha256` a stored package reports: "The SHA256 hash of the function's
/// deployment package."
///
/// For a .zip package it is the digest of the bytes this emulator holds, base64-encoded as
/// the member documents. For a CONTAINER IMAGE the deployment package is the image, and
/// what identifies it is the manifest digest the registry minted for it (see
/// [`super::image_source`]), so the hash reported is that digest's own hexadecimal — the
/// value a client compares against `DescribeImages`. A package whose content this emulator
/// holds nothing for reports no hash at all rather than one of something else.
#[must_use]
pub fn reported_code_sha256(code: &CodeRecord) -> Option<String> {
    if let Some(bytes) = &code.zip_bytes {
        return Some(code_sha256(bytes));
    }
    let image_uri = code.image_uri.as_deref()?;
    match code
        .resolved_image_digest
        .as_deref()
        .and_then(|digest| digest.split_once(':'))
    {
        Some((_, hexadecimal)) => Some(hexadecimal.to_string()),
        // No registry could be asked for this package, so the content it is frozen to is
        // the reference itself.
        None => Some(image_code_sha256(image_uri)),
    }
}

/// The refusal AWS words for a container-image function a request asks to mount layers
/// on: a fixed sentence with no part taken from the request.
///
/// "A list of function layers to add to the function's EXECUTION ENVIRONMENT"
/// (`CreateFunction$Layers`) — a layer is unpacked into the environment Lambda builds
/// around a .zip package, and a container image brings its own whole filesystem, so there
/// is nowhere in it for one to be mounted.
pub const LAYERS_UNSUPPORTED_FOR_IMAGE_MESSAGE: &str =
    "Lambda layers are not supported for functions created with container images.";

impl CodeRecord {
    /// Whether this package is a container image.
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.image_uri.is_some()
    }
}

/// The members of a request that name a deployment package.
///
/// `CreateFunction` carries them on its `Code` member and `UpdateFunctionCode` carries
/// them on the request itself, but they name the same package in the same forms, so both
/// resolve them through [`resolve_package_code`] rather than each deciding for itself
/// what a package may be.
#[derive(Debug, Default, Clone, Copy)]
pub struct PackageMembers<'a> {
    /// `ZipFile` — the deployment package bytes, uploaded inline.
    pub zip_file: Option<&'a [u8]>,
    /// `S3Bucket` — the bucket holding the deployment package.
    pub s3_bucket: Option<&'a str>,
    /// `S3Key` — the key of the deployment package.
    pub s3_key: Option<&'a str>,
    /// `S3ObjectVersion` — "For versioned objects, the version of the deployment package
    /// object to use."
    pub s3_object_version: Option<&'a str>,
    /// `S3ObjectStorageMode` — `COPY` or `REFERENCE`.
    pub s3_object_storage_mode: Option<&'a str>,
    /// `ImageUri` — the container image in the Amazon ECR registry.
    pub image_uri: Option<&'a str>,
    /// `SourceKMSKeyArn` — the customer managed key the .zip package is encrypted with.
    pub source_kms_key_arn: Option<&'a str>,
}

impl PackageMembers<'_> {
    /// Whether any member that only an S3-sourced package has was named.
    fn names_s3_object(&self) -> bool {
        self.s3_bucket.is_some()
            || self.s3_key.is_some()
            || self.s3_object_version.is_some()
            || self.s3_object_storage_mode.is_some()
    }
}

/// How the call that names a package describes itself in a refusal: the verb Lambda puts
/// in `Please don't provide <Member> when <verb> a function with packageType <Type>.`
///
/// A package member the function's package type does not use is refused the same way by
/// whichever write named it, so the only part that differs between the two is this word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageCall {
    /// `CreateFunction`, which names the package of a function it is creating.
    Creating,
    /// `UpdateFunctionCode`, which names a new package for a function that exists.
    Updating,
}

impl PackageCall {
    /// The verb this call reads as in a package refusal.
    fn verb(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Updating => "updating",
        }
    }
}

/// The refusal for a package member the function's package type does not use.
///
/// Fixed text around two names the request and the record already carry: the member that
/// was provided and the package type it does not belong to.
fn unexpected_package_member(call: PackageCall, member: &str, package_type: &str) -> String {
    format!(
        "Please don't provide {member} when {verb} a function with packageType {package_type}.",
        verb = call.verb()
    )
}

/// The refusal for the package member the function's package type requires and the
/// request left out.
fn missing_package_member(call: PackageCall, member: &str, package_type: &str) -> String {
    format!(
        "Please provide {member} when {verb} a function with packageType {package_type}.",
        verb = call.verb()
    )
}

/// The [`CodeRecord`] `package_type` admits for `members`, or the message of the refusal.
///
/// The model states the rule on both operations: "If the function's package type is
/// Image, then you must specify the code package in ImageUri as the URI of a container
/// image in the Amazon ECR registry. If the function's package type is Zip, then you must
/// specify the deployment package as a .zip file archive. Enter the Amazon S3 bucket and
/// key of the code .zip file location. You can also provide the function code inline
/// using the ZipFile field."
///
/// A member the package type does not use, and the one it requires and the request left
/// out, are each refused by naming that member, the call's verb and the package type —
/// see [`unexpected_package_member`] and [`missing_package_member`].
pub fn resolve_package_code(
    package_type: &str,
    call: PackageCall,
    members: PackageMembers<'_>,
) -> Result<CodeRecord, String> {
    if package_type == PACKAGE_TYPE_IMAGE {
        // The member an image package REQUIRES is the one named first: a request that
        // brought a .zip package to an image function is short of its `ImageUri` before it
        // is carrying anything extra.
        let Some(image_uri) = members.image_uri else {
            return Err(missing_package_member(call, "ImageUri", package_type));
        };
        if members.zip_file.is_some() {
            return Err(unexpected_package_member(call, "ZipFile", package_type));
        }
        if members.names_s3_object() {
            return Err(unexpected_package_member(call, "S3Bucket", package_type));
        }
        return Ok(CodeRecord {
            image_uri: Some(image_uri.to_string()),
            ..CodeRecord::default()
        });
    }

    if members.image_uri.is_some() {
        return Err(unexpected_package_member(call, "ImageUri", package_type));
    }
    match (members.zip_file, members.s3_bucket, members.s3_key) {
        (Some(zip_file), None, None) if !members.names_s3_object() => {
            // `AccountLimit$CodeSizeZipped` is "The maximum size of a deployment package
            // when it's uploaded directly to Lambda", which is the ceiling on these
            // bytes — and, since they are held in memory here, on the store too.
            if i64::try_from(zip_file.len()).unwrap_or(i64::MAX) > CODE_SIZE_ZIPPED_LIMIT {
                return Err(format!(
                    "ZipFile must be smaller than {CODE_SIZE_ZIPPED_LIMIT} bytes for a direct \
                     upload; use S3Bucket and S3Key for a larger deployment package"
                ));
            }
            // `AccountLimit$CodeSizeUnzipped` is the other ceiling the same package is
            // held to — "The maximum size of your function's code and layers when
            // they're extracted" ([`CODE_SIZE_UNZIPPED_LIMIT`]) — and it is measured on
            // what the archive EXPANDS to, which the stored length says nothing about: a
            // small archive whose entries declare more than the ceiling is a function
            // that could never run, so it is refused here and nothing is stored (see
            // [`super::archive`]).
            if is_over_the_unzipped_limit(Some(zip_file)) {
                return Err(unzipped_limit_message());
            }
            Ok(CodeRecord {
                zip_bytes: Some(zip_file.to_vec()),
                source_kms_key_arn: members.source_kms_key_arn.map(str::to_string),
                ..CodeRecord::default()
            })
        }
        (None, Some(bucket), Some(key)) => Ok(CodeRecord {
            s3_bucket: Some(bucket.to_string()),
            s3_key: Some(key.to_string()),
            s3_object_version: members.s3_object_version.map(str::to_string),
            s3_object_storage_mode: members.s3_object_storage_mode.map(str::to_string),
            source_kms_key_arn: members.source_kms_key_arn.map(str::to_string),
            ..CodeRecord::default()
        }),
        // Either neither form was named, or both were: the inline bytes and the S3
        // object each name a whole package, so a request carrying members of both names
        // no single one.
        _ => Err(
            "The code must name either ZipFile or both S3Bucket and S3Key for a function \
             whose PackageType is Zip"
                .to_string(),
        ),
    }
}
