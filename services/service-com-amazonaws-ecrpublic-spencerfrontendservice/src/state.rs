// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecrpublic-spencerfrontendservice service.
//!
//! This module owns the public registry the handlers in `src/ops/` share — its
//! repositories, the images pushed into them, the in-flight layer uploads
//! `InitiateLayerUpload` opens and the one catalog display name the registry itself
//! carries — plus the few pieces of behaviour
//! that no single operation file can own on its own:
//!
//! * the ARN / URI forms every repository carries ([`StoredRepository`] is built once,
//!   by `CreateRepository`, and read back verbatim by every reporting operation);
//! * the repository policy document ([`StoredRepository::policy_text`]) that
//!   `SetRepositoryPolicy` writes in the one rendering this service reports a policy in
//!   ([`canonical_policy_document`]) and `GetRepositoryPolicy` / `DeleteRepositoryPolicy`
//!   read back;
//! * the `sha256:` manifest digest ([`manifest_digest`]) that `PutImage` keys an image
//!   by and `DescribeImages` / `BatchDeleteImage` resolve against, and the JSON-document
//!   check ([`json_document`]) every member the model documents as JSON passes before it
//!   is stored;
//! * what a manifest references ([`manifest_content`]), which is what an image's
//!   reported size is computed over ([`StoredRepository::refresh_image_sizes`]);
//! * the opaque pagination token ([`encode_page_token`] / [`decode_page_token`]) that
//!   `DescribeRepositories`, `DescribeImages` and `DescribeImageTags` must agree on.
//!
//! Every collection is a `BTreeMap`/`BTreeSet`, so each listing walks one stable,
//! key-ordered sequence and a pagination token naming the last key of a page resumes
//! exactly after it.

/// Who a request speaks for, and whether IAM lets it do what it asks — the gate every
/// operation of this service consults before it reads or writes anything.
pub mod authorization;

/// The repository logo as a hosted asset: the payload a catalog write uploads, the URL
/// minted for it and the route that serves those bytes back.
pub mod logo_asset;

/// The repository policy document in the form this service reports it.
pub mod policy_document;

pub use logo_asset::{logo_asset_route, logo_url, StoredLogo};
pub use policy_document::canonical_policy_document;

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ImageDetail, ImageIdentifier, ImageTagDetail, ReferencedImageDetail, RegistryCatalogData,
    Repository, RepositoryCatalogData, RepositoryCatalogDataInput, Tag,
    UnsupportedCommandException,
};

/// The `ecr-public` ARN namespace, as the Smithy model's `awsService` trait declares it
/// (`OPERATIONS.md`, "Wire protocol": ARN namespace `ecr-public`).
pub const ARN_NAMESPACE: &str = "ecr-public";

/// The Regions an ECR Public API endpoint is published in — every Region a caller can
/// ADDRESS this surface at.
///
/// `UnsupportedCommandException` ("The action isn't supported in this Region", client,
/// HTTP 400) is a SERVICE-level error in this model: it is bound to every one of the 23
/// operations rather than declared per operation (`smithy_query(query="digest")` reports
/// it under `common_errors`), and `endpointResolution` is an endpoint rule set that
/// templates a host per Region (`endpointPrefix` `api.ecr-public`) — so the model says any
/// ECR Public call CAN be refused for the Region it was signed for, and says nothing about
/// which Regions the service answers in. AWS's service-endpoint reference lists exactly
/// these two for `api.ecr-public`, and the repository, image, layer upload, tag, policy
/// and repository-catalog surface — plus the registry catalog READ — is served from either
/// of them.
///
/// ADDRESSING this API from a Region is not the same as an action being OFFERED there: a
/// caller signed for any Region in this list reaches the surface, while the one
/// Region-scoped call is offered in a strictly narrower set ([`serves_region`]).
pub const ENDPOINT_REGIONS: &[&str] = &["us-east-1", "us-west-2"];

/// The ONE Region the Region-scoped call of this service is performed in: an
/// account-global control action is carried out in a single Region of its surface, which
/// here is the primary one (the first of [`ENDPOINT_REGIONS`]).
///
/// It is the Region the Region refusal below directs a caller to, and the Region ordinary
/// (Region-independent) calls are signed for.
pub const API_REGION: &str = ENDPOINT_REGIONS[0];

/// Whether a Region-scoped call a request names `region` for is performed there:
/// `UnsupportedCommandException` says the action is not supported in the caller's Region,
/// and an account-global control action is performed in exactly one Region
/// ([`API_REGION`]), so only a request whose credential scope names that Region is served.
///
/// A Region that merely publishes an endpoint of this API ([`ENDPOINT_REGIONS`]) is NOT
/// enough: a caller reaches every Region-independent operation there while this one is
/// still refused.
///
/// A Region name is case-insensitive, so the comparison is too.
pub fn serves_region(region: &str) -> bool {
    region.eq_ignore_ascii_case(API_REGION)
}

/// The `UnsupportedCommandException` a Region-scoped call naming `region` is refused with,
/// or `None` when that Region is one the call is served from ([`serves_region`]).
///
/// The message is a deterministic function of the request and of the service: the operation
/// the caller named, then the Region such a caller is directed to ([`API_REGION`]).
pub fn unsupported_for_region(
    region: &str,
    operation: &str,
) -> Option<UnsupportedCommandException> {
    if serves_region(region) {
        return None;
    }
    Some(UnsupportedCommandException {
        message: Some(format!(
            "{operation} command is only supported in {API_REGION}."
        )),
    })
}

/// The `UnsupportedCommandException` a Region-scoped ECR Public call signed for a Region
/// that does not perform it is refused with, or `None` when it is served
/// ([`unsupported_for_region`]).
///
/// This is the gate of the ONE Region-scoped operation: it is called by
/// `PutRegistryCatalogData` before it touches the account-global record, and by nothing
/// else — an operation AWS serves whatever Region its caller named must not consult it, or
/// a caller that reached ECR Public from another Region is refused everything.
///
/// The Region a request carries is the one in its SigV4 credential scope, parsed by
/// `emulator_server::sigv4_credential_scope` (the single source of SigV4-scope parsing). A
/// request that carries no well-formed SigV4 `Authorization` names no Region at all, so
/// there is nothing to refuse it for and it is served.
pub fn unsupported_in_region(
    parts: &::http::request::Parts,
    operation: &str,
) -> Option<UnsupportedCommandException> {
    let region = ::emulator_server::sigv4_credential_scope(&parts.headers)?.region;
    unsupported_for_region(&region, operation)
}

/// The universal host prefix every ECR Public repository URI carries, as
/// `Registry$registryUri` documents it ("The URI of a public registry. The URI contains
/// a universal prefix and the registry alias").
pub const GALLERY_HOST: &str = "public.ecr.aws";

/// The registry alias this emulator's single public registry is assigned.
///
/// `RegistryAlias$defaultRegistryAlias` documents that "when the first public repository
/// is created, your public registry is assigned a default registry alias"; the real
/// service mints an opaque one per account. The emulator has exactly one registry, so it
/// carries one fixed alias instead — it satisfies `RegistryAliasName`'s modelled
/// `^[a-z][a-z0-9]+(?:[._-][a-z0-9]+)*$` and 2..50 length, and being stable is what lets
/// a `repositoryUri` be compared across calls.
pub const DEFAULT_REGISTRY_ALIAS: &str = "emulator";

/// The status every alias of this registry carries.
///
/// `RegistryAliasStatus` is modelled as an enum of `ACTIVE` / `PENDING` / `REJECTED`
/// (`REGISTRY_ALIAS_STATUS_VALUES` in `ops/types/shared_types.rs`), and `PENDING` /
/// `REJECTED` describe a *requested* custom alias: this service exposes no operation that
/// requests one, so the alias the registry was assigned is the only one it has and it is
/// usable — `ACTIVE`.
pub const REGISTRY_ALIAS_STATUS_ACTIVE: &str = "ACTIVE";

/// The registry tier a repository this registry holds sits in.
///
/// A public registry hosts its repositories in one tier, and the caller does not pick it:
/// `CreateRepository` carries no tier member, so the tier is a property of the registry a
/// repository was created in rather than of the request or of any image pushed to it. The
/// default public registry of an account hosts free public repositories, which is the one
/// value `RepositoryType` is modelled with
/// ([`crate::ops::types::REPOSITORY_TYPE_VALUES`]), so every repository this store holds
/// carries this tier and every image in it reports it
/// ([`StoredRepository::image_detail`]).
///
/// Stated here rather than read off the value list, so the tier does not follow that
/// list's ordering if `RepositoryType` ever gains another value; that it is one of the
/// modelled values is asserted by `tests/all/image_detail_repository_type.rs`.
pub const DEFAULT_REPOSITORY_TYPE: &str = "FREE";

/// The largest page either `Describe*` operation will serve, and the page size a request
/// that named none resolves to: `MaxResults` is modelled `range 1..1000`, so 1000 is the
/// most a caller can ask for.
pub const MAX_PAGE_SIZE: i32 = 1000;

/// The most tags a repository may carry.
///
/// `TooManyTagsException` documents the limit itself: "The list of tags on the repository
/// is over the limit. The maximum number of tags that can be applied to a repository is
/// 50." The modelled `TagList` length (0..200) is wider, so this bound is enforced here.
pub const MAX_TAGS_PER_REPOSITORY: usize = 50;

