Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.threatentityset-lifecycle-create-delete
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.threatentityset-lifecycle-create-delete|_wait_until_refused#12|Timeout|time.sleep(interval_s)
Issue-Commit: 65b4e45f54c45e7e9fcce9255bb6d1e04dbae5a5

# guardduty: an accepted DeleteThreatEntitySet never completes — GetThreatEntitySet keeps answering 200 forever instead of settling to ResourceNotFoundException 'The specified entity set does not exist.'

> _Note: this issue was identified against the code base at commit `65b4e45f54c45e7e9fcce9255bb6d1e04dbae5a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteThreatEntitySet

## Description
Cause 1 of 1: a threat entity set whose delete was accepted stays readable indefinitely, so the deletion never reaches a terminal, observable outcome.
AWS: recorded behavior only — GetThreatEntitySet models just BadRequestException (400) and InternalServerErrorException (500) (Smithy: GetThreatEntitySet errors; DeleteThreatEntitySet likewise), so both outcomes the recording shows are unmodeled wire behavior. The recording shows exactly two legal endings for the delete of a freshly created, still-activating set: either the delete is refused with error code `ConflictException` and the fixed message `Another operation is currently in progress, please wait until it completes, then try again.` (and an independent read still shows a non-DELETED Status), or the delete is accepted and the set's removal completes, after which GetThreatEntitySet on that id is refused with error code `ResourceNotFoundException` and the fixed message `The specified entity set does not exist.` (both messages are entirely fixed text — no request or state values are interpolated). The check allows 1200 s for that second ending.
Emulator: neither ending is reached. DeleteThreatEntitySet answers 200, and GetThreatEntitySet on the deleted id keeps answering 200 (the deleted record is retained at DELETE_PENDING / DELETED and never stops resolving), so the check's only unbounded wait — the post-delete read loop that ends on the first refusal — never ends and the check is cut off: `Check timed out after 60.0s`. The handler takes the set off the detector but deliberately retains the record forever, with no transition that ever makes the id unresolvable.
Why: a client that deletes an entity set and polls GetThreatEntitySet until it 404s (the normal way to confirm a delete, and what the SDK waiters/retry loops do) hangs forever, and a caller re-reading the id gets a 200 describing a resource that no longer exists, so delete-then-recreate orchestration never proceeds.
Fix: the DeleteThreatEntitySet handler and the entity-set lifecycle it drives in the service state must carry the deletion to completion — the retained DELETE_PENDING/DELETED record has to stop resolving, so GetThreatEntitySet (and its TrustedEntitySet twin) refuses the id with ResourceNotFoundException and that fixed message once the removal is done.
Note for the fixer: two resolved issues sit on either side of this behavior and the current state satisfies neither — 20261117T132545Z-guardduty-deleted-entity-sets-keep-answering-200-instead-of.md (deleted sets must stop answering 200) and 20261121T203406Z-guardduty-entity-set-deletion-is-unobservable-get-threat.md (the DELETE_PENDING status must stay observable rather than the id being dropped immediately). Both are compatible only if the deleted record is readable for a bounded settling window and then becomes not-found; today the window never ends. This is not a refusal-regression of either fix: the failing call succeeds where a refusal is due.
Traces: guardduty.threatentityset-lifecycle-create-delete. Emulator source: services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/delete_threat_entity_set.rs lines 212-235 (record retained at ENTITY_SET_DELETE_PENDING with ENTITY_SET_DELETED owed, nothing ever removing it).

## Evidence
Suite: guardduty.threatentityset-lifecycle-create-delete — [mismatch] 'Check timed out after 60.0s'.

Expected (one of): DeleteThreatEntitySet -> ConflictException, message 'Another operation is currently in progress, please wait until it completes, then try again.'; OR DeleteThreatEntitySet -> 200 and, within the settling window, GetThreatEntitySet(DetectorId, ThreatEntitySetId) -> ResourceNotFoundException, message 'The specified entity set does not exist.'

Actual: DeleteThreatEntitySet -> 200 (no ConflictException branch taken, since that branch returns immediately), and GetThreatEntitySet keeps answering HTTP 200 for the remainder of the budget; the check's post-delete loop (_wait_until_refused, 1200 s) is the only unbounded wait in the body, so the run is cut off at the 60 s cap with no verdict.

## Steps to Reproduce
1. guardduty CreateDetector (or reuse the account's detector).
2. Upload a TXT threat list to an S3 bucket.
3. CreateThreatEntitySet(DetectorId, Name=<unique>, Format='TXT', Location='s3://<bucket>/list.txt', Activate=true) -> note ThreatEntitySetId.
4. GetThreatEntitySet -> Status ACTIVATING/ACTIVE.
5. DeleteThreatEntitySet(DetectorId, ThreatEntitySetId) -> observe 200 (no ConflictException).
6. Poll GetThreatEntitySet(DetectorId, ThreatEntitySetId) -> it keeps returning 200 indefinitely; it never refuses with ResourceNotFoundException 'The specified entity set does not exist.'
