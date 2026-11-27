Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2.jsonata-transformer-expression-over-8192-characters-rejected eventbridgev2.universal-target-jsonata-input-over-262144-characters-rejected eventbridgev2.subscriber-filters-over-combined-size-rejected eventbridgev2.deduplication-id-publishes-again-after-window-elapses
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2.jsonata-transformer-expression-over-8192-characters-rejected|run#10|AssertionError|assert got == ("InvalidInputException", _LENGTH_MESSAGE), (
Issue-Key: eventbridgev2.universal-target-jsonata-input-over-262144-characters-rejected|run#10|AssertionError|assert got == ("InvalidInputException", _LENGTH_MESSAGE), (
Issue-Key: eventbridgev2.subscriber-filters-over-combined-size-rejected|_rejection#11|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: eventbridgev2.deduplication-id-publishes-again-after-window-elapses|run#21|AssertionError|assert after["SuccessCode"] == "PUBLISHED", (
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# eventbridgev2: 3 net-new causes — length-validation messages echo the sensitive value AWS omits, the 4096-byte combined FilterConfiguration cap is not enforced, and a DeduplicationId never leaves its 300-second window

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscriber

## Description
Three independent root causes, all in the eventbridgev2 crate. A fixer must close all three before submitting.

=== Cause 1 of 3: the 'N validation error detected' message echoes the offending value for @sensitive members, where AWS omits it. Failing checks: eventbridgev2.jsonata-transformer-expression-over-8192-characters-rejected, eventbridgev2.universal-target-jsonata-input-over-262144-characters-rejected ===
AWS: both over-length requests are refused 400 InvalidInputException (Smithy: CreateSubscriber error InvalidInputException, errorKind client, httpError 400) and the message follows AWS's constraint-violation template with the VALUE OMITTED, because the containing shape is sensitive. Smithy: `JsonataConfiguration` @sensitive, member `Expression` @length min 1 max 8192, @pattern ^\{%[\s\S]+?%\}(?![\s\S]); `UniversalTargetParameters` @sensitive, member `Input` @length min 1 max 262144. The required text is, fixed parts verbatim: `1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must have length less than or equal to <max>` — the only variable parts are the member path taken from the request (`transformer.jsonataConfiguration.expression`, `invokeConfiguration.universalTargetParameters.input`) and the modelled maximum (8192, 262144). No `Value '<...>'` segment and no `[N bytes]` note appear.
Emulator: it emits the non-sensitive form, splicing the submitted value into the message. For the transformer it answered InvalidInputException with `1 validation error detected: Value '{% 'jjjj…' at 'transformer.jsonataConfiguration.expression' failed to satisfy constraint: Member must have length less than or equal to 8192`, and for the universal target `1 validation error detected: Value '{% {"QueueUrl": …' at 'invokeConfiguration.universalTargetParameters.input' failed to satisfy constraint: Member must have length less than or equal to 262144`, with a `... [262145 bytes]` elision marker the AWS template does not contain. The code, status, member path and limit are right; only the echoed value is wrong.
Why: the emulator leaks into an error body (and therefore into client logs) the very payload the model marks @sensitive — a JSONata transformer and a universal-target request body, which routinely embed credentials, tokens and customer data. Clients that assert on, or redact by, AWS's exact message also break, and the multi-kilobyte echo inflates every refusal.
Fix: the eventbridgev2 request-validation message formatter — the component that renders the `N validation error detected` template must drop the `Value '<value>' ` segment (rendering `Value at '<path>'`) whenever the member or its enclosing structure carries @sensitive.