/// The tag-key namespace AWS keeps for its own tags: a tag under it is a system tag,
/// which an API caller may neither write nor change.
///
/// AWS's resource-tagging rules state the prefix is for AWS use, so a tagging call that
/// names a key under it is refused rather than stored. `TagKey` carries no `@pattern` in
/// this model, so a key under it reaches a handler and is refused there.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The ceiling on how many repositories this store holds, how many images one repository
/// holds, and how many image tags point into one repository: the three caps that bound
/// caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create or a push is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeRepositories` and `DescribeImages` must still answer what `CreateRepository`
/// and `PutImage` stored. The two operations that grow the store refuse past these caps
/// with the `LimitExceededException` both of them model ("the operation didn't succeed
/// because it would have exceeded a service limit for your account") and no read can
/// return. The image cap is the one that matters most: `imageManifest` is modelled
/// `@length(1..4194304)`, so each image carries up to 4 MiB of caller-supplied text.
/// The numbers are the emulator's own bound, not a published AWS default — AWS publishes
/// the repository and image quotas as adjustable, so no single value is the contract —
/// and they are set high enough that no ordinary workflow meets one.
pub const MAX_REPOSITORIES: usize = 1_000;

/// See [`MAX_REPOSITORIES`].
pub const MAX_IMAGES_PER_REPOSITORY: usize = 1_000;

/// See [`MAX_REPOSITORIES`]. This one bounds `tag_index`, which grows by one entry per
/// push that introduces an image tag the repository did not carry.
pub const MAX_IMAGE_TAGS_PER_REPOSITORY: usize = 10_000;

/// See [`MAX_REPOSITORIES`]. This one bounds [`ComAmazonawsEcrpublicSpencerfrontendserviceState::uploads`],
/// which grows by one entry per `InitiateLayerUpload`. An upload a client opened and
/// then abandoned is never reclaimed — [`LayerUpload::started_at`] records when the
/// upload was opened, it is not an expiry the store acts on — so the registry needs a
/// ceiling on how many in-flight layer uploads it holds at once. Past it the open is
/// refused with the `ServerException` `InitiateLayerUpload` models: reaching the ceiling
/// is this registry's own capacity, not a fault in the caller's request.
pub const MAX_LAYER_UPLOADS: usize = 1_000;

/// The `partSize` `InitiateLayerUpload` reports: the size, in bytes, the registry tells
/// the client to send future layer parts in.
///
/// AWS publishes no single number for it — it is the registry's own answer, which is
/// exactly why the client is told it rather than assuming one — so this is the
/// emulator's choice, bounded by the only published constraint on a part: the model caps
/// `UploadLayerPart$layerPartBlob` at 20971520 bytes, so a part of `partSize` bytes must
/// fit under that cap.
pub const REPORTED_LAYER_PART_SIZE: i64 = 10 * 1024 * 1024;

/// The published ceiling on ONE layer part, in bytes: the model caps
/// `UploadLayerPart$layerPartBlob` at `@length(0..20971520)` and the operation's own doc
/// states it in words — "The maximum size of each image layer part can be 20971520 bytes
/// (about 20MB)". The boundary validator already refuses a blob longer than this before
/// a handler runs; the constant is what lets `UploadLayerPart` refuse a *declared* byte
/// range wider than any admissible part, and what a test reads the bound from.
pub const MAX_LAYER_PART_BYTES: i64 = 20_971_520;

/// The most bytes ONE in-flight layer upload accumulates before further parts are
/// refused with the `LimitExceededException` `UploadLayerPart` models.
///
/// A layer is held in memory until `CompleteLayerUpload` consumes it, and a part upload
/// is caller-driven, so the bytes one upload may accrue need a ceiling. AWS publishes an
/// adjustable quota rather than a fixed maximum layer size, so no single number is the
/// contract and this is the emulator's own bound — set well above any layer a test or an
/// ordinary `docker push` sends.
pub const MAX_LAYER_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// The same bound across EVERY in-flight upload at once: [`MAX_LAYER_UPLOADS`] uploads
/// each holding [`MAX_LAYER_UPLOAD_BYTES`] would be far more than this registry keeps in
/// memory, so the total is capped too, and a part that would carry the store past it is
/// refused with the same `LimitExceededException`. The emulator's own bound, for the same
/// reason as [`MAX_LAYER_UPLOAD_BYTES`].
pub const MAX_IN_FLIGHT_LAYER_BYTES: usize = 256 * 1024 * 1024;

/// The most layer blobs ONE repository records as held.
///
/// `CompleteLayerUpload` is the only operation that adds one, and a completed layer is
/// recorded for the life of the process (no read may stop answering what was stored), so
/// the count needs a ceiling for the same reason [`MAX_REPOSITORIES`] does. Only the
/// digest and the layer's size are kept per entry, so this is set far above the layer
/// count of any image a client pushes; past it the completion is refused with the
/// `ServerException` `CompleteLayerUpload` models — reaching the ceiling is this
/// registry's own capacity, not a fault in the caller's request.
pub const MAX_LAYERS_PER_REPOSITORY: usize = 10_000;

/// One in-flight layer upload: what `InitiateLayerUpload` opened, addressed by the
/// upload id it handed the client.
///
/// `UploadLayerPart` appends to it and `CompleteLayerUpload` consumes it, so what is kept
/// here is what opening the upload decided — which repository it is for, the part size
/// the client was told to send parts in, and when it was opened — plus the layer bytes
/// received so far.
#[derive(Debug, Clone)]
pub struct LayerUpload {
    /// The repository the upload was opened against. An upload id is only valid for the
    /// repository it was issued for, so every later call names both.
    pub repository_name: String,
    /// The `partSize` reported to the client when the upload was opened.
    pub part_size: i64,
    /// When `InitiateLayerUpload` opened it. Read by nothing that answers a request: AWS
    /// publishes no lifetime for an in-flight upload, so this registry expires none and
    /// the stamp is the audit record of when the upload was opened.
    pub started_at: SystemTime,
    /// The layer bytes accepted so far, in layer order.
    ///
    /// `UploadLayerPart` accepts a part only when its first byte is consecutive to the
    /// last byte already received (`InvalidLayerPartException`: "the first byte specified
    /// isn't consecutive to the last byte of a previous layer part upload"), so this is
    /// always the layer's leading bytes and its length is the offset the next part must
    /// start at. The bytes are kept verbatim: the layer `CompleteLayerUpload` digests is
    /// exactly what the parts carried.
    pub received: Vec<u8>,
}

impl LayerUpload {
    /// The offset the next layer part must declare as its `partFirstByte`: the number of
    /// bytes accepted so far, which is 0 for an upload that has received nothing.
    pub fn next_part_first_byte(&self) -> i64 {
        // A part is refused unless it fits under `MAX_LAYER_UPLOAD_BYTES`, which is far
        // inside `i64`, so the cast cannot wrap.
        self.received.len() as i64
    }

    /// The position of the last byte accepted for this upload — `lastByteReceived` on a
    /// success and `lastValidByteReceived` on a refusal — or `None` while the upload has
    /// received nothing and so has no last byte.
    pub fn last_byte_received(&self) -> Option<i64> {
        self.received
            .len()
            .checked_sub(1)
            .map(|position| position as i64)
    }
}

/// How many levels of manifest list [`StoredRepository::refresh_image_sizes`] resolves
/// when it recomputes the size of every stored image.
///
/// An index entry may name another index, so a list's size can rest on a chain of lists;
/// the documents and the chain are both caller-supplied, so the resolution is bounded.
/// Four is well past anything a registry client builds (an index over image manifests is
/// one level).
pub const MAX_MANIFEST_LIST_DEPTH: usize = 4;

/// The most `manifests[*]` entries this service reads from one manifest list.
///
/// A manifest list's entries are caller-supplied and `imageManifest` is modelled
/// `@length(1..4194304)`, so one document can name tens of thousands of children; this
/// is the bound that, together with [`MAX_MANIFEST_LIST_DEPTH`] and
/// [`MAX_IMAGES_PER_REPOSITORY`], caps the work of recomputing image sizes. AWS
/// publishes no limit on the entries of an index, and real indexes carry one entry per
/// platform (a handful), so a document that names more children than this states no size
/// this service reports rather than being read in part.
pub const MAX_MANIFEST_LIST_ENTRIES: usize = 100;

/// One image stored under a repository: what `PutImage` persisted, and everything
/// `DescribeImages` reports about it.
#[derive(Debug, Clone)]
pub struct StoredImage {
    /// The `sha256:<hex>` digest of [`Self::manifest`] — this image's identity.
    pub digest: String,
    /// The manifest document exactly as it was pushed.
    pub manifest: String,
    /// What [`Self::manifest`] references, read ONCE when the image was stored (see
    /// [`manifest_content`]): the blob sizes an image manifest declares, or the digests a
    /// manifest list names. Sizing an image never re-parses a manifest document.
    pub references: ManifestContent,
    /// `imageSizeInBytes`: the size this image reports, derived from [`Self::references`]
    /// and, for a manifest list, from the images stored beside it. Maintained by
    /// [`StoredRepository::refresh_image_sizes`] whenever the repository's image set
    /// changes, so a read projects it in constant time; `None` when nothing stored
    /// states a size for this image.
    pub size_in_bytes: Option<i64>,
    /// The manifest's own media type: the one the request declared, else the manifest
    /// document's top-level `mediaType`.
    pub manifest_media_type: Option<String>,
    /// The artifact media type: the manifest document's `config.mediaType`.
    pub artifact_media_type: Option<String>,
    /// When the manifest was pushed.
    pub pushed_at: SystemTime,
    /// Every tag currently pointing at this digest, in tag order.
    pub tags: BTreeSet<String>,
}

