Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.eks-anywhere-subscription-read-semantics eks.op-eks-anywhere-mutations-unknown-subscription
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.eks-anywhere-subscription-read-semantics|run#6|AssertionError|assert (code, message) == ("ResourceNotFoundException", "subscription not found."), (code, message)
Issue-Key: eks.op-eks-anywhere-mutations-unknown-subscription|run#6|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: 99702e197eaface5f0989bf47fe2417bf7d2146c

# eks: 2 net-new root causes in the EKS Anywhere subscription surface — not-found wording and un-implemented Update/Delete mutations

> _Note: this issue was identified against the code base at commit `99702e197eaface5f0989bf47fe2417bf7d2146c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEksAnywhereSubscription

## Description
Two distinct, net-new root causes in the EKS Anywhere subscription (`/eks-anywhere-subscriptions/{id}`) surface of the eks emulator. Neither is covered by any open issue: the only pre-existing EKS-Anywhere issue is `20261022T154613Z-eks-listeksanywheresubscriptions-stub.md`, which concerns `ListEksAnywhereSubscriptions` (that operation now answers — both failing checks call it successfully as their positive control before diverging elsewhere). Both causes must be closed before this issue is submitted.

=== Cause 1 of 2: DescribeEksAnywhereSubscription's ResourceNotFoundException carries the service-wide generic message instead of the subscription not-found wording ===

AWS: `DescribeEksAnywhereSubscription` models `ResourceNotFoundException` (HTTP 404) as its only not-found refusal (Smithy: DescribeEksAnywhereSubscription errors = ClientException 400, ResourceNotFoundException 404, ServerException 500, ServiceUnavailableException 503; input `id` is an @httpLabel required String on GET /eks-anywhere-subscriptions/{id}). For a subscription id the account/Region does not carry, the recorded behavior is 404 with `Error.Code` = `ResourceNotFoundException` and `Error.Message` = `subscription not found.` — that message is entirely FIXED text for this error class (no variable part; the named id is carried structurally in the payload's `subscriptionId` member, not interpolated into the message). The model itself documents only the generic prose for the shape, so the specific wording is recorded behavior only.

Emulator: returns the correct code and status but the wrong message — `('ResourceNotFoundException', 'The requested resource does not exist.')`. `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_eks_anywhere_subscription.rs` builds the refusal with `message: Some(state::RESOURCE_NOT_FOUND_MESSAGE.to_string())`, i.e. the eks crate's single service-wide not-found constant, rather than the per-resource wording this operation uses.

Why: a client that branches on, logs, or asserts the refusal text for a missing EKS Anywhere subscription sees a different sentence than AWS produces. Error messages are part of the contract for an error class; any consumer matching AWS's wording (tests, runbooks, support tooling, retry/classification logic keyed on message text) misclassifies the refusal, and the divergence is invisible at the code level because the code is already right.

Fix: the refusal construction in that operation's handler — `DescribeEksAnywhereSubscription`'s `handle` must emit the subscription-specific not-found message rather than reusing the crate-wide `RESOURCE_NOT_FOUND_MESSAGE`. If other subscription operations share it, the right home is a subscription-scoped message constant in the eks crate's state module rather than the generic one.

Traces: eks.eks-anywhere-subscription-read-semantics.

=== Cause 2 of 2: UpdateEksAnywhereSubscription and DeleteEksAnywhereSubscription answer 501 NotImplemented instead of refusing an unknown subscription id ===

AWS: both mutations are fully modeled and both model `ResourceNotFoundException` (HTTP 404). Smithy: `UpdateEksAnywhereSubscription` = POST /eks-anywhere-subscriptions/{id}, input {id @httpLabel required, autoRenew Boolean default false required, clientRequestToken @idempotencyToken}, output {subscription: EksAnywhereSubscription}, errors ClientException 400 / InvalidParameterException 400 / InvalidRequestException 400 / ResourceNotFoundException 404 / ServerException 500. `DeleteEksAnywhereSubscription` = DELETE /eks-anywhere-subscriptions/{id}, input {id @httpLabel required}, output {subscription}, errors ClientException 400 / InvalidRequestException 400 / ResourceNotFoundException 404 / ServerException 500. For an id that names no subscription in the account/Region, the recorded behavior for each is 404 `ResourceNotFoundException` with `Error.Message` = `subscription not found.` — again fixed text for the class, with the named id carried structurally rather than interpolated.

Emulator: both operations are unimplemented scaffolds and answer HTTP 501 with `Error.Code` = `NotImplemented`. `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/update_eks_anywhere_subscription.rs` and `.../delete_eks_anywhere_subscription.rs` still carry the `SCAFFOLDED, NOT IMPLEMENTED` marker and end their `handle` with `Err(crate::scaffolded::Scaffolded::not_implemented(...))`. The replay shows `UpdateEksAnywhereSubscription on unknown id: expected ResourceNotFoundException, got NotImplemented`; `DeleteEksAnywhereSubscription` is never reached in the check because the update assertion fails first, but it is the same stub and diverges identically.

Why: a client cannot distinguish "this subscription does not exist" from "this API is absent". 501/NotImplemented is not a modeled error for either operation, so SDK error-shape dispatch finds no matching modeled error and the caller falls through to a generic transport/unknown-error path instead of its ResourceNotFoundException branch — the opposite of the refusal AWS gives. Equally, nothing in the emulator can exercise subscription auto-renew updates or subscription deletion at all, so an over-permissive or silently-succeeding mutation is indistinguishable from a correct refusal for any consumer of this surface.

Fix: implement both operations' handlers against the eks crate's subscription store (`eks_anywhere_subscriptions`, keyed by (region, id), as `describe_eks_anywhere_subscription.rs` already reads it), materialising due status transitions the way the describe path does, refusing an id absent from the Region with the modeled 404 and the cause-1 wording, honouring `clientRequestToken` idempotency on update, and returning the updated/deleted `subscription` in the response; then delete the SCAFFOLDED marker lines from both files.

Traces: eks.op-eks-anywhere-mutations-unknown-subscription.

## Evidence
Cause 1 — check eks.eks-anywhere-subscription-read-semantics:
  assertion: assert (code, message) == ("ResourceNotFoundException", "subscription not found.")
  AssertionError: ('ResourceNotFoundException', 'The requested resource does not exist.')
  Expected: HTTP 404, Error.Code=ResourceNotFoundException, Error.Message="subscription not found." (fixed text for this error class)
  Actual:   HTTP 404, Error.Code=ResourceNotFoundException, Error.Message="The requested resource does not exist." (the crate-wide state::RESOURCE_NOT_FOUND_MESSAGE constant)
  Source: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_eks_anywhere_subscription.rs:161 — message: Some(state::RESOURCE_NOT_FOUND_MESSAGE.to_string())

Cause 2 — check eks.op-eks-anywhere-mutations-unknown-subscription:
  AssertionError: UpdateEksAnywhereSubscription on unknown id: expected ResourceNotFoundException, got NotImplemented
  Expected: HTTP 404, Error.Code=ResourceNotFoundException, Error.Message="subscription not found." for both UpdateEksAnywhereSubscription and DeleteEksAnywhereSubscription on an id no subscription carries
  Actual:   HTTP 501, Error.Code=NotImplemented, message of the form "operation <Name> is scaffolded but not implemented in this emulator"
  Source: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/update_eks_anywhere_subscription.rs:4 (SCAFFOLDED marker) and :171 Err(crate::scaffolded::Scaffolded::not_implemented("UpdateEksAnywhereSubscription")); services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/delete_eks_anywhere_subscription.rs:4 (same SCAFFOLDED marker)
  Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): both operations model ResourceNotFoundException with httpError 404; NotImplemented/501 is not a modeled error of either.

The check's positive control (ListEksAnywhereSubscriptions returning a list) passed in both checks, so the subscription surface is reachable and neither failure is a blanket-404/blanket-deny artefact.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point a boto3 eks client at it.
2. id = str(uuid.uuid4())  # an id no subscription carries
3. eks.list_eks_anywhere_subscriptions() -> confirm the id is not listed (positive control; this succeeds).
4. eks.describe_eks_anywhere_subscription(id=id)
5. Observe ClientError with Error.Code == "ResourceNotFoundException" but Error.Message == "The requested resource does not exist."; AWS answers Error.Message == "subscription not found."

Cause 2:
1. Same client and the same unknown id.
2. eks.list_eks_anywhere_subscriptions() -> returns a list (positive control; succeeds).
3. eks.update_eks_anywhere_subscription(id=id, autoRenew=True)
4. Observe HTTP 501 with Error.Code == "NotImplemented" instead of HTTP 404 ResourceNotFoundException / "subscription not found."
5. eks.delete_eks_anywhere_subscription(id=id) -> same 501 NotImplemented; AWS answers HTTP 404 ResourceNotFoundException / "subscription not found."

Replay to confirm the fix: eks.eks-anywhere-subscription-read-semantics and eks.op-eks-anywhere-mutations-unknown-subscription.
