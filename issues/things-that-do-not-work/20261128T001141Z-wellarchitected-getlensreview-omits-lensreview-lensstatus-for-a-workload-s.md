Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wellarchitected.answers-stay-editable-after-lens-deleted
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected.answers-stay-editable-after-lens-deleted|_lens_status#14|KeyError|status = wa.get_lens_review(WorkloadId=wid, LensAlias=arn)["LensReview"]["LensStatus"]
Issue-Commit: a301ce84f47538debe7c18f51774443720b7c560

# GetLensReview omits LensReview.LensStatus for a workload's review of a custom lens its owner deleted (never reports DELETED)

> _Note: this issue was identified against the code base at commit `a301ce84f47538debe7c18f51774443720b7c560`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLensReview

## Description
This issue carries ONE root cause in the wellarchitected emulator.

Cause 1 of 1: a workload's lens review of a custom lens whose owner has deleted it reports NO LensStatus member at all, so the review can never read DELETED.

AWS: Smithy: GetLensReview GET /workloads/{WorkloadId}/lensReviews/{LensAlias} -> 200, GetLensReviewOutput$LensReview is LensReview, whose member LensStatus targets the enum LensStatus ("The status of the lens.") with values CURRENT | NOT_CURRENT | DEPRECATED | DELETED | UNSHARED. The enum exists precisely so that a review can report a lens that is no longer a live, current, shared lens: DELETED is the value for a review whose lens has been deleted (DeleteLens DELETE /lenses/{LensAlias}, which takes LensStatus=DRAFT|PUBLISHED|ALL and answers 200 with an empty body). AWS keeps the lens REVIEW on the workload after the lens itself is gone — the workload still carries the lens among its Lenses, GetLensReview still answers 200 for it, and the review's LensStatus becomes DELETED. The model does not mark LensStatus @required, but it is a member AWS populates on every lens review, so the recorded behavior is: after a successful DeleteLens of a custom lens a workload reviews, GetLensReview on that workload/lens keeps answering 200 and its LensReview.LensStatus reads 'DELETED' (a fixed enum token; nothing in it comes from the request).

Emulator: GetLensReview answers 200 but the serialized LensReview carries no LensStatus key whatsoever, so a client reading response["LensReview"]["LensStatus"] raises KeyError: 'LensStatus'. The behavioral check fails with exactly that: `status = wa.get_lens_review(WorkloadId=wid, LensAlias=arn)["LensReview"]["LensStatus"]` -> KeyError: 'LensStatus'. The member is Option<String> and is only ever filled with CURRENT or NOT_CURRENT: the single place the value is derived computes it as `version.zip(latest).map(...)`, i.e. it is Some only while the lens still HAS content in the catalog to have a latest version; once DeleteLens drops the lens's content, both the pinned version and the latest resolve to None, the zip yields None, and the member is omitted instead of becoming DELETED. The same derivation has no branch for DEPRECATED or UNSHARED either, so the only two of the five modeled statuses the emulator can ever report are CURRENT and NOT_CURRENT.

