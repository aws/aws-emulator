// SPDX-License-Identifier: CC0-1.0

//! The IN-USE slice of this service's state: the one rule every operation that
//! would take a resource out from under another resource — or hand a resource to
//! a second holder — resolves its refusal through.
//!
//! MediaLive's channel / input / input-security-group graph is held by exactly
//! two derived "currently using" lists, both already owned by the state module:
//!
//! * an input's `AttachedChannels` — "A list of channel IDs that that input is
//!   attached to"
//!   ([`super::ComAmazonawsMedialiveMedialiveState::attached_channels`]), and
//! * an input security group's `Inputs` — "The list of inputs currently using
//!   this Input Security Group"
//!   ([`super::ComAmazonawsMedialiveMedialiveState::inputs_using_security_group`]).
//!
//! A write that would break one of those relationships is refused while the
//! relationship holds, and the refusal is derived from the SAME list the read
//! side reports, so a caller can always see why it was refused:
//!
//! * an input is attached to at most ONE channel at a time, so a channel write
//!   whose `InputAttachments` names an input another channel already attaches is
//!   refused ([`ComAmazonawsMedialiveMedialiveState::channel_attaching_input`],
//!   [`input_already_attached_message`]) — the condition the channel writes'
//!   modelled `ConflictException` (409) answers;
//! * an input security group is not deleted while inputs still use it, so the
//!   group cannot go away underneath an input that admits its pushed stream
//!   through it ([`input_security_group_in_use_message`], refused on the
//!   `Inputs` list
//!   [`super::ComAmazonawsMedialiveMedialiveState::inputs_using_security_group`]
//!   already derives for every read of the group).
//!
//! Both halves live in one module because they are one notion — "this resource
//! is in use, so that write is refused" — and because each is shared by more
//! than one operation: the attachment conflict by `CreateChannel` and
//! `UpdateChannel`, the group precondition by `DeleteInputSecurityGroup` and the
//! per-resource delete of `BatchDelete`. Neither is a store of its own: both are
//! derived from `channels` and `inputs` at the moment of the write, so they
//! track every create and delete that has happened since.

use super::ComAmazonawsMedialiveMedialiveState;
use crate::ops::types::InputAttachment;

impl ComAmazonawsMedialiveMedialiveState {
    /// The id of the channel that already attaches `input_id`, ignoring
    /// `written_channel` — the channel the write in progress is addressed to,
    /// which may of course go on attaching the inputs it already holds.
    ///
    /// Derived from [`Self::attached_channels`], the same list `DescribeInput`
    /// reports, so a channel whose delete this service has accepted attaches
    /// nothing any more: once the channel holding an input is deleted the input
    /// is free for the next channel, and `DeleteInput` is no longer refused for
    /// it.
    pub fn channel_attaching_input(
        &self,
        input_id: &str,
        written_channel: Option<&str>,
    ) -> Option<String> {
        self.attached_channels(input_id)
            .into_iter()
            .find(|channel| Some(channel.as_str()) != written_channel)
    }

    /// The first input one of `attachments` names that another channel already
    /// attaches, or `None` when the configuration attaches only free inputs.
    ///
    /// The entries are walked in the order the request declares them, so the
    /// refusal names the first input the caller would have double-booked.
    /// `written_channel` is the channel being written (`None` on a create, which
    /// holds nothing yet), so an update that re-declares the attachments its own
    /// channel already has is never refused for them.
    pub fn double_booked_input(
        &self,
        attachments: &[InputAttachment],
        written_channel: Option<&str>,
    ) -> Option<String> {
        attachments
            .iter()
            .filter_map(|attachment| attachment.input_id.as_deref())
            .find(|input_id| {
                self.channel_attaching_input(input_id, written_channel)
                    .is_some()
            })
            .map(str::to_string)
    }
}

/// The message a channel write refused because one of its attachments names an
/// input another channel already holds carries: the `InputAttachment$InputId`
/// member as the wire spells it, then the id the REQUEST carried.
///
/// The only value interpolated is the caller's own input id — which channel
/// holds the input is state the caller did not supply, and `DescribeInput`'s
/// `AttachedChannels` is what reports it.
///
/// ONE helper, so `CreateChannel` and `UpdateChannel` — which refuse the same
/// double booking — cannot word the refusal two ways.
pub fn input_already_attached_message(input_id: &str) -> String {
    format!("inputId={input_id} is already attached")
}

/// The message a group delete refused because inputs still use the group
/// carries: the resource as the API names it, the id the REQUEST carried in the
/// path, and that inputs are still using it — so a client tearing a topology
/// down learns it must delete those inputs first.
///
/// Which inputs they are is not interpolated: they are state the caller did not
/// supply, and `DescribeInputSecurityGroup`'s `Inputs` member is what reports
/// them.
pub fn input_security_group_in_use_message(group_id: &str) -> String {
    format!(
        "Input Security Group {group_id} cannot be deleted because it is still being used by inputs."
    )
}
