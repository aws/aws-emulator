Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.start-job-run-while-its-run-is-in-flight-rejected databrew.start-job-run-while-run-active-rejected databrew.recipe-job-run-past-its-timeout-ends-in-timeout-state databrew.ruleset-targeting-unknown-dataset-rejected databrew.session-action-on-closed-project-rejected
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.start-job-run-while-its-run-is-in-flight-rejected|_refusal#28|AssertionError|raise AssertionError(f"expected a refusal, but the call was accepted: {result!r}")
Issue-Key: databrew.start-job-run-while-run-active-rejected|_refusal#20|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: databrew.recipe-job-run-past-its-timeout-ends-in-timeout-state|run#36|AssertionError|assert timed["State"] == "TIMEOUT", (
Issue-Key: databrew.ruleset-targeting-unknown-dataset-rejected|run#20|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: databrew.session-action-on-closed-project-rejected|run#30|AssertionError|assert code == "ConflictException", f"an action on a closed project gave {code!r}"
Issue-Commit: 8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea

# databrew: 4 net-new causes — StartJobRun admits a concurrent run, Timeout is never enforced (no run ever reaches TIMEOUT), and the Ruleset and SendProjectSessionAction surfaces are still 501 scaffolds

> _Note: this issue was identified against the code base at commit `8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
This bundle carries 4 CAUSES, all in the databrew emulator. Each is listed with the failing behavioral check ids it is grounded in. The two open databrew issues do not cover any of them: '20261127T100937Z-databrew-not-found-and-validation-messages-diverge-from.md' is about DescribeJobRun/PublishRecipe message wording and '20261127T100937Z-databrew-profile-job-reads-listing-and-delete-404.md' is about PROFILE jobs never being stored. The stub issues auto-filed for this run name CreateProfileJob and StartProjectSession only, so Causes 3 and 4 (CreateRuleset/Ruleset reads and SendProjectSessionAction) are net new.

Cause 1 of 4 — StartJobRun accepts a second run while the job's previous run is still in flight, instead of refusing it with ConflictException.
AWS: Smithy: StartJobRun POST /jobs/{Name}/startJobRun, output RunId (required); modeled errors ConflictException (HTTP 409), ResourceNotFoundException (404), ServiceQuotaExceededException (402), ValidationException (400). A DataBrew job runs at most one run at a time: while the job's run is in a non-terminal state (STARTING/RUNNING), a further StartJobRun on that job is refused with 409 ConflictException. Recorded behavior: the message is entirely fixed text with no variable part — 'Job is already running.' The refusal creates no run, so the job still lists exactly the one run. Once the first run is terminal (SUCCEEDED/FAILED/TIMEOUT/STOPPED) StartJobRun is accepted again and mints a NEW RunId.
Emulator: the second StartJobRun answers HTTP 200 with a fresh RunId (a second run record), so the job accumulates two runs where AWS has one. No 409 is ever produced for this condition: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/start_job_run.rs resolves the job (lines 181-194), mints a run id and records the run unconditionally (lines 215-245); nothing inspects the job's existing runs for a non-terminal one, and the handler's ConflictException variant is never constructed.
Why: a client relying on DataBrew's single-run-per-job guarantee (a scheduler, a retry loop, an idempotent 'start if not running' wrapper) believes it launched a second concurrent run; its run bookkeeping and ListJobRuns counts diverge from AWS, and code whose error path handles ConflictException as 'already running' never executes.
Fix: StartJobRun's handler — an admission check, before the run id is minted, that refuses with ConflictException when crate::state holds a run of that job whose advanced state is not terminal.
Traces: databrew.start-job-run-while-its-run-is-in-flight-rejected, databrew.start-job-run-while-run-active-rejected.

Cause 2 of 4 — a job's Timeout is stored but never enforced: every run completes a fixed fraction of a second after it starts and reports SUCCEEDED, so the TIMEOUT state is unreachable.
AWS: Smithy: CreateRecipeJob input Timeout (target Timeout, range min 0, default 0), documented as "The job's timeout in minutes. A job that attempts to run longer than this timeout period ends with a status of TIMEOUT." DescribeJobRun/ListJobRuns report State from JobRunState, which includes TIMEOUT, plus StartedOn and CompletedOn. So a run whose work outlasts its Timeout reaches the terminal state TIMEOUT, and its CompletedOn is no earlier than StartedOn + Timeout minutes; an otherwise identical job given a Timeout longer than the work needs reaches SUCCEEDED and writes its output objects.
Emulator: the run's terminal state is whatever the synchronous execution answered and falls due a fixed 600 ms after StartedOn, regardless of the job's Timeout — services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs lines 410-421 (RUN_RUNNING_DUE_AFTER 300 ms, RUN_TERMINAL_DUE_AFTER 600 ms) and JobRunRecord::advance_to at lines 1261-1269. JobRecord carries `timeout` (state.rs line 504) but no code path reads it when a run is advanced, so a job created with Timeout=1 ends SUCCEEDED and TIMEOUT is never served for any job.
Why: a client that exercises or depends on DataBrew's timeout semantics (a pipeline that treats TIMEOUT differently from FAILED, an alarm on timed-out runs, a test of a deliberately short Timeout) can never observe the state; every run looks like a success, and the run's reported duration bears no relation to the configured Timeout.
Fix: the job-run state model in crate::state (JobRunRecord::advance_to and the due-after spans) together with the job record's stored `timeout`, so a run whose span exceeds Timeout minutes is advanced to TIMEOUT with CompletedOn at StartedOn + Timeout instead of to its work's outcome.
Traces: databrew.recipe-job-run-past-its-timeout-ends-in-timeout-state.

Cause 3 of 4 — the whole Ruleset surface is an unimplemented scaffold, so CreateRuleset against a dataset that does not exist answers 501 NotImplemented instead of DataBrew's 404 refusal.
AWS: Smithy: CreateRuleset POST /rulesets, required input Name (RulesetName, len 1..255), TargetArn (Arn, len 20..2048) and Rules (RuleList, len min 1), output Name (required); modeled errors ConflictException (409), ServiceQuotaExceededException (402), ValidationException (400). The model does not list ResourceNotFoundException for this operation — recorded behavior only: a TargetArn naming a dataset that was never created is refused with ResourceNotFoundException (HTTP 404) whose message's fixed text is 'Dataset ' + ' was not found', the variable part being the dataset name taken from the request's TargetArn ('Dataset <dataset-name> was not found'), and no ruleset is stored under the refused Name. The same recording shows the ruleset reads DataBrew serves for a name that does not exist: a 404 ResourceNotFoundException whose fixed text is 'Ruleset ' + " wasn't found" with NO terminating period, the variable part being the request's Name — from DescribeRuleset and from UpdateRuleset after a delete. A ruleset created against a real dataset is stored and read back with its TargetArn and Rules, UpdateRuleset replaces its Rules, and ListRulesets(TargetArn=<dataset arn>) lists it.
Emulator: every ruleset operation answers HTTP 501 with error code 'NotImplemented' — services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_ruleset.rs, describe_ruleset.rs, update_ruleset.rs, list_rulesets.rs and delete_ruleset.rs all still carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and return Scaffolded::not_implemented. The observed failure is CreateRuleset answering code 'NotImplemented' where 'ResourceNotFoundException' is owed.
Why: no data-quality workflow can be exercised at all — a client cannot create, read, update, list or delete a ruleset, and the one negative class it would rely on (a ruleset aimed at a missing dataset) surfaces as a server-side 501 that SDK retry logic treats as a transport/server fault rather than as a client error it must report.
Fix: the ruleset handlers (create_ruleset.rs, describe_ruleset.rs, update_ruleset.rs, list_rulesets.rs, delete_ruleset.rs) plus a ruleset record keyed by Name in crate::state; CreateRuleset must resolve the dataset named by TargetArn and refuse with the 'Dataset <name> was not found' 404, and the ruleset reads must serve the period-less "Ruleset <name> wasn't found" form rather than the period-appending generic crate::state::not_found_message (state.rs lines 45-94).
Traces: databrew.ruleset-targeting-unknown-dataset-rejected.

Cause 4 of 4 — SendProjectSessionAction is an unimplemented scaffold, so an action on a project whose session was never opened answers 501 NotImplemented instead of 409 ConflictException.
AWS: Smithy: SendProjectSessionAction PUT /projects/{Name}/sendProjectSessionAction, Name a required httpLabel (ProjectName, len 1..255), input RecipeStep/StepIndex/ClientSessionId/Preview/ViewFrame, output Name (required) + ActionId + Result; modeled errors ConflictException (HTTP 409), ResourceNotFoundException (HTTP 404), ValidationException (400). Recorded behavior: sending an action to an existing project that holds no open session is refused 409 ConflictException whose message's fixed text is 'The project ' + ' was closed.' with the request's Name as the only variable part; sending one to a project that does not exist is refused 404 ResourceNotFoundException whose fixed text is 'The project ' + " wasn't found." with the request's Name as the variable part. Neither refusal changes the project's recipe: its LATEST_WORKING steps are unchanged afterwards, and the same step is still accepted by UpdateRecipe.
Emulator: the call answers HTTP 501 with error code 'NotImplemented' — services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/send_project_session_action.rs lines 3-12 carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and the handler returns Scaffolded::not_implemented. Neither the closed-session 409 nor the missing-project 404 is distinguishable.
Why: an interactive-project client cannot tell 'you must open a session first' (ConflictException, retryable after StartProjectSession) from 'this project does not exist' (ResourceNotFoundException, fatal) — both arrive as a 501 that SDKs classify as a server fault and retry, and no interactive recipe-step flow can be emulated.
Fix: SendProjectSessionAction's handler (send_project_session_action.rs), resolving the project in crate::state and refusing with the ConflictException/ResourceNotFoundException wording above before any session action is applied.
Traces: databrew.session-action-on-closed-project-rejected.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator (emulator up and serving; databrew scored 7/19, 12 failed).

Cause 1:
- databrew.start-job-run-while-its-run-is-in-flight-rejected: after the first StartJobRun, the second StartJobRun on the same job was expected to raise ConflictException with message 'Job is already running.'; the emulator returned HTTP 200 with a new RunId. Check assertion: `got == ("ConflictException", "Job is already running.")`, harness raised "expected a refusal, but the call was accepted: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'RunId': '<minted run id>'}".
- databrew.start-job-run-while-run-active-rejected: identical divergence — "the call was expected to be refused but returned {... 'HTTPStatusCode': 200, 'RunId': '<minted run id>'}". (The RunId value itself is per-run and not part of the requirement.)
- Source: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/start_job_run.rs lines 181-245 — job lookup, mint_run_id and record_run with no non-terminal-run guard; the ConflictException variant declared at line 105 is unused.

Cause 2:
- databrew.recipe-job-run-past-its-timeout-ends-in-timeout-state: two identical recipe jobs over the same dataset/recipe, one with Timeout=1 minute and one with Timeout=30. Expected: the 30-minute job SUCCEEDED with the transformed rows, the 1-minute job terminal in State 'TIMEOUT' with CompletedOn - StartedOn >= ~60 s, and ListJobRuns reporting that one run in TIMEOUT. Got: AssertionError "the job with a 1-minute Timeout ended 'SUCCEEDED', expected 'TIMEOUT'".
- Source: services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs lines 410-421 (fixed 300 ms/600 ms transition spans), line 504 (JobRecord.timeout stored) and lines 1261-1269 (JobRunRecord::advance_to, which consults only those fixed spans and the recorded work outcome).

Cause 3:
- databrew.ruleset-targeting-unknown-dataset-rejected: CreateRuleset(Name=<new ruleset>, TargetArn=arn:aws:databrew:<region>:<account>:dataset/<name never created>, Rules=[...]) expected 404 ResourceNotFoundException with a message starting 'Dataset ' and ending ' was not found' and naming that dataset; got error code 'NotImplemented' (HTTP 501). AssertionError: "CreateRuleset on an absent dataset answered code 'NotImplemented'". The check's later steps (DescribeRuleset/UpdateRuleset 404 "Ruleset <name> wasn't found" with no trailing period, create/describe/update/list/delete of a valid ruleset) are all unreachable behind the same 501.
- Source: create_ruleset.rs lines 3-12 (SCAFFOLDED marker, 501 NotImplemented); describe_ruleset.rs, update_ruleset.rs, list_rulesets.rs, delete_ruleset.rs carry the same marker at lines 3-6.

Cause 4:
- databrew.session-action-on-closed-project-rejected: after CreateProject (which succeeds) and DescribeProject (which correctly omits SessionStatus), SendProjectSessionAction(Name=<project>, ClientSessionId=<id>, RecipeStep=<LOWER_CASE step>) expected 409 ConflictException with message 'The project <project> was closed.'; got error code 'NotImplemented' (HTTP 501). AssertionError: "an action on a closed project gave 'NotImplemented'". The companion assertion for a project that does not exist (404 'The project <name> wasn't found.') is unreachable behind the same 501.
- Source: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/send_project_session_action.rs lines 3-12.

## Steps to Reproduce
Boot the emulator from this checkout and point an AWS DataBrew (plus S3/IAM) client at it.

Cause 1 — concurrent run:
1. Create an S3 bucket with a CSV object, an IAM role DataBrew can assume, a dataset over that object, a recipe (one UPPER_CASE step) and publish it.
2. CreateRecipeJob(Name=<job>, DatasetName=<dataset>, RoleArn=<role>, RecipeReference={Name:<recipe>,RecipeVersion:'1.0'}, Outputs=[{Format:'CSV',Location:{Bucket:<bucket>,Key:'out/'}}], Timeout=30, MaxRetries=0).
3. StartJobRun(Name=<job>), then immediately StartJobRun(Name=<job>) again. Emulator: 200 with a second RunId. AWS: 409 ConflictException 'Job is already running.', and ListJobRuns still shows exactly one run.

Cause 2 — Timeout:
4. Create two jobs over the same dataset/recipe, one with Timeout=1 and one with Timeout=30, and StartJobRun on both.
5. Poll DescribeJobRun until each run is terminal. Emulator: both end 'SUCCEEDED' about 0.6 s after StartedOn. AWS: the Timeout=1 job's run ends 'TIMEOUT' with CompletedOn - StartedOn >= 60 s and ListJobRuns reports it in TIMEOUT; the Timeout=30 job's run ends 'SUCCEEDED'.

Cause 3 — rulesets:
6. CreateRuleset(Name=<any>, TargetArn='arn:aws:databrew:<region>:<account>:dataset/<a dataset that was never created>', Rules=[{Name:'amount-positive',CheckExpression:':col1 > :val1',SubstitutionMap:{':col1':'`amount`',':val1':'0'}}]). Emulator: 501 'NotImplemented'. AWS: 404 ResourceNotFoundException 'Dataset <that dataset name> was not found', and no ruleset is stored.
7. Repeat against a dataset that DOES exist and then DescribeRuleset/UpdateRuleset/ListRulesets(TargetArn=...)/DeleteRuleset. Emulator: 501 on each. AWS: the ruleset round-trips, and after the delete DescribeRuleset/UpdateRuleset answer 404 "Ruleset <name> wasn't found" (no trailing period).

Cause 4 — project session action:
8. Create a dataset and a recipe, then CreateProject(Name=<project>, DatasetName=<dataset>, RecipeName=<recipe>, RoleArn=<role>) and do NOT open a session.
9. SendProjectSessionAction(Name=<project>, ClientSessionId='c0ffee42', RecipeStep=<a valid step>). Emulator: 501 'NotImplemented'. AWS: 409 ConflictException 'The project <project> was closed.', with the recipe's LATEST_WORKING steps unchanged.
10. SendProjectSessionAction(Name=<a project name that does not exist>, ...). Emulator: 501 'NotImplemented'. AWS: 404 ResourceNotFoundException "The project <name> wasn't found."

Equivalently: replay behavioral checks databrew.start-job-run-while-its-run-is-in-flight-rejected, databrew.start-job-run-while-run-active-rejected, databrew.recipe-job-run-past-its-timeout-ends-in-timeout-state, databrew.ruleset-targeting-unknown-dataset-rejected and databrew.session-action-on-closed-project-rejected.
