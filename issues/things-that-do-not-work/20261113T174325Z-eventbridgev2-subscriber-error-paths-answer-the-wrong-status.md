Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-create-subscriber-filter-and-target-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-create-subscriber-filter-and-target-validation eventbridgev2-create-subscriber-starting-position-transformer-and-universal-target-validation eventbridgev2-describe-and-policy-against-missing-resources eventbridgev2-event-source-and-subscriber-reference-missing-bus eventbridgev2-update-subscriber-rejections-keep-stored-configuration
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|CreateSubscriber|4xx->2xx|$.Message
Issue-Key: eventbridgev2|CreateSubscriber|4xx->2xx|$.__type
Issue-Key: eventbridgev2|CreateSubscriber|4xx->2xx|status
Issue-Key: eventbridgev2|CreateSubscriber|4xx->4xx|$.Message
Issue-Key: eventbridgev2|CreateSubscriber|4xx->4xx|$.__type
Issue-Key: eventbridgev2|CreateSubscriber|4xx->4xx|status
Issue-Key: eventbridgev2|DescribeSubscriber|4xx->4xx|$.Message
Issue-Key: eventbridgev2|UpdateSubscriber|4xx->4xx|$.Message
Issue-Key: eventbridgev2|UpdateSubscriber|4xx->4xx|$.__type
Issue-Commit: 737fb93a0a7c6d2c5001b5b69dbb14ac16ed0b14

# EventBridgeV2 subscriber error paths answer the wrong status, exception type and message

