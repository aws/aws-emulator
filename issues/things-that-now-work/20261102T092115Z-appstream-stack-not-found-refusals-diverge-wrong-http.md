Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected appstream-asvpce-traj-reference-stack-on-pending-endpoint appstream-asvpce-traj-stack-lookup-and-delete-lag appstream-asvpce-traj-access-endpoints-request-validation appstream-asvpce-traj-invented-vpce-id-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|DescribeStacks|4xx->4xx|status
Issue-Key: appstream|DescribeStacks|4xx->2xx|$.Message
Issue-Key: appstream|DescribeStacks|4xx->2xx|$.__type
Issue-Key: appstream|DescribeStacks|4xx->2xx|status
Issue-Key: appstream|UpdateStack|4xx->4xx|status
Issue-Key: appstream|DeleteStack|4xx->4xx|status
Issue-Commit: 798fb32aa8eb1e75369deb4b48c94746eb1ca248

# appstream stack not-found refusals diverge: wrong HTTP status and missing refusal

> _Note: this issue was identified against the code base at commit `798fb32aa8eb1e75369deb4b48c94746eb1ca248`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Four net-new causes, all in how the appstream emulator refuses a stack-scoped operation whose named stack does not exist. The shared fault is the not-found path on the stack registry: the refusal is either emitted with the wrong HTTP status or not emitted at all.

Cause 1 of 4: DescribeStacks on a non-existent stack name answers HTTP 404 instead of HTTP 400.
AWS: AppStream 2.0 is awsJson1_1 (Smithy: service appstream, DescribeStacks -> error ResourceNotFoundException, ErrorMessage member `Message`). The recording shows the wire status for this refusal is HTTP 400 carrying `__type: ResourceNotFoundException`; note the on-disk model's `httpError` for ResourceNotFoundException is the protocol default 404, so the RECORDED status (400) is the requirement here, not the defaulted trait.
Emulator: answers HTTP 400 -> 404 (replay detail: "status: expected 400, got 404").
Why: an SDK retry/branch policy keyed on status sees a 404 where AWS sends 400; HTTP-level clients and proxies treat the response as a routing failure rather than a modeled service refusal, so the modeled ResourceNotFoundException is never surfaced as such.
Fix: the status mapping for appstream's ResourceNotFoundException in the error serializer, so every stack-scoped not-found refusal leaves with 400 and `__type: ResourceNotFoundException`.
Traces: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected#task#8 (representative; cluster also covers appstream-asvpce-traj-reference-stack-on-pending-endpoint#task#9, appstream-asvpce-traj-stack-lookup-and-delete-lag#task#2, #task#3, #task#8).

Cause 2 of 4: DescribeStacks on a stack that does not exist answers HTTP 200 with no refusal at all.
AWS: DescribeStacks refuses a name that names no stack with ResourceNotFoundException (Smithy: DescribeStacks errors = [ResourceNotFoundException]). Recorded response: HTTP 400, `__type: ResourceNotFoundException`, and a `Message` whose fixed text is "The stack <stack-name> for account <account-id> does not exist." — the stack name comes from the request's `Names` entry and the account id from the caller's account; both are variable, the sentence around them is fixed.
Emulator: answers HTTP 200 with neither `__type` nor `Message` (replay detail: "status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException'); $.message: missing").
Why: a client cannot distinguish "this stack is gone" from "this stack exists"; a delete-then-verify or a pre-create existence probe reads success and proceeds against a stack that is not there.
Fix: the DescribeStacks handler's name-resolution step, which must raise the modeled not-found refusal instead of returning an empty/implicit success.
Traces: appstream-asvpce-traj-access-endpoints-request-validation#task#6 (cluster also covers appstream-asvpce-traj-invented-vpce-id-rejected#task#8).

Cause 3 of 4: UpdateStack on a non-existent stack answers HTTP 404 instead of 400.
AWS: Smithy: UpdateStack errors include ResourceNotFoundException; the recorded wire status for that refusal is HTTP 400 with `__type: ResourceNotFoundException`.
Emulator: HTTP 400 -> 404 (replay detail: "status: expected 400, got 404").
Why: same as Cause 1 — status-keyed client logic mis-classifies a modeled refusal.
Fix: the same error-serializer status mapping, exercised from the UpdateStack handler.
Traces: appstream-asvpce-traj-stack-lookup-and-delete-lag#task#4.

Cause 4 of 4: DeleteStack on a non-existent stack answers HTTP 404 instead of 400.
AWS: Smithy: DeleteStack errors include ResourceNotFoundException; the recorded wire status is HTTP 400 with `__type: ResourceNotFoundException`.
Emulator: HTTP 400 -> 404 (replay detail: "status: expected 400, got 404").
Why: a teardown loop that treats 404 as "already gone" versus 400 as a modeled refusal takes a different branch than it does on AWS; idempotent-delete helpers diverge.
Fix: the same error-serializer status mapping, exercised from the DeleteStack handler.
Traces: appstream-asvpce-traj-stack-lookup-and-delete-lag#task#5 (cluster also covers appstream-asvpce-traj-stack-lookup-and-delete-lag#task#9).

Related open issue, which does NOT cover these: "appstream answers 501 NotImplemented where AWS performs a modeled refusal or an idempotent create" (20261029T194603Z) concerns unimplemented operations, not the status/presence of the stack not-found refusal on implemented ones.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (18 fixtures, 162 steps, 34 passed, score 21.0%).

Cause 1 — DescribeStacks, fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected step task#8:
  expected status 400 (body `__type: ResourceNotFoundException`), actual status 404.
  divergence_key service=appstream|DescribeStacks|status=400->404|keys=status; affected_count 5.

Cause 2 — DescribeStacks, fixture appstream-asvpce-traj-access-endpoints-request-validation step task#6:
  expected 400; actual 200.
  $.__type: missing (expected 'ResourceNotFoundException', got None)
  $.Message: missing (expected 'The stack sim2real-probe-asvpce-val-395f0a6a for account 000000000000 does not exist.', got None)
  (the stack name and account id above are this run's values filling the fixed sentence "The stack <name> for account <account> does not exist.")
  divergence_key service=appstream|DescribeStacks|status=400->200|keys=$.Message,$.__type,status; affected_count 2.

Cause 3 — UpdateStack, fixture appstream-asvpce-traj-stack-lookup-and-delete-lag step task#4:
  expected 400, actual 404. divergence_key service=appstream|UpdateStack|status=400->404|keys=status.

Cause 4 — DeleteStack, fixture appstream-asvpce-traj-stack-lookup-and-delete-lag step task#5:
  expected 400, actual 404. divergence_key service=appstream|DeleteStack|status=400->404|keys=status; affected_count 2.

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): DescribeStacks errors [ResourceNotFoundException]; DeleteStack errors [ConcurrentModificationException, OperationNotPermittedException, ResourceInUseException, ResourceNotFoundException]; UpdateStack errors include ResourceNotFoundException; shape ResourceNotFoundException = client error, members {Message: ErrorMessage}.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected, step task#8 — a DescribeStacks naming a stack that does not exist; observe HTTP 404 where the fixture expects HTTP 400 with __type ResourceNotFoundException.
4. Cause 2: replay fixture appstream-asvpce-traj-access-endpoints-request-validation, step task#6 — same call shape; observe HTTP 200 with no __type/Message.
5. Cause 3: replay fixture appstream-asvpce-traj-stack-lookup-and-delete-lag, step task#4 — UpdateStack on an absent stack; observe 404 instead of 400.
6. Cause 4: replay the same fixture, step task#5 — DeleteStack on an absent stack; observe 404 instead of 400.
