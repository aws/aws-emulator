// SPDX-License-Identifier: CC0-1.0

//! The preconditions a flow's SOURCE REPORTS are gated on: what MediaConnect requires of a
//! flow before it reports anything about what that flow's source is receiving.
//!
//! DescribeFlowSourceMetadata reports "the flow's source transport stream and programs" and
//! DescribeFlowSourceThumbnail "the thumbnail for the flow source" — both are reports on
//! content a RUNNING flow is taking in, and MediaConnect answers neither for a flow that is
//! not in a state where it could be taking any in, or (for the thumbnail) that is not having
//! thumbnails taken at all. Such a request is well formed and the resource it addresses
//! simply does not admit it, which is a semantic error: it is refused with the modelled
//! `BadRequestException` (HTTP 400), nothing is projected, and nothing about the flow moves.
//!
//! The wording of each refusal is fixed text for its error class: the only values in either
//! message are the status and state names the model itself declares ([`super::STATUS_ACTIVE`],
//! [`super::STATUS_UPDATING`], [`super::STATE_ENABLED`]), so nothing in it comes from the
//! request. Both are stated ONCE here, and the two handlers refuse through them, so neither
//! operation can word the rule its own way and a reader of this module sees the whole gate.
//!
//! The two refusals carry DIFFERENT envelopes, which is the service's own distinction
//! between its two kinds of 400 (see [`super::BAD_REQUEST_PREFIX`] and
//! [`super::VALIDATION_FAILED_PREFIX`]): the metadata read states the one precondition it
//! has as a semantic refusal, while the thumbnail read validates two preconditions together
//! and reports every one the flow fails, one sentence per line under the validation
//! envelope, so a caller learns in one answer everything it has to change.

/// The statuses a flow's source THUMBNAIL is reported in: a flow is taking content in while
/// it is [`super::STATUS_ACTIVE`], and a flow whose configuration is being changed
/// ([`super::STATUS_UPDATING`]) is still the running flow it was, so the read is served for
/// both.
const THUMBNAIL_REPORTING_STATUSES: [&str; 2] = [super::STATUS_ACTIVE, super::STATUS_UPDATING];

/// Whether thumbnails are taken for a flow carrying `config` as its stored source
/// monitoring configuration: `MonitoringConfig$ThumbnailState` ("indicates whether
/// thumbnails are enabled or disabled") is [`super::STATE_ENABLED`].
///
/// The model declares no default for that member and AWS documents none, so a flow whose
/// configuration never mentions thumbnails, and one carrying no configuration at all, is a
/// flow thumbnails are not enabled on — the member has to be SET to the enabled state for
/// the read to be served. Stated once here so the gate and anything else that asks whether a
/// flow is having thumbnails taken read one predicate, the way
/// [`super::ndi_is_enabled`] is the one reading of a flow's NDI state.
#[must_use]
pub fn thumbnails_are_enabled(config: Option<&crate::ops::types::MonitoringConfig>) -> bool {
    config.and_then(|config| config.thumbnail_state.as_deref()) == Some(super::STATE_ENABLED)
}

/// Why DescribeFlowSourceMetadata cannot report on `flow` — the whole
/// `BadRequestException$Message` the request is refused with — or `None` when the flow is in
/// the status that admits the read.
///
/// Source metadata is demultiplexed out of the transport stream the flow is RECEIVING, so
/// the read is served only while the flow is running ([`super::STATUS_ACTIVE`]): in every
/// other status there is no stream to describe, and the caller is told the precondition
/// rather than handed a success it would have to re-interpret.
#[must_use]
pub fn flow_source_metadata_refusal(flow: &super::StoredFlow) -> Option<String> {
    if flow.status == super::STATUS_ACTIVE {
        return None;
    }
    Some(super::bad_request_message(&format!(
        "The flow must be in {} state to perform this action",
        super::STATUS_ACTIVE
    )))
}

/// Why DescribeFlowSourceThumbnail cannot report on `flow` — the whole
/// `BadRequestException$Message` the request is refused with — or `None` when the flow
/// satisfies both preconditions.
///
/// A thumbnail is a still decoded out of the content the flow is receiving, and it is
/// decoded only for a flow thumbnails are turned on for, so the read needs BOTH: a status
/// among [`THUMBNAIL_REPORTING_STATUSES`], and thumbnails enabled in the flow's stored
/// source monitoring configuration ([`thumbnails_are_enabled`]). The two are reported
/// TOGETHER — one sentence per precondition the flow fails, in the order they are checked,
/// under the validation envelope — so a flow that fails both is refused once and the caller
/// does not have to fix one precondition to discover the other.
#[must_use]
pub fn flow_source_thumbnail_refusal(flow: &super::StoredFlow) -> Option<String> {
    let mut unmet: Vec<String> = Vec::new();
    if !THUMBNAIL_REPORTING_STATUSES.contains(&flow.status.as_str()) {
        unmet.push(format!(
            "To perform this action, the flow must be in {} or {} state.",
            super::STATUS_ACTIVE,
            super::STATUS_UPDATING
        ));
    }
    if !thumbnails_are_enabled(flow.source_monitoring_config.as_ref()) {
        unmet.push(format!(
            "This action can only be performed on a flow with ThumbnailState in \
             SourceMonitoringConfiguration set to {}.",
            super::STATE_ENABLED
        ));
    }
    (!unmet.is_empty()).then(|| super::validation_failed_message(&unmet.join("\n")))
}
