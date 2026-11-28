// SPDX-License-Identifier: CC0-1.0

//! The permit shapes reached by more than one of this service's hand-authored
//! operations — `CreateSupportPermit`, `DeleteSupportPermit`, `GetSupportPermit`
//! and `ListSupportPermits` — plus the one prelude structure two of them carry.
//!
//! Hand-authored, because the scaffold op-stub generator declines those four
//! operations and renders none of the shapes only they reach (see
//! `src/ops/SKIPPED_STUBS.md`). Everything here is still rendered from a model
//! file by `#[smithy_macros::smithy]`, exactly as the scaffold renders the shapes
//! in this directory: the member names, the shape ids, the member order and the
//! constraints all come from a model, and no schema or codec is written out here.
//!
//! Two declarations name the OVERLAY model `smithy-model/local-additions.json`
//! rather than the snapshot: `ActionSet` and `ResourceSet` each carry an "all"
//! member targeting the Smithy prelude shape `smithy.api#Unit`, which the
//! snapshot's JSON AST does not serialize, so codegen can classify neither union
//! from the snapshot alone. `smithy-model/LOCAL-ADDITIONS.md` records what that
//! overlay holds and why, and `tests/all/local_model_overlay.rs` pins it against
//! the snapshot.
//!
//! This module is declared from `src/ops/types_local/mod.rs`, not from the
//! `@generated` `types/mod.rs`, which the model update rewrites: see that file.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-supportauthz-supportauthz.json");

/// `smithy.api#Unit`: the structure with no members, which on the wire is the
/// empty object the "all" member of a permit's two unions carries
/// (`{"allActions": {}}`).
#[smithy_macros::smithy(
    shape = "smithy.api#Unit",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnitStructure;

/// `ActionSet`: "The set of actions authorized by a permit. Specify either all
/// actions or a list of specific actions."
#[smithy_macros::smithy(
    shape = "com.amazonaws.supportauthz#ActionSet",
    model = "smithy-model/local-additions.json",
    rename = ["AllActions=allActions", "Actions=actions"]
)]
#[derive(Debug, Clone)]
pub enum ActionSet {
    /// Every available support action — the arm whose target is `smithy.api#Unit`.
    AllActions(UnitStructure),
    /// The listed support actions, at most ten by the model's own bound.
    Actions(Vec<String>),
}

/// `ResourceSet`: "The set of resources authorized by a permit. Specify either
/// all resources in the Region or a list of specific resources."
#[smithy_macros::smithy(
    shape = "com.amazonaws.supportauthz#ResourceSet",
    model = "smithy-model/local-additions.json",
    rename = ["AllResourcesInRegion=allResourcesInRegion", "Resources=resources"]
)]
#[derive(Debug, Clone)]
pub enum ResourceSet {
    /// Every resource in the Region — the arm whose target is `smithy.api#Unit`.
    AllResourcesInRegion(UnitStructure),
    /// The listed resources, at most five by the model's own bound.
    Resources(Vec<String>),
}

/// `Condition`: "A time-window condition that constrains when a support permit is
/// valid."
#[smithy_macros::smithy(
    shape = "com.amazonaws.supportauthz#Condition",
    rename = ["AllowAfter=allowAfter", "AllowBefore=allowBefore"]
)]
#[derive(Debug, Clone)]
pub enum Condition {
    /// "The earliest time at which the permit becomes valid."
    AllowAfter(::aws_smithy_types::DateTime),
    /// "The latest time at which the permit remains valid."
    AllowBefore(::aws_smithy_types::DateTime),
}

/// `SigningKeyInfo`: "The signing key used to cryptographically sign a support
/// permit."
#[smithy_macros::smithy(
    shape = "com.amazonaws.supportauthz#SigningKeyInfo",
    rename = ["KmsKey=kmsKey"]
)]
#[derive(Debug, Clone)]
pub enum SigningKeyInfo {
    /// "The ARN of the AWS KMS key used to sign the permit."
    KmsKey(String),
}

/// `Permit`: "The permit definition specifying the authorized actions, resources,
/// and time-window conditions for a support operator."
#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#Permit")]
#[derive(Debug, Clone)]
pub struct Permit {
    pub actions: ActionSet,
    pub resources: ResourceSet,
    pub conditions: Option<Vec<Condition>>,
}

/// `SupportPermitSummary`: "A summary of a support permit" — the shape
/// `ListSupportPermits` projects each stored permit through.
#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#SupportPermitSummary")]
#[derive(Debug, Clone)]
pub struct SupportPermitSummary {
    pub name: String,
    pub arn: String,
    pub permit: Permit,
    pub status: String,
    pub signing_key_info: SigningKeyInfo,
    pub created_at: ::aws_smithy_types::DateTime,
    pub support_case_display_id: Option<String>,
}

/// The values `SupportPermitStatus` accepts on the wire, in the order the model
/// declares its enum members.
///
/// Emitted here in the form the scaffold emits a value list for an enum shape it
/// renders, because the one operation that takes a status FROM the client —
/// `ListSupportPermits`, whose `supportPermitStatuses` filter targets this enum —
/// is one of the four it declines.
pub const SUPPORT_PERMIT_STATUS_VALUES: &[&str] = &["ACTIVE", "INACTIVE", "DELETING"];
