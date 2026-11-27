Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__logs.put-destination-naming-a-role-not-yet-created-rejected iam__kinesis__logs.put-destination-role-scoped-to-another-stream-rejected iam__kinesis__logs.subscription-filter-role-trust-bound-to-another-log-group-rejected iam__kinesis__logs.infrequent-access-log-group-refuses-a-subscription-filter
Issue-Key: iam__kinesis__logs.put-destination-naming-a-role-not-yet-created-rejected|_refused#24|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {result!r}")
Issue-Key: iam__kinesis__logs.put-destination-role-scoped-to-another-stream-rejected|_refusal#23|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: iam__kinesis__logs.subscription-filter-role-trust-bound-to-another-log-group-rejected|_refusal#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: iam__kinesis__logs.infrequent-access-log-group-refuses-a-subscription-filter|run#33|Timeout|time.sleep(3.0)
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs: 4 net-new causes — PutDestination and PutSubscriptionFilter never resolve or evaluate the given role (absent role, stream-scoped role, log-group-bound trust all accepted), and an Infrequent Access log group accepts a subscription filter

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDestination

## Description
Cause 1 of 4 — `PutDestination` accepts a `roleArn` that does not exist.
AWS: PutDestination resolves `roleArn` against IAM and refuses a role that has not been created with HTTP 400 `InvalidParameterException` (`Could not deliver test message to specified destination. Check if the destination is valid.`). Smithy (logs): `PutDestination` models `InvalidParameterException`; `roleArn` and `targetArn` are required.
Emulator: the call returns 200 and stores the destination: `{'destination': {'destinationName': '<name>', 'targetArn': 'arn:aws:kinesis:<region>:<account>:stream/<stream>', 'roleArn': 'arn:aws:iam::<account>:role/<role-that-does-not-exist>', 'arn': 'arn:aws:logs:<region>:<account>:destination:<name>', 'creationTime': <ms>}}`.
Traces: iam__kinesis__logs.put-destination-naming-a-role-not-yet-created-rejected.

Cause 2 of 4 — `PutDestination` accepts a role whose policy is scoped to a different stream.
AWS: the test delivery is performed with the role, so a role allowed `kinesis:PutRecord` only on another stream is refused with the same 400 InvalidParameterException.
Emulator: 200, destination created with that roleArn.
Traces: iam__kinesis__logs.put-destination-role-scoped-to-another-stream-rejected.

Cause 3 of 4 — `PutSubscriptionFilter` accepts a role whose trust policy is bound to a different log group.
AWS: the role's trust policy conditions (e.g. `aws:SourceArn` on the log-group ARN) are evaluated when logs assumes it; a role trusted only for another log group cannot be assumed and the call is refused with 400 InvalidParameterException.
Emulator: 200 with an empty body; the filter is created.
Traces: iam__kinesis__logs.subscription-filter-role-trust-bound-to-another-log-group-rejected.

Cause 4 of 4 — a log group in the `INFREQUENT_ACCESS` class accepts a subscription filter.
AWS: the Infrequent Access log class does not support subscription filters; PutSubscriptionFilter against such a group is refused with a 400 error naming the unsupported log class. Smithy (logs): `CreateLogGroup.logGroupClass` is the `LogGroupClass` enum (STANDARD|INFREQUENT_ACCESS|DELIVERY) and the per-class feature restrictions are documented behavior.
Emulator: the check makes no further progress and ends `Check timed out after 60.0s` — no refusal is produced. Hypothesis: the filter is accepted and the check then waits for a state that cannot arrive.
Traces: iam__kinesis__logs.infrequent-access-log-group-refuses-a-subscription-filter.

Why: these are the four guards that stop a client from building a log pipeline that cannot deliver. All of them answer 200, so a mis-scoped or non-existent role, or an unsupported log class, is only discovered later as missing data at the destination — and nothing in the API ever reports it.

Fix: the PutDestination and PutSubscriptionFilter handlers' validation — resolve `roleArn` against IAM, evaluate the role's identity policy and trust policy for the specific target and log group, perform the test delivery, and enforce the log-class feature matrix before storing anything.

## Evidence
Cause 1: "expected a refusal, the call succeeded: {'destination': {'destinationName': 'sim2real-xsvc-dst-late-7774ee5bda9b', 'targetArn': 'arn:aws:kinesis:us-west-2:000000000000:stream/...', 'roleArn': 'arn:aws:iam::000000000000:role/sim2real-xsvc-dst-b-3cb4a9974d99', ...}}" (HTTP 200).
Cause 2: "expected the call to be refused, but it succeeded: {'destination': {'destinationName': 'sim2real-dst-neg-f22306837b22', 'targetArn': '...stream/sim2real-dst-scope-89d53527d6fc', 'roleArn': '...role/sim2real-dst-other-759f9350f48f', ...}}" (HTTP 200).
Cause 3: "expected the call to be refused, but it succeeded: {'ResponseMetadata': {... 'HTTPStatusCode': 200, 'content-length': '2' ...}}".
Cause 4: "Error: Check timed out after 60.0s" — no refusal for the INFREQUENT_ACCESS group.

## Steps to Reproduce
1. Call logs:PutDestination with a roleArn naming a role that has not been created — 200 instead of InvalidParameterException.
2. Create two Kinesis streams and a role allowed kinesis:PutRecord only on stream B; call PutDestination targeting stream A with that role — 200.
3. Create two log groups and a role whose trust policy restricts aws:SourceArn to group B; call PutSubscriptionFilter on group A with that role — 200.
4. CreateLogGroup with logGroupClass=INFREQUENT_ACCESS and call PutSubscriptionFilter on it — no refusal.
