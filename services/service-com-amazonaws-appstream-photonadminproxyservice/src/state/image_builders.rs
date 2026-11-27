// SPDX-License-Identifier: CC0-1.0

//! The image builders this service's store holds, the record form they are written in, and
//! the rules every write of one applies.
//!
//! An image builder is the virtual machine an account installs applications on before it
//! snapshots an image from it, so the resource has an instance lifecycle of its own:
//! `CreateImageBuilder` writes one [`ImageBuilderRecord`] whose state is `PENDING` and
//! records the instant from which `RUNNING` is due ([`super::PendingState`]),
//! `StopImageBuilder` moves it to `STOPPING` and records the instant from which `STOPPED`
//! is due, and `DescribeImageBuilders` materialises whatever the clock has already made due
//! before it reports
//! ([`super::ComAmazonawsAppstreamPhotonadminproxyserviceState::settle_image_builder_states`]).
//!
//! That state is also the gate `UpdateImageBuilder` applies: its own documentation states
//! that "the image builder must be in the STOPPED state", which is read here
//! ([`image_builder_accepts_update`]) beside the sentence a write outside it is refused
//! with, so the operation asks one question of one place.
//!
//! Clearing an attribute lives here too: `AttributesToDelete` names the attributes of a
//! builder the modelled `ImageBuilderAttribute` enum declares, and which stored field each
//! of those names is stated once ([`ImageBuilderRecord::delete_attribute`]), so a write and
//! a read cannot disagree about what a cleared attribute leaves behind.
//!
//! The record holds the MODELLED sub-shapes (`VpcConfig`, `VolumeConfig`,
//! `ImageBuilderAgentAccessConfig`, …) rather than copies of their members, so what a
//! Describe returns is literally what the create persisted.

use std::collections::BTreeMap;

use crate::ops::types::{
    AccessEndpoint, DomainJoinInfo, ImageBuilder, ImageBuilderAgentAccessConfig,
    ImageBuilderStateChangeReason, NetworkAccessConfiguration, ResourceError, VolumeConfig,
    VpcConfig,
};

use super::{reported, PendingState};

/// The words AWS's refusal sentences use for an image builder, the `<kind>` slot of
/// [`super::resource_not_found_message`] and [`super::resource_already_exists_message`] —
/// the resource word in prose, unlike the hyphenated `resource-type` segment its ARN
/// carries ([`super::IMAGE_BUILDER_ARN_RESOURCE_TYPE`]).
pub const IMAGE_BUILDER_KIND: &str = "image builder";

/// The `ImageBuilderState` of a builder whose instance is being brought up, one of
/// `IMAGE_BUILDER_STATE_VALUES` (`ops/types/shared_types.rs`). `CreateImageBuilder`'s own
/// documentation states it: "The initial state of the builder is PENDING. When it is ready,
/// the state is RUNNING."
pub const IMAGE_BUILDER_STATE_PENDING: &str = "PENDING";

/// The `ImageBuilderState` of a builder whose instance is up — the state a create and a
/// start settle it in.
pub const IMAGE_BUILDER_STATE_RUNNING: &str = "RUNNING";

/// The `ImageBuilderState` of a builder whose instance is being terminated.
pub const IMAGE_BUILDER_STATE_STOPPING: &str = "STOPPING";

/// The `ImageBuilderState` of a builder that has no instance — the state a stop settles it
/// in, and the one state `UpdateImageBuilder` admits a write in.
pub const IMAGE_BUILDER_STATE_STOPPED: &str = "STOPPED";

/// How long a created or started image builder takes to have its instance up, and a
/// stopped one to have it gone: the span this emulator gives a transition the clock owes,
/// which is never zero, so a caller acting straight after the call still sees the transient
/// state. AWS documents no duration for either transition.
pub const IMAGE_BUILDER_STATE_SPAN: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The records one `DescribeImageBuilders` page carries when the request asks for no
/// `MaxResults`, read the way every other paged read of this service reads its own ceiling
/// ([`super::page_size`]).
pub const IMAGE_BUILDER_PAGE_SIZE: usize = 25;

/// The `ImageBuilderAttribute` naming the agent access configuration of a builder, the one
/// value `IMAGE_BUILDER_ATTRIBUTE_VALUES` (`ops/types/shared_types.rs`) declares — the
/// attributes `UpdateImageBuilder.AttributesToDelete` can remove from a builder.
pub const IMAGE_BUILDER_ATTRIBUTE_AGENT_ACCESS_CONFIG: &str = "AGENT_ACCESS_CONFIG";

