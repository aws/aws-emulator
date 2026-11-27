// SPDX-License-Identifier: CC0-1.0

//! The shapes `get_stream_group` reaches, as the published model declares them.
//!
//! Every shape the operation reaches except its output, which this service answers with
//! the group-level capacity summary the published model does not declare and which is
//! therefore declared in [`super::overlay`].
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json");

#[smithy_macros::smithy(shape = "com.amazonaws.gameliftstreams#GetStreamGroupInput")]
#[derive(Debug, Clone, Default)]
pub struct GetStreamGroupInput {
    pub identifier: String,
}
