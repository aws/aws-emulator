Issue-Source: trace-sweep-divergence
Issue-Trace: events-start-replay-filter-arns-refuses-the-archive-rule-and-a-bare-name
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-start-replay-filter-arns-refuses-the-archive-rule-and-a-bare-name events-start-replay-filter-rule-and-destination-bus-must-exist
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|StartReplay|4xx->2xx|$.__type
Issue-Key: events|StartReplay|4xx->2xx|$.message
Issue-Key: events|StartReplay|4xx->2xx|status
Issue-Key: events|StartReplay|4xx->4xx|$.__type
Issue-Key: events|DescribeReplay|4xx->2xx|$.__type
Issue-Key: events|DescribeReplay|4xx->2xx|$.message
Issue-Key: events|DescribeReplay|4xx->2xx|status
Issue-Key: events|StartReplay|2xx->4xx|$.ReplayArn
Issue-Key: events|StartReplay|2xx->4xx|$.ReplayStartTime
Issue-Key: events|StartReplay|2xx->4xx|$.State
Issue-Key: events|StartReplay|2xx->4xx|status
Issue-Key: events|DescribeReplay|2xx->2xx|$.Destination.FilterArns[*]
Issue-Key: events|DescribeRule|2xx->2xx|$.ManagedBy
Issue-Related: 20261127T145950Z-events-control-plane-responses-serialize-timestamps-with-microsecond.md
Issue-Related: 20261127T145950Z-events-list-member-constraint-refusals-are-reported-at.md
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# events: StartReplay accepts replay destination FilterArns AWS refuses, then mints a phantom replay that blocks the valid one

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplay

## Description
6 replayed divergence clusters, 5 root causes, all in StartReplay's Destination.FilterArns validation and its fallout. EventBridge models no ValidationException, so the refusal bodies are recorded behavior: HTTP 400 with __type 'ValidationException'.

Cause 1 of 5: StartReplay accepts a FilterArns entry naming a SERVICE-MANAGED rule.
AWS: refuses with 400 ValidationException 'Parameter Destination.FilterArns is not valid. Reason: Must not be service rule.' - the fixed text is 'Parameter Destination.FilterArns is not valid. Reason: Must not be service rule.' and the member path is the request's (Smithy: StartReplay.Destination -> ReplayDestination.FilterArns -> ReplayDestinationFilters, member Arn @length 1..1600; the service-rule exclusion is the service's own rule). The archive's own managed rule (the 'Events-Archive-<archive>' rule the archive creates) is such a rule.
Emulator: answers HTTP 200 and starts a replay.
Traces: events-start-replay-filter-arns-refuses-the-archive-rule-and-a-bare-name#task#6.

Cause 2 of 5: StartReplay does not require FilterArns entries to be rule ARNs.
AWS: refuses a bare rule NAME with 400 ValidationException 'Parameter Destination.FilterArns is not valid. Reason: Must contain rules ARNs.'
Emulator: refuses with the wrong class and message - 400 ResourceAlreadyExistsException 'Replay <name> already exists.' - because Cause 1 already created a replay under that name; the FilterArns form is never judged at all.
Traces: events-start-replay-filter-arns-refuses-the-archive-rule-and-a-bare-name#task#8 (cluster also covers events-start-replay-filter-rule-and-destination-bus-must-exist#task#3 and #task#4).

Cause 3 of 5: DescribeReplay echoes a FilterArns entry the caller never sent.
AWS: returns Destination.FilterArns exactly as accepted - the rule ARN from the request.
Emulator: returns the ARN of the archive's managed rule ('.../rule/<bus>/Events-Archive-<archive>') in place of the requested rule ARN ('.../rule/<bus>/<rule>'), so the replay's destination filter cannot be read back.
Traces: events-start-replay-filter-arns-refuses-the-archive-rule-and-a-bare-name#task#11 (the same cluster also carries this issue's sibling timestamp defect, filed separately).

