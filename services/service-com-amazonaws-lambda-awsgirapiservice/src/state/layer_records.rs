// SPDX-License-Identifier: CC0-1.0

//! The LAYER slice of this service's state: what one layer version's archive came from, the
//! version record a read of it reports, and the layer those versions hang off.
//!
//! Split out of the state root unchanged, so a layer's records live together: the parent
//! re-exports every name here, and nothing about what a read reports changed with the move.

use super::*;

/// The `S3ObjectStorageMode` a layer archive is stored with when the caller names
/// none, from the member's own documentation: "`COPY` (default) – Uploads a copy of
/// your layer archive to Lambda."
pub const S3_OBJECT_STORAGE_MODE_COPY: &str = "COPY";

/// The `S3ObjectStorageMode` that leaves the archive where the caller put it:
/// "`REFERENCE` – Lambda references the layer archive from the specified Amazon S3
/// bucket."
pub const S3_OBJECT_STORAGE_MODE_REFERENCE: &str = "REFERENCE";

/// The most layers this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`]: the store is in
/// memory and every `PublishLayerVersion` adds to it, so past this point the slice
/// answers the modelled `CodeStorageExceededException` rather than growing without
/// bound. AWS bounds layers by the account's code-storage quota instead, and answers the
/// same modelled error.
pub const MAX_LAYERS: usize = 1_000;

/// The most versions this emulator keeps for one layer, bounding the store exactly as
/// [`MAX_FUNCTION_VERSIONS`] bounds a function's versions.
pub const MAX_LAYER_VERSIONS: usize = 500;

/// Where the archive of one layer version came from.
///
/// `zip_bytes` is the only variant whose bytes this emulator holds, so it is the only
/// one a size and a digest can be computed from; an S3 object is a reference this
/// emulator does not resolve, and nothing is reported for it that was not computed.
#[derive(Debug, Clone, Default)]
pub struct LayerArchiveRecord {
    /// The layer archive bytes, when the caller uploaded them inline.
    pub zip_bytes: Option<Vec<u8>>,
    /// The S3 bucket holding the layer archive, when the caller named one.
    pub s3_bucket: Option<String>,
    /// The S3 key of the layer archive.
    pub s3_key: Option<String>,
    /// The S3 object version of the layer archive.
    pub s3_object_version: Option<String>,
    /// How the archive is stored: [`S3_OBJECT_STORAGE_MODE_COPY`] or
    /// [`S3_OBJECT_STORAGE_MODE_REFERENCE`].
    pub s3_object_storage_mode: String,
}

/// One version of a layer: the archive it was published from, the metadata the request
/// carried and when it was published.
///
/// "Each time you call PublishLayerVersion with the same layer name, a new version is
/// created" (the model's own `PublishLayerVersion` documentation), so a version is
/// immutable once stored and keeps its own copy of everything a read reports.
#[derive(Debug, Clone)]
pub struct LayerVersionRecord {
    /// The version number, counting from 1 upwards.
    pub version: i64,
    /// The description of the version, when the caller gave one.
    pub description: Option<String>,
    /// The layer's software license, when the caller gave one.
    pub license_info: Option<String>,
    /// The runtimes the version declares itself compatible with, in the order given.
    pub compatible_runtimes: Vec<String>,
    /// The architectures the version declares itself compatible with, in the order
    /// given.
    pub compatible_architectures: Vec<String>,
    /// Where the archive came from.
    pub archive: LayerArchiveRecord,
    /// The size of the stored archive, in bytes; 0 when this emulator holds no bytes
    /// for it (an S3 object it does not resolve).
    pub code_size: i64,
    /// The base64 SHA-256 of the stored archive bytes; absent when this emulator holds
    /// no bytes to hash.
    pub code_sha256: Option<String>,
    /// ISO-8601 instant the version was published.
    pub created: String,
    /// The resource-based policy of this version, once a statement has been added to
    /// it.
    ///
    /// `AddLayerVersionPermission` "Adds permissions to the resource-based policy of a
    /// version of an Lambda layer", so the policy belongs to the single version the call
    /// addressed rather than to the layer as a whole, and `GetLayerVersionPolicy` reports
    /// exactly the statements that were added.
    ///
    /// The policy is a RECORD the first grant creates, and it outlives its last
    /// statement: removing every statement leaves the version holding an EMPTY policy
    /// rather than none at all. The two states are distinguishable — a version nobody has
    /// granted anything on has no policy record, and a READ of each answers its own
    /// refusal (see [`LayerVersionRecord::policy_read_absence_message`]) — so a caller can
    /// tell a version that never had a policy from one whose statements have all been
    /// taken out. Neither state is reported as a policy: both are the read's 404.
    pub policy: Option<PolicyRecord>,
}

