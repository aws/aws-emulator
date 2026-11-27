Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sqs__sts.create-queue-tag-keys-condition-rejects-an-unlisted-key iam__sqs__sts.list-queues-granted-only-by-queue-policy-denied iam__sqs__sts.list-queues-identity-grant-scoped-to-a-queue-arn-denied
Issue-Key: iam__sqs__sts.create-queue-tag-keys-condition-rejects-an-unlisted-key|_refusal#22|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: iam__sqs__sts.list-queues-granted-only-by-queue-policy-denied|_refusal#20|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: iam__sqs__sts.list-queues-identity-grant-scoped-to-a-queue-arn-denied|_refusal#19|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# sqs authorization is over-permissive: ListQueues is served without an account-level grant, and CreateQueue ignores the ForAllValues aws:TagKeys condition (2 causes)

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListQueues

## Description
Two net-new causes, both in the authorization decision for sqs control-plane calls.

Cause 1 of 2: ListQueues is authorized against no resource, so a principal holding no account-level ListQueues grant is served.
AWS: Smithy: sqs#ListQueues declares no authorization error (the refusal is IAM's standard AccessDenied, HTTP 403). Recorded behavior: `sqs:ListQueues` has no resource type — it is authorized against the ACCOUNT-level resource `arn:aws:sqs:<region>:<account>:`, so a grant naming a specific queue ARN does not authorize it, and neither does a QUEUE POLICY (a resource-based grant cannot authorize a listing that is not scoped to a queue). Both cases are refused with code `AccessDenied` and message "User: " + the caller's session ARN + " is not authorized to perform: sqs:listqueues on resource: arn:aws:sqs:<region>:<account>: because no identity-based policy allows the sqs:listqueues action". The session ARN, region and account come from the request; the action name, the trailing account-level ARN with its empty resource part, and the sentence are fixed. The same request SUCCEEDS once an identity policy allows sqs:ListQueues on the regional wildcard `arn:aws:sqs:<region>:<account>:*` or on `*`.
Emulator: HTTP 200 with QueueUrls in BOTH cases — for the role whose only grant is a queue policy allowing sqs:SendMessage+sqs:ListQueues on the queue ARN, and for the role whose identity policy allows sqs:ListQueues on a single queue ARN. The positive controls in the same policies (SendMessage via the queue policy; GetQueueAttributes via the queue-scoped statement) are honoured, so the policies are in effect — ListQueues simply is not authorized at all.
Why: a client cannot scope queue discovery. A least-privilege policy that deliberately withholds account-wide ListQueues still enumerates the account's queues, leaking queue names and URLs; and the documented distinction between a queue-scoped grant and the account-level resource ListQueues requires is unobservable, so a policy author cannot tell a correct policy from an ineffective one.
Fix: the sqs ListQueues handler's authorization step — authorize `sqs:ListQueues` against the account-level resource `arn:aws:sqs:<region>:<account>:` from the caller's IDENTITY policies only, and refuse with the AccessDenied message above.
Traces: iam__sqs__sts.list-queues-granted-only-by-queue-policy-denied, iam__sqs__sts.list-queues-identity-grant-scoped-to-a-queue-arn-denied.

Cause 2 of 2: CreateQueue ignores a `ForAllValues:StringEquals` condition on `aws:TagKeys`, so a queue carrying an unlisted tag key is created.
AWS: Smithy: sqs#CreateQueue takes a `tags` map and declares no authorization error; the refusal is IAM's AccessDenied (403). Recorded behavior: with `sqs:CreateQueue` allowed only under `Condition {"ForAllValues:StringEquals": {"aws:TagKeys": [<allowed keys>]}}`, a CreateQueue whose tag keys are drawn only from the allowed set succeeds, and one adding ANY key outside that set fails the multivalued condition and is REFUSED with code `AccessDenied` and message "User: " + the caller's session ARN + " is not authorized to perform: sqs:createqueue on resource: arn:aws:sqs:<region>:<account>:<requested queue name> because no identity-based policy allows the sqs:createqueue action". The refused request creates NOTHING: GetQueueUrl for that name then answers `AWS.SimpleQueueService.NonExistentQueue` / "The specified queue does not exist." Dropping only the unlisted key makes the identical name creatable.
Emulator: HTTP 200 with a QueueUrl for the request carrying the extra tag key — the condition is not evaluated, and the queue exists afterwards.
Why: tag-key governance on creation is unenforceable. A policy whose whole purpose is to confine which tag keys may be attached at create time admits any key, so untagged/mis-tagged queues are created under a policy that forbids them, and the author's only signal that the condition works — the refusal — never appears.
Fix: the sqs CreateQueue handler's authorization step — evaluate multivalued condition operators (ForAllValues/ForAnyValue) against `aws:TagKeys` built from the request's `tags` map before creating the queue.
Traces: iam__sqs__sts.create-queue-tag-keys-condition-rejects-an-unlisted-key.

## Evidence
Cause 1 — expected ('AccessDenied', 'User: <session arn> is not authorized to perform: sqs:listqueues on resource: arn:aws:sqs:<region>:<account>: because no identity-based policy allows the sqs:listqueues action'). Actual, verbatim:
- 'AssertionError: expected a refusal, but the call succeeded: {\'QueueUrls\': [\'https://<host>/<account>/sim2real-xsvc-lqqp-…\'], …HTTPStatusCode: 200…}' (queue-policy-only grant)
- 'AssertionError: expected a refusal, but the call succeeded: {\'QueueUrls\': [\'https://<host>/<account>/sim2real-xsvc-lqscope-…\'], …HTTPStatusCode: 200…}' (identity grant scoped to the queue ARN)

Cause 2 — expected ('AccessDenied', 'User: <session arn> is not authorized to perform: sqs:createqueue on resource: arn:aws:sqs:<region>:<account>:<name> because no identity-based policy allows the sqs:createqueue action') and no queue created. Actual, verbatim: 'AssertionError: expected a refusal, but the call succeeded: {\'QueueUrl\': \'https://<host>/<account>/sim2real-xsvc-cqtk-bad-…\', …HTTPStatusCode: 200…}'

Hosts, ports, queue names, role/session names and account ids above are per-run values and are not part of the requirement.

## Steps to Reproduce
Cause 1a: create a queue; create a role trusted by the account root with NO identity policy; set a queue policy allowing that role sqs:SendMessage+sqs:ListQueues on the queue ARN. As the assumed role, SendMessage (succeeds), then ListQueues(QueueNamePrefix=<name>) — expect AccessDenied on arn:aws:sqs:<region>:<account>:; observe 200 with the queue URL.
Cause 1b: same queue, a role whose identity policy allows sqs:GetQueueAttributes on the queue ARN and sqs:ListQueues on the QUEUE ARN. GetQueueAttributes succeeds; ListQueues — expect the same AccessDenied; observe 200. Re-grant ListQueues on arn:aws:sqs:<region>:<account>:* and the same call must succeed.
Cause 2: create a role whose policy allows sqs:CreateQueue on arn:aws:sqs:<region>:<account>:<prefix>-* under Condition ForAllValues:StringEquals aws:TagKeys = ["sim2real","costcenter"], plus sqs:TagQueue on the same resource. As that session, CreateQueue with tags {sim2real, costcenter} (succeeds), then CreateQueue with tags {sim2real, costcenter, owner} — expect AccessDenied and GetQueueUrl answering AWS.SimpleQueueService.NonExistentQueue; observe 200 and a created queue.