/// Whether `attribute` is an image builder attribute this service's MODEL declares, which
/// is what [`ImageBuilderRecord::delete_attribute`] must carry a field for.
fn modelled_attribute(attribute: &str) -> bool {
    crate::ops::types::IMAGE_BUILDER_ATTRIBUTE_VALUES.contains(&attribute)
}

/// Whether a builder in `state` admits a write of its fields: `UpdateImageBuilder`'s own
/// documentation states the rule — "the image builder must be in the STOPPED state" — so
/// every other state, including the ones a create, a start or a stop passes through,
/// admits none.
pub fn image_builder_accepts_update(state: &str) -> bool {
    state == IMAGE_BUILDER_STATE_STOPPED
}

/// AWS's `OperationNotPermittedException` sentence for a write of a builder whose current
/// state does not admit one.
///
/// The fixed text is AWS's; the variable parts are the builder's name, as the REQUEST
/// addressed it, and the state the stored builder is in right now.
pub fn image_builder_not_stopped_message(name: &str, state: &str) -> String {
    format!("Cannot update the attributes for ImageBuilder {name} when it is in {state} state. ")
}

/// AWS's `InvalidParameterCombinationException` sentence for a `CreateImageBuilder` that
/// names neither of the two members that select the image to launch.
///
/// Fixed text: the sentence names the members, which are the same two for every such
/// request.
pub fn image_selector_required_message() -> String {
    "Either ImageName or ImageArn must be specified.".to_string()
}

/// AWS's `InvalidParameterCombinationException` sentence for a `CreateImageBuilder` that
/// names BOTH members that select the image to launch: a builder is launched from one
/// image, addressed by name or by ARN, so the two together ask for two selections. The
/// reading is the one `DescribeImages` applies to the same pair of members.
pub fn image_selector_combination_message() -> String {
    "You must specify either the image name or the image ARN, not both.".to_string()
}

/// The version string `CreateImageBuilder.AppstreamAgentVersion` documents for "the latest
/// version of the WorkSpaces Applications agent" (`smithy_query(query="doc",
/// shape="CreateImageBuilderRequest", member="AppstreamAgentVersion")`: "To use the latest
/// version of the WorkSpaces Applications agent, specify [LATEST]").
pub const APPSTREAM_AGENT_VERSION_LATEST: &str = "[LATEST]";

/// The `LatestAppstreamAgentVersion` of a resource created with `requested` as its agent
/// version — `TRUE` for one that takes whatever the latest version is (a request that
/// names no version at all, or names [`APPSTREAM_AGENT_VERSION_LATEST`]) and `FALSE` for
/// one pinned to a version of its own, which is what the member reports: "indicates
/// whether the image builder is using the latest WorkSpaces Applications agent version or
/// not".
///
/// The values are `LatestAppstreamAgentVersion`'s
/// (`LATEST_APPSTREAM_AGENT_VERSION_VALUES` in `ops/types/shared_types.rs`).
pub fn latest_appstream_agent_version(requested: Option<&str>) -> &'static str {
    match requested {
        None => LATEST_APPSTREAM_AGENT_VERSION_TRUE,
        Some(version) if version == APPSTREAM_AGENT_VERSION_LATEST => {
            LATEST_APPSTREAM_AGENT_VERSION_TRUE
        }
        Some(_) => super::LATEST_APPSTREAM_AGENT_VERSION_FALSE,
    }
}

/// The `LatestAppstreamAgentVersion` of a resource that takes whatever the latest agent
/// version is, the other value of that enumerated type beside the `FALSE` the image
/// catalog states ([`super::LATEST_APPSTREAM_AGENT_VERSION_FALSE`]).
pub const LATEST_APPSTREAM_AGENT_VERSION_TRUE: &str = "TRUE";