> _Note: this issue was identified against the code base at commit `737fb93a0a7c6d2c5001b5b69dbb14ac16ed0b14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscriber

## Description
Seven net-new root causes in the eventbridgev2 subscriber refusal paths: invalid subscriber input is either accepted outright or refused with a status, __type and Message that are not the ones AWS answers. All seven are wire divergences replayed from the fixture corpus; a fixer must close all seven before submitting. Related open issues that do NOT carry these steps' replay keys: 20261104T173733Z-eventbridgev2-error-responses-omit-modeled-exception-type-and.md, 20261111T025659Z-eventbridgev2-constraint-validation-refusals-answer-validationexception-with-no.md, 20261113T145420Z-eventbridgev2-4-net-new-behavioral-causes-createsubscriber-s.md.

=== Cause 1 of 7: CreateSubscriber accepts an InvokeConfiguration whose parameter block does not match the TargetArn's service ===
AWS: Smithy — InvokeConfiguration carries required TargetArn plus exactly one service-specific parameter block (LambdaParameters / SqsParameters / SnsParameters / KinesisParameters / StepFunctionsParameters / HttpParameters / UniversalTargetParameters / EventBusV2Parameters); CreateSubscriber models InvalidInputException (errorKind client, httpError 400, "A request parameter is missing or not valid"). Recorded response: 400 with __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException' and Message "LambdaParameters is not supported for target <TargetArn>. Expected SqsParameters". Fixed text: "<supplied parameter member> is not supported for target <arn>. Expected <parameter member implied by the target ARN's service>"; the variable parts are the parameter member name taken from the request, the request's TargetArn, and the parameter member the target ARN's service requires.
Emulator: 200 with a created subscriber and no __type / Message at all.
Why: a client that mis-pairs its target and its parameter block is told the subscriber was created; the subscriber exists but can never deliver, and nothing surfaces the misconfiguration.
Fix: CreateSubscriber's input validation in the eventbridgev2 CreateSubscriber handler — the InvokeConfiguration target/parameter-block cross-check.
Traces: eventbridgev2-create-subscriber-filter-and-target-validation#task#3 (plus 5 further steps in the same cluster, incl. eventbridgev2-create-subscriber-starting-position-transformer-and-universal-target-validation#task#3).

=== Cause 2 of 7: a cross-account TargetArn is answered 409 IdempotentParameterMismatchException because the idempotency-token lookup runs before input validation ===
AWS: Smithy — InvokeConfiguration.RoleArn doc "IAM role the service assumes to invoke the target. Must belong to the calling account."; InvalidInputException httpError 400. Recorded response: 400 __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', Message "Target resource account (111111111111) must match the subscriber account (000000000000)". Fixed text: "Target resource account (" <account id parsed from the request's TargetArn> ") must match the subscriber account (" <caller's account id> ")". AWS validates the request before it records or consults the ClientToken, so a request refused 400 never binds its token.
Emulator: 409 with __type 'IdempotentParameterMismatchException' and Message "ClientToken <token> was already used to create subscriber <arn> with different parameters". Because the token lookup happens first, every later invalid-input probe that reuses the fixture's token is answered 409 as well — this single cluster covers 14 replayed steps across two fixtures.
Why: a client gets a retryable-looking conflict instead of a permanent 400, so it retries a request that can never succeed, and the real reason (a cross-account target) is never reported.
Fix: ordering in the eventbridgev2 CreateSubscriber handler — run input validation before the ClientToken lookup, and add the target-account check to that validation.
Traces: eventbridgev2-create-subscriber-filter-and-target-validation#task#4.

=== Cause 3 of 7: an empty filter pattern is refused with an emulator-authored JSON-parser message ===
AWS: Smithy — FilterConfiguration doc: "On CreateSubscriber, Filters is required and must contain at least one Filter with a non-empty Pattern." Recorded response: 400 InvalidInputException with Message 'Filter pattern must not be empty' — the whole message is fixed text with no variable part.
Emulator: 400 (status and __type match) with Message 'FilterConfiguration.Filters: Event pattern is not valid JSON: EOF while parsing a value at line 1 column 0' — the emulator's own wording, leaking its JSON parser's position report.
Why: a client or test asserting the AWS message text fails, and the message describes a parse failure rather than the modelled requirement that the pattern be non-empty.
Fix: the filter validation of the eventbridgev2 CreateSubscriber handler — detect the empty pattern before handing the string to the JSON parser.
Traces: eventbridgev2-create-subscriber-filter-and-target-validation#task#6 (3 steps in the cluster).

=== Cause 4 of 7: a permission refusal is answered 409 IdempotentParameterMismatchException instead of 403 AccessDeniedException ===
AWS: Smithy — AccessDeniedException errorKind client, httpError 403, "The caller does not have the permissions required to perform the operation. This error is also returned when the operation cannot use the AWS KMS key for the event bus." It is a modelled error of CreateSubscriber. Recorded response: 403 __type 'com.amazon.nexus.v20250515.exceptions#AccessDeniedException', Message 'Access denied.' — entirely fixed text.
Emulator: 409 __type 'IdempotentParameterMismatchException' with the ClientToken-reuse message (same validation/idempotency ordering as Cause 2).
Why: an authorization failure is indistinguishable from a conflict; a client's permission-error branch never runs and it retries instead of failing.
Fix: the eventbridgev2 CreateSubscriber handler's authorization check, evaluated before the ClientToken lookup.
Traces: eventbridgev2-create-subscriber-filter-and-target-validation#task#13.

=== Cause 5 of 7: a CreateSubscriber request missing ClientToken is answered 404 for the referenced bus instead of 400 for the missing parameter ===
AWS: recorded behavior only (Smithy models ClientToken as an idempotencyToken, length 1-64, pattern ^[\u0021-\u007E]+$, and does not mark it required; the recording shows the service requiring it). Recorded response: 400 __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', Message 'ClientToken is required' — entirely fixed text. Required-parameter validation precedes the event-bus lookup, so the missing token is reported even though the referenced bus does not exist.
Emulator: 404 __type 'ResourceNotFoundException', Message 'event bus <arn> does not exist'.
Why: a client that omitted a required parameter is told its event bus is missing, so it hunts a non-existent resource problem instead of fixing its request; the status class is wrong too (404 vs 400).
Fix: the eventbridgev2 CreateSubscriber handler — required-parameter validation ahead of resource resolution.
Traces: eventbridgev2-event-source-and-subscriber-reference-missing-bus#task#3.

=== Cause 6 of 7: UpdateSubscriber's Description length refusal answers an unmodelled ValidationException with no Message ===
AWS: Smithy — UpdateSubscriberRequest.Description targets Description with @length(min 0, max 512); InvalidInputException httpError 400 is modelled on UpdateSubscriber, and ValidationException is not among its errors (ConcurrentModification / InvalidState / LimitExceeded / ResourceNotFound / AccessDenied / Internal / InvalidInput / Throttling). Recorded response: 400 __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', Message "1 validation error detected: Value '<supplied value>' at 'description' failed to satisfy constraint: Member must have length less than or equal to 512". Fixed text: "<n> validation error(s) detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must have length less than or equal to " ...; the variable parts are the value from the request, the member path, and the modelled limit.
Emulator: 400 with __type 'ValidationException' and no Message member at all.
Why: a client switching on the exception type sees a code outside the modelled set and cannot classify the failure, and the empty Message leaves it with nothing to tell a user which member was rejected.
Fix: the eventbridgev2 UpdateSubscriber handler's constraint validation and the error serializer that maps a constraint failure to a modelled exception shape with a Message.
Traces: eventbridgev2-update-subscriber-rejections-keep-stored-configuration#task#7 (2 steps in the cluster).

=== Cause 7 of 7: DescribeSubscriber's not-found message is emulator-authored ===
AWS: Smithy — DescribeSubscriber models ResourceNotFoundException (httpError 404). Recorded response: 404 with Message 'Subscriber not found: <SubscriberArn>'. Fixed text: 'Subscriber not found: ' followed by the ARN taken from the request.
Emulator: 404 (status and __type match) with Message 'subscriber <arn> does not exist' — lower-cased, re-worded, ARN in the middle.
Why: a client that matches the message text to distinguish a missing subscriber from another 404 fails; the wording is not AWS's.
Fix: the not-found error construction in the eventbridgev2 DescribeSubscriber handler.
Traces: eventbridgev2-describe-and-policy-against-missing-resources#task#5.

## Evidence
Cause 1 — CreateSubscriber, expected 400, got 200. Expected $.__type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', $.Message 'LambdaParameters is not supported for target arn:aws:sqs:<region>:<account>:<queue>. Expected SqsParameters'; got neither member (6 affected steps).
Cause 2 — CreateSubscriber, expected 400, got 409. Expected __type InvalidInputException / Message 'Target resource account (<target account>) must match the subscriber account (<caller account>)'; got __type 'IdempotentParameterMismatchException' / Message 'ClientToken <token> was already used to create subscriber <arn> with different parameters' (14 affected steps).
Cause 3 — CreateSubscriber 400/400, $.Message value_mismatch: expected 'Filter pattern must not be empty', got 'FilterConfiguration.Filters: Event pattern is not valid JSON: EOF while parsing a value at line 1 column 0' (3 affected steps).
Cause 4 — CreateSubscriber, expected 403, got 409. Expected __type AccessDeniedException / Message 'Access denied.'; got IdempotentParameterMismatchException / ClientToken-reuse message.
Cause 5 — CreateSubscriber, expected 400, got 404. Expected __type InvalidInputException / Message 'ClientToken is required'; got __type 'ResourceNotFoundException' / Message 'event bus <arn> does not exist'.
Cause 6 — UpdateSubscriber 400/400. Expected __type InvalidInputException and Message "1 validation error detected: Value '<513+ char value>' at 'description' failed to satisfy constraint: Member must have length less than or equal to 512"; got __type 'ValidationException' and Message missing (2 affected steps).
Cause 7 — DescribeSubscriber 404/404, $.Message value_mismatch: expected 'Subscriber not found: <subscriber arn>', got 'subscriber <subscriber arn> does not exist'.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: fixture eventbridgev2-create-subscriber-filter-and-target-validation, step task#3 — CreateSubscriber with an SQS TargetArn but LambdaParameters in InvokeConfiguration.
3. Cause 2: same fixture, step task#4 — CreateSubscriber with a TargetArn in a different account from the caller, reusing the fixture's ClientToken.
4. Cause 3: same fixture, step task#6 — CreateSubscriber with FilterConfiguration.Filters containing a Filter whose Pattern is empty.
5. Cause 4: same fixture, step task#13 — CreateSubscriber under a caller lacking permission on the target.
6. Cause 5: fixture eventbridgev2-event-source-and-subscriber-reference-missing-bus, step task#3 — CreateSubscriber omitting ClientToken against a non-existent EventBusArn.
7. Cause 6: fixture eventbridgev2-update-subscriber-rejections-keep-stored-configuration, step task#7 — UpdateSubscriber with a Description longer than 512 characters.
8. Cause 7: fixture eventbridgev2-describe-and-policy-against-missing-resources, step task#5 — DescribeSubscriber on a well-formed but non-existent subscriber ARN.
