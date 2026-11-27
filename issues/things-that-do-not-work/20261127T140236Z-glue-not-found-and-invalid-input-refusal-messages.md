Issue-Source: trace-sweep-divergence
Issue-Trace: glue-batch-stop-job-run-input-refusals
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-batch-stop-job-run-input-refusals glue-batch-stop-job-run-refuses-every-entry-of-an-unknown-job glue-crawler-metrics-and-schedule-management glue-crawler-stop-not-running glue-trigger-tag-round-trip-and-delete glue-connection-type-registration-lifecycle glue-start-data-quality-rule-recommendation-run-cancel-and-validation
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|BatchStopJobRun|2xx->2xx|$.Errors[*].ErrorDetail.ErrorMessage
Issue-Key: glue|BatchStopJobRun|2xx->2xx|$.Errors[*]
Issue-Key: glue|UpdateCrawlerSchedule|4xx->4xx|$.Message
Issue-Key: glue|StopCrawler|4xx->4xx|$.Message
Issue-Key: glue|GetTags|4xx->4xx|$.Message
Issue-Key: glue|RegisterConnectionType|4xx->4xx|$.Message
Issue-Key: glue|StartDataQualityRuleRecommendationRun|4xx->4xx|$.Message
Issue-Commit: bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5

# Glue not-found and invalid-input refusal messages are emulator-authored instead of AWS's text