/// One image tag of a repository: the manifest it points at, and when the push that
/// put it there created it.
///
/// A tag is mutable — "a tag is mutable, so it moves onto this digest" is what
/// `PutImage` does with a tag already in the repository — so the digest is the tag's
/// CURRENT target rather than the first one it ever had.
#[derive(Debug, Clone)]
pub struct StoredTag {
    /// The `imageDigest` this tag currently names.
    pub digest: String,
    /// `ImageTagDetail$createdAt`: "the time stamp that indicates when the image tag was
    /// created" — the instant of the `PutImage` call that made this tag name
    /// [`Self::digest`]. A tag enters a repository only by being pushed and leaves it
    /// only by being deleted, so this is set once, when the tag is written, and is never
    /// touched by a read: two reads of one tag report the same stamp. Re-pushing a tag
    /// onto a DIFFERENT manifest writes the tag again, so the stamp then describes that
    /// push rather than the superseded one.
    pub created_at: SystemTime,
}

/// One public repository: what `CreateRepository` inserted.
#[derive(Debug, Clone)]
pub struct StoredRepository {
    pub name: String,
    pub arn: String,
    pub registry_id: String,
    pub uri: String,
    /// The registry tier this repository sits in — the `repositoryType` every image of it
    /// is reported with. Written once, by the create that inserted the repository
    /// ([`DEFAULT_REPOSITORY_TYPE`] for one created in this registry), so two reads of one
    /// repository report the same tier and a push never changes it.
    pub repository_type: String,
    pub created_at: SystemTime,
    pub catalog_data: RepositoryCatalogData,
    /// Resource tags, keyed by tag key.
    pub tags: BTreeMap<String, String>,
    /// The repository policy document in the form this service REPORTS it
    /// ([`canonical_policy_document`]), or `None` while this repository carries no policy.
    ///
    /// The document a caller submits is parsed and re-serialized before it is stored, so
    /// `SetRepositoryPolicy`, `GetRepositoryPolicy` and `DeleteRepositoryPolicy` all report
    /// the one canonical rendering of the policy rather than the bytes of whichever request
    /// happened to write it.
    pub policy_text: Option<String>,
    /// Images keyed by `imageDigest`, so `DescribeImages` pages in digest order.
    pub images: BTreeMap<String, StoredImage>,
    /// `imageTag` -> the tag's entry, so a tag-keyed `ImageIdentifier` resolves to a
    /// stored image in one probe and re-pushing a tag moves it between digests.
    ///
    /// Keyed by tag, so `DescribeImageTags` walks one stable, tag-ordered sequence.
    pub tag_index: BTreeMap<String, StoredTag>,
    /// The image layers this repository holds, keyed by the `sha256:<hex>` digest
    /// `CompleteLayerUpload` finalised, each carrying the size in bytes of the layer the
    /// upload's parts made up.
    ///
    /// A layer enters the registry only when an upload is completed ("informs Amazon ECR
    /// that the image layer upload is complete"), and `BatchCheckLayerAvailability`
    /// reports exactly this set — a digest in it is `AVAILABLE` with its size, one that is
    /// not is a `MissingLayerDigest` failure. The layer BYTES are not kept: nothing in
    /// this model serves layer content back (there is no `GetDownloadUrlForLayer` here),
    /// so the registry records what it can report — the digest it computed over the
    /// uploaded bytes and how many there were.
    pub layers: BTreeMap<String, i64>,
    /// The logo payload this repository currently publishes, which is the asset
    /// [`RepositoryCatalogData::logo_url`] names, or `None` while the catalog record
    /// supplies none.
    ///
    /// `logoImageBlob` is an INPUT-only member, so these bytes are observable only by
    /// fetching that URL — which is why they are kept here rather than counted and
    /// dropped. Exactly the payload the current catalog record names is held: a catalog
    /// write replaces the record as a whole, so it replaces this asset too, and a write
    /// that supplies no payload leaves the repository with no logo and no URL. That is
    /// what bounds what one repository holds — one payload, itself inside the modelled
    /// `@length(0..512000)` on the member — however many catalog writes it takes.
    pub logo: Option<StoredLogo>,
}

impl StoredRepository {
    /// The image a caller's [`ImageIdentifier`] names, or `None` when nothing stored
    /// matches it.
    ///
    /// An identifier may carry a digest, a tag, or both; when it carries both they must
    /// agree, which is what [`Self::resolve_identifier`]'s `Err` reports.
    pub fn resolve_identifier(
        &self,
        id: &ImageIdentifier,
    ) -> Result<&StoredImage, IdentifierProblem> {
        let by_digest = id.image_digest.as_deref().filter(|d| !d.is_empty());
        let by_tag = id.image_tag.as_deref().filter(|t| !t.is_empty());
        match (by_digest, by_tag) {
            (None, None) => Err(IdentifierProblem::MissingDigestAndTag),
            (Some(digest), None) => self
                .images
                .get(digest)
                .ok_or(IdentifierProblem::ImageNotFound),
            (None, Some(tag)) => self
                .tag_index
                .get(tag)
                .and_then(|tagged| self.images.get(&tagged.digest))
                .ok_or(IdentifierProblem::ImageNotFound),
            (Some(digest), Some(tag)) => {
                let tagged = &self
                    .tag_index
                    .get(tag)
                    .ok_or(IdentifierProblem::ImageNotFound)?
                    .digest;
                if tagged != digest {
                    return Err(IdentifierProblem::ImageTagDoesNotMatchDigest);
                }
                self.images
                    .get(digest)
                    .ok_or(IdentifierProblem::ImageNotFound)
            }
        }
    }

    /// This image as the modelled `ImageDetail` shape.
    ///
    /// `imageTags` is "the list of tags associated with this image": an UNTAGGED manifest
    /// has none, so the member is omitted rather than reported as an empty list — it is
    /// neither `@required` nor `@default`, and an empty list would claim the service had
    /// looked and found a tag set, which is a different statement from "this image carries
    /// no tag". A manifest loses its last tag when another push moves that tag onto a
    /// different manifest, so this is a state an ordinary push reaches.
    ///
    /// `registryId`, `repositoryName` and `repositoryType` describe where the image sits
    /// rather than the manifest itself, so all three are projected from the repository
    /// that holds it and are the same on every entry of one repository's page: the tier is
    /// a property of the repository ([`Self::repository_type`]), not of the image or of
    /// the request, so it is reported on every image rather than only on a tagged or a
    /// sized one.
    pub fn image_detail(&self, image: &StoredImage) -> ImageDetail {
        ImageDetail {
            registry_id: Some(self.registry_id.clone()),
            repository_name: Some(self.name.clone()),
            image_digest: Some(image.digest.clone()),
            image_tags: if image.tags.is_empty() {
                None
            } else {
                Some(image.tags.iter().cloned().collect())
            },
            image_size_in_bytes: image.size_in_bytes,
            image_pushed_at: Some(as_timestamp(image.pushed_at)),
            image_manifest_media_type: image.manifest_media_type.clone(),
            artifact_media_type: image.artifact_media_type.clone(),
            repository_type: Some(self.repository_type.clone()),
        }
    }

    /// The image a tag points at, as the modelled `ReferencedImageDetail` shape.
    ///
    /// `ReferencedImageDetail` is the `ImageDetail` members that describe the MANIFEST
    /// rather than its place in the registry: the same stored values
    /// [`Self::image_detail`] reports for `imageDigest`, `imageSizeInBytes`,
    /// `imagePushedAt`, `imageManifestMediaType` and `artifactMediaType`, so the two
    /// readers never disagree about one image. The shape carries no `imageTags` member —
    /// the tag is the entry it hangs off — and no registry, repository or repository-tier
    /// member: the request already named the repository those describe.
    pub fn referenced_image_detail(&self, image: &StoredImage) -> ReferencedImageDetail {
        ReferencedImageDetail {
            image_digest: Some(image.digest.clone()),
            image_size_in_bytes: image.size_in_bytes,
            image_pushed_at: Some(as_timestamp(image.pushed_at)),
            image_manifest_media_type: image.manifest_media_type.clone(),
            artifact_media_type: image.artifact_media_type.clone(),
        }
    }

    /// Every image TAG this repository currently carries, in tag order, each paired with
    /// the tag it is keyed by so a page token naming the last tag of a page resumes
    /// exactly after it.
    ///
    /// One entry per tag, never per image: a manifest carrying three tags is reported
    /// three times (once per tag, each naming the same `imageDetail`) and an untagged
    /// manifest is not reported at all, because there is no tag for it to be the detail
    /// of. A tag whose digest nothing stored holds is skipped rather than reported with
    /// an absent image — [`Self::tag_index`] and [`Self::images`] are maintained
    /// together, so that cannot arise from this service's own writes.
    pub fn image_tag_details(&self) -> Vec<(String, ImageTagDetail)> {
        self.tag_index
            .iter()
            .filter_map(|(tag, tagged)| {
                let image = self.images.get(&tagged.digest)?;
                Some((
                    tag.clone(),
                    ImageTagDetail {
                        image_tag: Some(tag.clone()),
                        created_at: Some(as_timestamp(tagged.created_at)),
                        image_detail: Some(self.referenced_image_detail(image)),
                    },
                ))
            })
            .collect()
    }

