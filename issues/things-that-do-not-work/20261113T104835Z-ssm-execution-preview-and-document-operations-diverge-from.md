Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-automation-execution-preview-workflow
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-automation-execution-preview-workflow ssm-start-execution-preview-nonexistent-document ssm-execution-preview-get-nonexistent-error ssm-execution-preview-not-found ssm-get-execution-preview-unknown-id ssm-association-create-and-list ssm-document-delete-and-permission-describe-invalid
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|StartExecutionPreview|2xx->4xx|$.ExecutionPreviewId
Issue-Key: ssm|StartExecutionPreview|2xx->4xx|status
Issue-Key: ssm|GetExecutionPreview|4xx->4xx|$.Message
Issue-Key: ssm|DeleteDocument|4xx->4xx|$.Message
Issue-Commit: b3586cbe985cc2412e94e4f6c5b37df3b09bc671

# SSM execution-preview and document operations diverge from AWS on refusals and acceptance

> _Note: this issue was identified against the code base at commit `b3586cbe985cc2412e94e4f6c5b37df3b09bc671`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecutionPreview

## Description
Three net-new root causes in the ssm surface, all on the refusal/acceptance boundary: one request AWS accepts is refused outright, and two refusals reach the right error shape and HTTP status but carry emulator-authored message text instead of the text AWS words for that error class. The HTTP status, the error code AND the error message are all part of the contract, so all three must be corrected. Each cause is independent and must be closed separately; the fixture/step for each is named under its Traces line.