=== Cause 2 of 3: CreateSubscriber does not enforce the 4096-byte combined FilterConfiguration size, so an over-size filter set is accepted and the subscriber is created. Failing check: eventbridgev2.subscriber-filters-over-combined-size-rejected ===
AWS: a subscriber's filters are capped in aggregate: a FilterConfiguration whose filter patterns total more than 4096 bytes is refused 400 InvalidInputException with the fixed message `FilterConfiguration exceeds maximum size of 4096 bytes` (no variable parts), and nothing is created — ListSubscribers with that NamePrefix stays empty. Smithy constrains only the parts: `Filter.Pattern` @length min 0 max 4096 (@sensitive), `FilterConfiguration.Filters` @length min 1 max 50, so the COMBINED cap is recorded behavior only (EventBridge quotas, https://docs.aws.amazon.com/eventbridge/latest/userguide/eb-quota.html); each half of the refused pair (one 2048-character DATA pattern, one 2048-character METADATA pattern) is accepted on its own, which the check confirms.
Emulator: the create of a subscriber carrying BOTH 2048-character patterns returned HTTP 200 with a full CreateSubscriberResponse (SubscriberArn/Name/EventBusArn/Type UNORDERED/StartingPosition LATEST/State STOPPED/CreationTime) instead of refusing. Only the per-pattern 4096 limit appears to be checked; the sum across Filters is not.
Why: a client that relies on AWS refusing an over-budget filter set gets a subscriber in the emulator that cannot exist in AWS, so the same template deploys in test and fails in production; and the emulator never exercises the client's handling of InvalidInputException for this class.
Fix: CreateSubscriber's (and UpdateSubscriber's) input validation in the eventbridgev2 crate — add the aggregate FilterConfiguration size check, evaluated before any state is written so the refused create stores nothing.