    /// Recompute the `imageSizeInBytes` every image in this repository reports.
    ///
    /// `ImageDetail$imageSizeInBytes` is "the size, in bytes, of the image in the
    /// repository": the size of the CONTENT the manifest references, which is the total of
    /// the sizes its content descriptors declare for the blobs that make the image up — its
    /// config blob and its layer blobs — never the size of the manifest document itself.
    /// "If the image is a manifest list, this is the max size of all manifests in the
    /// list", so a list reports the largest of the images it references, each child's size
    /// being its own blob total.
    ///
    /// A list's size is a statement about other images in the repository, so it is
    /// recomputed here whenever the image set changes — a push or a delete — rather than
    /// frozen when the list itself arrived, and every read then projects
    /// [`StoredImage::size_in_bytes`] in constant time. The recomputation reads no
    /// manifest document (each one was parsed once, into [`StoredImage::references`]) and
    /// its work is the product of three documented bounds:
    /// [`MAX_IMAGES_PER_REPOSITORY`] entries, each resolving at most
    /// [`MAX_MANIFEST_LIST_ENTRIES`] children, over at most [`MAX_MANIFEST_LIST_DEPTH`]
    /// levels of nesting.
    ///
    /// A size is `None` for an image nothing stored states one for: the member is
    /// neither required nor defaulted, so an unknown size is absent rather than zero.
    /// That covers a document that references no blob or manifest array at all, a blob
    /// whose size neither its descriptor nor this repository's record of it states,
    /// a total no `long` can represent — the addends are caller-supplied, so they are
    /// added with checked arithmetic and never wrapped — and a list none of whose
    /// children the repository holds. An image manifest with an EMPTY `layers` array is
    /// a statement, not a gap: it references no layer content, so its total is the size of
    /// the config blob alone, and `0` for a document that declares neither.
    ///
    /// What a blob's size is read from: the size its descriptor declares, else the byte
    /// count this repository recorded when the layer upload was completed
    /// ([`StoredRepository::layers`]) — a descriptor that states no size still references
    /// content the registry has counted.
    pub fn refresh_image_sizes(&mut self) {
        // An image manifest's total rests on its own descriptors and on the blobs this
        // repository holds for them.
        let mut sizes: BTreeMap<String, Option<i64>> = self
            .images
            .iter()
            .map(|(digest, image)| {
                let own = match &image.references {
                    ManifestContent::Blobs(blobs) => blobs.iter().try_fold(0i64, |total, blob| {
                        let size = blob
                            .size
                            .or_else(|| self.layers.get(&blob.digest).copied())?;
                        total.checked_add(size)
                    }),
                    ManifestContent::List(_) | ManifestContent::Opaque => None,
                };
                (digest.clone(), own)
            })
            .collect();

        // A manifest list is the max over the images it names, so a list of lists
        // settles one level per pass; a pass that changes nothing has settled.
        for _ in 0..MAX_MANIFEST_LIST_DEPTH {
            let mut settled = true;
            let mut next = sizes.clone();
            for (digest, image) in &self.images {
                if let ManifestContent::List(children) = &image.references {
                    let resolved = children
                        .iter()
                        .filter_map(|child| sizes.get(child).copied().flatten())
                        .max();
                    if next.insert(digest.clone(), resolved) != Some(resolved) {
                        settled = false;
                    }
                }
            }
            sizes = next;
            if settled {
                break;
            }
        }

        for (digest, size) in sizes {
            if let Some(image) = self.images.get_mut(&digest) {
                image.size_in_bytes = size;
            }
        }
    }

    /// Associate `requested` with this repository, keyed by tag key.
    ///
    /// `TagResource` documents the merge: "if existing tags on a resource aren't
    /// specified in the request parameters, they aren't changed" — so a key the request
    /// names takes the requested value, a key it does not name keeps the value it had,
    /// and the repository ends up carrying the union of the two sets.
    ///
    /// The limit is on what the repository ENDS UP carrying, not on the size of one
    /// request: re-tagging a key the repository already holds replaces it rather than
    /// adding to it. Past the limit nothing is written at all, so a refused call leaves
    /// the repository exactly as it was.
    pub fn apply_tags(&mut self, requested: BTreeMap<String, String>) -> Result<(), TagProblem> {
        let resulting = self
            .tags
            .keys()
            .chain(requested.keys())
            .collect::<BTreeSet<&String>>()
            .len();
        if resulting > MAX_TAGS_PER_REPOSITORY {
            return Err(TagProblem::TooManyTags(TOO_MANY_TAGS_MESSAGE.to_string()));
        }
        self.tags.extend(requested);
        Ok(())
    }

    /// Drop the tags `keys` names from this repository, keyed by tag key.
    ///
    /// `UntagResource` "deletes specified tags from a resource": exactly the keys the
    /// request names leave the repository and every other tag it carries stays, with the
    /// value it had. A key the repository does not carry removes nothing, so asking for
    /// one is not a failure and repeating the same removal is the same single removal.
    pub fn remove_tags(&mut self, keys: &BTreeSet<String>) {
        self.tags.retain(|key, _| !keys.contains(key));
    }

    /// This repository's resource tags as the modelled `TagList`.
    ///
    /// The stored map is keyed by tag key, so the list comes out in key order: a stable
    /// sequence, which is what lets two reads of one repository be compared.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: Some(key.clone()),
                value: Some(value.clone()),
            })
            .collect()
    }

    /// This repository as the modelled `Repository` shape.
    pub fn as_repository(&self) -> Repository {
        Repository {
            repository_arn: Some(self.arn.clone()),
            registry_id: Some(self.registry_id.clone()),
            repository_name: Some(self.name.clone()),
            repository_uri: Some(self.uri.clone()),
            created_at: Some(as_timestamp(self.created_at)),
        }
    }
}

/// How a requested `imageIds` entry reads inside an error message.
///
/// ECR names the whole requested [`ImageIdentifier`] back to the caller, both members
/// spelled out and an absent one rendered as `null`, so the caller sees the entry
/// exactly as it sent it rather than only the half that was matched.
///
/// The private-registry service renders it identically
/// (`service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`'s
/// `state::identifier_description`); the two are separate functions only because
/// `ImageIdentifier` is generated per service crate. Keep them in step.
pub fn identifier_description(id: &ImageIdentifier) -> String {
    format!(
        "imageId {{imageDigest:'{}', imageTag:'{}'}}",
        id.image_digest.as_deref().unwrap_or("null"),
        id.image_tag.as_deref().unwrap_or("null")
    )
}

/// Why an [`ImageIdentifier`] names no stored image. Each variant is one of the
/// `ImageFailureCode` values the model declares, which is how `BatchDeleteImage` reports
/// it; `DescribeImages` answers the same three cases with its own modelled errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierProblem {
    /// The identifier carried neither a digest nor a tag.
    MissingDigestAndTag,
    /// Nothing stored carries that digest or that tag.
    ImageNotFound,
    /// Both were given and the tag points at a different digest.
    ImageTagDoesNotMatchDigest,
}

impl IdentifierProblem {
    /// The `ImageFailureCode` value for this problem.
    pub fn failure_code(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => "MissingDigestAndTag",
            Self::ImageNotFound => "ImageNotFound",
            Self::ImageTagDoesNotMatchDigest => "ImageTagDoesNotMatchDigest",
        }
    }

    /// The reason to report beside the code.
    pub fn failure_reason(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => {
                "Invalid request parameters: both tag and digest cannot be null"
            }
            Self::ImageNotFound => "Requested image not found",
            Self::ImageTagDoesNotMatchDigest => {
                "Requested image tag does not match the specified image digest"
            }
        }
    }

    /// Whether this problem is one the REQUEST states, rather than one the repository's
    /// contents decide.
    ///
    /// An identifier carrying neither member names no image whatever the registry holds, so
    /// it is refused while the request's identifiers are being read; the other two are the
    /// verdict of a lookup against a repository. A batch reports the first kind before the
    /// second (see `BatchDeleteImage`), so this is what splits the two passes.
    pub fn states_an_unusable_identifier(self) -> bool {
        matches!(self, Self::MissingDigestAndTag)
    }
}

/// Per-service state for com-amazonaws-ecrpublic-spencerfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEcrpublicSpencerfrontendserviceState {
    /// The 12-digit account id of the one public registry this emulator serves. It is
    /// the `registryId` every response carries and the account segment of every ARN.
    pub registry_id: String,
    /// The registry's alias, which is the second segment of every `repositoryUri`.
    pub registry_alias: String,
    /// Repositories keyed by `repositoryName`, so `DescribeRepositories` pages in name
    /// order.
    pub repositories: BTreeMap<String, StoredRepository>,
    /// The registry's catalog display name — the ONE `RegistryCatalogData` this registry
    /// has (`RegistryCatalogData` is "the metadata for a public registry" and carries
    /// `displayName` alone), so it lives here beside the registry id rather than per
    /// repository. `None` until a `PutRegistryCatalogData` call sets one, which is what
    /// lets `GetRegistryCatalogData` report an unset display name as absent rather than
    /// as an invented string.
    pub registry_display_name: Option<String>,
    /// In-flight layer uploads keyed by the `uploadId` `InitiateLayerUpload` minted, the
    /// handle `UploadLayerPart` and `CompleteLayerUpload` address an upload by.
    pub uploads: BTreeMap<String, LayerUpload>,
}

