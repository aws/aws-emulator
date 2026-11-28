Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: omics.tag-and-untag-resource-round-trip
Issue-Shape-Id: com.amazonaws.omics#Omics
Issue-Key: omics.tag-and-untag-resource-round-trip|run#20|AssertionError|assert tag_code == "ValidationException", (
Issue-Commit: 60321a6474ea40ecacfbfad91587a33095c060e9

# omics TagResource on a deleted resource's ARN answers 404 ResourceNotFoundException instead of 400 ValidationException "Resource was not found"

> _Note: this issue was identified against the code base at commit `60321a6474ea40ecacfbfad91587a33095c060e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource against a syntactically valid HealthOmics resource ARN whose resource no longer exists (here a run group that DeleteRunGroup already removed) is refused with the wrong error class.

AWS: HealthOmics TagResource models BOTH ValidationException (HTTP 400) and ResourceNotFoundException (HTTP 404) — Smithy: TagResource.resourceArn target TagArn @length 1..150, @pattern ^arn:.+$, @required; errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), RequestTimeoutException(408), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); http POST /tags/{resourceArn} -> 204. Which of the two AWS actually picks for a resource that is gone is recorded behavior only: the recording shows TagResource on the ARN of an already-deleted run group refused with HTTP 400, error code `ValidationException`, message `Resource was not found` — the whole message is fixed text, it interpolates no name, id or ARN. The recording also shows this differs deliberately from the read path on the same deleted resource: GetRunGroup on the same id is refused 404 `ResourceNotFoundException` with the templated message `Run group <id> not found` (fixed text `Run group ` / ` not found` around the request's `id`), and the emulator already matches that one. So the tagging path and the read path must NOT answer alike.

Emulator: TagResource resolves the ARN shape, finds no record for it in the store, and returns the modelled ResourceNotFoundException (HTTP 404) with the store's `resource_arn_not_found_message(..)` text. The check observes error code 'ResourceNotFoundException' where AWS gives 'ValidationException'.

Why: a client that distinguishes "bad input" from "resource gone" is told the opposite of what AWS says. Code catching ValidationException (botocore `omics.exceptions.ValidationException`, or a 400-only retry/abort branch) to detect tagging a resource that has been deleted never fires against the emulator, while code that treats 404 ResourceNotFoundException as "retry after propagation" or "recreate then re-tag" fires spuriously. The status code also diverges (404 vs 400), so middleware and SDK retry classification keyed on status behave differently than against AWS.

Fix: the TagResource handler's missing-resource branch in services/service-com-amazonaws-omics-omics/src/ops/tag_resource.rs — the arm taken when `store.resource_tags_mut(&input.resource_arn)` yields None must raise the modelled ValidationException with the fixed message `Resource was not found` rather than ResourceNotFoundException. Note the earlier branch in the same handler (ARN that names no HealthOmics resource type at all) already raises ValidationException, with a different message; keep its wording and only change the resolved-but-absent case. UntagResource and ListTagsForResource on the same ARN should be reviewed for the same divergence while the handler is open, but only TagResource is grounded in a failing check here.

Traces: omics.tag-and-untag-resource-round-trip — assertion at check line 71: "expected ValidationException tagging a deleted run group, got 'ResourceNotFoundException'". The earlier phases of the same check (TagResource/UntagResource/ListTagsForResource merge and single-key removal, and GetRunGroup's 404 wording on the deleted group) all passed.

## Evidence
Check omics.tag-and-untag-resource-round-trip, verdict mismatch.

Expected (AWS, recorded): TagResource(resourceArn=<arn of deleted run group>, tags={...}) -> HTTP 400, Error.Code = "ValidationException", Error.Message = "Resource was not found" (message is entirely fixed text).

Actual (emulator): HTTP 404, Error.Code = "ResourceNotFoundException", message from crate::state::resource_arn_not_found_message(..).

Harness output:
  File "check_omics_tag_and_untag_resource_round_trip.py", line 71, in run
    assert tag_code == "ValidationException", (
  AssertionError: expected ValidationException tagging a deleted run group, got 'ResourceNotFoundException'

Control in the same check that passes: GetRunGroup(id=<deleted id>) -> 404 ResourceNotFoundException, message "Run group <id> not found" — so the deletion itself is modelled correctly; only the tagging refusal's class/status/message diverge.

Code path: services/service-com-amazonaws-omics-omics/src/ops/tag_resource.rs lines 197-210 return TagResourceErrorModeled::ResourceNotFoundException for an ARN with no record in the store.

## Steps to Reproduce
1. omics.create_run_group(requestId=<unique>, name=<unique>) — keep the returned `arn` and `id`.
2. omics.tag_resource(resourceArn=arn, tags={"k": "v"}) and omics.list_tags_for_resource(resourceArn=arn) — succeeds today.
3. omics.delete_run_group(id=id).
4. omics.get_run_group(id=id) -> 404 ResourceNotFoundException "Run group <id> not found" (matches AWS).
5. omics.tag_resource(resourceArn=arn, tags={"any": "value"}).
   Expected: ClientError with HTTP 400, Code "ValidationException", Message "Resource was not found".
   Actual: ClientError with HTTP 404, Code "ResourceNotFoundException".