/// A persisted AppStream image builder.
#[derive(Debug, Clone)]
pub struct ImageBuilderRecord {
    pub arn: String,
    pub name: String,
    /// The ARN of the image the builder was launched from, RESOLVED against this store's
    /// image catalog by the create: the request may select the image by name, and the
    /// `ImageBuilder` shape reports only the ARN, so the create stores the ARN of the image
    /// it found rather than echoing what the request wrote.
    pub image_arn: Option<String>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    /// The MODELLED sub-shape rather than a copy of its members, so what a Describe returns
    /// is literally what the create persisted.
    pub vpc_config: Option<VpcConfig>,
    pub instance_type: String,
    /// One of `PLATFORM_TYPE_VALUES`, taken from the image the builder was launched from:
    /// the operating system a builder runs is the one its image carries, so it is read off
    /// that image rather than from the request, which models no platform at all.
    pub platform: Option<String>,
    pub iam_role_arn: Option<String>,
    /// One of `IMAGE_BUILDER_STATE_VALUES`; [`IMAGE_BUILDER_STATE_PENDING`] for a freshly
    /// created builder.
    pub state: String,
    /// The state the clock owes this builder, and the instant from which it is owed: what
    /// the create and `StopImageBuilder` record so that a builder reports `PENDING` until
    /// its instance is up and `STOPPING` until it is gone, without anything sleeping for
    /// the span.
    pub pending_state: Option<PendingState>,
    /// Why the builder last changed state; `None` until something changed it.
    pub state_change_reason: Option<ImageBuilderStateChangeReason>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub enable_default_internet_access: Option<bool>,
    pub domain_join_info: Option<DomainJoinInfo>,
    /// The elastic network interface the builder's instance is reached through. Held so a
    /// read reports what the record holds; nothing in this emulator provisions an
    /// interface, so a builder created here holds none.
    pub network_access_configuration: Option<NetworkAccessConfiguration>,
    /// The errors that stopped the builder reaching its state; empty while none has
    /// occurred.
    pub image_builder_errors: Vec<ResourceError>,
    pub appstream_agent_version: Option<String>,
    pub access_endpoints: Vec<AccessEndpoint>,
    pub root_volume_config: Option<VolumeConfig>,
    /// One of `LATEST_APPSTREAM_AGENT_VERSION_VALUES`, decided by the create from the agent
    /// version the request asked for.
    pub latest_appstream_agent_version: Option<String>,
    pub disable_imdsv1: Option<bool>,
    pub agent_access_config: Option<ImageBuilderAgentAccessConfig>,
    pub tags: BTreeMap<String, String>,
}

impl ImageBuilderRecord {
    /// Apply the transition the clock owes this builder, if `now` is past its due instant.
    ///
    /// This materialises a change the CLOCK already made due — the builder would have
    /// reached the state at that instant even if nobody had called — so a read may apply
    /// it. Nothing else about the builder changes, and once applied there is nothing owed.
    pub fn settle_state(&mut self, now: ::std::time::Instant) {
        let Some(pending) = self.pending_state else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.state = pending.state.to_string();
        self.pending_state = None;
    }

    /// Whether this builder has no instance, which is the state a stop settles it in and
    /// the only state a write of its fields is admitted in.
    pub fn is_stopped(&self) -> bool {
        self.state == IMAGE_BUILDER_STATE_STOPPED
    }

    /// The state the clock owes this builder, when one is owed: what tells a builder on its
    /// way to one state from a builder that is already there, so a repeated stop asks for
    /// nothing new.
    pub fn owed_state(&self) -> Option<&'static str> {
        self.pending_state.map(|pending| pending.state)
    }

    /// Drop the builder's value for one of the attributes the modelled
    /// `ImageBuilderAttribute` enum names — what `UpdateImageBuilder.AttributesToDelete`
    /// asks for. An attribute outside that set names no stored value, so there is nothing
    /// to drop.
    ///
    /// A cleared attribute leaves the record without a value for it, which is what makes
    /// its member absent from the rendered `ImageBuilder`.
    pub fn delete_attribute(&mut self, attribute: &str) {
        match attribute {
            IMAGE_BUILDER_ATTRIBUTE_AGENT_ACCESS_CONFIG => self.agent_access_config = None,
            other => debug_assert!(
                !modelled_attribute(other),
                "no field carries the modelled image builder attribute {other}"
            ),
        }
    }

    /// Render the stored record as the modelled `ImageBuilder` every reader returns.
    pub fn to_image_builder(&self) -> ImageBuilder {
        ImageBuilder {
            name: self.name.clone(),
            arn: Some(self.arn.clone()),
            image_arn: self.image_arn.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            vpc_config: self.vpc_config.clone(),
            instance_type: Some(self.instance_type.clone()),
            platform: self.platform.clone(),
            iam_role_arn: self.iam_role_arn.clone(),
            state: Some(self.state.clone()),
            state_change_reason: self.state_change_reason.clone(),
            created_time: Some(self.created_time),
            enable_default_internet_access: self.enable_default_internet_access,
            domain_join_info: self.domain_join_info.clone(),
            network_access_configuration: self.network_access_configuration.clone(),
            // A builder with no errors reports none: the member describes something that
            // happened, so until it has, it is ABSENT rather than present-and-empty — the
            // representation every other collection of this store uses.
            image_builder_errors: reported(&self.image_builder_errors),
            appstream_agent_version: self.appstream_agent_version.clone(),
            access_endpoints: reported(&self.access_endpoints),
            root_volume_config: self.root_volume_config.clone(),
            latest_appstream_agent_version: self.latest_appstream_agent_version.clone(),
            disable_imdsv1: self.disable_imdsv1,
            agent_access_config: self.agent_access_config.clone(),
        }
    }
}
