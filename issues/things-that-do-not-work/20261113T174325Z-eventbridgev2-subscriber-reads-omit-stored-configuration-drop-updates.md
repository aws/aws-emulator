Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-update-subscriber-description-state-logging-and-batching
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-update-subscriber-description-state-logging-and-batching eventbridgev2-update-subscriber-rejections-keep-stored-configuration eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.BatchConfiguration
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.CreationTime
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.Description
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.LastModifiedTime
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.LogConfiguration
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.RetryPolicy
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.State
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.Transformer
Issue-Key: eventbridgev2|DescribeSubscriber|2xx->2xx|$.FilterConfiguration.Language
Issue-Key: eventbridgev2|CreateSubscriber|2xx->2xx|$.CreationTime
Issue-Key: eventbridgev2|DescribeSubscriber|4xx->2xx|$
Issue-Key: eventbridgev2|DescribeSubscriber|4xx->2xx|status
Issue-Commit: 737fb93a0a7c6d2c5001b5b69dbb14ac16ed0b14

# EventBridgeV2 subscriber reads omit stored configuration, drop updates and survive deletion

> _Note: this issue was identified against the code base at commit `737fb93a0a7c6d2c5001b5b69dbb14ac16ed0b14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubscriber

## Description
Six net-new root causes in the eventbridgev2 subscriber read path and subscriber state: accepted updates are not applied, service-applied defaults are never returned, timestamps are serialized outside the recorded shape, and a deleted subscriber stays readable forever. A fixer must close all six before submitting. Related open issues that do NOT carry these steps' replay keys: 20261111T025659Z-eventbridgev2-event-bus-and-event-source-updates-are.md (event buses and event sources, not subscribers), 20261111T025659Z-eventbridgev2-reads-omit-revoked-and-serialize-timestamps-outside.md (event-source reads), 20261111T025659Z-eventbridgev2-deleted-event-sources-stay-readable-describeeventsource-answers.md (event sources, not subscribers), 20261113T094431Z-eventbridgev2-iam-sqs-9-net-new-causes-putevents.md (behavioral, UpdateSubscriber stub).

=== Cause 1 of 6: UpdateSubscriber's Description, State and LogConfiguration are accepted but never applied ===
AWS: Smithy — UpdateSubscriber (idempotent) takes Description (@length 0-512), State (SubscriberState enum RUNNING|STOPPED, "Customer-controlled run state of a subscriber, set on create or update") and LogConfiguration (Level: LogLevel, IncludePayload); DescribeSubscriber's output echoes Description, State and LogConfiguration. Recorded behavior: after an accepted UpdateSubscriber the following DescribeSubscriber reports the new Description, State STOPPED and the supplied LogConfiguration (Level ERROR in the recording).
Emulator: DescribeSubscriber still reports State 'RUNNING' and omits Description and LogConfiguration entirely — the update is answered but not stored.
Why: a client cannot pause a subscriber or change its logging; it reads back RUNNING after being told STOPPED, so an operator believes delivery is stopped when it is not.
Fix: the eventbridgev2 UpdateSubscriber handler and the subscriber record in service state (the write), plus the DescribeSubscriber serializer for those members.
Traces: eventbridgev2-update-subscriber-description-state-logging-and-batching#task#5.

=== Cause 2 of 6: UpdateSubscriber's BatchConfiguration change and a cleared LogConfiguration are likewise not applied ===
AWS: Smithy — BatchConfiguration{MaxBatchSize @range 1-500, MaxBatchWindowInSeconds @range 0-300}; LogConfiguration members default (Level "Defaults to OFF", IncludePayload "Defaults to ON_ERROR_ONLY"). Recorded behavior: after UpdateSubscriber sets BatchConfiguration MaxBatchSize to a new value and supplies an empty LogConfiguration, DescribeSubscriber reports BatchConfiguration {MaxBatchSize: <new value>, MaxBatchWindowInSeconds: <stored window>} and LogConfiguration {}.
Emulator: both members are absent from the response.
Why: batching and logging are unconfigurable and unobservable; a client tuning batch size cannot confirm or read back its setting.
Fix: same as Cause 1 — the UpdateSubscriber write path and the DescribeSubscriber serializer.
Traces: eventbridgev2-update-subscriber-description-state-logging-and-batching#task#7.

=== Cause 3 of 6: after a refused UpdateSubscriber, DescribeSubscriber omits the stored configuration and the filter language ===
AWS: Smithy — FilterConfiguration.Language doc "Defaults to EVENT_BRIDGE_PATTERN when not specified"; RetryPolicy docs "MaxRetryAttempts (0-185, default: 5)", "MaxEventAgeInSeconds (60-86400, default: 300)", "RetryStrategy ... Default: ALL"; Transformer.Type (RAW|WITH_METADATA|JSONATA). Recorded behavior: a refused update leaves the stored configuration intact and readable — DescribeSubscriber reports FilterConfiguration.Language 'EVENT_BRIDGE_PATTERN', BatchConfiguration {MaxBatchSize: 10, MaxBatchWindowInSeconds: 0}, RetryPolicy {MaxRetryAttempts: 5, MaxEventAgeInSeconds: 300, RetryStrategy: 'ALL'} and Transformer {Type: 'RAW'}.
Emulator: FilterConfiguration.Language, BatchConfiguration, RetryPolicy and Transformer are all absent.
Why: a client cannot verify that a rejected update left its subscriber unchanged, and cannot discover the effective retry, batching or filter-language settings at all.
Fix: the eventbridgev2 DescribeSubscriber serializer plus the default-materialisation of these members on the stored subscriber record.
Traces: eventbridgev2-update-subscriber-rejections-keep-stored-configuration#task#11.

=== Cause 4 of 6: a freshly created subscriber reads back without its service-applied BatchConfiguration and RetryPolicy ===
AWS: Smithy — same defaults as Cause 3 (RetryPolicy MaxRetryAttempts default 5, MaxEventAgeInSeconds default 300, RetryStrategy default ALL; BatchConfiguration MaxBatchSize/MaxBatchWindowInSeconds). Recorded behavior: a subscriber created without BatchConfiguration or RetryPolicy reads back with BatchConfiguration {MaxBatchSize: 10, MaxBatchWindowInSeconds: 0} and RetryPolicy {MaxRetryAttempts: 5, MaxEventAgeInSeconds: 300, RetryStrategy: 'ALL'} — the service materialises the defaults on create.
Emulator: both members are absent from DescribeSubscriber.
Why: a client reading a subscriber to learn its effective retry and batching behaviour (e.g. to size a dead-letter budget) gets nothing and must guess.
Fix: default materialisation in the eventbridgev2 CreateSubscriber handler's state write, and the DescribeSubscriber serializer.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#8.

=== Cause 5 of 6: subscriber timestamps are serialized outside the recorded shape ===
AWS: recorded behavior only for the encoding. Smithy — CreateSubscriberResponse.CreationTime and DescribeSubscriberResponse.CreationTime / LastModifiedTime target the Timestamp shape, and the service (AWSEventsV2 2025-05-15) speaks rpcv2Cbor, so those members travel in the protocol's timestamp representation. The fixture asserts the SHAPE of these members and deliberately not their value, because the instant differs per run.
Emulator: emits a bare non-integral double of epoch seconds (e.g. CreationTime 1794591136.286888), which does not satisfy the recorded timestamp shape. This is a shape divergence, not a clock-value difference.
Why: an SDK decoding the modelled timestamp member gets a raw number where a timestamp is expected, so the member either fails to deserialize or surfaces as a float a client must convert itself.
Fix: the timestamp serializer used by the eventbridgev2 CreateSubscriber / DescribeSubscriber responses.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#7.

=== Cause 6 of 6: a deleted subscriber stays readable — DescribeSubscriber answers 200 forever after DeleteSubscriber ===
AWS: Smithy — DeleteSubscriber (idempotent) returns an empty output; DescribeSubscriber models ResourceNotFoundException (httpError 404). Recorded behavior: after DeleteSubscriber the subscriber stops resolving and DescribeSubscriber settles on 404 ResourceNotFoundException (its Message is 'Subscriber not found: <SubscriberArn>').
Emulator: DescribeSubscriber keeps answering 200 with the full subscriber record (State 'RUNNING') — the replay polled 37 times over 180 s and never saw a ResourceNotFoundException, in three separate fixtures' teardown.
Why: deletion is unobservable, so a client that waits for a subscriber to disappear waits forever, and a name/ARN that should be free still resolves.
Fix: the eventbridgev2 DeleteSubscriber handler's state removal (or tombstoning) and DescribeSubscriber's lookup against it.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#teardown#12 (3 affected steps, incl. eventbridgev2-update-subscriber-description-state-logging-and-batching#teardown#9 and eventbridgev2-update-subscriber-rejections-keep-stored-configuration#teardown#13).

## Evidence
Cause 1 — DescribeSubscriber 200/200: $.State value_mismatch (expected 'STOPPED', got 'RUNNING'); $.Description missing (expected 'paused for maintenance'); $.LogConfiguration missing (expected {'Level': 'ERROR'}); $.BatchConfiguration missing (expected {'MaxBatchSize': 10, 'MaxBatchWindowInSeconds': 0}); $.RetryPolicy missing (expected {'MaxEventAgeInSeconds': 300, 'MaxRetryAttempts': 5, 'RetryStrategy': 'ALL'}); $.Transformer missing (expected {'Type': 'RAW'}).
Cause 2 — DescribeSubscriber 200/200: $.BatchConfiguration missing (expected {'MaxBatchSize': 5, 'MaxBatchWindowInSeconds': 0}); $.LogConfiguration missing (expected {}); $.RetryPolicy and $.Transformer missing.
Cause 3 — DescribeSubscriber 200/200: $.FilterConfiguration.Language missing (expected 'EVENT_BRIDGE_PATTERN'); $.BatchConfiguration missing (expected {'MaxBatchSize': 10, 'MaxBatchWindowInSeconds': 0}); $.RetryPolicy missing (expected {'MaxEventAgeInSeconds': 300, 'MaxRetryAttempts': 5, 'RetryStrategy': 'ALL'}); $.Transformer missing (expected {'Type': 'RAW'}).
Cause 4 — DescribeSubscriber 200/200: $.BatchConfiguration missing (expected {'MaxBatchSize': 10, 'MaxBatchWindowInSeconds': 0}); $.RetryPolicy missing (expected {'MaxEventAgeInSeconds': 300, 'MaxRetryAttempts': 5, 'RetryStrategy': 'ALL'}).
Cause 5 — CreateSubscriber 200/200: $.CreationTime value_mismatch (expected the recorded timestamp shape, asserted as '<shaped-not-asserted>'; got the bare double 1794591136.286888). The same member diverges the same way on DescribeSubscriber in causes 1-4.
Cause 6 — DescribeSubscriber, expected 404, got 200: poll capped at 180s, 'error code None not in [ResourceNotFoundException] after 37 attempt(s)'; the response was the full subscriber {SubscriberArn, Name, EventBusArn, InvokeConfiguration, Type 'UNORDERED', StartingPosition 'POINT_IN_TIME', Transformer {Type 'RAW'}, State 'RUNNING', ...} after DeleteSubscriber had succeeded.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: fixture eventbridgev2-update-subscriber-description-state-logging-and-batching, step task#5 — UpdateSubscriber with Description, State STOPPED and LogConfiguration {Level: ERROR}, then DescribeSubscriber.
3. Cause 2: same fixture, step task#7 — UpdateSubscriber with BatchConfiguration {MaxBatchSize: 5} and an empty LogConfiguration, then DescribeSubscriber.
4. Cause 3: fixture eventbridgev2-update-subscriber-rejections-keep-stored-configuration, step task#11 — DescribeSubscriber after a refused UpdateSubscriber.
5. Cause 4: fixture eventbridgev2-subscriber-delivers-published-event-to-sqs, step task#8 — DescribeSubscriber on a subscriber created without BatchConfiguration or RetryPolicy.
6. Cause 5: same fixture, step task#7 — read CreationTime from the CreateSubscriber response.
7. Cause 6: same fixture, step teardown#12 — DeleteSubscriber, then poll DescribeSubscriber for ResourceNotFoundException.
