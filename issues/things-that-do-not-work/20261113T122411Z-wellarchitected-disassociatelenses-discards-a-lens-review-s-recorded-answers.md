Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wellarchitected.disassociate-default-lens-rejected
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected.disassociate-default-lens-rejected|run#26|AssertionError|assert restored["SelectedChoices"] == ["sqdrop_c1"], (
Issue-Commit: 32d49edbcd877a4f674acf9925bcee3510eafef8

# DisassociateLenses discards a lens review's recorded answers, so re-associating the lens returns a blank review

> _Note: this issue was identified against the code base at commit `32d49edbcd877a4f674acf9925bcee3510eafef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateLenses

## Description
This issue has 1 net-new cause, in service wellarchitected, grounded in 1 failing behavioral check of suite simparity-behavioral-aws-v1.

NOT covered by an existing issue. Open issue 20261103T105038Z-wellarchitected-answer-choiceanswers-projection-improvement-plan-details-and-two.md carries this same check's replay key for its *DisassociateLenses default-lens refusal message* (cause 2 there, failing at the `assert message == expected` on the default-lens refusal). The suite no longer marks this check OWNED and it no longer fails there: it now proceeds past the refusal-message assertions, past the successful detach of the custom lens, past the detached-review ValidationException and its message, and past the re-association, and fails ~50 lines later on the restored answer. This is therefore a different, deeper failure point on the same check, not the already-filed message defect.

Cause 1 of 1: detaching a custom lens from a workload destroys the lens review's recorded answers instead of retaining them, so re-attaching the same lens produces an empty review rather than the review as it was.

AWS: DisassociateLenses (Smithy: PATCH /workloads/{WorkloadId}/disassociateLenses, code 200, LensAliases required @length min 1; documented 'Disassociate a lens from a workload. ... The Amazon Web Services Well-Architected Framework lens (wellarchitected) cannot be removed from a workload.') and AssociateLenses (PATCH /workloads/{WorkloadId}/associateLenses, code 200) are association operations only: the model describes them as attaching/detaching a lens to a workload, and says nothing about discarding the workload's answers. Recorded behavior: after DisassociateLenses removes a lens, that lens no longer appears among the workload's lens reviews and GetLensReview for it is refused with 400 ValidationException; but the answers previously recorded for it survive, and after AssociateLenses re-attaches the SAME lens, GetAnswer for a question answered before the detach returns that answer intact — Answer$SelectedChoices equal to the choice ids submitted to the pre-detach UpdateAnswer, Answer$Notes equal to the Notes string submitted with it, and Answer$Risk equal to the risk the lens's riskRules derive for that selection (the check's authored lens makes the single first choice MEDIUM). Detaching hides a lens review; it does not reset it.

Emulator: the re-attached lens's review comes back blank. GetAnswer on the previously answered question returns Answer$SelectedChoices == [] (the recorded selection is gone). The detach and re-attach themselves succeed and the review reappears in ListLensReviews, so only the answer state is lost.

Why: a client that temporarily detaches a lens — to swap lens versions, to re-scope a review, or as part of an automated workload rebuild — silently loses every answer, note and per-choice reason the reviewer entered for that lens, and the derived Risk/RiskCounts along with them. Against AWS the same sequence is non-destructive, so a tool that is safe in production destroys review data when pointed at the emulator, and a test that detaches then re-attaches to assert persistence sees silent data loss instead of an error.

Fix: the workload/lens-review state behind the DisassociateLenses handler — detaching must mark the review as not associated (so it is excluded from ListLensReviews and GetLensReview is refused) and keep its stored per-question answers; the AssociateLenses handler must re-activate an existing stored review for that lens alias rather than constructing a fresh empty one. The answer store must be keyed by (workload, lens, question) independently of the association flag.

Traces: wellarchitected.disassociate-default-lens-rejected.

## Evidence
Suite simparity-behavioral-aws-v1 against the emulator: 9/12 passed (75%); wellarchitected 9/12, 3 failed. Two of the three failures (custom-lens-risk-rules-derive-answer-risk, improvements-track-at-risk-answers) are marked OWNED by 20261103T105038Z-wellarchitected-answer-choiceanswers-projection-improvement-plan-details-and-two.md and are excluded. No failure was marked ALREADY FILED. The one below is the surviving, unowned failure.

wellarchitected.disassociate-default-lens-rejected [mismatch], at check line 223:
  assert restored["SelectedChoices"] == ["sqdrop_c1"]
  AssertionError: the re-attached lens's answer reads [], expected the selection recorded before it was detached

Sequence the check executed, with every earlier assertion passing:
1. ImportLens a custom lens with one pillar, one question ('sqdrop') holding two choices and authored riskRules ('c1 && c2' -> NO_RISK, 'c1' -> MEDIUM_RISK, default -> HIGH_RISK); CreateLensVersion '1.0'.
2. CreateWorkload with Lenses ['wellarchitected', <lens arn>]; both reviews present.
3. UpdateAnswer(QuestionId='sqdrop', SelectedChoices=['<q>_c1'], Notes=<a notes string>) -> Answer$Risk == MEDIUM. PASSED.
4. DisassociateLenses(LensAliases=['wellarchitected']) -> 400 ValidationException with message '[Validation] Cannot disassociate default lens: wellarchitected.' and the default review retained. PASSED (this is the assertion the already-filed cause 2 covered; it now holds).
5. DisassociateLenses(LensAliases=[<lens arn>]) -> succeeds; the custom review leaves ListLensReviews, leaving only the default lens. PASSED.
6. GetLensReview for the detached lens -> 400 ValidationException, message '[Validation] No lens review for lens alias <lensAlias> was found in workload <workloadId>.' PASSED (fixed text '[Validation] No lens review for lens alias ' / ' was found in workload ' / '.', variable parts the alias from the request and the workload id).
7. AssociateLenses(LensAliases=[<lens arn>]) -> succeeds; the review reappears in ListLensReviews. PASSED.
8. GetAnswer(WorkloadId=<wid>, LensAlias=<lens arn>, QuestionId='sqdrop') -> Answer$SelectedChoices == [] . FAILED; AWS returns the selection recorded in step 3. The check's next two assertions (Answer$Notes equal to the notes submitted in step 3, Answer$Risk == MEDIUM) are unreached and would also fail on an empty answer.

The question id, choice ids, lens ARN, workload id and notes text above are the per-run values of the check's authored lens and are not themselves requirements; the requirement is that the answer recorded before a detach is the answer read back after the matching re-attach.

Smithy (services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json): AssociateLenses PATCH /workloads/{WorkloadId}/associateLenses -> 200, Unit output, errors ValidationException(400)/ResourceNotFoundException(404)/ConflictException(409); DisassociateLenses PATCH /workloads/{WorkloadId}/disassociateLenses -> 200, Unit output, same errors, documented 'Disassociate a lens from a workload' with the note that the wellarchitected lens cannot be removed; GetAnswer GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId} -> 200 {WorkloadId, MilestoneNumber, LensAlias, LensArn, Answer}; Answer{QuestionId, PillarId, QuestionTitle, Choices, SelectedChoices, ChoiceAnswers, IsApplicable, Risk, Notes (@length max 2084), Reason, ...}. Nothing in the model makes DisassociateLenses destructive of answer state.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay suite simparity-behavioral-aws-v1 and look at service wellarchitected (3 failures of 12; this is the only one not OWNED by an existing issue).
3. Manually: ImportLens a custom lens with one pillar and one two-choice question carrying riskRules ('c1 && c2' -> NO_RISK, 'c1' -> MEDIUM_RISK, default -> HIGH_RISK), then CreateLensVersion '1.0'.
4. CreateWorkload with Lenses ['wellarchitected', <lens arn>], AwsRegions=[<region>].
5. PATCH /workloads/{wid}/lensReviews/{lensArn}/answers/{questionId} with SelectedChoices=['<questionId>_c1'] and a Notes string; observe Answer$Risk == MEDIUM.
6. PATCH /workloads/{wid}/disassociateLenses with LensAliases=[<lens arn>]; confirm the review leaves ListLensReviews and GetLensReview for it is refused with 400 ValidationException.
7. PATCH /workloads/{wid}/associateLenses with LensAliases=[<lens arn>]; confirm the review reappears in ListLensReviews.
8. GET /workloads/{wid}/lensReviews/{lensArn}/answers/{questionId}. The emulator returns Answer$SelectedChoices == [] with the Notes and the derived MEDIUM Risk gone; AWS returns the answer exactly as recorded in step 5.
9. Re-replay wellarchitected.disassociate-default-lens-rejected to confirm the fix.
