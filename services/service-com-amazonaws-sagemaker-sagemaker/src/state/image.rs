// SPDX-License-Identifier: CC0-1.0

//! The custom-image slice of this service's state: the [`Image`] and [`ImageVersion`]
//! records `CreateImage` / `CreateImageVersion` write, the bounds and statuses they
//! take, the sentences a missing one is refused with, and the clock-owed settle every
//! reader materialises.

use super::*;

/// How long after `CreateImage` / `CreateImageVersion` the clock owes the
/// `CREATING` -> `CREATED` transition.
///
/// SageMaker AI returns a freshly created image and image version in `CREATING` and
/// moves it to `CREATED` once the registry path is resolved, so a caller that describes
/// straight after creating still observes `CREATING` and one that waits observes
/// `CREATED`. Real SageMaker takes seconds; the span is compressed, never zero.
pub const IMAGE_CREATED_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`].
pub const IMAGE_QUOTA: usize = 1_000;

/// See [`MODEL_QUOTA`]: the bound is per image, since versions are numbered within one
/// image and `CreateImageVersion` is the only call that adds one.
pub const IMAGE_VERSION_QUOTA: usize = 1_000;

/// The most aliases one image version carries, after which `CreateImageVersion` is
/// refused with the modelled `ResourceLimitExceeded`.
///
/// `SageMakerImageVersionAliases` is modelled with no length, so nothing at the request
/// boundary bounds the list: without this the request would choose both how much is
/// stored per version and how much work the alias-collision check does. It is the
/// emulator's OWN bound rather than a claim about any account's limit, far above any
/// workflow an emulator serves (a version is normally tagged with a handful of aliases).
pub const MAX_ALIASES_PER_IMAGE_VERSION: usize = 50;

/// The `ImageStatus` / `ImageVersionStatus` members this emulator moves an image and an
/// image version through. Both modelled enums spell the creating and created states the
/// same way (`CREATING`, `CREATED`) and also carry the failure, update and delete states,
/// of which this slice reaches the IMAGE's `CREATE_FAILED` — what an image whose creation
/// resolved something that did not hold up settles into. Named once here so no handler
/// compares a bare string literal.
pub const IMAGE_STATUS_CREATING: &str = "CREATING";
/// See [`IMAGE_STATUS_CREATING`].
pub const IMAGE_STATUS_CREATED: &str = "CREATED";
/// See [`IMAGE_STATUS_CREATING`].
pub const IMAGE_STATUS_CREATE_FAILED: &str = "CREATE_FAILED";

/// `VendorGuidance`'s member for a version whose maintainer named no stability, which
/// the model documents as "The maintainers did not provide a status for image version
/// stability" — so it is what a version created without `VendorGuidance` reports.
pub const VENDOR_GUIDANCE_NOT_PROVIDED: &str = "NOT_PROVIDED";

/// The ARN resource kind an image carries, i.e. the `<kind>` in [`resource_arn`], as the
/// modelled `ImageArn` pattern spells it
/// (`^arn:aws(-[\w]+)*:sagemaker:.+:[0-9]{12}:image/[a-zA-Z0-9]([-.]?[a-zA-Z0-9])*$`).
/// Named once here so the creates mint, the readers resolve and the refusals match the
/// one kind.
pub const IMAGE_ARN_KIND: &str = "image";

/// The message a `ResourceNotFound` carries for an image no store holds: the image's ARN
/// inside the service's sentence, built from the request's `ImageName`.
///
/// Stated once so every refusal that reports a MISSING IMAGE words it the same way —
/// `DescribeImage`, `DeleteImage` and the `CreateImageVersion` / `DescribeImageVersion`
/// that can only resolve a version of an image that exists.
pub fn image_does_not_exist(image_arn: &str) -> String {
    format!("Image with ARN {image_arn} does not exist")
}

/// The message a `ResourceNotFound` carries for an image version the request identifies
/// by its number but no store holds: that version's own ARN inside the sentence this
/// service refuses a missing resource with, the way [`image_does_not_exist`] carries the
/// image's.
///
/// Stated once so every refusal that reports a MISSING IMAGE VERSION words it the same
/// way: the `DescribeImageVersion` reader and the `DeleteImageVersion` that resolves the
/// same `Version`.
pub fn image_version_does_not_exist(version_arn: &str) -> String {
    format!("ImageVersion with ARN {version_arn} does not exist")
}

/// The message a `ResourceNotFound` carries when the image IS stored and the request's
/// selection determines no version ARN — an alias attached to none of its versions, or a
/// request for the latest version of an image that holds none: what could not be resolved
/// beside the image's ARN.
///
/// Stated once so `DescribeImageVersion`, whose alias and latest-version selections this
/// serves, and the `DeleteImageVersion` that resolves the same alias word it the same way.
pub fn image_version_not_resolved(image_arn: &str, selector: &str) -> String {
    format!("Could not find {selector} of image \"{image_arn}\".")
}

/// The ARN a version of `image_name` numbered `version` carries in `region`.
///
/// The modelled `ImageVersionArn` pattern spells the image name in lowercase
/// (`image-version/[a-z0-9]([-.]?[a-z0-9])*/[0-9]+`) where the image ARN's keeps the
/// name's own case, so the name is lowercased for this ARN only. Stated once so the
/// create that mints a version and the readers and the delete that name one in a refusal
/// build the same ARN.
pub fn image_version_arn(region: &str, image_name: &str, version: i32) -> String {
    resource_arn(
        region,
        "image-version",
        &format!("{}/{version}", image_name.to_lowercase()),
    )
}

/// A custom SageMaker AI image created by `CreateImage`, as `DescribeImage` replays it
/// and `CreateImageVersion` reads it.
///
/// `created_at` is the instant the clock owes this image its settled status;
/// it is stored, never slept on, and materialised by whichever reporting call first
/// runs at or after it. `creation_failure_reason` is why that settled status is the
/// modelled failure rather than `CREATED`: something the image needs was resolved by the
/// create and did not hold up, so the image was accepted and never becomes usable.
/// `next_version` is the number the NEXT version of this image
/// takes: it only ever increases, so a version number — which an image version's ARN
/// carries — is never handed to two different container images.
#[derive(Debug, Clone)]
pub struct Image {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub role_arn: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
    pub next_version: i32,
}

/// A version of an image created by `CreateImageVersion`, as `DescribeImageVersion`
/// replays it.
///
/// `created_at` carries the same owed transition as [`Image::created_at`].
/// `client_token` is the request's `@idempotencyToken`, kept so a retried create
/// answers with the version the first call minted instead of minting a second one.
#[derive(Debug, Clone)]
pub struct ImageVersion {
    pub image_name: String,
    pub image_arn: String,
    pub version: i32,
    pub arn: String,
    pub base_image: String,
    pub client_token: String,
    pub aliases: Vec<String>,
    pub vendor_guidance: String,
    pub job_type: Option<String>,
    pub ml_framework: Option<String>,
    pub programming_lang: Option<String>,
    pub processor: Option<String>,
    pub horovod: Option<bool>,
    pub release_notes: Option<String>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
}

/// Materialise the `CREATING` -> `CREATED` transition the clock already owes to the
/// record whose `status` is `*status` and whose transition was due at `created_at`.
///
/// This is the one write a reporting operation may make: the transition was due
/// whether or not anybody described the resource, so the observer makes no difference
/// to the result. `LastModifiedTime` moves to the instant the transition was due, not
/// to "now", so the value is stable across later reads.
///
/// Answers whether the transition happened on this call, so the caller can record it.
pub(super) fn settle_created(
    status: &mut String,
    last_modified_time: &mut SystemTime,
    created_at: SystemTime,
    now: SystemTime,
) -> bool {
    if status == IMAGE_STATUS_CREATING && now >= created_at {
        *status = IMAGE_STATUS_CREATED.to_string();
        *last_modified_time = created_at;
        return true;
    }
    false
}

impl Image {
    /// Materialise the settled status the clock already owes this image: `CREATED` when
    /// everything the create resolved held up, and the modelled `CREATE_FAILED` carrying
    /// [`Image::creation_failure_reason`] when something did not — a create SageMaker
    /// accepts and then fails, which is terminal, so the image never becomes usable.
    ///
    /// See [`settle_created`] for why a reporting operation may make this write, and for
    /// where `LastModifiedTime` lands.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if !settle_created(
            &mut self.status,
            &mut self.last_modified_time,
            self.created_at,
            now,
        ) {
            return false;
        }
        if let Some(reason) = self.creation_failure_reason.clone() {
            self.status = IMAGE_STATUS_CREATE_FAILED.to_string();
            self.failure_reason = Some(reason);
        }
        true
    }

    /// Whether a `CreateImage` carrying these members states exactly the image this record
    /// already holds — i.e. whether that create is a REPEAT of the one that registered it.
    ///
    /// Every member a create can state takes part: the description, the display name, the
    /// role the image is built under, and the tag set. The tags are compared as the SET
    /// they are ("Tag keys must be unique per resource", `AddTags`), so the same pairs in
    /// another order are the same request, and a create that states no tags at all is the
    /// same as one that states an empty list. A member stating anything else describes a
    /// DIFFERENT image, and the caller is told the name is taken rather than handed this
    /// image's ARN for a configuration it did not ask for.
    pub fn was_created_by(
        &self,
        description: Option<&str>,
        display_name: Option<&str>,
        role_arn: &str,
        tags: &[Tag],
    ) -> bool {
        self.description.as_deref() == description
            && self.display_name.as_deref() == display_name
            && self.role_arn == role_arn
            && tag_set(&self.tags) == tag_set(tags)
    }
}

/// A tag list as the SET of (key, value) pairs it states, so two lists holding the same
/// pairs in different orders compare equal.
fn tag_set(tags: &[Tag]) -> BTreeMap<&str, &str> {
    tags.iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect()
}

impl ImageVersion {
    /// See [`settle_created`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_created(
            &mut self.status,
            &mut self.last_modified_time,
            self.created_at,
            now,
        )
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another image fits, i.e. the store is below [`IMAGE_QUOTA`].
    pub fn images_below_quota(&self) -> bool {
        self.images.len() < IMAGE_QUOTA
    }

    /// Every stored version of `image_name`, in ascending version order.
    pub fn versions_of<'store>(
        &'store self,
        image_name: &str,
    ) -> impl DoubleEndedIterator<Item = &'store ImageVersion> + 'store {
        self.image_versions
            .range((image_name.to_string(), i32::MIN)..=(image_name.to_string(), i32::MAX))
            .map(|(_, version)| version)
    }

    /// Whether another version of `image_name` fits, i.e. the image holds fewer than
    /// [`IMAGE_VERSION_QUOTA`] of them.
    pub fn image_versions_below_quota(&self, image_name: &str) -> bool {
        self.versions_of(image_name).count() < IMAGE_VERSION_QUOTA
    }

    /// The highest-numbered stored version of `image_name` — what
    /// `DescribeImageVersion` describes when the request names neither a version nor an
    /// alias.
    pub fn latest_version_of(&self, image_name: &str) -> Option<&ImageVersion> {
        self.versions_of(image_name).next_back()
    }

    /// Release every stored version of `image_name`, answering how many were released.
    ///
    /// `DeleteImage` deletes an image AND all versions of it (its own model doc), so the
    /// image and its versions leave the store in one call: a version left behind would be
    /// replayed by a later reader for an image no store holds any more, and a later
    /// `CreateImage` under the same name would inherit it.
    pub fn remove_versions_of(&mut self, image_name: &str) -> usize {
        let keys: Vec<(String, i32)> = self
            .versions_of(image_name)
            .map(|version| (version.image_name.clone(), version.version))
            .collect();
        let removed = keys.len();
        for key in keys {
            self.image_versions.remove(&key);
        }
        removed
    }

    /// The ARN of a Studio app launched on the image `image_name` names — its
    /// `ResourceSpec` names either the image itself or one of the image's versions — or
    /// `None` when no stored app does.
    ///
    /// An app keeps the spec it launched with (`CreateApp` stores it verbatim), so an app
    /// in the store is one still running on that image: releasing the image would leave
    /// the app naming an image nothing holds.
    pub fn app_running_on_image(&self, image_name: &str) -> Option<&str> {
        let image_arn = self.images.get(image_name).map(|image| image.arn.as_str())?;
        self.apps
            .values()
            .find_map(|app| {
                let spec = app.resource_spec.as_ref()?;
                (spec.sage_maker_image_arn.as_deref() == Some(image_arn))
                    .then_some(app.arn.as_str())
            })
            .or_else(|| {
                self.versions_of(image_name)
                    .find_map(|version| self.app_running_on_image_version(&version.arn))
            })
    }

    /// The ARN of a Studio app launched on the image VERSION whose ARN is `version_arn`,
    /// or `None` when no stored app names it.
    ///
    /// An app keeps the spec it launched with (`CreateApp` stores it verbatim), so an app
    /// in the store is one still running on that version: releasing the version would
    /// leave the app naming a version nothing holds. This is what `DeleteImageVersion`
    /// refuses on, and the version half of what [`Self::app_running_on_image`] answers.
    pub fn app_running_on_image_version(&self, version_arn: &str) -> Option<&str> {
        self.apps.values().find_map(|app| {
            let spec = app.resource_spec.as_ref()?;
            (spec.sage_maker_image_version_arn.as_deref() == Some(version_arn))
                .then_some(app.arn.as_str())
        })
    }
}
