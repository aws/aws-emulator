Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-describe-job-run-unknown-run-and-job-not-found
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-describe-job-run-unknown-run-and-job-not-found databrew-job-run-and-session-name-bounds databrew-publish-recipe-description-length-bounds databrew-recipe-publish-missing-and-tag-round-trip
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|DescribeJobRun|4xx->4xx|$.Message
Issue-Key: databrew|PublishRecipe|4xx->4xx|$.message
Issue-Key: databrew|PublishRecipe|4xx->4xx|$.Message
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# databrew: not-found and validation messages diverge from the wording DataBrew serves

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobRun

## Description
This bundle carries 3 CAUSES. Each returns the right HTTP status and the right error shape, but the error MESSAGE text diverges from what DataBrew words for that error class, so clients that surface or match on the message are misled.

Cause 1 of 3 — DescribeJobRun's ResourceNotFoundException does not word the (job name, runId) PAIR.
AWS: Smithy: DescribeJobRun GET /jobs/{Name}/jobRun/{RunId} with BOTH Name and RunId as required httpLabels; errors ResourceNotFoundException (404, member Message) and ValidationException (400). Because the run is addressed by the pair, the 404 message names both halves. Recorded behavior: the message's fixed text is 'Job Run with job name: ' + ' and runId: ' + ' was not found.' and its variable parts are the request's Name and RunId — e.g. 'Job Run with job name: <Name> and runId: <RunId> was not found.' This wording is served whether the job is missing or only the run id is unknown.
Emulator: answers 404 with 'Job <Name> wasn't found.' when the job name is not in store.jobs, and 'Job run <RunId> wasn't found.' when only the run is unknown (services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/describe_job_run.rs lines 193-211, via crate::state::not_found_message at src/state.rs line 86). Neither sentence is the pair sentence AWS serves, and the emulator leaks which half it failed to resolve in a shape AWS does not use.
Why: a client that logs or matches the ResourceNotFoundException message (dashboards, retry predicates, user-facing errors for a polled run) sees text AWS never produces, and loses the runId from the message entirely in the job-missing case.
Fix: DescribeJobRun's handler error construction — build one pair message from the request's Name and RunId for both not-found branches instead of calling the generic single-resource not_found_message helper.
Traces: databrew-describe-job-run-unknown-run-and-job-not-found#task#8, #task#9, #task#12 and databrew-job-run-and-session-name-bounds#task#2.

Cause 2 of 3 — PublishRecipe's ValidationException TRUNCATES the offending value in the message.
AWS: Smithy: PublishRecipe POST /recipes/{Name}/publishRecipe, input Description targets RecipeDescription with @length min 0 max 1024; error ValidationException (400). The constraint message is AWS's standard framework sentence, whose fixed text is '1 validation error detected: Value ' + QUOTED VALUE + ' at ' + QUOTED MEMBER PATH + ' failed to satisfy constraint: Member must have length less than or equal to ' + the limit — with the value reproduced IN FULL (the whole rejected string), the member path taken from the request ('description') and the limit from the model (1024).
Emulator: returns 400 ValidationException with the same sentence but the value replaced by a truncated rendering plus a byte annotation the real message never contains: "Value '<first ~250 chars>... [1025 bytes]' at 'description' failed to satisfy constraint: Member must have length less than or equal to 1024".
Why: the message is the only place the rejected value is reported; a client (or a human reading a log) that echoes it back or diffs it against its own input sees a mangled value, and the invented '... [N bytes]' marker makes the message unparseable by anything matching AWS's format.
Fix: the centralized request-validation layer that renders constraint ValidationException messages for this service (the deserialize -> validate step that runs before the handler), which must emit the full rejected value rather than an abbreviated one.
Traces: databrew-publish-recipe-description-length-bounds#task#2.

Cause 3 of 3 — PublishRecipe's ResourceNotFoundException adds a trailing period DataBrew does not serve.
AWS: Smithy: PublishRecipe POST /recipes/{Name}/publishRecipe, Name a required httpLabel; errors ResourceNotFoundException (404, member Message), ServiceQuotaExceededException (402), ValidationException (400). Recorded behavior: for a recipe name that does not exist this operation's 404 Message is the period-LESS sentence whose fixed text is 'Recipe ' + " wasn't found" with the request's Name as the only variable part — 'Recipe <Name> wasn't found'.
Emulator: answers 404 with 'Recipe <Name> wasn't found.' — the sentence-form wording with a terminating period, because the handler routes through crate::state::not_found_message (src/state.rs lines 86-94), which appends a period for every kind not listed in SHORT_FORM_RESOURCE_KINDS.
Why: clients and test suites that compare the error message exactly (or render it verbatim) see an extra character AWS does not send; it also hides that PublishRecipe words this refusal differently from the recipe reads, so the divergence is invisible until the message is compared.
Fix: PublishRecipe's not-found error construction (its handler), or the wording table in crate::state that decides which refusals take the terminating period, so this operation serves the period-less form.
Traces: databrew-publish-recipe-description-length-bounds#task#6 and databrew-recipe-publish-missing-and-tag-round-trip#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up):
- databrew-describe-job-run-unknown-run-and-job-not-found#task#8 DescribeJobRun: status 404 as expected, but $.Message expected 'Job Run with job name: <job name> and runId: <run id> was not found.', got 'Job <job name> wasn\u2019t found.' (literally "Job sim2real-probe-databrew-pj-dfb6419e wasn't found."). Same cluster: #task#9, #task#12, databrew-job-run-and-session-name-bounds#task#2 (4 steps).
- databrew-publish-recipe-description-length-bounds#task#2 PublishRecipe: status 400 as expected, $.message expected "1 validation error detected: Value '<the full 1025-character value>' at 'description' failed to satisfy constraint: Member must have length less than or equal to 1024", got the same sentence with the value rendered as "'<~250 chars>... [1025 bytes]'".
- databrew-publish-recipe-description-length-bounds#task#6 PublishRecipe: status 404 as expected, $.Message expected "Recipe <name> wasn't found" (no trailing period), got "Recipe <name> wasn't found." Same cluster: databrew-recipe-publish-missing-and-tag-round-trip#task#2.
Source evidence: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/describe_job_run.rs lines 193-211 (two single-resource messages for one pair lookup); services/service-com-amazonaws-databrew-awsgluedatabrew/src/state.rs lines 45-94 (not_found_message and the per-kind wording tables).

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS DataBrew client at it.
2. Call DescribeJobRun(Name=<any job name>, RunId=<any run id>) for a run that does not exist. Emulator: 404 'Job <name> wasn't found.' / 'Job run <runId> wasn't found.'; AWS: 404 'Job Run with job name: <name> and runId: <runId> was not found.'
3. Create a recipe, then call PublishRecipe(Name=<recipe>, Description=<a 1025-character string>). Emulator: 400 ValidationException whose message truncates the value and appends '... [1025 bytes]'; AWS: the same sentence carrying the full value.
4. Call PublishRecipe(Name=<a recipe name that does not exist>). Emulator: 404 "Recipe <name> wasn't found."; AWS: 404 "Recipe <name> wasn't found" with no trailing period.
Equivalently: replay fixtures databrew-describe-job-run-unknown-run-and-job-not-found (task#8), databrew-publish-recipe-description-length-bounds (task#2, task#6), databrew-recipe-publish-missing-and-tag-round-trip (task#2) and databrew-job-run-and-session-name-bounds (task#2).
