// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_campaign_state_batch` operation.
//!
//! The `@generated` marker is absent on purpose, and this file is HAND-WRITTEN at a
//! generated path: the published model snapshot beside it is kept EXACTLY as shipped, and
//! the one member whose published constraint is not the one the service enforces —
//! `GetCampaignStateBatchRequest$campaignIds`, whose snapshot `@length` maximum is
//! twenty-five where the recorded service answers a batch of five hundred — is declared
//! from the small overlay model `smithy-model/local-additions.json` instead (see
//! `smithy-model/LOCAL-ADDITIONS.md`). The overlay restates that structure against the
//! snapshot's own target shapes, so every other constraint of the request — the entry
//! length and pattern of a `CampaignId`, the list's own minimum, the member being
//! `@required` — is captured exactly as the snapshot spells it, and only the maximum is
//! the service's. Nothing about the wire is hand-written: the member names, the shape ids
//! and the constraints all come from a model file.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-connectcampaigns-amazonconnectcampaignservice.json");

/// The batch state read's input, declared from the overlay model — the one shape of this
/// service whose published constraint and enforced constraint differ (see the module docs).
#[smithy_macros::smithy(
    shape = "com.amazonaws.connectcampaigns#GetCampaignStateBatchRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct GetCampaignStateBatchInput {
    pub campaign_ids: Vec<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connectcampaigns#GetCampaignStateBatchResponse")]
#[derive(Debug, Clone, Default)]
pub struct GetCampaignStateBatchOutput {
    pub successful_requests: Option<Vec<SuccessfulCampaignStateResponse>>,
    pub failed_requests: Option<Vec<FailedCampaignStateResponse>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connectcampaigns#SuccessfulCampaignStateResponse")]
#[derive(Debug, Clone, Default)]
pub struct SuccessfulCampaignStateResponse {
    pub campaign_id: Option<String>,
    pub state: Option<String>,
}

// `FailedCampaignStateResponse` is declared in `super::failed_campaign_state_wire` instead
// of here: its wire carries a member the on-disk model does not declare, which the derive
// above cannot express, so its schema and codec are written out there.