impl Default for ComAmazonawsEcrpublicSpencerfrontendserviceState {
    fn default() -> Self {
        Self {
            registry_id: ::engine::EMULATOR_ACCOUNT_ID.to_string(),
            registry_alias: DEFAULT_REGISTRY_ALIAS.to_string(),
            repositories: BTreeMap::new(),
            registry_display_name: None,
            uploads: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsEcrpublicSpencerfrontendserviceState {
    /// The ARN a repository of this registry carries ([`repository_arn`]).
    pub fn repository_arn(&self, repository_name: &str) -> String {
        repository_arn(repository_name)
    }

    /// The ARN of the registry itself.
    ///
    /// `Registry$registryArn` is "the Amazon Resource Name (ARN) of the public registry",
    /// and ECR Public is a partition-wide service, so — exactly as for a repository ARN —
    /// the region segment is empty and the account segment is the registry id, which is
    /// also the registry resource's own name.
    pub fn registry_arn(&self) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, "", &format!("registry/{}", self.registry_id))
    }

    /// The URI of the registry: `Registry$registryUri` documents it as "a universal
    /// prefix and the registry alias", which is the `repositoryUri` form without the
    /// trailing repository segment.
    pub fn registry_uri(&self) -> String {
        format!("{GALLERY_HOST}/{}", self.registry_alias)
    }

    /// The `docker pull`-able URI of a repository: the universal gallery prefix, the
    /// registry alias, then the repository name.
    pub fn repository_uri(&self, repository_name: &str) -> String {
        format!("{}/{repository_name}", self.registry_uri())
    }

    /// This registry's catalog metadata as the modelled `RegistryCatalogData` shape: what
    /// `GetRegistryCatalogData` reports and `PutRegistryCatalogData` answers with after a
    /// write. There is exactly one per registry, so both operations project this one
    /// stored value.
    pub fn registry_catalog_data(&self) -> RegistryCatalogData {
        RegistryCatalogData {
            display_name: self.registry_display_name.clone(),
        }
    }

    /// How many layer bytes every in-flight upload holds between them, which is what
    /// [`MAX_IN_FLIGHT_LAYER_BYTES`] bounds.
    pub fn in_flight_layer_bytes(&self) -> usize {
        self.uploads
            .values()
            .map(|upload| upload.received.len())
            .sum()
    }

    /// The logo payload `asset_id` of `repository_name` in the registry `registry_id`
    /// names, or `None` when this registry holds no such asset — a registry that is not
    /// this one, an unknown repository, a repository with no logo, or an asset id the
    /// repository's current logo is not.
    ///
    /// The registry is part of the lookup rather than assumed: the fetch of a logo carries
    /// no credential of its own (AWS serves a gallery logo anonymously), so the scope the
    /// asset is read in is the one the URL this service minted named, and a path naming
    /// another registry resolves to nothing rather than to this registry's asset.
    ///
    /// A pure read: the route that serves a logo resolves the asset through this, so the
    /// bytes it answers are the ones a catalog write stored.
    pub fn logo_asset(
        &self,
        registry_id: &str,
        repository_name: &str,
        asset_id: &str,
    ) -> Option<&StoredLogo> {
        if registry_id != self.registry_id {
            return None;
        }
        self.repositories
            .get(repository_name)?
            .logo
            .as_ref()
            .filter(|logo| logo.asset_id == asset_id)
    }

    /// Whether `registry_id` — a request's `registryId`, which the model lets be either
    /// an account id or a registry alias — names this registry.
    ///
    /// This leniency belongs to the `registryId` REQUEST MEMBER only: an ARN's account
    /// segment is an account id, so [`Self::repository_by_arn`] compares that segment
    /// against this registry's id and not against its alias.
    pub fn is_this_registry(&self, registry_id: &str) -> bool {
        registry_id == self.registry_id || registry_id == self.registry_alias
    }

    /// What `RepositoryNotFoundException` states for a name this registry does not hold:
    /// the name asked for and the registry it was looked for in, which is the pair the
    /// modelled description names ("Check the spelling of the specified repository and
    /// ensure that you're performing operations on the correct registry").
    pub fn repository_not_found_message(&self, repository_name: &str) -> String {
        format!(
            "The repository with name '{repository_name}' does not exist in the registry \
             with id '{}'",
            self.registry_id
        )
    }

    /// What to tell a caller whose `registryId` names some other registry: this emulator
    /// serves exactly one, so a request for another names a parameter it cannot satisfy.
    pub fn foreign_registry_message(&self) -> String {
        format!(
            "Invalid parameter at 'registryId' failed to satisfy constraint: 'Member must \
             name the public registry of this account ({})'",
            self.registry_id
        )
    }

    /// The repository a `resourceArn` names, or why nothing stored answers to it.
    ///
    /// The ARN is parsed by [`parse_repository_arn`] and then resolved against this
    /// registry: it must agree with the ARN this registry mints for that repository
    /// ([`Self::repository_arn`]) on partition, account and name, so an ARN naming
    /// another partition or another registry names no repository here — the same
    /// not-found answer as a name this registry never held ("ensure that you're
    /// performing operations on the correct registry",
    /// `RepositoryNotFoundException$message`). The region segment is not compared:
    /// ECR Public is partition-wide, so the ARNs it mints carry none and a caller that
    /// supplies one still names the same resource.
    pub fn repository_by_arn(
        &self,
        resource_arn: &str,
    ) -> Result<&StoredRepository, ResourceArnProblem> {
        let name = self.repository_name_by_arn(resource_arn)?;
        self.repositories
            .get(&name)
            .ok_or_else(|| self.arn_not_found(&name))
    }

    /// The repository a `resourceArn` names, for a caller that writes to it; the same
    /// resolution [`Self::repository_by_arn`] makes for a caller that reads it.
    pub fn repository_by_arn_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut StoredRepository, ResourceArnProblem> {
        let name = self.repository_name_by_arn(resource_arn)?;
        let absent = self.arn_not_found(&name);
        self.repositories.get_mut(&name).ok_or(absent)
    }

    /// The NAME of the repository a `resourceArn` names: the one resolution both
    /// [`Self::repository_by_arn`] and [`Self::repository_by_arn_mut`] are built on, so a
    /// reader and a writer never disagree about which repository an ARN names.
    pub fn repository_name_by_arn(
        &self,
        resource_arn: &str,
    ) -> Result<String, ResourceArnProblem> {
        let arn = parse_repository_arn(resource_arn)?;
        // The ARN this registry mints for the same name, so neither the partition nor
        // the account id is written out here: both come from `engine::emulator_arn`.
        let minted = parse_repository_arn(&self.repository_arn(&arn.name))
            .map_err(|_| self.arn_not_found(&arn.name))?;
        if arn.partition != minted.partition || arn.registry_id != minted.registry_id {
            return Err(self.arn_not_found(&arn.name));
        }
        if !self.repositories.contains_key(&arn.name) {
            return Err(self.arn_not_found(&arn.name));
        }
        Ok(arn.name)
    }

    /// What a tagging call is told about a `resourceArn` no repository of this registry
    /// answers to: the name asked for and the registry it was looked for in
    /// ([`Self::repository_not_found_message`]).
    fn arn_not_found(&self, repository_name: &str) -> ResourceArnProblem {
        ResourceArnProblem::RepositoryNotFound(self.repository_not_found_message(repository_name))
    }
}

/// Why a `resourceArn` names no repository of this registry.
///
/// The two variants are the two modelled refusals every tagging operation declares:
/// a string that is not a well-formed ECR Public repository ARN is an
/// `InvalidParameterException`, while a well-formed one naming a repository (or a
/// registry) this emulator does not hold is a `RepositoryNotFoundException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceArnProblem {
    /// Not a well-formed ECR Public repository ARN; carries the message that names
    /// the offending member and the constraint it failed.
    Malformed(String),
    /// Well-formed, but nothing stored answers to it.
    RepositoryNotFound(String),
}

/// The ARN a repository of this registry carries.
///
/// ECR Public is a global (partition-wide) service, so the region segment is empty; the
/// account segment is the registry id, which is this account's id
/// (`engine::emulator_arn`). The ONE place a repository ARN is minted: the reporting
/// operations read the ARN stored on the repository, and the authorization gate
/// ([`authorization::repository_resource`]) builds the one a decision is made about for a
/// repository that may not exist yet, so neither can spell it differently.
pub fn repository_arn(repository_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, "", &format!("repository/{repository_name}"))
}

/// The registry and repository a well-formed ECR Public repository ARN names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryArn {
    /// The ARN's partition segment (`aws` and its siblings).
    pub partition: String,
    /// The ARN's account segment, which for this service is the registry id.
    pub registry_id: String,
    /// The repository name, which may itself carry `/` separators (a namespaced
    /// repository such as `team/app`), as `RepositoryName`'s modelled pattern
    /// `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$` admits.
    pub name: String,
}

/// What the service tells a caller whose `resourceArn` is not a well-formed ECR Public
/// repository ARN: the parameter-constraint form the real service answers with, naming
/// the member and the constraint it failed (`InvalidParameterException`: "the specified
/// parameter is invalid; review the available parameters for the API request").
pub const INVALID_ARN_MESSAGE: &str =
    "Invalid parameter at 'resourceArn' failed to satisfy constraint: 'Invalid ARN'";