=== Cause 3 of 3: PutEvents deduplication state never expires — the same DeduplicationId is still DEDUPLICATED after the 300-second window. Failing check: eventbridgev2.deduplication-id-publishes-again-after-window-elapses ===
AWS: a DeduplicationId suppresses a repeat only inside the fixed 300-second deduplication interval (EventBridge quotas, https://docs.aws.amazon.com/eventbridge/latest/userguide/eb-quota.html; Smithy documents the member only as 'Deduplication ID for FIFO deduplication.', so the window length is recorded behavior only). Inside the window a repeat answers the first event's EventId and SequenceNumber with SuccessCode `DEDUPLICATED`; once the window has elapsed, the same id publishes a NEW event — SuccessCode `PUBLISHED`, a fresh EventId distinct from every earlier one, and a SequenceNumber greater than the preceding publishes'. FailedEntryCount stays 0 throughout.
Emulator: after sleeping out the 300-second window plus a margin, and retrying for a further ~2 minutes with backoff, PutEvents for the same DeduplicationId still answered `SuccessCode: DEDUPLICATED` carrying the first event's EventId and SequenceNumber ('1'). The in-window behavior is correct (immediate repeat DEDUPLICATED with the first ids, a different id PUBLISHED), so the defect is specifically that the dedup entry is kept forever instead of being aged out at 300 seconds.
Why: a producer that legitimately re-sends a business key after the window — a retry hours later, a daily job reusing a stable id — has its events silently swallowed by the emulator and never delivered to subscribers, with a success-shaped response and FailedEntryCount 0 that hides the loss. Behavior diverges more the longer a test runs.
Fix: the PutEvents deduplication state in the eventbridgev2 crate — each DeduplicationId entry must record its first-publish instant and be treated as absent once 300 seconds have elapsed, so a later publish mints a new EventId and a strictly increasing SequenceNumber.

Related but NOT covering any of these: 20261121T230755Z-eventbridgev2-listsubscribers-returns-a-phantom-subscriber-left-behind (a refused create leaving a phantom; here cause 2 is the create not being refused at all), 20261127T021207Z-eventbridgev2-creates-no-longer-refuse-a-clienttoken-less (ClientToken handling and validation ordering), and 20261113T174325Z-eventbridgev2-subscriber-error-paths-answer-the-wrong-status (subscriber error status/type, not these messages).

## Evidence
Cause 1 — expected vs actual:
  Expected (AWS): ('InvalidInputException', "1 validation error detected: Value at 'transformer.jsonataConfiguration.expression' failed to satisfy constraint: Member must have length less than or equal to 8192")
  Actual: ('InvalidInputException', "1 validation error detected: Value '{% 'jjjjjjjj…[8193-character value spliced in]' at 'transformer.jsonataConfiguration.expression' failed to satisfy constraint: Member must have length less than or equal to 8192")
  Expected (AWS): ('InvalidInputException', "1 validation error detected: Value at 'invokeConfiguration.universalTargetParameters.input' failed to satisfy constraint: Member must have length less than or equal to 262144")
  Actual: ('InvalidInputException', '1 validation error detected: Value \'{% {"QueueUrl": …"MessageBody": "uuuu…... [262145 bytes]\' at \'invokeConfiguration.universalTargetParameters.input\' failed to satisfy constraint: Member must have length less than or equal to 262144')
  Fixed text: "1 validation error detected: Value at '" + path + "' failed to satisfy constraint: Member must have length less than or equal to " + max. Variable parts: the member path (from the request) and the modelled maximum. The value itself and the '[N bytes]' note are NOT part of the template. Both members are @sensitive in the model.

Cause 2 — expected vs actual:
  Expected (AWS): CreateSubscriber with Filters=[{Scope:DATA, Pattern:<2048 chars>}, {Scope:METADATA, Pattern:<2048 chars>}] is refused ('InvalidInputException', 'FilterConfiguration exceeds maximum size of 4096 bytes') and ListSubscribers(NamePrefix=<name>) stays [].
  Actual: HTTP 200 with {'SubscriberArn': 'arn:aws:events:<region>:<account>:subscriber/<name>/<id>', 'Name': <name>, 'EventBusArn': 'arn:aws:events:<region>:<account>:event-busv2/<bus>/<id>', 'Type': 'UNORDERED', 'StartingPosition': 'LATEST', 'State': 'STOPPED', 'CreationTime': <ts>} — the subscriber was created. (ARNs/ids/timestamps above are per-run values, not requirements.) Each 2048-character filter alone was accepted and read back intact, confirming only the aggregate check is missing.

Cause 3 — expected vs actual:
  Expected (AWS): after the 300-second window elapses, PutEvents with the same DeduplicationId answers SuccessCode 'PUBLISHED' with a new EventId and a SequenceNumber greater than the earlier publishes'.
  Actual: AssertionError: the id was still deduplicated after the 300-second window: {'EventId': <first event's id>, 'SequenceNumber': '1', 'SuccessCode': 'DEDUPLICATED'} — still returned after a further ~6 retries with backoff. In-window legs passed (immediate repeat DEDUPLICATED echoing the first EventId/SequenceNumber; a different id PUBLISHED).

## Steps to Reproduce
Cause 1:
1. CreateEventBus, wait for State ACTIVE; create an SQS queue and an IAM role events.amazonaws.com may assume with sqs:SendMessage on it.
2. With client-side parameter validation disabled, call CreateSubscriber(..., Transformer={Type:JSONATA, JsonataConfiguration:{Expression:<8193-character '{% \'j…\' %}' expression>}}).
   -> Observed message splices the 8193-character expression after "Value ". Expected "Value at 'transformer.jsonataConfiguration.expression'".
3. Likewise CreateSubscriber with InvokeConfiguration.UniversalTargetParameters.Input of 262145 characters (TargetArn arn:aws:events:::aws-sdk:sqs:sendMessage).
   -> Observed message splices the value and adds '... [262145 bytes]'. Expected "Value at 'invokeConfiguration.universalTargetParameters.input'".

Cause 2:
1. CreateEventBus, wait ACTIVE; build an InvokeConfiguration naming an own queue and role.
2. CreateSubscriber with a single {Scope:DATA, Pattern:<2048 chars>} — accepted (correct); again with a single {Scope:METADATA, Pattern:<2048 chars>} — accepted (correct).
3. CreateSubscriber with BOTH of those filters in one FilterConfiguration.
   -> Observed: HTTP 200, subscriber created. Expected: 400 InvalidInputException 'FilterConfiguration exceeds maximum size of 4096 bytes', and ListSubscribers(NamePrefix=<name>) == [].

Cause 3:
1. CreateEventBus, wait for State ACTIVE.
2. PutEvents one entry with SystemMetadata.DeduplicationId=<id> -> SuccessCode PUBLISHED (correct).
3. PutEvents the same id immediately -> DEDUPLICATED with the first EventId/SequenceNumber (correct); a different id -> PUBLISHED (correct).
4. Sleep until 300 s after the first publish, plus a margin, then PutEvents the same id again, retrying with backoff.
   -> Observed: still DEDUPLICATED with the first event's EventId and SequenceNumber. Expected: PUBLISHED with a new EventId and a greater SequenceNumber.