Cause 1 of 3: StartExecutionPreview refuses a preview start that AWS accepts.
  AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models StartExecutionPreview with input DocumentName (required, pattern ^[a-zA-Z0-9_\-.]{3,128}$), DocumentVersion (pattern ^([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$)$) and ExecutionInputs, output ExecutionPreviewId (@length 36..36, UUID pattern), and only two errors: InternalServerError (HTTP 500) and ValidationException (HTTP 400, documented 'The request isn't valid. Verify that you entered valid contents for the command and try again.'). For the recorded request AWS answers HTTP 200 with an ExecutionPreviewId minted for the preview (the recording asserts the member is present and shaped, not a literal id), i.e. the runbook the start names resolves and the preview is created. Recorded behavior only for which runbook resolves: the recording shows the start succeeding on the Automation runbook it names with no preceding CreateDocument of that name in the same step phase, so AWS resolves it out of the published runbook catalogue it serves to every account.
  Emulator: answers HTTP 400 and no ExecutionPreviewId at all ('status: expected 200, got 400' / '$.ExecutionPreviewId: missing (expected <shaped-not-asserted>, got None)'), so the preview is never minted and nothing is persisted for GetExecutionPreview to report.
  Why: a client cannot preview a runbook at all. The whole execution-preview flow (StartExecutionPreview -> poll GetExecutionPreview to a terminal Status -> read ExecutionPreview) is unreachable, and because the refusal is a 400 ValidationException the client is told its own request was malformed when it was valid — so a caller retries or surfaces a bogus input error instead of getting the preview id. Every downstream step of the preview workflow is blocked by this one refusal.
  Fix: the StartExecutionPreview handler's pre-persist validation chain in services/service-com-amazonaws-ssm-amazonssm/src/ops/start_execution_preview.rs — every branch there returns the same ValidationException/400, so the one that fires must be made to pass for a runbook AWS resolves. Hypothesis: it is the document resolution branch (`store.resolve_document_for_request(&input.document_name, ..)` returning None, or the following `document_type != AUTOMATION_DOCUMENT_TYPE` guard), i.e. the state's seeded document catalogue does not carry the AWS-published Automation runbook the start names, so the fix belongs in the document store's published-catalogue seeding rather than in the handler's control flow. The other candidate branches in the same chain are version selection (`document.select`), `automation::target_rejection`, `commands::parameter_rejection`, `runbook::steps_of` and `execution_previews::compute`; the handler emits no distinguishing body for this step, so the fixer must replay the step to see which message is rendered before choosing.
  Traces: ssm-automation-execution-preview-workflow#task#1 (representative, carried in Issue-Trace/Issue-Step), and ssm-start-execution-preview-nonexistent-document#task#1 (the positive control of a fixture whose later step exercises the unknown-runbook refusal).

Cause 2 of 3: GetExecutionPreview's ResourceNotFoundException message for an unknown or expired preview id is emulator-authored.
  AWS: Smithy models GetExecutionPreview with input ExecutionPreviewId (required, @length min 36 max 36, pattern ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$) and errors InternalServerError (500) and ResourceNotFoundException (client error, HTTP 400). The model does not state the message text, so this is recorded behavior only: for a syntactically valid but never-issued preview id AWS answers HTTP 400 / ResourceNotFoundException with the message `Execution preview '<ExecutionPreviewId>' does not exist or is expired`. Fixed text: `Execution preview '` then `' does not exist or is expired` — the id is wrapped in single quotes, the clause `or is expired` is present (an expired preview is refused identically, so the message does not distinguish them), and there is NO trailing period. Variable part: the ExecutionPreviewId copied verbatim from the request; the all-zero UUID in the recording is only a filled-in slot, never part of the requirement.
  Emulator: answers HTTP 400 with the right code but the message `Execution preview <id> does not exist.` — id unquoted, the `or is expired` clause dropped, and a trailing period added.
  Why: a client that branches on the refusal text cannot tell a never-issued preview id from one whose result window has lapsed, which is exactly the distinction AWS's combined wording exists to collapse; any client, runbook or test matching AWS's text treats this as an unrecognised error and fails even though code and status are right.
  Fix: the message builder behind GetExecutionPreview's not-found branch — crate::state::execution_previews::execution_preview_not_found_message, called from the handler's store-miss branch. The handler's control flow, region scoping and store lookup are correct; only the rendered text diverges.
  Traces: ssm-execution-preview-get-nonexistent-error#task#1 (cluster representative, listed in trace_ids), plus the same cluster's ssm-execution-preview-not-found#task#1 and ssm-get-execution-preview-unknown-id#task#1. NOTE: the open behavioral issue 20261113T052724Z-ssm-execution-preview-refusal-messages-are-emulator-authored.md describes this same wording defect from the behavioral corpus, but it carries no wire replay key, so this wire cause is filed here with the fixture/step a fixer must re-replay.

Cause 3 of 3: DeleteDocument's InvalidDocument message for a document the account does not hold is the generic document-not-found text, not the text AWS words for a delete.
  AWS: Smithy models DeleteDocument with input Name (required, pattern ^[a-zA-Z0-9_\-.]{3,128}$), DocumentVersion, VersionName and Force (default false), and errors AssociatedInstances, InternalServerError, InvalidDocument (client error, HTTP 400), InvalidDocumentOperation and TooManyUpdates. The model does not state the message text, so this is recorded behavior only: deleting a name the account holds no document for answers HTTP 400 / InvalidDocument with the message `Document <Name> does not exist in your account`. Fixed text: `Document ` then ` does not exist in your account` — note the account-scoped clause and NO trailing period. Variable part: the Name copied verbatim from the request (the probe document name in the recording is only a filled-in slot).
  Emulator: answers HTTP 400 / InvalidDocument with `Document with name <Name> does not exist.` — it inserts `with name`, drops the ` in your account` clause and adds a trailing period.
  Why: AWS's delete wording is account-scoped on purpose: it tells the caller the name is not THEIRS to delete, which is the distinction between this refusal and InvalidDocumentOperation on an AWS-published document that resolves but cannot be removed. A client matching AWS's text mis-classifies a missing-document delete, and a caller reading the error is not told the name was looked up in its own account.
  Fix: the DeleteDocument handler's not-found branch in services/service-com-amazonaws-ssm-amazonssm/src/ops/delete_document.rs, which builds its message from the shared crate::state::document_not_found_message. That shared builder renders the wording AWS uses for document READS, so DeleteDocument needs its own account-scoped message rather than a change to the shared builder — correcting the shared builder in place would push this delete wording onto every other document operation that reads it. The resolution logic and the chosen error shape are correct.
  Traces: ssm-association-create-and-list#teardown#10 (cluster representative, listed in trace_ids) and ssm-document-delete-and-permission-describe-invalid#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 850 steps, 436 passed, score 51.3%).

Cause 1 — StartExecutionPreview, fixture ssm-automation-execution-preview-workflow, step task#1 (cluster key service=ssm|StartExecutionPreview|status=200->400|keys=$.ExecutionPreviewId,status; 2 affected steps):
  expected: HTTP 200 with $.ExecutionPreviewId present (recorded as <shaped-not-asserted>)
  actual:   HTTP 400, $.ExecutionPreviewId missing (None)
  affected: ssm-automation-execution-preview-workflow#task#1, ssm-start-execution-preview-nonexistent-document#task#1

Cause 2 — GetExecutionPreview, fixture ssm-execution-preview-get-nonexistent-error, step task#1 (cluster key service=ssm|GetExecutionPreview|status=400->400|keys=$.Message; 3 affected steps):
  expected: HTTP 400, $.Message == "Execution preview '00000000-0000-0000-0000-000000000000' does not exist or is expired"
  actual:   HTTP 400 (status correct), $.Message == 'Execution preview 00000000-0000-0000-0000-000000000000 does not exist.'
  affected: ssm-execution-preview-get-nonexistent-error#task#1, ssm-execution-preview-not-found#task#1, ssm-get-execution-preview-unknown-id#task#1
  (the all-zero UUID is the request's value filling the message's variable slot; the requirement is the quoting, the 'or is expired' clause and the absent trailing period)

Cause 3 — DeleteDocument, fixture ssm-association-create-and-list, step teardown#10 (cluster key service=ssm|DeleteDocument|status=400->400|keys=$.Message; 2 affected steps):
  expected: HTTP 400, $.Message == 'Document sim2real-probe-ssm-ssmdoc-0a8f1c27 does not exist in your account'
  actual:   HTTP 400 (status correct), $.Message == 'Document with name sim2real-probe-ssm-ssmdoc-0a8f1c27 does not exist.'
  affected: ssm-association-create-and-list#teardown#10, ssm-document-delete-and-permission-describe-invalid#task#1
  (the probe document name is the request's value filling the variable slot; the requirement is the absence of 'with name', the ' in your account' clause and the absent trailing period)

Emulator code read for the Fix citations: services/service-com-amazonaws-ssm-amazonssm/src/ops/start_execution_preview.rs lines 137-261 (every pre-persist branch returns ValidationException/400 before the preview id is minted at line 263) and services/service-com-amazonaws-ssm-amazonssm/src/ops/delete_document.rs lines 189-194 (InvalidDocument built from crate::state::document_not_found_message).

## Steps to Reproduce
Cause 1:
1. Boot the emulator and replay fixture ssm-automation-execution-preview-workflow, step task#1 — call ssm:StartExecutionPreview naming the Automation runbook that fixture names (optionally with ExecutionInputs.Automation parameters/targets as recorded).
2. Observe HTTP 400 with no ExecutionPreviewId instead of HTTP 200 carrying a minted ExecutionPreviewId, and inspect the ValidationException message to see which pre-persist branch of the handler fired.
3. Same refusal on ssm-start-execution-preview-nonexistent-document#task#1. Positive control that must keep working: the later step of that same fixture, where an unknown runbook name IS refused with HTTP 400 / ValidationException.

Cause 2:
1. Call ssm:GetExecutionPreview with a syntactically valid but never-issued ExecutionPreviewId (any UUID matching the modeled pattern, e.g. the all-zero UUID the fixture uses).
2. Observe HTTP 400 / ResourceNotFoundException whose message reads 'Execution preview <id> does not exist.' instead of "Execution preview '<id>' does not exist or is expired".
3. Replay ssm-execution-preview-get-nonexistent-error#task#1, ssm-execution-preview-not-found#task#1 and ssm-get-execution-preview-unknown-id#task#1 to confirm.

Cause 3:
1. Call ssm:DeleteDocument with a Name matching the modeled pattern that the account holds no document for.
2. Observe HTTP 400 / InvalidDocument whose message reads 'Document with name <Name> does not exist.' instead of 'Document <Name> does not exist in your account'.
3. Replay ssm-association-create-and-list#teardown#10 and ssm-document-delete-and-permission-describe-invalid#task#1 to confirm. Positive control: deleting an AWS-published document must still be refused with InvalidDocumentOperation, not with this message.