/// Parse `resource_arn` as the ARN of an ECR Public repository.
///
/// `ListTagsForResource$resourceArn` documents that "currently, the supported resource
/// is an Amazon ECR Public repository", so the only string this accepts is the six-field
/// ARN form `arn:<partition>:ecr-public:<region>:<account>:repository/<name>` — the
/// general ARN syntax with this service's namespace and the `repository` resource type.
/// Every other string fails the one `resourceArn` constraint, so they all share
/// [`INVALID_ARN_MESSAGE`].
///
/// The region segment is left free: ECR Public is partition-wide, so the ARNs it mints
/// carry none, but a caller that supplies one still names the same resource.
pub fn parse_repository_arn(resource_arn: &str) -> Result<RepositoryArn, ResourceArnProblem> {
    // The resource segment may itself contain `:`, so only the first five fields are
    // split off; everything after the fifth colon is the resource.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    let malformed = || {
        Err(ResourceArnProblem::Malformed(
            INVALID_ARN_MESSAGE.to_string(),
        ))
    };
    let [arn, partition, service, _region, account, resource] = fields[..] else {
        return malformed();
    };
    if arn != "arn" || partition.is_empty() || service != ARN_NAMESPACE || account.is_empty() {
        return malformed();
    }
    // A repository name may itself carry `/` separators (`team/app`), so everything
    // after the resource type is the name.
    let Some(name) = resource.strip_prefix("repository/").filter(|n| !n.is_empty()) else {
        return malformed();
    };
    Ok(RepositoryArn {
        partition: partition.to_string(),
        registry_id: account.to_string(),
        name: name.to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEcrpublicSpencerfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEcrpublicSpencerfrontendserviceState>)
}

/// The `sha256:<hex>` digest of a manifest, which is an image's identity in a registry.
///
/// `ImageDetail$imageDigest` documents it as "the `sha256` digest of the image
/// manifest", so it is computed over the manifest bytes exactly as pushed.
pub fn manifest_digest(manifest: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()))
}

/// The `sha256:<hex>` digest of the bytes a layer upload carried, which is a layer's
/// identity in a registry.
///
/// `CompleteLayerUpload$layerDigest` and `Layer$layerDigest` are both "the `sha256`
/// digest of the image layer", so it is computed over the uploaded bytes themselves —
/// the same derivation [`manifest_digest`] makes over a manifest document, over a blob
/// rather than text.
pub fn layer_digest(layer: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(layer))
}

/// `candidate` under the spelling a layer digest is stored and compared by: the hex half
/// lower-cased, the algorithm half verbatim.
///
/// Hex is a case-insensitive rendering of the same bytes — the modelled `LayerDigest`
/// pattern admits either case for the digits (`^[a-zA-Z0-9-_+.]+:[a-fA-F0-9]+$`) — so two
/// spellings of one digest resolve to one layer. The algorithm half is NOT case-folded: an
/// OCI digest's algorithm is a token, so a differently cased token names a different
/// algorithm rather than the same one.
pub fn canonical_layer_digest(candidate: &str) -> String {
    match candidate.split_once(':') {
        Some((algorithm, hex)) => format!("{algorithm}:{}", hex.to_ascii_lowercase()),
        None => candidate.to_string(),
    }
}

/// Whether `candidate` has the form the model gives a layer digest: `<algorithm>:<hex>`,
/// as `LayerDigest`'s pattern `^[a-zA-Z0-9-_+.]+:[a-fA-F0-9]+$` spells it
/// (`smithy_query(query="shape", shape="LayerDigest")`).
///
/// `BatchCheckLayerAvailability` takes its digests as `BatchedOperationLayerDigest`,
/// which carries a length bound and NO pattern, so a digest that is not of this form
/// reaches the handler and is what the modelled `InvalidLayerDigest` failure code is
/// for. Both halves are checked here rather than with a regular expression: the
/// expression has no alternation or repetition beyond one split, so the split is the
/// rule.
pub fn is_layer_digest_form(candidate: &str) -> bool {
    let Some((algorithm, hex)) = candidate.split_once(':') else {
        return false;
    };
    !algorithm.is_empty()
        && algorithm
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.'))
        && !hex.is_empty()
        && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// The document a member whose content the model documents as JSON carries — an image
/// manifest (`ImageManifest`: "The image manifest that corresponds to the image to be
/// uploaded"), a repository policy (`RepositoryPolicyText`: "The policy text will be in
/// JSON format") — parsed BEFORE anything is stored.
///
/// `Err` carries the `InvalidParameterException` message ECR Public fills from the
/// request for a body that is not parseable JSON: the member at fault and the rule it
/// failed, and never the rejected body itself (a manifest is modelled up to 4 MiB).
/// `member` is the member as the service names it in that message.
pub fn json_document(member: &str, text: &str) -> Result<::serde_json::Value, String> {
    ::serde_json::from_str(text).map_err(|_| invalid_json_message(member))
}

/// The `InvalidParameterException` message ECR Public fills from the request for a member
/// whose content is not the JSON document that member has to carry — a body that does not
/// parse, and a manifest that does not satisfy the schema it declares itself to be.
///
/// This service states every refusal OF A DOCUMENT with this one clause: it names the
/// member at fault and that the content is not the document that member takes, so the two
/// manifest-level refusals (a body that is not JSON, and a document whose content is not
/// the schema it declares) read alike on the wire, as the recorded refusal does.
///
/// `member` is the member as the service names it in that message. The rejected content is
/// never part of it.
pub fn invalid_json_message(member: &str) -> String {
    format!("Invalid parameter at '{member}' failed to satisfy constraint: 'Invalid JSON syntax'")
}

/// The content member the schema a manifest media type names requires a document to carry.
///
/// A registry manifest declares its own schema in its `mediaType`, and each schema is
/// defined by where it lists its content: a manifest list / OCI image index lists other
/// manifests under `manifests`, an image manifest lists layer descriptors under `layers`,
/// and the Docker V2 Schema 1 form lists them under `fsLayers`. A media type this table
/// does not name declares no schema this registry knows, so nothing is required of the
/// document that carries it.
const MANIFEST_SCHEMA_MEMBERS: &[(&str, &str)] = &[
    (
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "manifests",
    ),
    ("application/vnd.oci.image.index.v1+json", "manifests"),
    (
        "application/vnd.docker.distribution.manifest.v2+json",
        "layers",
    ),
    ("application/vnd.oci.image.manifest.v1+json", "layers"),
    (
        "application/vnd.docker.distribution.manifest.v1+json",
        "fsLayers",
    ),
    (
        "application/vnd.docker.distribution.manifest.v1+prettyjws",
        "fsLayers",
    ),
];

/// The media type under the spelling [`MANIFEST_SCHEMA_MEMBERS`] is keyed by: without the
/// parameters a media type may carry after a `;`, trimmed, and case-folded, because a
/// media type's type and subtype are case-insensitive.
fn media_type_key(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase()
}

/// The `InvalidParameterException` message for a manifest that does not satisfy the schema
/// its own declared media type names, or `None` when the document and the schema agree.
///
/// A manifest is only a manifest of the kind it says it is: the registry is the only place
/// that consistency can be enforced, so a document declaring a schema while carrying no
/// content of that schema (an index media type over a document with no `manifests` array,
/// an image-manifest media type over a document with no layer array) is refused rather
/// than stored, and nothing is created for it. `media_type` is the type declared for the
/// document, which is the one the request declared and otherwise the document's own ("if
/// you push an image manifest that doesn't contain the mediaType field, you must specify
/// the imageManifestMediaType in the request", `PutImage$imageManifestMediaType`). A
/// document that declares no schema at all, or one this registry does not know, is left to
/// the content it carries.
///
/// `member` is the member the message names (`ImageManifest` for a pushed manifest).
pub fn manifest_schema_violation(
    member: &str,
    media_type: Option<&str>,
    document: &serde_json::Value,
) -> Option<String> {
    let declared = media_type_key(media_type?);
    let (_, required) = MANIFEST_SCHEMA_MEMBERS
        .iter()
        .find(|(media_type, _)| *media_type == declared)?;
    let carried = document
        .get(required)
        .is_some_and(::serde_json::Value::is_array);
    if carried {
        return None;
    }
    Some(invalid_json_message(member))
}

/// The blob digests an image manifest names: its config descriptor's, then each layer
/// descriptor's, as the document spelled them, each named once and in document order.
///
/// These are the blobs that have to be in the repository before the manifest that
/// references them can be stored ("when an image is pushed and all new image layers have
/// been uploaded, the PutImage API is called", `PutImage`), so this is what the push
/// resolves against the repository's completed layers. A descriptor that names no digest
/// points at no blob and so states nothing to resolve.
///
/// A manifest list names other MANIFESTS rather than blobs, and a list document carries
/// neither `config` nor `layers`, so it names no blob here: what it references is read by
/// [`manifest_content_of`] instead. A document that carries BOTH — a `layers` array beside
/// a `manifests` one — still names the blobs of its layer descriptors, so they are read:
/// what a document names is read from the members it actually carries, never skipped
/// because another member is present.
///
/// `None` for a document naming more layer descriptors than
/// [`MAX_MANIFEST_LAYER_ENTRIES`]: the count is the caller's, so the walk is bounded
/// before any per-entry work rather than after it, and a document past the bound is not
/// read in part — the push that carries it is refused instead.
pub fn manifest_blob_digests(document: &serde_json::Value) -> Option<Vec<String>> {
    let digest_of = |descriptor: &serde_json::Value| -> Option<String> {
        descriptor
            .get("digest")
            .and_then(serde_json::Value::as_str)
            .filter(|digest| !digest.is_empty())
            .map(ToString::to_string)
    };
    let layers = match document.get("layers").and_then(|l| l.as_array()) {
        Some(layers) if layers.len() > MAX_MANIFEST_LAYER_ENTRIES => return None,
        Some(layers) => layers.as_slice(),
        None => &[],
    };
    // Each blob is named once and in document order, so the order is kept in the `Vec`
    // while membership is decided by the set beside it: one descriptor costs one set
    // lookup, whatever the document's length.
    let mut named: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut push = |digest: Option<String>| {
        if let Some(digest) = digest {
            if seen.insert(digest.clone()) {
                named.push(digest);
            }
        }
    };
    push(document.get("config").and_then(digest_of));
    for layer in layers {
        push(digest_of(layer));
    }
    Some(named)
}

/// The most `layers[*]` descriptors this service reads from one image manifest.
///
/// A push commits a manifest over the blobs the repository holds, and a repository records
/// at most [`MAX_LAYERS_PER_REPOSITORY`] layers, so a document naming more layer
/// descriptors than that names blobs no repository of this registry could hold. The count
/// is caller-supplied and `imageManifest` is modelled `@length(1..4194304)`, so one
/// document could otherwise name hundreds of thousands of descriptors; this is the bound
/// that caps the work of reading one, the way [`MAX_MANIFEST_LIST_ENTRIES`] caps a
/// manifest list's.
pub const MAX_MANIFEST_LAYER_ENTRIES: usize = MAX_LAYERS_PER_REPOSITORY;

/// The `InvalidParameterException` message for a manifest naming more layer descriptors
/// than [`MAX_MANIFEST_LAYER_ENTRIES`].
///
/// The service's parameter template over the member at fault and the rule it failed, as
/// every refused parameter of this API is stated: here the rule is this registry's own
/// ceiling on how many layer descriptors it reads from one document, so the clause states
/// that count rather than the document's syntax.
pub fn manifest_layer_count_message(member: &str) -> String {
    format!(
        "Invalid parameter at '{member}' failed to satisfy constraint: 'Member must have \
         no more than {MAX_MANIFEST_LAYER_ENTRIES} layers'"
    )
}

/// One blob an image manifest references: the digest the descriptor names it by, and the
/// size that descriptor declares for it, if any.
///
/// The digest is kept beside the size because what a manifest does not state about a blob
/// the repository's own record of that blob does: a completed layer upload counted the
/// bytes, so [`StoredRepository::refresh_image_sizes`] can read a size the document left
/// out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestBlob {
    /// The digest the descriptor names, under the spelling layer digests are stored and
    /// compared by ([`canonical_layer_digest`]) so a read of the repository's layer record
    /// costs one lookup and no allocation; empty for a descriptor that names no blob at
    /// all.
    pub digest: String,
    /// The byte count the descriptor declares, or `None` when it declares none usable.
    pub size: Option<i64>,
}