> _Note: this issue was identified against the code base at commit `bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchStopJobRun

## Description
Six net-new root causes in glue: each refusal carries the right HTTP status and (except where noted) the right error code, but a message the emulator wrote itself rather than the sentence AWS emits for that error class.

Cause 1 of 6: BatchStopJobRun's per-entry ErrorDetail.ErrorMessage for a run that cannot be stopped says 'Job not found.' instead of 'Job Run not found.'
AWS: BatchStopJobRun answers HTTP 200 with Errors[], one BatchStopJobRunError per id that could not be stopped, each naming JobName and JobRunId from the request and carrying ErrorDetail with ErrorCode `EntityNotFoundException` and ErrorMessage `Job Run not found.` (Smithy: BatchStopJobRunErrorList -> BatchStopJobRunError{JobName, JobRunId, ErrorDetail}; ErrorDetail{ErrorCode, ErrorMessage}). The message is entirely fixed text — it names no request value — and it is the per-entry wording for both an unknown run id and an unknown job name: when the JOB does not exist, every requested id still comes back as its own entry with this same text, not a job-level message.
Emulator: the same entries carry ErrorCode `EntityNotFoundException` with ErrorMessage `Job not found.` — different fixed text, wrong in both the unknown-id case and the unknown-job case (where the emulator substitutes a job-level sentence for the per-run one).
Why: a client that classifies partial-batch failures on the per-entry message, and every tool that logs or diffs it, sees text no AWS region produces; the unknown-job and unknown-run cases become indistinguishable from each other in the opposite direction to AWS.
Fix: the per-entry error construction in the BatchStopJobRun handler of the glue service crate (one batch-specific message constant for every unstoppable entry).
Traces: glue-batch-stop-job-run-input-refusals#task#1, glue-batch-stop-job-run-input-refusals#task#3, glue-batch-stop-job-run-refuses-every-entry-of-an-unknown-job#task#1.
Note: the open behavioral issue 20261127T094450Z-glue-5-net-new-causes-batchdeletepartition-and-batchstopjobrun.md records a DIFFERENT wrong text on this same member ('Job run not found for the given job name and runId.', the GetJobRun sentence); the wire replay shows 'Job not found.' and that issue carries no replay key for these fixtures, so it does not cover them.

Cause 2 of 6: UpdateCrawlerSchedule's EntityNotFoundException message is emulator-authored and drops the account-qualified crawler identity.
AWS: UpdateCrawlerSchedule.CrawlerName is `@required`; an unknown crawler is refused 400 `EntityNotFoundException` (Smithy: UpdateCrawlerSchedule models EntityNotFoundException) with message `Crawler <accountId>:<crawlerName> not found.` — fixed text `Crawler ` / `:` / ` not found.` around the caller's catalog/account id and the CrawlerName from the request.
Emulator: `Crawler entry with name <crawlerName> does not exist` — different fixed text and no account qualifier.
Why: a client matching the refusal text, or a human reading it, cannot tell which account the crawler was looked up in, and an exact-match assertion on AWS's sentence fails.
Fix: the not-found message constant in the UpdateCrawlerSchedule handler of the glue service crate.
Traces: glue-crawler-metrics-and-schedule-management#task#9.

Cause 3 of 6: StopCrawler's EntityNotFoundException message uses the emulator's 'Crawler entry with name' phrasing.
AWS: StopCrawler.Name is `@required`; an unknown crawler is refused 400 `EntityNotFoundException` (Smithy: StopCrawler models CrawlerNotRunningException, CrawlerStoppingException, EntityNotFoundException, OperationTimeoutException) with message `Crawler with name <crawlerName> does not exist` — fixed text `Crawler with name ` and ` does not exist` around the request's Name, with no terminal period. Note AWS's stop path wording differs from the schedule path in Cause 2; they are not one constant.
Emulator: `Crawler entry with name <crawlerName> does not exist` — the word `entry` inserted into otherwise-correct fixed text.
Why: the sentence is a requirement of the error class; a client or test asserting AWS's text fails, and the emulator collapses two distinct AWS wordings (Cause 2's and this one) into one.
Fix: the not-found message constant in the StopCrawler handler of the glue service crate.
Traces: glue-crawler-stop-not-running#task#8.

Cause 4 of 6: GetTags on a deleted trigger ARN returns the generic 'Entity not found' instead of AWS's per-resource-type sentence.
AWS: GetTags.ResourceArn is `@required` and must be a glue ARN; for an ARN that names a trigger which no longer exists the refusal is 400 `EntityNotFoundException` with message `Trigger not found.` — fixed text naming the resource type parsed out of the ARN (Smithy: GetTags models EntityNotFoundException, InvalidInputException).
Emulator: `Entity not found` — a single generic sentence for every resource type, with no type name and no terminal period.
Why: the message is the only place the refusal says WHAT was missing, so a client cannot distinguish a vanished trigger from any other vanished taggable resource.
Fix: the ARN-to-resource-type resolution and the per-type not-found message in the GetTags handler of the glue service crate.
Traces: glue-trigger-tag-round-trip-and-delete#task#8.
Note: 20261120T124947Z-glue-2-net-new-causes-gettags-post-delete.md describes this defect class on GetTags but carries no replay key for this fixture step, so this wire divergence is unowned.

Cause 5 of 6: RegisterConnectionType's invalid-ConnectionType message states only part of AWS's rule.
AWS: RegisterConnectionType.ConnectionType is `@required`; a value that is not a well-formed custom connection type is refused 400 with message `Invalid connection type. ConnectionType must start with REST- prefix, be in all uppercase, and contain only alphanumeric characters and hyphens.` (Smithy: RegisterConnectionType models InvalidInputException/ValidationException; the sentence is recorded behavior). The whole message is fixed text and it states three rules: the REST- prefix, all-uppercase, and the alphanumeric-plus-hyphen character set.
Emulator: `Invalid ConnectionType format. Valid connection types begin with REST-` — different fixed text that states only the prefix rule and omits the case and character-set rules.
Why: the message is how a caller learns why the name was rejected; a caller fixing only the prefix is refused again with no new information.
Fix: the ConnectionType validation message in the RegisterConnectionType handler of the glue service crate.
Traces: glue-connection-type-registration-lifecycle#task#3.

Cause 6 of 6: StartDataQualityRuleRecommendationRun's invalid-DataSource message is re-worded.
AWS: StartDataQualityRuleRecommendationRun.DataSource is `@required` (Smithy: input DataSource -> DataSource, required; InvalidInputException modeled). A DataSource that specifies neither table form is refused 400 with message `Invalid DataSource: Must contain either GlueTable or DataQualityGlueTable` — fixed text, no request values, no terminal period, and it names the two member names as the model spells them.
Emulator: `DataSource must specify either a GlueTable or a DataQualityGlueTable.` — the same meaning in different fixed text, with a trailing period AWS does not send.
Why: an exact-match or prefix-match on AWS's refusal text fails; the `Invalid DataSource: ` prefix AWS uses to group this error class is absent.
Fix: the DataSource validation message in the StartDataQualityRuleRecommendationRun handler of the glue service crate.
Traces: glue-start-data-quality-rule-recommendation-run-cancel-and-validation#task#10.

Related but not covering: 20261030T000205Z-glue-delete-path-refusals-carry-emulator-authored-messages.md (DeleteIntegrationTableProperties/DeletePartition) and 20261127T055032Z-glue-400-refusal-bodies-diverge-an-emulator-authored.md (GetTables catalog id) are the same defect class on other operations and own none of the steps above.

## Evidence
Cause 1 — fixture glue-batch-stop-job-run-input-refusals step task#1, expected 200 / actual 200:
  $.Errors[0].ErrorDetail.ErrorMessage: value_mismatch (expected 'Job Run not found.', got 'Job not found.')
  Same cluster at step task#3 (25 entries) and at glue-batch-stop-job-run-refuses-every-entry-of-an-unknown-job#task#1 (unknown job, 3 entries): every Errors[i].ErrorDetail is {'ErrorCode': 'EntityNotFoundException', 'ErrorMessage': 'Job not found.'} where AWS sends 'Job Run not found.'; JobName/JobRunId echo correctly.

Cause 2 — fixture glue-crawler-metrics-and-schedule-management step task#9 (UpdateCrawlerSchedule), expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Crawler <accountId>:<crawlerName> not found.', got 'Crawler entry with name <crawlerName> does not exist')
  Recorded instance: expected 'Crawler 000000000000:sim2real-probe-glue-crm-missing-e8844b47 not found.' — the account id and crawler name are request/state values, the rest is fixed text.

Cause 3 — fixture glue-crawler-stop-not-running step task#8 (StopCrawler), expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Crawler with name <crawlerName> does not exist', got 'Crawler entry with name <crawlerName> does not exist')

Cause 4 — fixture glue-trigger-tag-round-trip-and-delete step task#8 (GetTags), expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Trigger not found.', got 'Entity not found')

Cause 5 — fixture glue-connection-type-registration-lifecycle step task#3 (RegisterConnectionType), expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Invalid connection type. ConnectionType must start with REST- prefix, be in all uppercase, and contain only alphanumeric characters and hyphens.', got 'Invalid ConnectionType format. Valid connection types begin with REST-')

Cause 6 — fixture glue-start-data-quality-rule-recommendation-run-cancel-and-validation step task#10, expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Invalid DataSource: Must contain either GlueTable or DataQualityGlueTable', got 'DataSource must specify either a GlueTable or a DataQualityGlueTable.')

## Steps to Reproduce
Cause 1: CreateJob, then BatchStopJobRun(JobName, JobRunIds=[ids the job never issued]) and read Errors[*].ErrorDetail.ErrorMessage; repeat with a JobName that does not exist and read every entry's ErrorDetail.
Cause 2: POST AWSGlue.UpdateCrawlerSchedule with a CrawlerName no crawler has; read $.Message.
Cause 3: POST AWSGlue.StopCrawler with a Name no crawler has; read $.Message.
Cause 4: CreateTrigger with Tags, DeleteTrigger, then GetTags on the trigger ARN; read $.Message.
Cause 5: POST AWSGlue.RegisterConnectionType with a ConnectionType lacking the REST- prefix; read $.Message.
Cause 6: POST AWSGlue.StartDataQualityRuleRecommendationRun with a DataSource containing neither GlueTable nor DataQualityGlueTable; read $.Message.
