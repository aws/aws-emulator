Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-free-subscription-token-replay-and-etag-rules
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-free-subscription-token-replay-and-etag-rules
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|CancelSubscriptionChange|4xx->4xx|$.message
Issue-Key: pricing-plan-manager|CancelSubscriptionChange|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pricing-plan-manager|CancelSubscriptionChange|4xx->4xx|status
Issue-Commit: 0c5cb36cb8e8253fc1120b6ad7fe39ce6f6094b8

# CancelSubscriptionChange on a subscription with no pending change returns 409 ConflictException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `0c5cb36cb8e8253fc1120b6ad7fe39ce6f6094b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelSubscriptionChange

## Description
Cause 1 of 1: CancelSubscriptionChange classifies "there is no scheduled change to cancel" as a state conflict instead of a business-rule validation failure, so it answers with the wrong HTTP status, the wrong error code and a non-AWS message. Fixture pricing-plan-manager-free-subscription-token-replay-and-etag-rules, step task#11.

AWS: CancelSubscriptionChange "[c]ancels a pending scheduled change on a subscription" (Smithy: CancelSubscriptionChange documentation). When the named subscription exists and the If-Match ETag is current but the subscription carries NO scheduled change, AWS rejects the request as a business-rule validation failure: HTTP 400 with error code ValidationException, whose modeled meaning is "The request failed a business rule validation ... or the subscription might not be in the required state for this operation" (Smithy: CancelSubscriptionChange errors include ValidationException @httpError 400 and ConflictException @httpError 409; ValidationException members message (required) and resourceId (optional); ConflictException members message and resourceId, both required). The recorded response carries the message "No scheduled updates found for this subscription." — that whole string is FIXED text for this error class, with no variable parts interpolated from the request or from state (the subscription ARN is not named in it). ConflictException is reserved for an actual state conflict, documented as "[t]his typically occurs when the ETag value in the If-Match header does not match the current version of the subscription" (Smithy: ConflictException documentation), which is not the case here.

Emulator: answers HTTP 409 with x-amzn-errortype ConflictException and the message "the subscription has no scheduled change to cancel", and populates resourceId with the subscription ARN. Replay detail: "status: expected 400, got 409 / $.message: value_mismatch (expected 'No scheduled updates found for this subscription.', got 'the subscription has no scheduled change to cancel') / $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ConflictException')". So all three requirements diverge: status 409 vs 400, code ConflictException vs ValidationException, and a lowercase ad-hoc sentence vs AWS's fixed sentence.

Why: a client that distinguishes AWS's two failure classes behaves wrongly. 409/ConflictException is the documented signal that the caller's ETag is stale, so SDK and application retry logic re-reads the subscription with GetSubscription and retries the cancel with a fresh ETag — against the emulator that retry loops forever, because re-reading never produces a scheduled change to cancel. The condition AWS reports is non-retryable (400/ValidationException), which a client surfaces to the caller instead of retrying. Error-code-based branching, waiter/poller predicates and idempotent cleanup paths ("cancel the pending change if there is one") therefore take the wrong branch, and the user-visible message is not AWS's.

Fix: the CancelSubscriptionChange handler's no-pending-change branch — the error it selects and the message it mints. It must return the operation's modeled ValidationException (HTTP 400) carrying AWS's fixed message for this class, rather than ConflictException with an emulator-authored sentence; ConflictException stays for genuine state conflicts. The adjacent "within the processing window" branch in the same handler should be reviewed for the same mis-classification, though no fixture exercises it. Note the open issue 20261111T004423Z-pricing-plan-manager-cancelsubscriptionchange-stub.md, which was filed when this operation was still an unimplemented scaffold answering 501 and names the same behavioral check: the operation is now implemented, and the surviving defect is this error classification, not the stub.

Traces: pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#11 (cluster of 1).

## Evidence
Fixture pricing-plan-manager-free-subscription-token-replay-and-etag-rules, step task#11, operation CancelSubscriptionChange.

Expected (recorded AWS response): HTTP 400, x-amzn-errortype ValidationException, body $.message == "No scheduled updates found for this subscription."

Actual (emulator): HTTP 409, x-amzn-errortype ConflictException, body $.message == "the subscription has no scheduled change to cancel", $.resourceId == the subscription ARN.

Replay divergence_key: service=pricing-plan-manager|CancelSubscriptionChange|status=400->409|keys=$.message,$headers.x-amzn-errortype,status (verdict mismatch, affected_count 1).

Smithy evidence: CancelSubscriptionChange models both ValidationException (@httpError 400, "The request failed a business rule validation ... the subscription might not be in the required state for this operation") and ConflictException (@httpError 409, "This typically occurs when the ETag value in the If-Match header does not match the current version of the subscription"). Handler source: services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/cancel_subscription_change.rs lines 209-219 return ConflictException with the literal string "the subscription has no scheduled change to cancel" when record.scheduled_change is None.

## Steps to Reproduce
1. Boot the pricing-plan-manager emulator from this checkout.
2. CreateSubscription for a FREE CloudFront plan and GetSubscription to obtain its ARN and current ETag; confirm the subscription has no scheduledChange.
3. Call CancelSubscriptionChange with that arn and If-Match set to the current ETag.
4. Observe HTTP 409 with x-amzn-errortype ConflictException and message "the subscription has no scheduled change to cancel"; AWS answers HTTP 400 with x-amzn-errortype ValidationException and message "No scheduled updates found for this subscription."
5. Equivalently: replay fixture pricing-plan-manager-free-subscription-token-replay-and-etag-rules and observe step task#11 mismatch.