Cause 4 of 5: the phantom replay created by Cause 1 is observable and poisons the replay name.
AWS: since the StartReplay in Cause 1 is refused, no replay exists - DescribeReplay answers 400 ResourceNotFoundException 'Replay <name> does not exist.', and a later, well-formed StartReplay for that name succeeds with 200 (ReplayArn, ReplayStartTime, State).
Emulator: DescribeReplay answers 200 describing the phantom replay, and the later legitimate StartReplay is refused 400 (name already taken), so the client can never start the replay it is entitled to.
Traces: events-start-replay-filter-arns-refuses-the-archive-rule-and-a-bare-name#task#7 (DescribeReplay, cluster also covers #task#9), #task#10 (StartReplay 200 -> 400).

Cause 5 of 5: DescribeRule on the archive's managed rule reports a ManagedBy value the emulated environment does not serve.
AWS/recorded: the fixture asserts only the SHAPE of ManagedBy (Smithy: DescribeRule output ManagedBy -> ManagedBy @length 1..128) and the replay verdict is 'host_not_own' - the emulator answered with the literal AWS-owned host 'events.amazonaws.com' rather than a principal/host belonging to the environment it is reached on.
Emulator: ManagedBy = 'events.amazonaws.com'.
Traces: events-start-replay-filter-arns-refuses-the-archive-rule-and-a-bare-name#task#5.

Why: a replay is a bulk re-delivery of archived events. Accepting a destination filter AWS rejects means events are re-delivered through the archive's own managed rule - fan-out the client never asked for - while the API reports success; and because the refused name is consumed, the corrective, well-formed StartReplay is then refused as a duplicate, so the client is wedged with a replay it cannot describe correctly and cannot re-create.
Fix: StartReplay's Destination.FilterArns validation in the events replay handler (reject non-rule-ARN entries and service-managed rules BEFORE any replay record is created), the replay record's FilterArns storage/serialization read back by DescribeReplay, and the ManagedBy value the rule state assigns to archive-managed rules.

## Evidence
Cause 1 (StartReplay, ...#task#6): "status: expected 400, got 200"; "$.__type: missing (expected 'ValidationException', got None)"; "$.message: missing (expected 'Parameter Destination.FilterArns is not valid. Reason: Must not be service rule.', got None)".
Cause 2 (StartReplay, ...#task#8, 400/400): "$.__type: value_mismatch (expected 'ValidationException', got 'ResourceAlreadyExistsException')"; "$.message: value_mismatch (expected 'Parameter Destination.FilterArns is not valid. Reason: Must contain rules ARNs.', got 'Replay <name> already exists.')".
Cause 3 (DescribeReplay, ...#task#11, 200/200): "$.Destination.FilterArns[0]: value_mismatch (expected 'arn:aws:events:us-west-2:000000000000:rule/<bus>/<rule>', got 'arn:aws:events:us-west-2:000000000000:rule/<bus>/Events-Archive-<archive>')".
Cause 4 (DescribeReplay, ...#task#7): "status: expected 400, got 200"; "$.__type: missing (expected 'ResourceNotFoundException', got None)"; "$.message: missing (expected 'Replay <name> does not exist.', got None)". (StartReplay, ...#task#10): "status: expected 200, got 400"; "$.ReplayArn: missing (expected 'arn:aws:events:us-west-2:000000000000:replay/<name>', got None)".
Cause 5 (DescribeRule, ...#task#5, 200/200): "$.ManagedBy: host_not_own (expected '<shaped-not-asserted>', got 'events.amazonaws.com')".

## Steps to Reproduce
1. CreateEventBus, PutRule on it, CreateArchive on the bus (which creates the managed 'Events-Archive-<archive>' rule).
2. StartReplay with Destination.Arn=<bus arn> and Destination.FilterArns=[<the archive's managed rule ARN>] - expect 400 ValidationException 'Parameter Destination.FilterArns is not valid. Reason: Must not be service rule.', observe 200.
3. DescribeReplay(<that name>) - expect 400 ResourceNotFoundException, observe 200.
4. StartReplay again with FilterArns=[<a bare rule name>] - expect 'Parameter Destination.FilterArns is not valid. Reason: Must contain rules ARNs.', observe ResourceAlreadyExistsException.
5. StartReplay with a well-formed FilterArns=[<rule arn>] under the same name - expect 200, observe 400; then DescribeReplay and compare Destination.FilterArns[0] against the ARN sent.
6. DescribeRule on the archive's managed rule and inspect ManagedBy.
7. Re-replay the fixture's steps listed per cause.
