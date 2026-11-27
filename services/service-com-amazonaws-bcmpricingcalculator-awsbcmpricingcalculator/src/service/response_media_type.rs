// SPDX-License-Identifier: CC0-1.0

//! The media types this service's responses are announced under — there are two.
//!
//! # What the wire shows
//!
//! The model declares `aws.protocols#awsJson1_0`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json")`),
//! which implies `application/x-amz-json-1.0`, and the framework's renderers stamp exactly
//! that. The recorded wire shows that value on some of this service's responses and
//! `application/x-amz-json-1.1` on others, and the split is visible INSIDE one recorded
//! session rather than across sessions: in one fixture the create of a workload estimate is
//! recorded under `1.1` while the tag listing, tag write and tag removal of that same
//! estimate are recorded under `1.0`, and in another the refusal of a name outside its
//! `@pattern` is recorded under `1.0` while the not-found answers of the reads beside it are
//! recorded under `1.1`. So this service answers through two announcers:
//!
//! * the WORKLOAD-ESTIMATE, BILL-SCENARIO, BILL-ESTIMATE and PREFERENCES operations are
//!   fronted by a dispatcher announcing `application/x-amz-json-1.1` on what it returns —
//!   their successes, and the errors their handlers raise about stored state (a resource
//!   that does not exist, a quota that is spent);
//! * a refusal that reports the REQUEST ITSELF as invalid — a member outside the constraint
//!   its shape declares — is answered by the request validator in front of that dispatcher,
//!   and announces the protocol-implied `application/x-amz-json-1.0`. So do the other
//!   responses rendered in front of it: an unroutable request, a transport-level failure,
//!   the authentication refusal;
//! * the RESOURCE-TAGGING operations announce the protocol-implied media type throughout,
//!   on every status.
//!
//! The two announcers are told apart by WHICH refusal a response is, not by the shape it is
//! worded in: this service answers an input-constraint violation and a refusal of a
//! WELL-FORMED request from a member account in the one modelled `ValidationException`
//! (`crate::state::...::member_account_refusal`), and the recorded wire announces the first
//! under the protocol-implied media type and the second under the dispatcher's — the
//! account-class gate is part of the dispatcher's front door, which is also what its
//! `reason` says (`INVALID_REQUEST_FROM_MEMBER`, where a constraint refusal reports
//! `fieldValidationFailed`). So the question below is asked of the GROUND the refusal states
//! and not of its `__type`.
//!
//! Which layer renders a refusal about the request is the EMULATOR's business and does not
//! change what it announces: the boundary validator refuses what the model's constraints
//! express and a handler refuses what they cannot (a reserved tag key, a retention instant
//! already past), so the predicate below reads the refusal the client receives rather than
//! the seam that produced it.
//!
//! The two lists are therefore read on different things, and each on what the wire record
//! shows: the tagging family is a fact of the OPERATION, so it is read off the routing key
//! before the walk runs, while the validator's refusal is a fact of the RESPONSE. The
//! responses rendered outside the walk — an unroutable request, a transport-level failure,
//! the authentication refusal — reach neither list: no operation of this service ran, so
//! they keep what their own renderer stamped, which is the protocol-implied value. A failure
//! the framework renders INSIDE the walk of an operation the dispatcher fronts and that is
//! not a modelled constraint refusal (a body the codec cannot read) is announced as the
//! dispatcher's, because the recorded wire shows no such response under the other value.
//!
//! The REQUEST side is untouched by all of it: the media type a caller sends is never
//! re-read, requests still route by `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`, and
//! the awsJson `1.0` and `1.1` codecs are the same `JsonCodec` over the same body, so a call
//! sent under either one reads byte for byte.
//!
//! # Where the rule lives
//!
//! Not here. The mechanism — the one function that re-announces a response
//! ([`ResponseMediaType::announce`]), the predicate type a service states its announcers
//! with, the operation-seam wrapper that carries both around the centralized walk
//! ([`ResponseMediaTypeOperation`](service_common::response_media_type::ResponseMediaTypeOperation))
//! and the question "is this response the request validator's?"
//! ([`service_common::response_media_type::not_an_input_validation_refusal`]) — is
//! [`service_common::response_media_type`], the tree's one statement of it. This module
//! holds only what is this SERVICE's: the pair its dispatcher announces across and which
//! operations that dispatcher fronts, named once so every seam in [`crate::service`] reads
//! the same values. `tests/all/response_media_type.rs` pins both announcers over the wire.

use aws_smithy_runtime_api::http::Response as RtResponse;
use service_common::response_media_type::{ResponseMediaType, AWS_JSON_1_0, AWS_JSON_1_1};

/// The pair a response of the workload-estimate and bill-planning dispatcher is
/// re-announced across: the `application/x-amz-json-1.0` the model's protocol trait
/// implies — stamped by the framework's own renderers, and the ONLY value replaced — and the
/// `application/x-amz-json-1.1` that dispatcher announces.
pub const DISPATCHER_RESPONSE_MEDIA_TYPE: ResponseMediaType =
    ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1);

/// The resource-tagging operations this service models — the whole family, not a chosen few
/// (source: `smithy_query(query="list_operations")`, which binds these three and no other
/// tagging operation). Their responses are not the dispatcher's.
const TAGGING_OPERATIONS: [&str; 3] = ["ListTagsForResource", "TagResource", "UntagResource"];

/// Whether `routing_key` — the operation name the protocol reads out of `X-Amz-Target` —
/// names an operation the workload-estimate and bill-planning dispatcher fronts, as opposed
/// to a resource-tagging one.
pub fn fronted_by_the_dispatcher(routing_key: &str) -> bool {
    !TAGGING_OPERATIONS.contains(&routing_key)
}

