// SPDX-License-Identifier: CC0-1.0

//! The mapping between the message-body shapes of `VoiceMessageContent` and their stored
//! records in [`crate::state`].
//!
//! RULE: a stored body record is the member-for-member image of the wire shape it was
//! built from, and that mapping is written ONCE for each set of members — so a member
//! added to those shapes, or a renamed state field, is carried by every body that shares
//! those members or by none; it cannot be carried by one and dropped by another.
//!
//! `PlainTextMessageType` and `SSMLMessageType` declare the SAME three members
//! (`smithy_query(query="shape",shape="CallInstructionsMessageType,PlainTextMessageType,SSMLMessageType",model_path="services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json")`)
//! and are stored in the one record type [`crate::state::VoiceTextRecord`], so those
//! three members are named once and `spoken_body_record!` expands that one body into the
//! `From` impl each of the two shapes needs. `CallInstructionsMessageType` declares only
//! `Text` and has a record of its own, so it carries its own impl — nothing is shared
//! between a one-member shape and a three-member one.
//!
//! These are `From` impls rather than functions, so the handler keeps its own shape and
//! only the per-member mapping is shared: it hands each body it already holds to
//! `.into()`. Nothing here decides WHETHER a body is written, which stays the handler's
//! own decision, and an absent member stays absent.
//!
//! Not a modelled shape, so this module carries no `_types` suffix and is not
//! glob-re-exported; trait impls need no import to be in scope.

use super::{CallInstructionsMessageType, PlainTextMessageType, SSMLMessageType};
use crate::state::{CallInstructionsRecord, VoiceTextRecord};

/// Map each listed spoken-message shape onto [`VoiceTextRecord`], member for member.
///
/// The three members those shapes share are named ONCE, in the body this expands per
/// shape: a member added to the record and to the shapes is carried by every one of them,
/// and a renamed field stops compiling here rather than in one copy of a mapping that was
/// written twice.
macro_rules! spoken_body_record {
    ($($shape:ty),+ $(,)?) => {
        $(
            impl From<$shape> for VoiceTextRecord {
                fn from(body: $shape) -> Self {
                    Self {
                        text: body.text,
                        language_code: body.language_code,
                        voice_id: body.voice_id,
                    }
                }
            }
        )+
    };
}

spoken_body_record!(PlainTextMessageType, SSMLMessageType);

impl From<CallInstructionsMessageType> for CallInstructionsRecord {
    fn from(body: CallInstructionsMessageType) -> Self {
        Self { text: body.text }
    }
}