impl LayerVersionRecord {
    /// Why a READ of this version's resource-based policy has nothing to report, as the
    /// message of its refusal — and `None` when the version holds one.
    ///
    /// The version itself EXISTS in both cases (it was resolved out of the store), so
    /// neither message is the one for an absent layer version, which is what lets a
    /// caller tell "no policy yet" (add one) from "wrong ARN" (fix the ARN). The two no-
    /// policy states are distinguished by whether the version has a policy RECORD at all:
    /// a version nobody has granted anything on has none, and one whose statements have
    /// all been removed has an empty one, which the read names the version for.
    #[must_use]
    pub fn policy_read_absence_message(&self, layer_version_arn: &str) -> Option<String> {
        match &self.policy {
            None => Some(NO_POLICY_MESSAGE.to_string()),
            Some(policy) if policy.statements.is_empty() => Some(
                layer_version_without_policy_message(layer_version_arn),
            ),
            Some(_) => None,
        }
    }

    /// The size of this version's archive once EXPANDED, in bytes: the quantity Lambda
    /// measures a function's layers against its unzipped-size limit, measured off the
    /// stored archive by [`unzipped_bytes`] rather than held as a second copy of a number
    /// the archive itself carries. 0 for a version whose bytes this emulator does not hold,
    /// exactly as [`LayerVersionRecord::code_size`] is.
    #[must_use]
    pub fn unzipped_code_size(&self) -> i64 {
        unzipped_bytes(self.archive.zip_bytes.as_deref())
    }

    /// The `LayerVersionContentOutput` a read reports for this version's archive,
    /// `location` being the link into the Lambda-held copy of it.
    #[must_use]
    pub fn content(&self, location: String) -> LayerVersionContentOutput {
        LayerVersionContentOutput {
            location: Some(location),
            code_sha256: self.code_sha256.clone(),
            code_size: self.code_size,
            // `UncompressedCodeSize` is the size of the archive's contents as Lambda has
            // EXPANDED them, and a layer archive is stored exactly as it was uploaded —
            // neither storage mode unpacks it — so no expanded copy of it exists to
            // measure and the member carries its own default. RECORDED: AWS carries the
            // member on every layer version it reports, with the value 0.
            uncompressed_code_size: 0,
            // "The resolved Amazon S3 object that contains the layer archive" — in
            // `REFERENCE` mode that object is the caller's, because "Lambda references
            // the layer archive from the specified Amazon S3 bucket". In the default
            // `COPY` mode Lambda holds a copy of its own and the caller's object is no
            // longer where the archive is served from, so none is reported.
            resolved_s3_object: self
                .archive
                .s3_bucket
                .clone()
                .filter(|_| self.archive.s3_object_storage_mode == S3_OBJECT_STORAGE_MODE_REFERENCE)
                .map(|bucket| ResolvedS3Object {
                    s3_bucket: Some(bucket),
                    s3_key: self.archive.s3_key.clone(),
                    s3_object_version: self.archive.s3_object_version.clone(),
                }),
            ..LayerVersionContentOutput::default()
        }
    }

    /// Whether this version declares itself compatible with `runtime` and with
    /// `architecture`, which is the filter `ListLayers` and `ListLayerVersions` share:
    /// "Specify a runtime identifier to list only layers that indicate that they're
    /// compatible with that runtime. Specify a compatible architecture to include only
    /// layers that are compatible with that instruction set architecture."
    ///
    /// A filter the caller did not name matches every version. A version that declares
    /// no compatible runtime (or architecture) at all indicates compatibility with
    /// none, so a request filtered to one does not match it.
    #[must_use]
    pub fn is_compatible_with(&self, runtime: Option<&str>, architecture: Option<&str>) -> bool {
        runtime.is_none_or(|runtime| {
            self.compatible_runtimes
                .iter()
                .any(|declared| declared == runtime)
        }) && architecture.is_none_or(|architecture| {
            self.compatible_architectures
                .iter()
                .any(|declared| declared == architecture)
        })
    }
}