/// Whether `resp` is a response the workload-estimate and bill-planning dispatcher
/// produced, rather than one the request validator in front of it answered.
///
/// The validator's is the refusal reporting the REQUEST ITSELF as invalid; everything else
/// the walk of a dispatcher-fronted operation answers — a success, an error about stored
/// state, a scaffold's 501, and the account-class gate's refusal of a well-formed request —
/// is the dispatcher's. The question is `service_common`'s, so this service and every other
/// one on that wire ask it the same way, and it reads the GROUND the refusal states rather
/// than the shape both grounds are worded in.
pub fn produced_by_the_dispatcher(resp: &RtResponse) -> bool {
    service_common::response_media_type::not_an_input_validation_refusal(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// Which of the walk's responses the dispatcher produced is read from the GROUND a
    /// refusal states, not from the shape it is worded in: both the modelled
    /// input-constraint refusal and the account-class gate's refusal are a 400
    /// `ValidationException`, and only the first is the request validator's.
    ///
    /// The rows span the responses one walk of a dispatcher-fronted operation can answer
    /// with: a success, an error about stored state, the scaffold's 501, the
    /// input-constraint refusal (with and without the `reason` it states), and the
    /// account-class refusal in both spellings the enum value is put on the wire in.
    #[test]
    fn the_ground_of_a_refusal_decides_which_announcer_produced_it() {
        /// A response carrying `body` under `status`, as a layer of the walk renders one.
        fn answered(status: u16, body: &str) -> RtResponse {
            RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                SdkBody::from(body.as_bytes().to_vec()),
            )
        }

        let cases: [(&str, RtResponse, bool); 7] = [
            ("a success", answered(200, r#"{"id":"x"}"#), true),
            (
                "an error about stored state",
                answered(404, r#"{"__type":"ResourceNotFoundException","message":"x"}"#),
                true,
            ),
            (
                "the scaffold's 501",
                answered(501, r#"{"__type":"NotImplemented","message":"x"}"#),
                true,
            ),
            (
                "the input-constraint refusal",
                answered(
                    400,
                    r#"{"__type":"ValidationException","message":"1 validation error detected: x"}"#,
                ),
                false,
            ),
            (
                "the input-constraint refusal stating its ground",
                answered(
                    400,
                    r#"{"__type":"ValidationException","reason":"fieldValidationFailed","message":"x"}"#,
                ),
                false,
            ),
            (
                "the account-class refusal, the gate's spelling of the reason",
                answered(
                    400,
                    r#"{"__type":"ValidationException","reason":"INVALID_REQUEST_FROM_MEMBER","message":"x"}"#,
                ),
                true,
            ),
            (
                "the account-class refusal, the model's spelling of the reason",
                answered(
                    400,
                    r#"{"__type":"ValidationException","reason":"invalidRequestFromMember","message":"x"}"#,
                ),
                true,
            ),
        ];

        for (case, resp, expected) in cases {
            let actual = produced_by_the_dispatcher(&resp);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected produced_by_the_dispatcher {expected}, got {actual}"
            );
        }
    }

    /// The dispatcher fronts every operation this service models except the resource-tagging
    /// family.
    ///
    /// The rows are every operation the service binds (`smithy_query(query="digest")`),
    /// split into the two families, so an operation added to the model lands in one of them
    /// deliberately rather than by default.
    #[test]
    fn the_dispatcher_fronts_everything_but_the_tagging_operations() {
        let tagging = ["ListTagsForResource", "TagResource", "UntagResource"];
        let dispatched = [
            "BatchCreateBillScenarioCommitmentModification",
            "BatchCreateBillScenarioUsageModification",
            "BatchCreateWorkloadEstimateUsage",
            "BatchDeleteBillScenarioCommitmentModification",
            "BatchDeleteBillScenarioUsageModification",
            "BatchDeleteWorkloadEstimateUsage",
            "BatchUpdateBillScenarioCommitmentModification",
            "BatchUpdateBillScenarioUsageModification",
            "BatchUpdateWorkloadEstimateUsage",
            "CreateBillEstimate",
            "CreateBillScenario",
            "CreateWorkloadEstimate",
            "DeleteBillEstimate",
            "DeleteBillScenario",
            "DeleteWorkloadEstimate",
            "GetBillEstimate",
            "GetBillScenario",
            "GetPreferences",
            "GetUsageLinesEstimatedCost",
            "GetWorkloadEstimate",
            "ListBillEstimateCommitments",
            "ListBillEstimateInputCommitmentModifications",
            "ListBillEstimateInputUsageModifications",
            "ListBillEstimateLineItems",
            "ListBillEstimates",
            "ListBillScenarioCommitmentModifications",
            "ListBillScenarioUsageModifications",
            "ListBillScenarios",
            "ListWorkloadEstimateUsage",
            "ListWorkloadEstimates",
            "UpdateBillEstimate",
            "UpdateBillScenario",
            "UpdatePreferences",
            "UpdateWorkloadEstimate",
        ];

        for operation in tagging {
            assert!(
                !fronted_by_the_dispatcher(operation),
                "case {operation:?}: a resource-tagging operation is not fronted by the \
                 dispatcher, got true"
            );
        }
        for operation in dispatched {
            assert!(
                fronted_by_the_dispatcher(operation),
                "case {operation:?}: a workload-estimate or bill-planning operation is \
                 fronted by the dispatcher, got false"
            );
        }
    }
}
