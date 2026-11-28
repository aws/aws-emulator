Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-workflow-build-versions-accumulate-per-semantic-version
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-workflow-build-versions-accumulate-per-semantic-version imagebuilder-ibp-traj-component-listing-and-paging imagebuilder-list-workflow-step-executions-validates-paging-before-it-looks-up-the-execution
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateWorkflow|4xx->2xx|$.message
Issue-Key: imagebuilder|CreateWorkflow|4xx->2xx|$.requestId
Issue-Key: imagebuilder|CreateWorkflow|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateWorkflow|4xx->2xx|status
Issue-Key: imagebuilder|ListComponents|4xx->2xx|$.message
Issue-Key: imagebuilder|ListComponents|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|ListComponents|4xx->2xx|status
Issue-Key: imagebuilder|ListWorkflowStepExecutions|4xx->4xx|$.message
Issue-Key: imagebuilder|ListWorkflowStepExecutions|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 7e7f1782eb72d186b4a02ad6793c94a317471370

# Image Builder skips its own request-validation and duplicate-version refusals on workflow and component APIs

> _Note: this issue was identified against the code base at commit `7e7f1782eb72d186b4a02ad6793c94a317471370`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
Three causes, one shared fault: the imagebuilder handlers do not run the service's own pre-dispatch refusals — a conflicting resource is accepted, an out-of-range page size is accepted, and a constraint violation is reported with generic framework wording instead of Image Builder's.

Cause 1 of 3: CreateWorkflow accepts a semantic version that already exists instead of refusing it.
  AWS: Smithy: CreateWorkflow errors include ResourceInUseException (client, httpError 400) — "The resource that you are trying to operate on is currently in use." The recording shows AWS refusing a repeat CreateWorkflow for the same name/type/semanticVersion with HTTP 400 and message "The following resource 'Workflow' already exists: '<workflowBuildVersionArn>'." Fixed text: "The following resource 'Workflow' already exists: '" ... "'." — the resource label (Workflow) is the conflicting resource type and the quoted ARN is the existing workflow build-version ARN taken from the emulator's state for the requested name/type/semanticVersion (the recorded ARN's account, region, random name suffix and build number are environment values, not part of the requirement). The response also carries a top-level requestId and an x-amzn-errortype header for the modeled error.
  Emulator: returns 200 and creates another build version ($.message missing, $.requestId missing, $headers.x-amzn-errortype missing; status expected 400, got 200).
  Why: a client that relies on AWS's conflict refusal to detect "this workflow version is already published" is told the publish succeeded, and the emulator's state silently accumulates duplicate build versions under one semantic version — so subsequent ListWorkflowBuildVersions/Get reads and any idempotency logic diverge from AWS from that point on.
  Fix: the CreateWorkflow handler's pre-insert conflict check plus the ResourceInUseException error path (state lookup by name+type+semanticVersion before allocating a new build version).
  Traces: imagebuilder-workflow-build-versions-accumulate-per-semantic-version#task#3.

Cause 2 of 3: ListComponents does not enforce the service's maxResults ceiling.
  AWS: Smithy: ListComponents.maxResults targets RestrictedInteger @range min 1 max 100, and its documentation says the default page is 25 items; the recording shows the real service refusing the request with HTTP 400 and message "The value supplied for parameter 'maxResults' is not valid. maxResults must be smaller than or equal to 25" (modeled error family: InvalidParameterValueException / ClientException, both client 400; the recorded x-amzn-errortype is not asserted). Fixed text: "The value supplied for parameter '" <member name> "' is not valid. " <member name> " must be smaller than or equal to " <limit> — the member name comes from the request and the limit is the operation's enforced ceiling; recorded behavior only for the ceiling being 25 rather than the model's 100.
  Emulator: accepts the oversized page size and answers 200 with a normal listing (status expected 400, got 200; $.message and $headers.x-amzn-errortype absent).
  Why: a client probing or paging with a large page size gets a success where AWS refuses, so paging code written against AWS's refusal (clamp-and-retry) is never exercised and the emulator reports page sizes AWS will not serve.
  Fix: the ListComponents input validation (maxResults bound check) ahead of the listing, emitting the modeled 400 client error.
  Traces: imagebuilder-ibp-traj-component-listing-and-paging#task#6.

