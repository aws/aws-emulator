Issue-Source: trace-sweep-divergence
Issue-Trace: events-create-archive-event-source-arn-length-bound
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-create-archive-event-source-arn-length-bound events-describe-endpoint-home-region-length-bound events-list-tags-for-resource-arn-length-bound events-create-event-bus-dead-letter-arn-length-bound events-list-rule-names-by-target-empty-target-arn-rejected events-update-event-bus-log-level-enum events-update-api-destination-invocation-endpoint-escapes
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CreateArchive|4xx->4xx|$.__type
Issue-Key: events|CreateArchive|4xx->4xx|$.message
Issue-Key: events|DescribeEndpoint|4xx->4xx|$.__type
Issue-Key: events|DescribeEndpoint|4xx->4xx|$.message
Issue-Key: events|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: events|ListTagsForResource|4xx->4xx|$.message
Issue-Key: events|CreateEventBus|4xx->4xx|$.message
Issue-Key: events|ListRuleNamesByTarget|4xx->2xx|$.__type
Issue-Key: events|ListRuleNamesByTarget|4xx->2xx|$.message
Issue-Key: events|ListRuleNamesByTarget|4xx->2xx|status
Issue-Key: events|UpdateEventBus|4xx->4xx|$.message
Issue-Key: events|UpdateApiDestination|4xx->4xx|$.message
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# events: malformed-input refusals answer ResourceNotFoundException, a model-derived message, or nothing at all instead of AWS's ValidationException

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateArchive

## Description
7 replayed divergence clusters, 7 root causes. In each, the emulator judges a malformed request against STATE (or against the raw Smithy constraint) instead of refusing its FORMAT the way AWS does. EventBridge does not model ValidationException, so every expected body below is recorded behavior: HTTP 400 with __type 'ValidationException'. AWS uses two message templates here - 'Parameter <offending value> is not valid. Reason: Provided Arn is not in correct format.' when an ARN-shaped member is malformed, and 'Parameter <member path> is not valid.[ Reason: <reason>]' when a named member is semantically wrong.

Cause 1 of 7: CreateArchive validates EventSourceArn's FORM before looking the bus up.
AWS: 400 ValidationException 'Parameter EventSourceArn is not valid. Reason: Must contain an event bus ARN.' for an EventSourceArn that is not an event-bus ARN (Smithy: CreateArchive.EventSourceArn -> EventBusArn, required, @length 1..1600, @pattern ^arn:aws([a-z]|\\-)*:events:([a-z]|\\d|\\-)*:([0-9]{12})?:.+\\/.+$).
Emulator: 400 ResourceNotFoundException 'Event bus <the whole bare value> does not exist.' - it treats the non-ARN string as a bus name and reports absence.
Traces: events-create-archive-event-source-arn-length-bound#task#3.

Cause 2 of 7: DescribeEndpoint validates HomeRegion before resolving the endpoint.
AWS: 400 ValidationException 'Parameter HomeRegion is not valid.' when HomeRegion is not a real region, even though it satisfies the model (Smithy: DescribeEndpoint.HomeRegion -> HomeRegion @length 9..20 @pattern ^[\\-a-z0-9]+$).
Emulator: 400 ResourceNotFoundException 'Endpoint <name> does not exist in region <value>.'
Traces: events-describe-endpoint-home-region-length-bound#task#2 (cluster also covers #task#3).

