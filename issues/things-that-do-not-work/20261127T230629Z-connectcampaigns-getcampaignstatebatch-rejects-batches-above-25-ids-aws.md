Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.get-campaign-state-batch-too-many-ids-rejected
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns|GetCampaignStateBatch|ValidationException
Issue-Commit: 11892c8a0fd001958f088427b8ee0a7691e93494

# ConnectCampaigns GetCampaignStateBatch rejects batches above 25 ids (AWS allows 500) and its over-limit ValidationException message omits the offending value

> _Note: this issue was identified against the code base at commit `11892c8a0fd001958f088427b8ee0a7691e93494`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCampaignStateBatch

## Description
TWO causes on ConnectCampaigns v1 GetCampaignStateBatch (Smithy: services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/smithy-model/com-amazonaws-connectcampaigns-amazonconnectcampaignservice.json — AmazonConnectCampaignService). Both are reached by the same call and both must be closed. Neither is covered by the open issue 20261124T184219Z-connect-campaigns-connectcampaigns-v1-error-text-and-batch-failure-reasons.md: its Cause 2 is about the missing per-entry failure-reason text on a 200 body and carries only the 2xx replay key for this operation, not any over-limit refusal; nor by 20261117T041857Z-connect-campaigns-getcampaignstatebatch-failedrequests-entries-omit-the-failurereason-text.md, same reason.

Cause 1 of 2: the accepted batch size is capped at the modeled 25 instead of the service's real 500.
AWS: Smithy models GetCampaignStateBatch as POST /campaigns-state with input member campaignIds -> CampaignIdList @required @length min 1 max 25 (the list shape CampaignIdList itself carries no length constraint; the bound sits on the request member), output successfulRequests/failedRequests @length 0..25, errors ValidationException(400), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500). The enforced bound is recorded behavior only and it is NOT the modeled one: the recording shows AWS accepting a request of 500 well-formed campaignIds with HTTP 200, returning successfulRequests = [] and failedRequests with one entry per requested id (500 entries) when none of those campaigns exist — i.e. the real limit is 500 and the modeled 25 is stale documentation. Only at 501 ids does AWS refuse.
Emulator: refuses the 500-id request outright with HTTP 400 ValidationException, message "1 validation error detected: Value at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 25" — it enforces the stale modeled maximum, so every batch of 26..500 ids that AWS serves is rejected.
Why: a client that batches up to the service's real limit (the documented-and-advertised batching win of this API) has every such call refused as a validation error. The caller cannot retry its way out — the request is well-formed for AWS — so batch state lookups silently cap at 25 ids per call or fail hard, and partial-failure handling built on the 500-wide failedRequests list is never exercised.
Fix: GetCampaignStateBatch's input validation for connect-campaigns — the campaignIds list-length check must use the service's enforced maximum of 500 rather than the modeled 25, while keeping the min-1 check. The response-side 0..25 bound on successfulRequests/failedRequests must likewise not truncate or reject a 500-element answer.
Traces: connectcampaigns.get-campaign-state-batch-too-many-ids-rejected.

Cause 2 of 2: the over-limit ValidationException message drops the offending value echo.
AWS: ValidationException is httpError 400 with a @required message member; the wording is recorded behavior only. For a list-length violation on this input AWS returns the standard framework template whose FIXED text is "1 validation error detected: Value '[", then "]' at '", then "' failed to satisfy constraint: Member must have length less than or equal to " — and whose VARIABLE parts are, in order, the rejected value rendered as the request's list elements joined by ", "; the member path (here campaignIds); and the limit (here 500). The ids and the count in any one recording merely fill that template.
Emulator: for its own (wrong, see Cause 1) bound it emits "1 validation error detected: Value at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 25" — the "Value '[<joined elements>]'" clause is absent entirely, so the message does not echo the rejected value at all, and the limit it names is 25 rather than 500.
Why: callers and operators match and log this message to learn WHICH value was rejected and what the limit is. With the value clause missing and the limit misstated, a client cannot derive its real batching ceiling from the refusal, and any assertion on AWS's standard validation wording diverges.
Fix: the connect-campaigns ValidationException construction for GetCampaignStateBatch's list-length check (the operation's validation path, where the message is formatted) must render the full standard template including the "Value '[...]' at '<member>'" clause populated from the request's campaignIds, with the limit taken from the corrected bound.
Traces: connectcampaigns.get-campaign-state-batch-too-many-ids-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, emulator up (60/63 checks passed overall, so the service is serving).

Failing check [error] connectcampaigns.get-campaign-state-batch-too-many-ids-rejected.

Cause 1 — the at-limit call:
  Request: GetCampaignStateBatch with 500 well-formed, nonexistent campaignIds.
  Expected (recorded AWS): HTTP 200, successfulRequests == [], len(failedRequests) == 500.
  Actual (emulator): HTTP 400 ValidationException, raised out of botocore as
    botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetCampaignStateBatch operation: 1 validation error detected: Value at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 25
  The call never reaches the batch-lookup path, so the check aborts before its over-limit assertion.

Cause 2 — the over-limit call (unreached in this run because Cause 1 aborts first, but the same formatter produces it):
  Request: GetCampaignStateBatch with 501 ids.
  Expected (recorded AWS): HTTP 400, Error.Code == 'ValidationException', Error.Message ==
    "1 validation error detected: Value '[<campaignIds joined by ', '>]' at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 500"
    Fixed text: "1 validation error detected: Value '[" / "]' at '" / "' failed to satisfy constraint: Member must have length less than or equal to ". Variable: the joined request ids, the member name campaignIds, the limit 500.
  Actual (emulator): the same formatter that produced the Cause 1 message — no "Value '[...]'" clause, limit rendered as 25.

Model evidence (smithy_query): GetCampaignStateBatch http {POST /campaigns-state, code 200}; input campaignIds -> CampaignIdList, @required, @length {min 1, max 25}; CampaignIdList has no own length constraint, member CampaignId @length 0..256 @pattern ^[\S]*$; output successfulRequests/failedRequests @length {min 0, max 25}; ValidationException errorKind client, httpError 400, members message (@required) + x-amzn-ErrorType header. The 500 figure is not in the model — it is the recorded service behavior.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout.
2. Replay behavioral check connectcampaigns.get-campaign-state-batch-too-many-ids-rejected.
3. Cause 1: call GetCampaignStateBatch with 500 distinct, well-formed campaignIds that name no existing campaign. The emulator answers HTTP 400 ValidationException naming a maximum of 25; AWS answers HTTP 200 with successfulRequests == [] and 500 failedRequests entries (one per requested id, in order).
4. Cause 2: call GetCampaignStateBatch with 501 such ids. AWS refuses with HTTP 400 ValidationException whose Message is "1 validation error detected: Value '[<the 501 ids joined by ', '>]' at 'campaignIds' failed to satisfy constraint: Member must have length less than or equal to 500". Compare the emulator's message: it omits the "Value '[...]'" clause and names the wrong limit.
5. After the fix, step 3 must return 200 with 500 failedRequests and step 4 must return the exact AWS message template above; the check passes with no assertion failure.