Cause 3 of 3: ListWorkflowStepExecutions reports an empty nextToken with generic framework wording and the wrong error type.
  AWS: Smithy: ListWorkflowStepExecutions.nextToken targets PaginationToken @length min 1 max 65535; the recording shows HTTP 400 with message "The value supplied for parameter 'nextToken' is not valid. nextToken must be longer than or equal to 1 characters" (modeled errors for this op: InvalidPaginationTokenException / InvalidRequestException / ClientException, all client 400; the recorded x-amzn-errortype is not asserted, but it is one of Image Builder's modeled error shapes, not a generic ValidationException). Fixed text: "The value supplied for parameter '" <member> "' is not valid. " <member> " must be longer than or equal to " <min> " characters"; the member name and the min come from the request and the member's @length constraint.
  Emulator: right status (400) but the wrong message and error type — message "1 validation error detected: Value '' at 'nextToken' failed to satisfy constraint: Member must have length greater than or equal to 1" and header x-amzn-errortype: ValidationException.
  Why: a client that matches on Image Builder's error type or parses its message wording to decide whether to drop a stale pagination token does not recognize the emulator's generic framework error, so it surfaces an unexpected failure rather than restarting pagination.
  Fix: the ListWorkflowStepExecutions input validation — raise Image Builder's own parameter-value refusal (and its modeled errortype) instead of letting the generic constraint-validation layer format the response.
  Traces: imagebuilder-list-workflow-step-executions-validates-paging-before-it-looks-up-the-execution#task#4.

No open issue covers these three: 20261127T125601Z-imagebuilder-image-builder-create-time-validation-gaps-createworkflow-accepts concerns invalid workflow STEP DOCUMENTS accepted at create time, not duplicate semantic versions, the maxResults ceiling, or the nextToken refusal wording.

## Evidence
Replayed against the emulator at the bound data listener:

Cause 1 — CreateWorkflow, fixture imagebuilder-workflow-build-versions-accumulate-per-semantic-version step task#3:
  status: expected 400, got 200
  $.message: missing (expected "The following resource 'Workflow' already exists: 'arn:aws:imagebuilder:us-west-2:000000000000:workflow/test/<name>/1.0.0/2'.", got None)
  $.requestId: missing (expected '<shaped-not-asserted>', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  (the account id, region and generated workflow name in the recorded ARN are environment values; the template and the resource label are the requirement)

Cause 2 — ListComponents, fixture imagebuilder-ibp-traj-component-listing-and-paging step task#6:
  status: expected 400, got 200
  $.message: missing (expected "The value supplied for parameter 'maxResults' is not valid. maxResults must be smaller than or equal to 25", got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)

Cause 3 — ListWorkflowStepExecutions, fixture imagebuilder-list-workflow-step-executions-validates-paging-before-it-looks-up-the-execution step task#4:
  status 400 expected and 400 returned, but:
  $.message: value_mismatch (expected "The value supplied for parameter 'nextToken' is not valid. nextToken must be longer than or equal to 1 characters", got "1 validation error detected: Value '' at 'nextToken' failed to satisfy constraint: Member must have length greater than or equal to 1")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')

Model evidence (smithy_query): CreateWorkflow errors include ResourceInUseException (400); ListComponents.maxResults RestrictedInteger @range 1..100 with docs stating Image Builder returns up to 25 items by default; ListWorkflowStepExecutions.nextToken PaginationToken @length 1..65535 with InvalidPaginationTokenException/InvalidRequestException/ClientException modeled.

## Steps to Reproduce
Cause 1: PUT /CreateWorkflow (name N, type test, semanticVersion 1.0.0, inline data, clientToken) — succeeds. Repeat the identical PUT /CreateWorkflow: the emulator returns 200 with a new build version instead of 400 ResourceInUseException "The following resource 'Workflow' already exists: '<arn>'.". Replay imagebuilder-workflow-build-versions-accumulate-per-semantic-version#task#3.
Cause 2: POST /ListComponents with maxResults above the enforced ceiling (e.g. 26) — the emulator returns 200 with a listing instead of a 400 parameter-value refusal. Replay imagebuilder-ibp-traj-component-listing-and-paging#task#6.
Cause 3: POST /ListWorkflowStepExecutions with workflowExecutionId set and nextToken set to the empty string — the emulator returns 400 but with the generic "1 validation error detected: ..." message and x-amzn-errortype: ValidationException instead of Image Builder's "The value supplied for parameter 'nextToken' is not valid. ..." wording. Replay imagebuilder-list-workflow-step-executions-validates-paging-before-it-looks-up-the-execution#task#4.