/// What content a stored manifest document references.
///
/// A registry manifest is one of two things: an image manifest, which names the blobs of
/// one image — its config blob and its layer blobs — or a manifest list / OCI index, which
/// lists other manifests by digest. Anything else states the size of nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestContent {
    /// An image manifest: the DISTINCT blobs it references — its `config` descriptor, then
    /// each entry of `layers`, in document order.
    Blobs(Vec<ManifestBlob>),
    /// A manifest list or OCI index: the distinct digests of the manifests it
    /// references, in digest order.
    List(BTreeSet<String>),
    /// A document that references neither blobs nor manifests.
    Opaque,
}

/// Read a PARSED manifest document for the content it references.
///
/// The push path has already parsed the document (it reads the media types from it), so
/// this takes the `Value` rather than the text: one request parses one manifest once.
///
/// Both forms carry OCI content descriptors — a `mediaType`, a `digest` and a `size` —
/// under `config` and `layers` for an image manifest and under `manifests` for a list, so
/// this reads those members and nothing else. A document that carries none of them is
/// [`ManifestContent::Opaque`], and so is one that could not be parsed at all (the
/// caller has no `Value` to pass for it).
///
/// An image manifest's descriptors are read in document order, the `config` one first:
/// the config blob is content of the image exactly as a layer blob is
/// ([`StoredRepository::refresh_image_sizes`] totals them). They are DEDUPLICATED by
/// digest, under the spelling digests are compared by ([`canonical_layer_digest`]), because
/// one blob a manifest names twice — as its configuration and as its only layer, which is
/// what a minimal probe image does — is one blob in the repository and is counted once. A
/// descriptor that names no digest references nothing to deduplicate, so each one is kept.
///
/// A descriptor's `size` is the byte count of the blob it points at, so a value that is
/// absent, not an integer, or below zero states no size at all — the document is
/// caller-supplied, and nothing it says may be read as a byte count it did not give.
///
/// A list's digests are collected as a SET, so naming one child a thousand times costs
/// what naming it once costs; a document naming more than [`MAX_MANIFEST_LIST_ENTRIES`]
/// children, or more layer descriptors than [`MAX_MANIFEST_LAYER_ENTRIES`], is
/// [`ManifestContent::Opaque`] rather than read in part.
pub fn manifest_content_of(document: &serde_json::Value) -> ManifestContent {
    if let Some(manifests) = document.get("manifests").and_then(|m| m.as_array()) {
        if manifests.len() > MAX_MANIFEST_LIST_ENTRIES {
            return ManifestContent::Opaque;
        }
        return ManifestContent::List(
            manifests
                .iter()
                .filter_map(|descriptor| descriptor.get("digest"))
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect(),
        );
    }
    let config = document.get("config");
    let layers = document.get("layers").and_then(|l| l.as_array());
    if config.is_none() && layers.is_none() {
        return ManifestContent::Opaque;
    }
    // The descriptor count is the caller's and `imageManifest` is modelled
    // `@length(1..4194304)`, so the walk is bounded before any per-descriptor work — the
    // same ceiling [`manifest_blob_digests`] refuses a push over, applied here too so
    // nothing past it is read or stored even when a document reaches this reader by
    // another path. A document naming more descriptors than this registry reads states no
    // size it reports, rather than being read in part.
    if layers.is_some_and(|layers| layers.len() > MAX_MANIFEST_LAYER_ENTRIES) {
        return ManifestContent::Opaque;
    }
    let descriptor_blob = |descriptor: &serde_json::Value| ManifestBlob {
        digest: descriptor
            .get("digest")
            .and_then(serde_json::Value::as_str)
            .map(canonical_layer_digest)
            .unwrap_or_default(),
        size: descriptor
            .get("size")
            .and_then(serde_json::Value::as_i64)
            .filter(|size| *size >= 0),
    };
    let layer_descriptors: &[serde_json::Value] = layers.map(Vec::as_slice).unwrap_or_default();
    let mut blobs: Vec<ManifestBlob> = config
        .map(descriptor_blob)
        .into_iter()
        .chain(layer_descriptors.iter().map(descriptor_blob))
        .collect();
    let mut named: BTreeSet<String> = BTreeSet::new();
    blobs.retain(|blob| blob.digest.is_empty() || named.insert(blob.digest.clone()));
    ManifestContent::Blobs(blobs)
}

/// A `SystemTime` as a modelled timestamp.
pub fn as_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The catalog record a repository ends up with, given the `RepositoryCatalogDataInput` a
/// request supplied: the document every reader reports, and the logo payload it names.
///
/// The two travel together because `logoUrl` is a statement ABOUT the payload: a record
/// that names an asset is only reportable while the registry holds that asset's bytes.
#[derive(Debug, Clone)]
pub struct CatalogRecord {
    /// The catalog document as every reader of this repository reports it.
    pub catalog_data: RepositoryCatalogData,
    /// The payload [`RepositoryCatalogData::logo_url`] names, or `None` when the record
    /// names no logo.
    pub logo: Option<StoredLogo>,
}

/// The catalog record a repository ends up with, given the `RepositoryCatalogDataInput` a
/// request supplied, the registry it is a repository of, and the endpoint this emulator is
/// served at.
///
/// Every text/list member passes through unchanged. `logoUrl` is documented as "the URL
/// that contains the logo that's associated with the repository", so it exists only once
/// a logo payload has been supplied, and names that payload on the endpoint this emulator
/// serves the asset on ([`logo_asset::logo_url`]) — its last segment being the payload's
/// own digest, so the same logo always yields the same URL and a replacement always yields
/// a different one. `marketplaceCertified` is "whether the repository is certified by
/// Amazon Web Services Marketplace": it is neither required nor defaulted, and nothing a
/// caller of this API can do makes a certification statement about a repository, so it
/// stays unset and is absent from every response — `false` would claim the service had
/// decided the repository is not certified.
pub fn catalog_record_of(
    registry_id: &str,
    repository_name: &str,
    input: Option<&RepositoryCatalogDataInput>,
    endpoint_base: &str,
) -> CatalogRecord {
    let Some(input) = input else {
        return CatalogRecord {
            catalog_data: RepositoryCatalogData::default(),
            logo: None,
        };
    };
    let logo = logo_asset::logo_of(Some(input));
    CatalogRecord {
        catalog_data: RepositoryCatalogData {
            description: input.description.clone(),
            architectures: input.architectures.clone(),
            operating_systems: input.operating_systems.clone(),
            logo_url: logo.as_ref().map(|logo| {
                logo_url(
                    endpoint_base,
                    registry_id,
                    repository_name,
                    &logo.asset_id,
                )
            }),
            about_text: input.about_text.clone(),
            usage_text: input.usage_text.clone(),
            marketplace_certified: None,
        },
        logo,
    }
}

/// Why a request's `tags` cannot be applied to a repository.
///
/// Each variant is one of the modelled refusals every tagging operation declares, so a
/// handler maps a variant onto its own error enum rather than deciding for itself which
/// error a tag problem is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagProblem {
    /// A tag this registry cannot store as a pair at all (one carrying no key), or a tag
    /// key no caller may write (one under the namespace AWS keeps for its own tags,
    /// [`RESERVED_TAG_KEY_PREFIX`]). Answered as `InvalidTagParameterException`.
    InvalidTagParameter(String),
    /// A write that would leave the repository carrying more than
    /// [`MAX_TAGS_PER_REPOSITORY`] tags. Answered as `TooManyTagsException`.
    TooManyTags(String),
}

