Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.subscription-request-double-accept-conflict-rejected datazone.subscription-request-reject-transitions-status
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone.subscription-request-double-accept-conflict-rejected|run#7|AssertionError|assert request["status"] == "ACCEPTED", (
Issue-Key: datazone.subscription-request-reject-transitions-status|run#7|AssertionError|assert request["status"] == "ACCEPTED", (
Issue-Commit: 94817d8c9159e4d6c08dc7022582961316307e03

# datazone: CreateSubscriptionRequest always records a new request PENDING, never auto-resolving the request a producer-owner makes for a listing with no subscription target

> _Note: this issue was identified against the code base at commit `94817d8c9159e4d6c08dc7022582961316307e03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscriptionRequest

## Description
1 net-new root cause in datazone's subscription-request create-time status rule.

Cause 1 of 1: CreateSubscriptionRequest hard-codes the created request's status to PENDING, so the class of request DataZone resolves at create time is never resolved, and both decision-refusal state machines that hang off an already-resolved request are unreachable.

NOTE ON RELATED ISSUES. A fixed issue, 20261112T132443Z-datazone-createsubscriptionrequest-never-auto-accepts-a-request-for.md ("datazone CreateSubscriptionRequest never auto-accepts a request for a listing with no subscription target (always PENDING)"), named exactly this behaviour; the code now hard-codes PENDING again, so the auto-resolve arm is gone a second time. The still-open 20261117T035248Z-datazone-createsubscriptionrequest-resolves-a-consumer-s-request-to.md reports the OPPOSITE direction (a consumer-only requester wrongly born ACCEPTED) and does not cover these two checks — the suite report does not mark them owned by it. Both observations are consistent and must be fixed together, not by flipping a single constant: the create-time status depends on the REQUESTER's relationship to the producing project, not on a fixed value and not on whether the listing has a subscription target behind it.

AWS: Smithy — CreateSubscriptionRequest (POST /v2/domains/{domainIdentifier}/subscription-requests, HTTP 200) returns a REQUIRED `status` of enum SubscriptionRequestStatus (PENDING | ACCEPTED | REJECTED), plus optional `reviewerId`, `decisionComment` and `existingSubscriptionId`. The model does not state WHEN a created request is already decided, so for that rule this is recorded behavior only. The recordings show: when the caller is an owner of the project that published the listing and the subscribed listing has no subscription target behind it, CreateSubscriptionRequest answers HTTP 200 with status "ACCEPTED" at create time — no reviewer took a decision, no Subscription is materialised (`existingSubscriptionId` is absent on both the create response and a subsequent GetSubscriptionRequestDetails), and `updatedAt` does not move afterwards. Because the request is then already resolved, the two decision operations refuse it: AcceptSubscriptionRequest answers HTTP 400 ValidationException with the fixed text "The approved subscription is already revoked or unsubscribed." (entirely fixed text — no part is taken from the request or the store), and RejectSubscriptionRequest answers HTTP 400 ValidationException "Cannot reject a subscription request with status <STATUS>", where "Cannot reject a subscription request with status " is the fixed text and <STATUS> is the SubscriptionRequestStatus the stored request currently carries (here ACCEPTED); a refused reject writes no `decisionComment` and leaves `status` unchanged.

Emulator: CreateSubscriptionRequest answers HTTP 200 with status "PENDING" for that same request. services/service-com-amazonaws-datazone-datazone/src/ops/create_subscription_request.rs:369 stores `SUBSCRIPTION_REQUEST_STATUS_PENDING` unconditionally — there is no create-time resolution rule left in the crate at all (the former `created_subscription_request_status` helper is gone from src/state/subscription_requests.rs, whose module doc now asserts at lines 29-33 that "a request is an ASK, so it is created awaiting a decision" and that only a decision operation can move it). Consequently `SubscriptionRequest::awaits_a_decision` (src/state/subscription_requests.rs:59-61) is true for every freshly created request, and neither refusal path can be reached: both behavioral checks die on the create response's own status before issuing the decision call, with "expected a listing with no SubscriptionTarget to auto-resolve to ACCEPTED, got 'PENDING'" and "expected auto-accept for an asset with no subscription target, got 'PENDING'".

Why: a client that models DataZone's approval flow reads the create response's `status` to decide whether an approval step is still owed. Against this emulator every request looks un-decided, so a workflow or console waits for — or issues — an approval that AWS already granted, and the self-service grant path (producer-owner subscribes to its own no-target listing and is immediately entitled) never happens. The whole already-resolved arm of the state machine is also unobservable: the ValidationException refusals of AcceptSubscriptionRequest ("The approved subscription is already revoked or unsubscribed.") and RejectSubscriptionRequest ("Cannot reject a subscription request with status ACCEPTED") are dead code against this emulator, so a client's idempotency/conflict handling for a double-accept or a late reject is never exercised and silently succeeds where AWS refuses with 400.

Fix: the create-time status rule of the CreateSubscriptionRequest handler, and the subscription-request state module it should live in (a single helper both the handler and the decision guards read, so the create and `awaits_a_decision` cannot drift). The rule must be derived from the requester's relationship to the producing project of each subscribed listing — resolved ACCEPTED (with no reviewerId, no decisionComment, no materialised Subscription and `updatedAt` == `createdAt`) when the caller already owns that project, PENDING otherwise — rather than a constant in either direction; the existing refusal paths and messages in the Accept/Reject handlers already match AWS and need no change once a request can be born ACCEPTED again.

Traces: checks datazone.subscription-request-double-accept-conflict-rejected and datazone.subscription-request-reject-transitions-status (both mismatch on the create response's status, before their decision-refusal assertions are reached).

## Evidence
Check datazone.subscription-request-double-accept-conflict-rejected [mismatch]:
  Setup: CreateDomain -> two projects in it (producer, consumer) -> publish an asset listing owned by the producer project (no subscription target created for it).
  Call: CreateSubscriptionRequest(domainIdentifier=<domain>, subscribedPrincipals=[{project:{identifier:<consumer project>}}], subscribedListings=[{identifier:<listing>}], requestReason="...").
  Expected (AWS, recorded): HTTP 200 with status == "ACCEPTED"; GetSubscriptionRequestDetails confirms "ACCEPTED" and carries no existingSubscriptionId; a redundant AcceptSubscriptionRequest is then refused HTTP 400 ValidationException with message exactly "The approved subscription is already revoked or unsubscribed." and leaves status/updatedAt/existingSubscriptionId unchanged.
  Actual (emulator): HTTP 200 with status "PENDING".
  Assertion: AssertionError: expected a listing with no SubscriptionTarget to auto-resolve to ACCEPTED, got 'PENDING'

Check datazone.subscription-request-reject-transitions-status [mismatch]:
  Same setup and same create call.
  Expected (AWS, recorded): HTTP 200 with status == "ACCEPTED"; RejectSubscriptionRequest(domainIdentifier=<domain>, identifier=<request id>, decisionComment="...") is then refused HTTP 400 ValidationException with message "Cannot reject a subscription request with status ACCEPTED" (fixed text "Cannot reject a subscription request with status " + the stored request's current SubscriptionRequestStatus); GetSubscriptionRequestDetails still reports status "ACCEPTED" and no decisionComment.
  Actual (emulator): HTTP 200 with status "PENDING".
  Assertion: AssertionError: expected auto-accept for an asset with no subscription target, got 'PENDING'

Neither check reached its refusal assertion, so the two ValidationException paths are reported as unreachable rather than as diverging text; the emulator's constants for them (SUBSCRIPTION_ALREADY_DECIDED_MESSAGE and cannot_reject_message) already read as AWS words them.

Code path (citations only):
  services/service-com-amazonaws-datazone-datazone/src/ops/create_subscription_request.rs:361-391 — the created record, with status fixed to SUBSCRIPTION_REQUEST_STATUS_PENDING at line 369 and existing_subscription_id forced to None
  services/service-com-amazonaws-datazone-datazone/src/state/subscription_requests.rs:26-36 — the status constants; lines 29-33 document "created awaiting a decision" as an invariant, which is where the missing resolution rule used to be
  services/service-com-amazonaws-datazone-datazone/src/state/subscription_requests.rs:38-53, 55-61 — SUBSCRIPTION_ALREADY_DECIDED_MESSAGE, cannot_reject_message(status) and awaits_a_decision, the guards the unreachable refusals come from

## Steps to Reproduce
1. CreateDomain and wait for AVAILABLE.
2. Create two projects in the domain: a producer and a consumer, both owned by the calling identity.
3. Publish an asset listing owned by the producer project (create a form type and an asset, then a PUBLISH listing change set). Do NOT create a subscription target for the listing.
4. As that same identity (an owner of the producing project), call CreateSubscriptionRequest(domainIdentifier=<domain id>, subscribedPrincipals=[{project:{identifier:<consumer project id>}}], subscribedListings=[{identifier:<listing id>}], requestReason="...").
5. Read `status` on the response: the emulator returns "PENDING"; AWS returns "ACCEPTED" (with no reviewerId, no decisionComment and no existingSubscriptionId). GetSubscriptionRequestDetails(domainIdentifier=<domain id>, identifier=<request id>) shows the same divergence.
6. Call AcceptSubscriptionRequest(domainIdentifier=<domain id>, identifier=<request id>): AWS refuses it HTTP 400 ValidationException "The approved subscription is already revoked or unsubscribed." and mutates nothing; the emulator accepts it because the request is still PENDING.
7. Repeat steps 1-4 and call RejectSubscriptionRequest(domainIdentifier=<domain id>, identifier=<request id>, decisionComment="..."): AWS refuses it HTTP 400 ValidationException "Cannot reject a subscription request with status ACCEPTED" and writes no decisionComment; the emulator performs the reject because the request is still PENDING.
