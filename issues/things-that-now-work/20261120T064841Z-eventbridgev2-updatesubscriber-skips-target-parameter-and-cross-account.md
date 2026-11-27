Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-update-subscriber-rejections-keep-stored-configuration
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-update-subscriber-rejections-keep-stored-configuration eventbridgev2-update-subscriber-description-state-logging-and-batching
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|UpdateSubscriber|4xx->2xx|$.Message
Issue-Key: eventbridgev2|UpdateSubscriber|4xx->2xx|$.__type
Issue-Key: eventbridgev2|UpdateSubscriber|4xx->2xx|status
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.FilterConfiguration.Filters[*]
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.FilterConfiguration.Filters[*].Pattern
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.InvokeConfiguration.RoleArn
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.Transformer.Type
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.BatchConfiguration.MaxBatchWindowInSeconds
Issue-Related: 20261113T174325Z-eventbridgev2-subscriber-reads-omit-stored-configuration-drop-updates.md
Issue-Commit: 5f184b6d595470414f1315ced82dbc505d6a982b

# eventbridgev2: UpdateSubscriber skips target-parameter and cross-account role validation, then persists the updates it should refuse

> _Note: this issue was identified against the code base at commit `5f184b6d595470414f1315ced82dbc505d6a982b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSubscriber

## Description
Four net-new wire causes in the eventbridgev2 subscriber update/read path: UpdateSubscriber validates neither the target-parameter variant nor the role's account, and because the refusals never happen the rejected values are written to the stored subscriber, which DescribeSubscriber then reports back incompletely. All four must be closed before this issue is submitted.

Cause 1 of 4: UpdateSubscriber accepts an InvokeConfiguration whose parameter variant does not match the subscriber's target.
AWS: UpdateInvokeConfiguration carries exactly one target-parameter member (LambdaParameters | SqsParameters | SnsParameters | KinesisParameters | StepFunctionsParameters | HttpParameters | UniversalTargetParameters | EventBusV2Parameters) and that member must match the service of the subscriber's existing TargetArn (Smithy: UpdateSubscriber -> UpdateInvokeConfiguration; errors include InvalidInputException httpError 400, "A request parameter is missing or not valid"). Recorded behavior for the mismatch: HTTP 400, __type com.amazon.nexus.v20250515.exceptions#InvalidInputException, Message "<SuppliedParametersMember> is not supported for target <targetArn>. Expected <ExpectedParametersMember>" — the fixed text is " is not supported for target " and ". Expected ", with the supplied member name, the subscriber's target ARN and the member the target's service requires as the variable parts (in the recording: LambdaParameters, an SQS queue ARN, SqsParameters).
Emulator: answers HTTP 200 with no __type and no Message — the update is accepted.
Why: a client that mis-wires a target silently gets a subscriber whose invoke configuration can never deliver, instead of the synchronous 400 that tells it which parameters member the target needs. The misconfiguration surfaces later as undelivered events rather than at the call.
Fix: UpdateSubscriber's input validation in the eventbridgev2 crate — cross-check the supplied parameters variant against the stored subscriber's TargetArn before mutating state.
Traces: eventbridgev2-update-subscriber-rejections-keep-stored-configuration#task#4 (cluster also covers task#5 and task#6); the same unvalidated mismatch on the create path is CreateSubscriber, already owned by 20261113T174325Z-eventbridgev2-subscriber-error-paths-answer-the-wrong-status.md.

Cause 2 of 4: UpdateSubscriber accepts a RoleArn from another account instead of denying it.
AWS: UpdateInvokeConfiguration.RoleArn is documented "IAM role the service assumes to invoke the target. Must belong to the calling account" (pattern ^arn:aws(-[a-z0-9]+)*:iam::\d{12}:role\/[\w+=,.@/-]+$). A role in a different account is refused with HTTP 403, __type com.amazon.nexus.v20250515.exceptions#AccessDeniedException and the message "Access denied." — that message is entirely fixed text, with no request or state values in it (Smithy: AccessDeniedException httpError 403, "The caller does not have the permissions required to perform the operation").
Emulator: answers HTTP 200 with no __type and no Message, and stores the foreign-account role (the later read shows RoleArn with the foreign account id where AWS still shows the subscriber's own-account role).
Why: a cross-account escalation that AWS blocks is accepted, so a client cannot test its own authorization-failure handling and the emulator's state diverges from what AWS would hold.
Fix: UpdateSubscriber's authorization/validation step — compare the RoleArn account id against the calling account and raise AccessDeniedException(403, "Access denied.") before the mutation.
Traces: eventbridgev2-update-subscriber-rejections-keep-stored-configuration#task#9.

Cause 3 of 4: a refused UpdateSubscriber still mutates the stored subscriber, and the read omits configuration AWS always returns.
AWS: "Updates a subscriber. Fields omitted from the request are left unchanged" (Smithy: UpdateSubscriber) — and an update that is refused changes nothing, so DescribeSubscriber keeps reporting the pre-update configuration: the single stored filter pattern, the FilterConfiguration.Language it was created with (Smithy: FilterConfiguration.Language "Defaults to EVENT_BRIDGE_PATTERN when not specified"), the own-account InvokeConfiguration.RoleArn, and Transformer.Type RAW. DescribeSubscriber also always returns the effective BatchConfiguration (MaxBatchSize/MaxBatchWindowInSeconds) and RetryPolicy (MaxRetryAttempts default 5, MaxEventAgeInSeconds default 300, RetryStrategy default ALL — Smithy: RetryPolicy member docs), whether or not the client set them.
Emulator: after the refused updates the read shows the rejected values applied — FilterConfiguration.Filters[0].Pattern replaced, a second filter appended, InvokeConfiguration.RoleArn pointing at the foreign account, Transformer.Type switched to JSONATA — while BatchConfiguration, RetryPolicy and FilterConfiguration.Language are missing from the response entirely.
Why: a client reading back a subscriber after a failed update sees state AWS never committed and cannot see the defaults it must honour (batch size, retry limits, filter language), so reconciliation loops and drift detection both act on wrong data.
Fix: UpdateSubscriber must validate before it writes (no partial/optimistic mutation of subscriber state), and the DescribeSubscriber serializer must emit the effective BatchConfiguration, RetryPolicy, Transformer and FilterConfiguration.Language.
Traces: eventbridgev2-update-subscriber-rejections-keep-stored-configuration#task#11. The read-side omission overlaps the open issue 20261113T174325Z-eventbridgev2-subscriber-reads-omit-stored-configuration-drop-updates.md, which does not carry this step's replay key and does not cover the rejected-update persistence.

Cause 4 of 4: DescribeSubscriber omits defaulted configuration and reports a blank Description that AWS does not keep.
AWS: DescribeSubscriber returns the effective BatchConfiguration (including MaxBatchWindowInSeconds), RetryPolicy and Transformer for every subscriber (Smithy: DescribeSubscriber output; RetryPolicy defaults MaxRetryAttempts 5 / MaxEventAgeInSeconds 300 / RetryStrategy ALL; Transformer.Type RAW for an untransformed subscriber). Recorded behavior only: an update setting Description to a whitespace-only string leaves the subscriber with no description, so Description is absent from the read rather than returned as " ".
Emulator: BatchConfiguration.MaxBatchWindowInSeconds, RetryPolicy and Transformer are missing from the response, and Description is present with the literal whitespace value.
Why: a client cannot read the batching/retry/transform contract it is about to rely on, and sees a description AWS reports as unset, so a round-trip comparison of its own desired state always mismatches.
Fix: the DescribeSubscriber serializer / subscriber state defaults, plus UpdateSubscriber's Description handling (a whitespace-only description clears the field).
Traces: eventbridgev2-update-subscriber-description-state-logging-and-batching#task#7.

## Evidence
Cause 1 — fixture eventbridgev2-update-subscriber-rejections-keep-stored-configuration, step task#4 (cluster: task#4, task#5, task#6), UpdateSubscriber:
  expected: status 400, $.__type = 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', $.Message = 'LambdaParameters is not supported for target <sqs queue arn>. Expected SqsParameters'
  actual:   status 200, $.__type absent, $.Message absent

Cause 2 — same fixture, step task#9, UpdateSubscriber:
  expected: status 403, $.__type = 'com.amazon.nexus.v20250515.exceptions#AccessDeniedException', $.Message = 'Access denied.'
  actual:   status 200, $.__type absent, $.Message absent

Cause 3 — same fixture, step task#11, DescribeSubscriber (status 200 both sides):
  $.FilterConfiguration.Filters[0].Pattern: expected the pre-update pattern, got the pattern from the refused update
  $.FilterConfiguration.Filters[1]: unexpected extra filter from the refused update
  $.FilterConfiguration.Language: expected 'EVENT_BRIDGE_PATTERN', got absent
  $.InvokeConfiguration.RoleArn: expected the own-account role, got the foreign-account role from the refused update
  $.Transformer.Type: expected 'RAW', got 'JSONATA'
  $.BatchConfiguration: expected {MaxBatchSize, MaxBatchWindowInSeconds}, got absent
  $.RetryPolicy: expected {MaxEventAgeInSeconds 300, MaxRetryAttempts 5, RetryStrategy 'ALL'}, got absent

Cause 4 — fixture eventbridgev2-update-subscriber-description-state-logging-and-batching, step task#7, DescribeSubscriber (status 200 both sides):
  $.BatchConfiguration.MaxBatchWindowInSeconds: expected 0, got absent
  $.RetryPolicy: expected {MaxEventAgeInSeconds 300, MaxRetryAttempts 5, RetryStrategy 'ALL'}, got absent
  $.Transformer: expected {Type 'RAW'}, got absent
  $.Description: expected absent, got ' '

Smithy evidence: UpdateSubscriber errors InvalidInputException (400) and AccessDeniedException (403); UpdateInvokeConfiguration.RoleArn "Must belong to the calling account"; FilterConfiguration.Language defaults to EVENT_BRIDGE_PATTERN; RetryPolicy defaults 5 / 300 / ALL; DescribeSubscriber output includes BatchConfiguration, RetryPolicy, Transformer.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: create a subscriber whose target is an SQS queue, then call UpdateSubscriber with InvokeConfiguration.LambdaParameters — replay fixture eventbridgev2-update-subscriber-rejections-keep-stored-configuration step task#4. Observe 200 instead of 400 InvalidInputException.
3. Cause 2: call UpdateSubscriber on the same subscriber with InvokeConfiguration.RoleArn in a different 12-digit account — step task#9. Observe 200 instead of 403 AccessDeniedException 'Access denied.'.
4. Cause 3: call DescribeSubscriber on that subscriber — step task#11. Observe the refused updates' filter, role and transformer persisted, and BatchConfiguration / RetryPolicy / FilterConfiguration.Language missing.
5. Cause 4: replay fixture eventbridgev2-update-subscriber-description-state-logging-and-batching step task#7 (DescribeSubscriber after an update with a whitespace Description). Observe missing BatchConfiguration.MaxBatchWindowInSeconds / RetryPolicy / Transformer and a Description of ' '.