impl TagProblem {
    /// The text the refusal carries.
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidTagParameter(message) | Self::TooManyTags(message) => message,
        }
    }
}

/// What a caller is told about a tag key in the namespace AWS keeps for its own tags
/// ([`RESERVED_TAG_KEY_PREFIX`]): a tag under it belongs to the system rather than to the
/// caller, so neither writing one nor changing one is something an API caller may do.
///
/// The text is the service's own, fixed and request-independent: it states who may not
/// make the change rather than which key was asked for, so it has no variable part to
/// fill and never echoes the rejected key.
pub const SYSTEM_TAG_KEY_MESSAGE: &str =
    "Caller is an end user and not allowed to mutate system tags";

/// What `TooManyTagsException` states for a write that would leave a repository over the
/// tag limit ([`MAX_TAGS_PER_REPOSITORY`]).
///
/// Fixed and request-independent, like [`SYSTEM_TAG_KEY_MESSAGE`]: it states that the
/// limit was passed and names neither the limit nor how far past it the request went, so
/// there is nothing in it to fill from the request.
pub const TOO_MANY_TAGS_MESSAGE: &str = "The number of tags exceeds the limit";

/// What a tagging WRITE is told about a `resourceArn` that is not a well-formed ECR
/// Public repository ARN: the parameter-constraint form this service words every refused
/// parameter in, naming `resourceArn` and the step that could not be completed.
///
/// The tag READ words the same member's refusal as [`INVALID_ARN_MESSAGE`]: the two paths
/// state the clause differently, so neither message is shared between them.
pub const UNPARSEABLE_ARN_MESSAGE: &str =
    "Invalid parameter at 'resourceArn' failed to satisfy constraint: 'Unable to parse ARN'";

/// The tags a request supplied, keyed by key, once every one of them is a tag a caller
/// may write.
///
/// A tag is a key-value pair whose key "acts like a category" (`Tag$Key`), so a pair with
/// no key is nothing this registry can store; a key in the namespace AWS keeps for its own
/// tags ([`RESERVED_TAG_KEY_PREFIX`]) names a system tag, which an API caller may not
/// write ([`SYSTEM_TAG_KEY_MESSAGE`]). The modelled 1..128 / 0..256 lengths are already
/// refused before a handler runs. Two tags sharing a key are one tag of that key: the
/// request's last value for it is what gets stored.
pub fn stored_tags(tags: Option<&Vec<Tag>>) -> Result<BTreeMap<String, String>, TagProblem> {
    let mut stored = BTreeMap::new();
    for tag in tags.map(Vec::as_slice).unwrap_or_default() {
        let Some(key) = tag.key.as_deref().filter(|key| !key.is_empty()) else {
            return Err(TagProblem::InvalidTagParameter(
                "Invalid tag: a tag must specify a Key".to_string(),
            ));
        };
        if is_system_tag_key(key) {
            return Err(TagProblem::InvalidTagParameter(
                SYSTEM_TAG_KEY_MESSAGE.to_string(),
            ));
        }
        stored.insert(key.to_string(), tag.value.clone().unwrap_or_default());
    }
    Ok(stored)
}

/// The tag keys a request asked to have removed, deduplicated, once every one of them
/// names a tag a caller may change.
///
/// The rule is the one [`stored_tags`] applies to the keys a write supplies: a key in the
/// namespace AWS keeps for its own tags ([`RESERVED_TAG_KEY_PREFIX`]) names a system tag,
/// which an API caller may neither write nor delete ([`SYSTEM_TAG_KEY_MESSAGE`]). The
/// modelled 1..128 length is already refused before a handler runs. A key named twice in
/// one request is one key: removing a tag is settled by which keys were named, not by how
/// many times each was.
pub fn removed_tag_keys(tag_keys: &[String]) -> Result<BTreeSet<String>, TagProblem> {
    let mut keys = BTreeSet::new();
    for key in tag_keys {
        if is_system_tag_key(key) {
            return Err(TagProblem::InvalidTagParameter(
                SYSTEM_TAG_KEY_MESSAGE.to_string(),
            ));
        }
        keys.insert(key.clone());
    }
    Ok(keys)
}

/// Whether `key` names a system tag: one in the namespace AWS keeps for its own tags
/// ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// The prefix is a literal, so it is matched case-sensitively: a key spelled with any
/// other casing is an ordinary key of the caller's own.
pub fn is_system_tag_key(key: &str) -> bool {
    key.starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// One page of a listing, and the token that resumes after it.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// The page size to serve a request that asked for `max_results`, clamped to the
/// modelled `1..1000` so a caller can neither ask for an unbounded page nor for none.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

/// Page `entries` — already in the listing's stable key order — from `resume_after`
/// onwards, taking at most `page_size` of them.
///
/// The returned token names where this page ENDED, so the next call resumes at the
/// following entry rather than restarting.
pub fn page<T>(
    entries: Vec<(String, T)>,
    resume_after: Option<&str>,
    page_size: usize,
) -> Page<T> {
    let mut remaining = entries
        .into_iter()
        .filter(|(key, _)| resume_after.is_none_or(|after| key.as_str() > after));

    let mut items = Vec::new();
    let mut last_key = None;
    for (key, item) in remaining.by_ref().take(page_size) {
        items.push(item);
        last_key = Some(key);
    }

    Page {
        next_token: match (remaining.next().is_some(), last_key) {
            (true, Some(key)) => Some(encode_page_token(&key)),
            _ => None,
        },
        items,
    }
}

/// The opaque pagination token naming the last key of a page.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key.as_bytes())
}

/// The key a `nextToken` resumes after, or the `InvalidParameterException` message for a
/// token this service did not mint.
pub fn decode_page_token(token: Option<&str>) -> Result<Option<String>, String> {
    let Some(token) = token else {
        return Ok(None);
    };
    let bytes = hex::decode(token).map_err(|_| invalid_token_message())?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| invalid_token_message())
}

/// The parameter name ECR Public states for the pagination token when it refuses one.
///
/// The WIRE member is lower-camel `nextToken` (`smithy_query(query="operation",
/// operation="DescribeImageTags")` — input member `nextToken` -> `NextToken`), but the
/// member path this service quotes in an `InvalidParameterException` is its own
/// PascalCase parameter name for the token, which is the shape name `NextToken`. The
/// same service states `PolicyText` the same way (`set_repository_policy.rs`), so the
/// path a refusal quotes is the parameter name the service publishes and not a
/// mechanical copy of the JSON key.
pub const NEXT_TOKEN_PARAMETER: &str = "NextToken";

/// What to tell a caller that presented a token this service did not mint: the service's
/// `Invalid parameter at '<member>' failed to satisfy constraint: '<rule>'` template over
/// the token parameter ([`NEXT_TOKEN_PARAMETER`]) and the VERDICT on the token supplied —
/// the cursor is not one this service issued — rather than a prerequisite on how the
/// caller should have come by it. The rejected token itself is never quoted back, like
/// every other clause this service renders.
pub fn invalid_token_message() -> String {
    format!(
        "Invalid parameter at '{NEXT_TOKEN_PARAMETER}' failed to satisfy constraint: \
         'Invalid next token provided'"
    )
}

/// The parameter name ECR Public states for the page size when it refuses one, spelled the
/// way [`NEXT_TOKEN_PARAMETER`] is: the shape name `MaxResults` behind the lower-camel wire
/// member `maxResults`.
pub const MAX_RESULTS_PARAMETER: &str = "MaxResults";

/// Why a paginated reader cannot serve a request that pages AND names the resources it
/// wants, or `None` when the request asks for only one of the two.
///
/// Every paginated reader of this service whose input also takes an explicit identifier
/// list documents the two as mutually exclusive — `DescribeRepositories$maxResults` and
/// `$nextToken` each end "If you specify repositories with `repositoryNames`, you can't use
/// this option", and `DescribeImages`' pair says the same of `imageIds`
/// (`smithy_query(query="doc", shape="DescribeRepositoriesRequest",
/// member="maxResults")`). A request naming the resources it wants is not a scan to page
/// through, so the combination is refused rather than served: the service's
/// `Invalid parameter at '<member>' failed to satisfy constraint: '<rule>'` template over
/// the pagination parameter and the rule that it cannot be given beside the list, with
/// nothing from the request interpolated.
///
/// `list_parameter` is the identifier list as the service names it in that message
/// (`RepositoryNames`, `ImageIds`). The page size is judged before the token, so a request
/// carrying both is told about the one the clause names first.
pub fn pagination_beside_list_message(
    list_parameter: &str,
    names_resources: bool,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<String> {
    if !names_resources {
        return None;
    }
    let offending = if max_results.is_some() {
        MAX_RESULTS_PARAMETER
    } else if next_token.is_some() {
        NEXT_TOKEN_PARAMETER
    } else {
        return None;
    };
    Some(format!(
        "Invalid parameter at '{offending}' failed to satisfy constraint: 'Cannot provide \
         {offending} if providing {list_parameter}'"
    ))
}

/// The `RepositoryNameList` parameter as this service names it in a refusal clause.
pub const REPOSITORY_NAMES_PARAMETER: &str = "RepositoryNames";

/// The `ImageIdentifierList` parameter as this service names it in a refusal clause.
pub const IMAGE_IDS_PARAMETER: &str = "ImageIds";
