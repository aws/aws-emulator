// SPDX-License-Identifier: CC0-1.0

//! The `SafetyLever` every safety-lever operation reports, as AWS carries it on the wire:
//! the three members the published model snapshot declares, plus the one member such a
//! report carries that the snapshot does not declare — the account the lever belongs to.
//!
//! The snapshot in this crate's `smithy-model/` is kept EXACTLY as shipped, so this shape
//! is declared HERE rather than beside the other shapes several operations reach —
//! generated, like every modelled shape in this crate, from the small overlay model
//! `smithy-model/local-additions.json`, which restates the shape with that member beside
//! the three the snapshot declares, each against the snapshot's own target, so every
//! length and pattern constraint is captured exactly as the snapshot spells it (see
//! `smithy-model/LOCAL-ADDITIONS.md`). Nothing about the wire is hand-written: the member
//! names, the shape id and the member targets all come from a model file.
//!
//! What the member CARRIES is not decided here:
//! [`crate::state::SafetyLeverRecord::to_safety_lever`] fills it in with the account this
//! emulator serves, the one account whose lever these operations report.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// A safety lever: its id, its ARN, the state it is held in and the account it belongs to.
#[smithy_macros::smithy(shape = "com.amazonaws.fis#SafetyLever")]
#[derive(Debug, Clone, Default)]
pub struct SafetyLever {
    pub id: Option<String>,
    pub arn: Option<String>,
    pub state: Option<SafetyLeverState>,
    pub account_id: Option<String>,
}