Cause 3 of 7: ListTagsForResource validates the ARN format before the lookup.
AWS: 400 ValidationException 'Parameter <the offending ARN value> is not valid. Reason: Provided Arn is not in correct format.' - the value is echoed in full (Smithy: ListTagsForResource.ResourceARN -> Arn, required, @length 1..1600, no pattern; the format rule is the service's).
Emulator: 400 ResourceNotFoundException 'Event bus <truncated name parsed out of the ARN> does not exist.'
Traces: events-list-tags-for-resource-arn-length-bound#task#4.

Cause 4 of 7: CreateEventBus answers the raw Smithy length clause for a malformed dead-letter ARN instead of the ARN-format refusal.
AWS: 400 ValidationException 'Parameter  is not valid. Reason: Provided Arn is not in correct format.' - the same template as Cause 3 with an empty value segment (Smithy: DeadLetterConfig.Arn -> ResourceArn @length 1..1600).
Emulator: "1 validation error detected: Value '' at 'deadLetterConfig.arn' failed to satisfy constraint: Member must have length greater than or equal to 1".
Traces: events-create-event-bus-dead-letter-arn-length-bound#task#3.

Cause 5 of 7: ListRuleNamesByTarget does not refuse a TargetArn that is not an ARN at all - it answers 200.
AWS: 400 ValidationException 'Parameter <the offending value> is not valid. Reason: Provided Arn is not in correct format.' (Smithy: ListRuleNamesByTarget.TargetArn -> TargetArn, required, @length 1..1600).
Emulator: HTTP 200 with a rule-name listing and no error members at all.
Traces: events-list-rule-names-by-target-empty-target-arn-rejected#task#3.

Cause 6 of 7: UpdateEventBus misclassifies an out-of-enum LogConfig.IncludeDetail as a missing required argument.
AWS: 400 ValidationException 'Parameter IncludeDetail is not valid.' (Smithy: LogConfig.IncludeDetail -> IncludeDetail enum, NOT required; Level likewise).
Emulator: 'Parameter LogConfig.IncludeDetail is not valid. Reason: LogConfig.IncludeDetail is a required argument.' - wrong member path and a required-argument reason for a member the model does not require.
Traces: events-update-event-bus-log-level-enum#task#5.

Cause 7 of 7: a @pattern refusal strips the anchors from the pattern it quotes.
AWS: quotes the model's pattern verbatim, anchors included: "1 validation error detected: Value '<endpoint>' at 'invocationEndpoint' failed to satisfy constraint: Member must satisfy regular expression pattern: ^((%[0-9A-Fa-f]{2}|[-()_.!~*';/?:@\\x26=+$,A-Za-z0-9])+)([).!';/?:,])?$" (Smithy: UpdateApiDestination.InvocationEndpoint -> HttpsEndpoint, exactly that pattern).
Emulator: emits the same sentence with the leading ^ and trailing $ removed.
Traces: events-update-api-destination-invocation-endpoint-escapes#task#8.

Why (all causes): error CLASS, not just wording, is part of the contract. A client retrying on ResourceNotFoundException (or creating the 'missing' bus) loops forever on a request that is simply malformed; a client that trusts a 200 from ListRuleNamesByTarget acts on a result for an ARN it never addressed; and clients that match AWS's 'Parameter ... is not valid' text - or compile the quoted pattern - get the wrong reason or an unanchored regex that accepts values AWS rejects.
Fix: the per-operation input validation in the events handlers for CreateArchive, DescribeEndpoint, ListTagsForResource, CreateEventBus, ListRuleNamesByTarget and UpdateEventBus - ARN/parameter FORM must be checked, and refused with ValidationException, before any state lookup - plus the shared pattern-clause renderer for Cause 7.

## Evidence
Cause 1 (CreateArchive, events-create-archive-event-source-arn-length-bound#task#3, 400/400): "$.__type: value_mismatch (expected 'ValidationException', got 'ResourceNotFoundException')"; "$.message: value_mismatch (expected 'Parameter EventSourceArn is not valid. Reason: Must contain an event bus ARN.', got 'Event bus bbbb...bbb does not exist.')".
Cause 2 (DescribeEndpoint, events-describe-endpoint-home-region-length-bound#task#2, 400/400): expected ('ValidationException', 'Parameter HomeRegion is not valid.'), got ('ResourceNotFoundException', 'Endpoint sim2real-probe-evhr-missing-05a8d2d03e does not exist in region aaaaaaaaa.').
Cause 3 (ListTagsForResource, events-list-tags-for-resource-arn-length-bound#task#4, 400/400): expected ('ValidationException', 'Parameter arn:aws:events:us-west-2:000000000000:event-bus/<long name> is not valid. Reason: Provided Arn is not in correct format.'), got ('ResourceNotFoundException', 'Event bus <name truncated out of the ARN> does not exist.').
Cause 4 (CreateEventBus, events-create-event-bus-dead-letter-arn-length-bound#task#3, 400/400): expected 'Parameter  is not valid. Reason: Provided Arn is not in correct format.', got "1 validation error detected: Value '' at 'deadLetterConfig.arn' failed to satisfy constraint: Member must have length greater than or equal to 1".
Cause 5 (ListRuleNamesByTarget, events-list-rule-names-by-target-empty-target-arn-rejected#task#3): "status: expected 400, got 200"; "$.__type: missing (expected 'ValidationException', got None)"; "$.message: missing (expected 'Parameter q is not valid. Reason: Provided Arn is not in correct format.', got None)".
Cause 6 (UpdateEventBus, events-update-event-bus-log-level-enum#task#5, 400/400): expected 'Parameter IncludeDetail is not valid.', got 'Parameter LogConfig.IncludeDetail is not valid. Reason: LogConfig.IncludeDetail is a required argument.'
Cause 7 (UpdateApiDestination, events-update-api-destination-invocation-endpoint-escapes#task#8, 400/400): expected pattern clause '... pattern: ^((%[0-9A-Fa-f]{2}|[-()_.!~*\';/?:@\\x26=+$,A-Za-z0-9])+)([).!\';/?:,])?$', got the same clause without ^ and $.

## Steps to Reproduce
1. CreateArchive with EventSourceArn set to a bare (non-ARN) string - expect ValidationException 'Parameter EventSourceArn is not valid. Reason: Must contain an event bus ARN.', observe ResourceNotFoundException.
2. DescribeEndpoint with HomeRegion='aaaaaaaaa' (model-valid, not a region) - expect 'Parameter HomeRegion is not valid.'
3. ListTagsForResource with a malformed/over-long ARN - expect 'Parameter <value> is not valid. Reason: Provided Arn is not in correct format.'
4. CreateEventBus with DeadLetterConfig.Arn='' - expect the same ARN-format message with an empty value segment.
5. ListRuleNamesByTarget with a TargetArn that is not an ARN - expect a 400, observe 200.
6. UpdateEventBus with LogConfig.IncludeDetail set outside the enum - expect 'Parameter IncludeDetail is not valid.'
7. UpdateApiDestination with an InvocationEndpoint containing a space - compare the quoted pattern's anchors.
8. Re-replay the fixtures listed per cause.
