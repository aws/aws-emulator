Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.bus-policy-role-statement-admits-put-events-but-account-statement-leaves-bare-role-denied events.update-event-bus-policy-enforced-on-put-events events.identity-policy-events-source-condition-denies-put-events-for-any-other-source events.identity-explicit-deny-overrides-a-bus-policy-allow-naming-the-same-role events.put-targets-with-role-arn-denied-without-iam-pass-role-until-granted events.policy-scoped-to-one-bus-rules-denies-put-targets-on-the-same-rule-name-on-another-bus events.cmk-without-events-grant-rejected-on-create-and-update
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.bus-policy-role-statement-admits-put-events-but-account-statement-leaves-bare-role-denied|_error#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.update-event-bus-policy-enforced-on-put-events|_put_until_denied#26|Timeout|time.sleep(3.0)
Issue-Key: events.identity-policy-events-source-condition-denies-put-events-for-any-other-source|_error#23|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.identity-explicit-deny-overrides-a-bus-policy-allow-naming-the-same-role|_put_until_refused#27|Timeout|time.sleep(3)
Issue-Key: events.put-targets-with-role-arn-denied-without-iam-pass-role-until-granted|_error#28|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.policy-scoped-to-one-bus-rules-denies-put-targets-on-the-same-rule-name-on-another-bus|_error#26|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.cmk-without-events-grant-rejected-on-create-and-update|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge performs no authorization: event-bus resource policies, caller identity policies, iam:PassRole and KMS grants are never evaluated (5 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Every EventBridge call is served as if the caller were unconditionally allowed. Five distinct authorization inputs are ignored; each is its own cause because each is a different policy source consulted at a different point.

Cause 1 of 5 — the event bus's resource policy is not consulted on PutEvents.
AWS: an event bus carries a resource policy (EventBus.Policy in the Smithy model; written with PutPermission or the Policy parameter). PutEvents from a principal the policy does not admit is refused — the entry/call does not succeed. A statement that admits a specific role ARN does not admit a bare account, and vice versa.
Emulator: PutEvents returns HTTP 200 with FailedEntryCount 0 and an EventId for a principal the bus policy does not admit.
Why: a client that relies on the bus policy as its trust boundary gets events accepted from principals it deliberately excluded — a silent authorization bypass.
Fix: PutEvents' authorization step and the bus-policy evaluator, services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs.
Traces: events.bus-policy-role-statement-admits-put-events-but-account-statement-leaves-bare-role-denied, events.update-event-bus-policy-enforced-on-put-events.

Cause 2 of 5 — the caller's IAM identity policy (including explicit Deny and condition keys) is not evaluated.
AWS: PutEvents is authorized against the caller's identity policy too; an `events:source` condition that restricts the allowed source refuses PutEvents for any other source, and an explicit Deny in the identity policy overrides a bus-policy Allow naming the same role.
Emulator: the call succeeds (HTTP 200, FailedEntryCount 0) in both cases.
Why: least-privilege identity policies have no effect; an explicitly denied role can publish.
Fix: the same authorization step, reading the caller's identity policy and condition keys.
Traces: events.identity-policy-events-source-condition-denies-put-events-for-any-other-source, events.identity-explicit-deny-overrides-a-bus-policy-allow-naming-the-same-role.

Cause 3 of 5 — PutTargets does not require iam:PassRole on the RoleArn it is handed.
AWS: PutTargets with a target RoleArn is refused unless the caller holds iam:PassRole for that role; once the permission is granted the same call succeeds.
Emulator: PutTargets returns HTTP 200 with FailedEntryCount 0 and FailedEntries [] before the permission is granted.
Why: any caller can make EventBridge assume any role in the account — a privilege-escalation path that AWS closes with PassRole.
Fix: PutTargets' authorization step, .../src/ops/put_targets.rs.
Traces: events.put-targets-with-role-arn-denied-without-iam-pass-role-until-granted.

Cause 4 of 5 — a policy scoped to one bus's rules does not deny PutTargets on a same-named rule of another bus.
AWS: the rule ARN embeds the bus (`rule/<bus>/<rule>` for a custom bus), so a policy granting events:PutTargets only on one bus's rule ARNs refuses the identical rule name on a different bus.
Emulator: PutTargets succeeds (HTTP 200, FailedEntryCount 0) on the other bus's rule.
Why: resource-level scoping by rule ARN is not honoured; a grant intended for one bus silently covers every bus.
Fix: the resource-ARN construction and matching in PutTargets' authorization step.
Traces: events.policy-scoped-to-one-bus-rules-denies-put-targets-on-the-same-rule-name-on-another-bus.

Cause 5 of 5 — CreateEventBus/UpdateEventBus accept a customer-managed KMS key that has no grant for EventBridge.
AWS: Smithy `CreateEventBus`/`UpdateEventBus` take KmsKeyIdentifier; a key whose policy does not let events.amazonaws.com use it is refused (the bus is not created/updated).
Emulator: CreateEventBus returns HTTP 200 with EventBusArn and the KmsKeyIdentifier echoed back, for a CMK with no events grant.
Why: the bus is reported encrypted with a key EventBridge cannot actually use; the failure surfaces later, or never, instead of at configuration time.
Fix: the KmsKeyIdentifier validation in .../src/ops/create_event_bus.rs and .../src/ops/update_event_bus.rs.
Traces: events.cmk-without-events-grant-rejected-on-create-and-update.

## Evidence
All five replay as unexpected successes rather than refusals:
- PutEvents under a bus policy that does not admit the caller: `{'FailedEntryCount': 0, 'Entries': [{'EventId': '...'}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` — check reports "expected the call to be refused, but it succeeded".
- PutEvents under an identity policy with an `events:source` condition for a different source: same 200/FailedEntryCount 0 shape.
- PutTargets without iam:PassRole: `{'FailedEntryCount': 0, 'FailedEntries': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
- PutTargets on another bus's same-named rule under a bus-scoped policy: same 200/FailedEntryCount 0 shape.
- CreateEventBus with an ungranted CMK: `{'EventBusArn': 'arn:aws:events:...:event-bus/<bus>', 'KmsKeyIdentifier': 'arn:aws:kms:...:key/<id>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.

Expected in every case: a 4xx refusal, not a 2xx. (The precise code AWS returns for each is taken from the recording of each check rather than asserted here; the requirement this issue states is that the call is refused and the side effect does not happen.)

## Steps to Reproduce
1. CreateEventBus(Name=b); PutPermission/Policy on b admitting ONLY a specific role ARN. Call PutEvents against b as a bare account principal -> expect a refusal, observe 200 FailedEntryCount 0.
2. Attach an identity policy to the calling role allowing events:PutEvents only when events:source == "allowed.src"; PutEvents with Source="other.src" -> expect refusal, observe 200.
3. Add an explicit Deny for events:PutEvents on b to the role's identity policy while the bus policy allows it -> expect refusal, observe 200.
4. Without granting iam:PassRole, call PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<state machine arn>, RoleArn:<role arn>}]) -> expect refusal, observe FailedEntryCount 0.
5. Grant events:PutTargets only on arn:aws:events:...:rule/busA/* and call PutTargets for the same rule name on busB -> expect refusal, observe FailedEntryCount 0.
6. Create a KMS key whose policy does not allow events.amazonaws.com, then CreateEventBus(Name=b2, KmsKeyIdentifier=<that key>) -> expect refusal, observe 200.
