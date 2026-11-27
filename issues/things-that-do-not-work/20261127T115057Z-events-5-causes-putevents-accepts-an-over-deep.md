Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.put-events-detail-nested-one-thousand-levels-fails-only-that-entry-rejected events.put-rule-role-arn-of-1601-characters-rejected-and-an-unassumable-role-refused events.put-targets-non-ascii-header-value-on-queue-target-rejected-as-unsupported-parameter events.put-targets-propagate-tags-outside-enum-rejected events.put-targets-query-string-value-with-a-control-character-rejected
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.put-events-detail-nested-one-thousand-levels-fails-only-that-entry-rejected|run#8|AssertionError|assert batch["FailedEntryCount"] == 1, (
Issue-Key: events.put-rule-role-arn-of-1601-characters-rejected-and-an-unassumable-role-refused|_refusal#19|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: events.put-targets-non-ascii-header-value-on-queue-target-rejected-as-unsupported-parameter|run#15|AssertionError|assert got == want, f"a non-ASCII header value answered {got!r}, expected {want!r}"
Issue-Key: events.put-targets-propagate-tags-outside-enum-rejected|_refusal#18|AssertionError|raise AssertionError(f"the call was expected to be refused, yet it returned {out!r}")
Issue-Key: events.put-targets-query-string-value-with-a-control-character-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events: 5 causes — PutEvents accepts an over-deep detail, PutRule accepts an over-long RoleArn, and PutTargets accepts a FIFO/enum/control-character violation or re-words its refusal

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 5: PutEvents accepts an entry whose Detail nests ~1000 levels. AWS: an entry whose detail exceeds EventBridge's nesting depth fails THAT ENTRY ONLY — the response is 200 with FailedEntryCount 1 and a FailedEntries member carrying the entry's ErrorCode/ErrorMessage, the sibling entries succeeding. Smithy: PutEvents output {FailedEntryCount default 0, Entries}; the per-entry failure channel is modeled, PutEvents has no client error. Emulator: `expected exactly one failed entry, got 0` — all three entries were accepted with EventIds, so an event AWS rejects is admitted to the bus. Checks: events.put-events-detail-nested-one-thousand-levels-fails-only-that-entry-rejected.

Cause 2 of 5: PutRule accepts a RoleArn of 1601 characters. AWS: refuses with a 400 ValidationException naming the constraint — Smithy: PutRule.RoleArn targets RoleArn @length min 1 max 1600, so 1601 characters must be refused (and an unassumable role refused in turn). Emulator: returns 200 with a RuleArn (`the call was expected to be refused but returned {'RuleArn': ...}`). Checks: events.put-rule-role-arn-of-1601-characters-rejected-and-an-unassumable-role-refused.

Cause 3 of 5: PutTargets accepts HttpParameters on a target type that does not support them, and when it does refuse it refuses for the wrong reason. AWS: HttpParameters are only valid for API Gateway / ApiDestination targets (see the Target.HttpParameters documentation); on any other target the call is refused ValidationException with `Parameter(s) HttpParameters not supported for target: <target-id>.` — fixed text with the target's Id from the request. Emulator: answers a member-pattern violation instead: `1 validation error detected: Value 'café' at 'targets.1.member.httpParameters.headerParameters.X-Sample' failed to satisfy constraint: Member must satisfy regular expression pattern: ...` — it validates the header value on a target where the whole HttpParameters block should have been rejected. Checks: events.put-targets-non-ascii-header-value-on-queue-target-rejected-as-unsupported-parameter.

Cause 4 of 5: PutTargets accepts a PropagateTags value outside the enum. AWS: an unmodeled enum value is refused with a 400 ValidationException naming the member and the accepted values; Smithy: Target.EcsParameters.PropagateTags is an enum shape, so a value outside it is not serializable. Emulator: returns 200 `{'FailedEntryCount': 0, 'FailedEntries': []}` — the target is attached with an invalid enum value. Checks: events.put-targets-propagate-tags-outside-enum-rejected.

Cause 5 of 5: PutTargets accepts an HTTP query-string value containing a control character. AWS: refuses with a 400 ValidationException quoting the member path `targets.<n>.member.httpParameters.queryStringParameters.<key>` and the printable-ASCII pattern it must satisfy. Emulator: returns 200 `{'FailedEntryCount': 0, 'FailedEntries': []}`. Checks: events.put-targets-query-string-value-with-a-control-character-rejected.

Why (all five): a request AWS refuses is accepted here, so the emulator stores state real EventBridge cannot hold — the caller's validation and error handling are never exercised, the bus accepts events/targets that will be rejected in production, and the one refusal that does happen carries the wrong diagnosis.
Fix: the request-validation layer of the events PutEvents / PutRule / PutTargets handlers (entry-level detail depth with per-entry failure reporting, modeled @length and enum constraints, and the per-target-type parameter-support rule that must precede member pattern checks).
Traces: the five check ids listed per cause above.

## Evidence
Cause 1: `AssertionError: expected exactly one failed entry, got 0: [{'EventId': ...}, {'EventId': ...}, {'EventId': ...}]`
Cause 2: `AssertionError: the call was expected to be refused but returned {'RuleArn': 'arn:aws:events:...:rule/<bus>/<rule>', ...}`
Cause 3: expected ('ValidationException', 'Parameter(s) HttpParameters not supported for target: hv-value.'), actual ('ValidationException', "1 validation error detected: Value 'café' at 'targets.1.member.httpParameters.headerParameters.X-Sample' failed to satisfy constraint: Member must satisfy regular expression pattern: [ \\t]*[\\x20-\\x7E]+([ \\t]+[\\x20-\\x7E]+)*[ \\t]*")
Causes 4 and 5: `the call was expected to be refused, yet it returned {'FailedEntryCount': 0, 'FailedEntries': [], ...}`

## Steps to Reproduce
1. PutEvents with three entries, one whose Detail is ~1000 levels of nested objects — observe FailedEntryCount 0.
2. PutRule with RoleArn of 1601 characters — observe 200 + RuleArn.
3. PutTargets on an SQS-queue target carrying HttpParameters.HeaderParameters — observe the pattern-violation message instead of the unsupported-parameter refusal.
4. PutTargets with EcsParameters.PropagateTags set to a value outside the enum — observe 200.
5. PutTargets with HttpParameters.QueryStringParameters containing a control character — observe 200.
