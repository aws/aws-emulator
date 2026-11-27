Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-answer-refusal-causes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-answer-refusal-causes
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$.Message
Issue-Key: wellarchitected|UpdateAnswer|4xx->2xx|$.Fields
Issue-Key: wellarchitected|UpdateAnswer|4xx->2xx|$.Message
Issue-Key: wellarchitected|UpdateAnswer|4xx->2xx|$.Reason
Issue-Key: wellarchitected|UpdateAnswer|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|UpdateAnswer|4xx->2xx|status
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*].ChoiceId
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*].Status
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.SelectedChoices[*]
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$.Message
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$.ResourceId
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$.ResourceType
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|status
Issue-Commit: d2c660dcba0958f9f444e52348f6b67098dedf4a

# Answer refusals and answer state diverge from Well-Architected

> _Note: this issue was identified against the code base at commit `d2c660dcba0958f9f444e52348f6b67098dedf4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAnswer

## Description
Replaying the wire corpus against the emulator shows FOUR distinct causes in how this service refuses and records the answer to one question of a workload's lens review (UpdateAnswer / GetAnswer). All four are in service wellarchitected; this issue bundles them and each must be closed before the fix is submitted.

Cause 1 of 4: an UpdateAnswer whose Notes is over the modelled ceiling is refused with the wrong Message.
AWS: Smithy: UpdateAnswer.Notes targets Notes, a string with @length(max: 2084); the operation declares ValidationException (@error("client"), @httpError(400)) whose Message is @required. The recording shows the refusal stated as this service's schema-finding report: Message = '[Validation] [string "<the Notes value as submitted>" is too long (length: <the counted character length>, maximum allowed: <the modelled maximum>)]'. Fixed text: '[Validation] [string "', '" is too long (length: ', ', maximum allowed: ', ')]'. Variable parts, all taken from the request and the model: the submitted Notes string, the number of characters counted in it, and the @length maximum of Notes (2084). The recorded response quotes the WHOLE submitted value, not a prefix of it.
Emulator: answers 400 (status matches) but with a different Message — the replay's only diverging key for this step is $.Message. Hypothesis (from the checkout, not from the recording): the boundary refusal renders a MaxLength finding only while the ECHOED value still shows the length the finding states (services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/service.rs:586-596), and the framework elides every echoed value to 256 characters plus '... [N bytes]' (vendor/smithy-schema-server/src/constraints.rs:401-427). For a Notes value past 2084 characters the elided echo therefore counts well under the maximum, the finding is suppressed, schema_findings() falls to all-or-nothing None (service.rs:539-550), and the message falls back to the framework envelope '<N> validation error(s) detected: …' (constraints.rs:1398-1413) instead of the bracketed finding AWS words.
Why: a client that parses or surfaces the service's validation diagnostic gets a sentence in the wrong voice, and cannot see which value and which ceiling were measured — the length and maximum AWS states are absent.
Fix: the boundary-refusal wording for UpdateAnswer's input constraints — the schema-finding renderer in this service's front door (its validation path in service.rs), which must state the length finding from the ORIGINAL value it validated rather than from the framework's elided echo.
Traces: wellarchitected-answer-refusal-causes#task#4.

Cause 2 of 4: UpdateAnswer ACCEPTS a SelectedChoices list containing duplicate choice ids.
AWS: Smithy: UpdateAnswer.SelectedChoices targets SelectedChoices, a list of ChoiceId (@length 1..64); UpdateAnswer declares ValidationException (@httpError(400)) with Reason (ValidationExceptionReason, one of UNKNOWN_OPERATION | CANNOT_PARSE | FIELD_VALIDATION_FAILED | OTHER) and Fields (ValidationExceptionField: @required Name = 'the field name for which validation failed', @required Message). The model states no uniqueItems rule, so this is recorded behavior: AWS refuses the request with 400, Reason = 'FIELD_VALIDATION_FAILED', Message = '[Validation] The validation of one or more fields has failed.' (entirely fixed text, no variable part), and one entry in Fields whose Name is the offending member as the API spells it ('SelectedChoices', taken from the request) and whose Message is the fixed sentence 'SelectedChoices contains duplicate choices.'; x-amzn-errortype names ValidationException.
Emulator: answers 200 and applies the write. Replay: 'status: expected 400, got 200' with $.Message, $.Reason, $.Fields and the x-amzn-errortype header all missing from the response. The UpdateAnswer handler validates the enum values and the undefined choice ids but never checks SelectedChoices for repeated ids (services/.../src/ops/update_answer.rs:250-294).
Why: a client gets a success for a request AWS rejects, so a bug in its request is never surfaced and the workload records an answer the real service would never have stored.
Fix: UpdateAnswer's validation (the handler's refusal sequence, beside the existing undefined-choice check), raising the modelled ValidationException with Reason FIELD_VALIDATION_FAILED and the SelectedChoices field finding.
Traces: wellarchitected-answer-refusal-causes#task#5 (and #task#6 in the same cluster).

Cause 3 of 4: after the refused duplicate update, GetAnswer reports the wrong selected choices and the wrong choice statuses.
AWS: Smithy: GetAnswer returns Answer, whose SelectedChoices lists the ChoiceIds in place for the question and whose ChoiceAnswers carries one ChoiceAnswer (ChoiceId + Status) per choice, in the question's own choice order. Recorded behavior: because the duplicate-choice update was refused, the answer still stands as the last ACCEPTED write left it — SelectedChoices[0] = the choice that write selected, and ChoiceAnswers[0] = that same ChoiceId with Status 'SELECTED'.
Emulator: reports Answer.SelectedChoices with no entry at all ($.Answer.SelectedChoices[0]: missing) and ChoiceAnswers[0] carrying a DIFFERENT choice of the question with Status 'NOT_APPLICABLE'. Replay: $.Answer.ChoiceAnswers[0].ChoiceId value_mismatch (expected the selected choice, got another choice of the question), $.Answer.ChoiceAnswers[0].Status value_mismatch (expected 'SELECTED', got 'NOT_APPLICABLE').
Why: a read-back of the answer shows state the real service never recorded — the question looks unanswered/not-applicable, so risk roll-ups and any client that drives its UI from SelectedChoices diverge.
Fix: the answer's state and its projection (the answer record written by UpdateAnswer and crate::state::answer_projection / select_only), so that a refused write leaves the stored answer untouched and SelectedChoices is derived from the choices whose status is SELECTED, in the question's choice order. Hypothesis: this is the state left behind by Cause 2 applying a request AWS refuses; it must still be verified by replaying this step after Cause 2 is fixed. Note: the open issue 20261125T234632Z-wellarchitected-aws-official-lens-content-served-by-well-architected.md covers the lens CONTENT and prose of answer projections; it does not cover the selected-choice state this step diverges on, and the replay did not mark this cluster as owned by it.
Traces: wellarchitected-answer-refusal-causes#task#9.

Cause 4 of 4: GetAnswer with a MilestoneNumber above the modelled bound answers ValidationException instead of ResourceNotFoundException.
AWS: Smithy: GetAnswer.MilestoneNumber is @httpQuery("MilestoneNumber") with @range(min: 1, max: 100) ('A workload can have a maximum of 100 milestones'); the operation declares both ValidationException (@httpError(400)) and ResourceNotFoundException (@httpError(404)) whose Message, ResourceId and ResourceType are all @required. Recorded behavior: the service does not refuse such a number at its front door — it resolves it as a LOOKUP and answers 404 ResourceNotFoundException with ResourceType = 'MILESTONE' (fixed), ResourceId = '<WorkloadId>.<MilestoneNumber>' (both values from the request) and Message = '[ResourceNotFound] No milestone with number <MilestoneNumber> in a workload with ID <WorkloadId> was found in account <accountId>.' — fixed text '[ResourceNotFound] No milestone with number ', ' in a workload with ID ', ' was found in account ', '.', with the milestone number and workload id taken from the request and the account id from the caller's identity. x-amzn-errortype names ResourceNotFoundException.
Emulator: answers 400 with Message '[Validation] MilestoneNumber must be between 1 and 100.' and x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.wellarchitected/', and carries no ResourceId or ResourceType. Replay: 'status: expected 404, got 400'; $.ResourceId and $.ResourceType missing; $.Message value_mismatch. The sentence is the @range field-validation refusal built at the boundary (services/.../src/service.rs:719-753, reached through ModelledValidation before any handler runs).
Why: a client branching on the error shape sees an input-validation failure where AWS reports a missing resource, and loses the ResourceId/ResourceType pair it uses to name what was not found; retry/あlert logic keyed on ResourceNotFoundException never fires.
Fix: GetAnswer's milestone resolution — the request's MilestoneNumber must reach the handler and be answered as a 404 ResourceNotFoundException for a milestone the workload does not hold, rather than being refused by the @range boundary check for this member.
Traces: wellarchitected-answer-refusal-causes#task#12.

## Evidence
Cause 1 — wellarchitected-answer-refusal-causes#task#4, UpdateAnswer: expected 400 with $.Message = '[Validation] [string "<submitted Notes>" is too long (length: <N>, maximum allowed: 2084)]'; actual 400 with a different $.Message (replay verdict: mismatch, keys=$.Message).
Cause 2 — wellarchitected-answer-refusal-causes#task#5 (cluster also covers #task#6), UpdateAnswer: expected 400 with $.Reason='FIELD_VALIDATION_FAILED', $.Message='[Validation] The validation of one or more fields has failed.', $.Fields=[{'Name':'SelectedChoices','Message':'SelectedChoices contains duplicate choices.'}] and an x-amzn-errortype header; actual 200 with none of those members present.
Cause 3 — wellarchitected-answer-refusal-causes#task#9, GetAnswer: expected $.Answer.SelectedChoices[0] = the choice the last accepted write selected and $.Answer.ChoiceAnswers[0] = that same ChoiceId with Status 'SELECTED'; actual SelectedChoices[0] missing and ChoiceAnswers[0] naming a different choice of the question with Status 'NOT_APPLICABLE'.
Cause 4 — wellarchitected-answer-refusal-causes#task#12, GetAnswer with MilestoneNumber above the modelled maximum: expected 404 with $.ResourceType='MILESTONE', $.ResourceId='<WorkloadId>.<MilestoneNumber>' and $.Message='[ResourceNotFound] No milestone with number <MilestoneNumber> in a workload with ID <WorkloadId> was found in account <accountId>.'; actual 400 with $.Message='[Validation] MilestoneNumber must be between 1 and 100.', x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.wellarchitected/', and no ResourceId/ResourceType.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1); all four causes are steps of fixture wellarchitected-answer-refusal-causes.
3. task#4 — UpdateAnswer on the workload's lens review with a Notes value past the Notes @length maximum (2084 characters): compare the ValidationException Message with the recorded bracketed schema finding.
4. task#5/task#6 — UpdateAnswer with SelectedChoices containing the same ChoiceId twice: the emulator answers 200 where the recording expects 400 ValidationException with Reason FIELD_VALIDATION_FAILED and a SelectedChoices field finding.
5. task#9 — GetAnswer for the same workload/lens/question: the emulator reports no SelectedChoices and a ChoiceAnswers[0] of NOT_APPLICABLE instead of the choice the last accepted write selected with Status SELECTED.
6. task#12 — GetAnswer with MilestoneNumber above the modelled maximum: the emulator answers 400 ValidationException instead of 404 ResourceNotFoundException with ResourceType MILESTONE and ResourceId '<WorkloadId>.<MilestoneNumber>'.