/// One layer and every version published under its name.
#[derive(Debug, Clone)]
pub struct LayerRecord {
    /// The layer's own ARN, which carries no version.
    pub arn: String,
    /// The region the layer was published in, which its ARNs carry.
    pub region: String,
    /// The bare layer name (never an ARN).
    pub name: String,
    /// The highest version number ever published under this name; a number is never
    /// reused, so the next version follows this one rather than the count held.
    pub latest_version: i64,
    /// Every version this emulator holds, keyed by version number so a read of a
    /// layer's versions is ordered.
    pub versions: BTreeMap<i64, LayerVersionRecord>,
}

impl LayerRecord {
    /// The ARN of `version` of this layer: the layer ARN with the version appended, as
    /// the model's `LayerVersionArn` pattern requires.
    #[must_use]
    pub fn version_arn(&self, version: i64) -> String {
        format!("{}:{version}", self.arn)
    }

    /// The `GetLayerVersionResponse` the wire reports for one of this layer's versions,
    /// built entirely out of what is stored.
    ///
    /// `ctx` is what the archive's `Content$Location` is announced against: the link is one
    /// the caller follows, so it is presigned for the base URL this emulator is reached on
    /// (see [`layer_archive_location`]).
    #[must_use]
    pub fn version_detail(
        &self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        version: &LayerVersionRecord,
    ) -> GetLayerVersionOutput {
        GetLayerVersionOutput {
            content: Some(version.content(layer_archive_location(
                ctx,
                &self.region,
                &self.name,
                version.version,
            ))),
            layer_arn: Some(self.arn.clone()),
            layer_version_arn: Some(self.version_arn(version.version)),
            // `Description` is a member of every layer version AWS reports from the
            // version's OWN read: a version published without one carries it as the empty
            // string rather than not at all, exactly as a function configuration does
            // (see [`FunctionRecord::configuration`]), so a client can read the member
            // unconditionally. A LISTING entry is the other way round — see
            // [`LayerRecord::version_summary`].
            description: Some(version.description.clone().unwrap_or_default()),
            created_date: Some(version.created.clone()),
            version: version.version,
            // A list the caller named nothing in is absent from the answer rather than
            // reported empty: the model gives neither member a default, so an unnamed
            // one is a member this version carries no value for.
            compatible_architectures: (!version.compatible_architectures.is_empty())
                .then(|| version.compatible_architectures.clone()),
            compatible_runtimes: (!version.compatible_runtimes.is_empty())
                .then(|| version.compatible_runtimes.clone()),
            license_info: version.license_info.clone(),
        }
    }

    /// The newest version of this layer the store still holds, or `None` once every
    /// version published under the name has been deleted.
    ///
    /// This is NOT [`LayerRecord::latest_version`], which is the highest number ever
    /// published so a number is never handed out twice: `DeleteLayerVersion` takes a
    /// version out of [`LayerRecord::versions`] and leaves that counter where it is, so
    /// the newest version a read can report is the highest one still stored.
    #[must_use]
    pub fn latest_held_version(&self) -> Option<&LayerVersionRecord> {
        self.versions.values().next_back()
    }

    /// The `LayerVersionsListItem` a listing reports for one of this layer's versions,
    /// built entirely out of what is stored.
    #[must_use]
    pub fn version_summary(&self, version: &LayerVersionRecord) -> LayerVersionsListItem {
        LayerVersionsListItem {
            layer_version_arn: Some(self.version_arn(version.version)),
            version: version.version,
            // A LISTING reports only what the version carries: an omitted description is
            // absent from the entry, while the version's own read defaults it to the
            // empty string (see [`LayerRecord::version_detail`]). RECORDED: AWS's
            // `ListLayerVersions` entry for a version published with no optional member
            // at all carries exactly `CreatedDate`, `LayerVersionArn` and `Version`.
            description: version.description.clone(),
            created_date: Some(version.created.clone()),
            // A list the caller named nothing in is absent from the answer rather than
            // reported empty, exactly as [`LayerRecord::version_detail`] reports it.
            compatible_architectures: (!version.compatible_architectures.is_empty())
                .then(|| version.compatible_architectures.clone()),
            compatible_runtimes: (!version.compatible_runtimes.is_empty())
                .then(|| version.compatible_runtimes.clone()),
            license_info: version.license_info.clone(),
        }
    }
}
