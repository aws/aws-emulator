Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch
Issue-Key: events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch|_collect#47|Timeout|resp = sqs.receive_message(
Issue-Commit: 8e16a3c23f08087db32488ead8b12bb820d9911b

# EventBridge does not enforce an explicit Deny in an event-bus resource policy: PutEvents is accepted indefinitely instead of refused with AccessDeniedException, and a mixed batch is not refused as a whole

> _Note: this issue was identified against the code base at commit `8e16a3c23f08087db32488ead8b12bb820d9911b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: an explicit `Deny` statement attached to an event bus with PutPermission is stored and readable, but never evaluated when PutEvents is served, so the request-level refusal AWS performs never happens and the whole-batch semantics of that refusal are never exercised.

AWS: an event bus carries a resource policy (Smithy: `PutPermission` takes `Policy`/`StatementId`/`Action` (@pattern `^events:[a-zA-Z]+$`)/`Principal`/`Condition`; `DescribeEventBus` returns it in `Policy`). Authorization of `PutEvents` is a REQUEST-level decision taken before any entry is published: an explicit `Deny` for `events:PutEvents` on the bus ARN, scoped by a `StringEquals` condition on the `events:source` condition key, refuses the call. `PutEvents` models no authorization error of its own (Smithy: PutEvents errors = `InternalException` only), so the refusal arrives as the protocol-level authorization error — the recorded behavior is error code `AccessDeniedException` on a 4xx response, NOT a 200 with per-entry `ErrorCode` in `Entries` and NOT a partial success. Because the decision is request-level, a two-entry batch carrying ONE entry whose `Source` the Deny names and one entry it does not name is refused in its entirety and NEITHER entry is published — the condition is not a per-entry filter. Removing only that statement with `RemovePermission(StatementId=...)` restores the identical two-entry batch to full acceptance (`FailedEntryCount == 0`) with BOTH entries delivered.
  The refusal message AWS words for this error class is: `User: <caller-arn> is not authorized to perform: events:PutEvents on resource: <event-bus-arn> with an explicit deny in a resource-based policy`. Its FIXED text is `User: `, ` is not authorized to perform: events:PutEvents on resource: ` and ` with an explicit deny in a resource-based policy`; its VARIABLE parts are the caller's principal ARN (the signing identity, i.e. what STS GetCallerIdentity reports for the caller) and the ARN of the bus named by the request's `EventBusName` — both are taken from the request and from emulator state, never from any one recording.

Emulator: the Deny is accepted and round-trips (DescribeEventBus returns a `Policy` containing the statement id), but `PutEvents` for the denied `Source` keeps being answered successfully. The check re-issues a single denied-source PutEvents every 3 s and never observes any refusal; it is still polling when the harness kills the check — replay verdict `mismatch`, detail `Check timed out after 60.0s`. Because no refusal is ever produced, the downstream requirements of this behavior are unobserved too: the mixed two-entry batch is never refused as a whole, and neither the error code `AccessDeniedException` nor the message above is ever emitted. Hypothesis: PutEvents has no bus-resource-policy evaluation step at all, so a `Deny` is inert exactly as an `Allow`-absence is; the recording does not state the internal cause, only that the call is never refused.

Why: the event bus resource policy is a client's trust boundary. A client that writes an explicit Deny to stop one event source from publishing to a bus sees the policy persist and read back correctly, while the denied source keeps publishing — a silent authorization bypass with no signal anywhere. A client that handles `AccessDeniedException` from PutEvents never sees it, and a client that relies on the all-or-nothing nature of a request-level refusal (re-submitting the whole batch after fixing the policy) would, on real AWS, get a clean refusal, whereas here events it believed rejected are already on the bus and were already fanned out to the rule's targets.

Fix: the authorization step of PutEvents and the bus-policy evaluator it must call, in the eventbridge service's `PutEvents` handler (`services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs`) together with wherever `PutPermission`/`RemovePermission` persist the bus policy. The decision must be taken once per REQUEST (before any entry is published or matched against rules), must support `Effect: Deny` with explicit-deny precedence over any Allow, and must evaluate the `events:source` condition key against the entries of the request. The refusal serializer must emit the `AccessDeniedException` code and the message template quoted above.

Related but NOT covering this: open issue `20261025T002357Z-events-eventbridge-performs-no-authorization-event-bus-resource-policies.md` describes bus-policy Allow/identity-policy evaluation being skipped (its Cause 1 covers an Allow that does not admit the caller). It does not carry this check's replay key, does not cover explicit-Deny precedence, does not state the whole-batch (request-level vs per-entry) requirement, and does not state this error code or message wording.

## Evidence
Failing check: `events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch` — verdict `mismatch`, detail `Check timed out after 60.0s`.

Expected (AWS):
- The bus-local delivery path is warmed first (two consecutive control events round-trip from PutEvents through the rule to the SQS target), so a later absence is suppression and not warm-up.
- `PutPermission` attaches `{"Effect":"Deny","Principal":"*","Action":"events:PutEvents","Resource":<bus-arn>,"Condition":{"StringEquals":{"events:source":<denied-source>}}}`; `DescribeEventBus` shows the statement id.
- A single-entry `PutEvents` on the denied source then settles on a 4xx whose error code is `AccessDeniedException` and whose message is `User: <caller-arn> is not authorized to perform: events:PutEvents on resource: <event-bus-arn> with an explicit deny in a resource-based policy` (fixed text as quoted; `<caller-arn>` and `<event-bus-arn>` filled from the caller identity and the request's bus).
- A two-entry `PutEvents` carrying the denied source AND an allowed source is refused with the SAME code and message; NEITHER entry reaches the rule's queue, verified after a separately-nonced positive control (the allowed source alone, `FailedEntryCount == 0`, non-empty `EventId`) has already arrived.
- `RemovePermission(StatementId=...)` then restores the identical two-entry batch to `FailedEntryCount == 0` with BOTH nonces delivered to the queue.

Actual (emulator):
- `PutEvents` for the denied source is never refused. The check's `_poll_until_denied` helper re-issues the denied-source single entry every 3 s against a 180 s budget and gets a successful response each time; it is still inside that loop when the harness terminates the check at 60 s. No `AccessDeniedException` and no refusal message are ever produced, so the mixed-batch (request-level, all-or-nothing) assertion and the post-RemovePermission restoration assertion are never reached.
- The policy itself is NOT the problem: the check's `DescribeEventBus` assertion (statement id present in `Policy`) is upstream of the polling loop and did not fail. The policy is stored and readable; it is simply not consulted when PutEvents is served.

## Steps to Reproduce
1. `CreateEventBus(Name=<bus>)`; note the returned `EventBusArn`.
2. `CreateQueue(...)`, `PutRule(Name=<rule>, EventBusName=<bus>, EventPattern={"source":[<denied-source>,<allowed-source>]})`, grant `events.amazonaws.com` `sqs:SendMessage` on the queue scoped by `aws:SourceArn` = the rule ARN, and `PutTargets(Rule=<rule>, EventBusName=<bus>, Targets=[{Id:"t1", Arn:<queue-arn>}])`.
3. Warm the path: `PutEvents` single entries on `<allowed-source>` until two consecutive events round-trip into the queue, then drain the queue.
4. `PutPermission(EventBusName=<bus>, Policy=json({"Version":"2012-10-17","Statement":[{"Sid":"DenyOneSource","Effect":"Deny","Principal":"*","Action":"events:PutEvents","Resource":<bus-arn>,"Condition":{"StringEquals":{"events:source":<denied-source>}}}]}))`.
5. `DescribeEventBus(Name=<bus>)` -> `Policy` contains `DenyOneSource` (this already passes).
6. `PutEvents(Entries=[{Source:<denied-source>, DetailType:"DenyProbe", Detail:"{}", EventBusName:<bus>}])`, retrying every 3 s. EXPECT: a 4xx with `Code == "AccessDeniedException"` and `Message == "User: <caller-arn> is not authorized to perform: events:PutEvents on resource: <bus-arn> with an explicit deny in a resource-based policy"`. OBSERVE: a successful PutEvents response every time; the loop never terminates and the check is killed at the harness timeout.
7. (Blocked by step 6, also required) `PutEvents(Entries=[<denied-source entry>, <allowed-source entry>])` -> EXPECT the same `AccessDeniedException` for the WHOLE request and NO message in the queue for either entry (confirmed against a separately-nonced allowed-source-only positive control that does arrive).
8. (Blocked by step 6, also required) `RemovePermission(EventBusName=<bus>, StatementId="DenyOneSource")`, then re-issue the identical two-entry batch -> EXPECT `FailedEntryCount == 0` and BOTH entries delivered to the queue.