Why: a client that reads LensReview.LensStatus to decide how to treat a review — show it as deleted/unshared, stop offering an upgrade, warn before further edits, or simply poll until the deletion is visible — gets no member at all. In a typed SDK the field is absent/None and in a dict-based one (botocore) the lookup raises KeyError, so the deletion of a lens is INVISIBLE through the review: the review silently looks like an ordinary review with an unknown status, and a client waiting for DELETED waits forever. The whole post-deletion contract (the review stays readable and its answers stay editable under the deleted version's risk rules) is unreachable from the client's side because the status that announces it is never served.

Fix: the lens-review projection's status derivation in this service's state — the single place that resolves what the catalog says about the lens a review is against (`reviewed_lens_at`, which fills `ReviewedLens::lens_status` and is read by both `WorkloadRecord::to_lens_review` and `ReviewTemplateRecord::to_lens_review`). It must keep answering the review after the lens's content is gone and map that case to the modeled DELETED token rather than leaving the member unset, and it needs the remaining modeled cases (DEPRECATED, UNSHARED) covered rather than collapsing them into "no value". DeleteLens's own state change (dropping the lens content) is what the derivation must read, so the delete path and this projection have to agree on how a deleted-but-still-reviewed lens is represented.

Traces: behavioral check wellarchitected.answers-stay-editable-after-lens-deleted (import a custom lens, publish version 1.0, create a workload reviewing it, answer a question, DeleteLens with LensStatus=PUBLISHED, then poll GetLensReview for LensStatus == 'DELETED'). File/line citations from the checkout: services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/state/store.rs:1111-1128 (`reviewed_lens_at`, `lens_status: version.zip(latest).map(...)` yielding only CURRENT/NOT_CURRENT and None otherwise); src/state/workload.rs:414-426 (`WorkloadRecord::to_lens_review` copying `reviewed.lens_status` straight through); src/ops/get_lens_review.rs:199-222 (handler building GetLensReviewOutput from that projection); src/state.rs:2413-2424 (`ReviewedLens::lens_status`), src/state.rs:389-401 (only CURRENT and NOT_CURRENT are named as review statuses).

## Evidence
Behavioral suite replay against the emulator built from this checkout: 14/15 passed, 1 errored, service wellarchitected.

Failing check: [error] wellarchitected.answers-stay-editable-after-lens-deleted
  File "check_wellarchitected_answers_stay_editable_after_lens_deleted.py", line 117, in run
    _lens_status(wa, wid, arn, "DELETED")
  File "check_wellarchitected_answers_stay_editable_after_lens_deleted.py", line 63, in _lens_status
    status = wa.get_lens_review(WorkloadId=wid, LensAlias=arn)["LensReview"]["LensStatus"]
  KeyError: 'LensStatus'

Expected: after DeleteLens(LensAlias=<custom lens ARN>, LensStatus='PUBLISHED') succeeds, GetLensReview(WorkloadId=<workload reviewing that lens>, LensAlias=<same ARN>) answers 200 with LensReview.LensStatus == 'DELETED' (one of the modeled LensStatus values CURRENT | NOT_CURRENT | DEPRECATED | DELETED | UNSHARED).
Actual: GetLensReview answers 200 with a LensReview that contains NO LensStatus member, so the lookup raises KeyError and the review never reports the deletion. The member is only ever populated with 'CURRENT' or 'NOT_CURRENT', and only while the lens still has catalog content; once the lens is deleted it is omitted.

The status is a fixed enum token, not a value derived from the request, so no per-run value is involved in this divergence. Checked against the open wellarchitected issues: 20261120T200450Z (ListAnswers PillarId ValidationException message), 20261125T234632Z (AWS-official lens catalog content, choice prose, URLs, ListAnswers cursor) and 20261127T201518Z (UpdateAnswer/GetAnswer refusals and answer state) — none of them covers LensReview.LensStatus or the deleted-lens review status, and the replay did not mark this check as owned by any of them.

## Steps to Reproduce
1. git_clone() the project and let the behavioral suite build and boot the emulator from the checkout.
2. run_behavioral_suite() and observe check wellarchitected.answers-stay-editable-after-lens-deleted erroring with KeyError: 'LensStatus'.
3. Manually: ImportLens a custom lens document (schemaVersion 2021-11-01, one pillar, one question with two choices and riskRules), then CreateLensVersion(LensAlias=<returned LensArn>, LensVersion='1.0', IsMajorVersion=True).
4. CreateWorkload with Lenses=['wellarchitected', <that LensArn>] and UpdateAnswer on the custom lens's question so the review carries an answer.
5. GetLensReview(WorkloadId=<wid>, LensAlias=<LensArn>) — note LensReview.LensStatus reads 'CURRENT' here.
6. DeleteLens(LensAlias=<LensArn>, LensStatus='PUBLISHED', ClientRequestToken=<uuid>) -> 200.
7. GetLensReview(WorkloadId=<wid>, LensAlias=<LensArn>) again: the emulator answers 200 with a LensReview that has no LensStatus member at all; AWS reports LensStatus = 'DELETED'. Assert the member is present and equals 'DELETED'.
8. After the fix, re-replay wellarchitected.answers-stay-editable-after-lens-deleted end to end: past the status poll it also asserts the review's answers stay editable and that the deleted version's risk rules still rate the safe choice 'NONE' and the risky choice 'HIGH'.
