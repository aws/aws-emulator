Issue-Source: trace-sweep-divergence
Issue-Trace: backupsearch-export-jobs-filter-identifier-forms
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backupsearch-export-jobs-filter-identifier-forms
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch|ListSearchResultExportJobs|4xx->2xx|$.message
Issue-Key: backupsearch|ListSearchResultExportJobs|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backupsearch|ListSearchResultExportJobs|4xx->2xx|status
Issue-Key: backupsearch|ListSearchResultExportJobs|4xx->4xx|$.message
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# BackupSearch ListSearchResultExportJobs accepts invalid filter inputs: malformed SearchJobIdentifier not rejected and NextToken refusal text diverges

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSearchResultExportJobs

## Description
Two distinct root causes in how the BackupSearch (sdkId BackupSearch, restJson1) emulator validates the query-string filters of ListSearchResultExportJobs, found by replaying the recorded wire corpus (simparity-wire-aws-v1). Both must be closed before this issue is done; each names its own fixture#step below. The open issue 20261022T215802Z-backupsearch-refusal-fidelity-on-the-wire-not-found.md covers other BackupSearch refusal paths (GetSearchJob not-found classification, the shared not-found text, the StartSearchJob enum-in-list message, the StartSearchResultExportJob @pattern anchors, TagResource's success status) and carries no replay key for either cause below; neither of these two causes is in it.

=== Cause 1 of 2: a malformed SearchJobIdentifier filter is accepted and answered 200 instead of refused 400 ValidationException ===
AWS: Smithy ListSearchResultExportJobs, GET /export-search-jobs, input member SearchJobIdentifier -> GenericId bound @httpQuery("SearchJobIdentifier"); modeled errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). GenericId carries no @pattern in the model, so the syntax rule is recorded behavior only: the recording shows that when the SearchJobIdentifier query filter is not a well-formed search-job identifier, the service refuses the call with HTTP 400 and x-amzn-errortype ValidationException before it lists anything, and the recorded message for this error class is the fixed text "Invalid Job Id" (no variable part - the message does not echo the rejected identifier). Two different malformed identifier forms in the same fixture are refused the same way. The requirement is the refusal: a syntactically invalid identifier filter is a request-level validation failure, not an empty result set.
Emulator: answers HTTP 200 with a successful ListSearchResultExportJobs body (an export-jobs list) and no error at all - no $.message, no x-amzn-errortype header. The malformed filter is silently treated as a filter that simply matches nothing.
Why: a client cannot distinguish "your identifier is wrong" from "this search job has no export jobs". Callers that pass a user-supplied or mistyped id, or that probe an id's validity through a list call, get an authoritative-looking empty page and conclude the search job has no export jobs; against AWS the same call fails fast and terminally with 400 ValidationException. Tooling that branches on the error type (retry, surface to the user, abort a workflow) takes the wrong branch, and the bug is invisible in logs because the status is a success.
Fix: the ListSearchResultExportJobs handler's input validation - validate the SearchJobIdentifier query filter's syntax and raise the modeled ValidationException (HTTP 400, x-amzn-errortype ValidationException, message "Invalid Job Id") before the export-job store is queried.
Traces: backupsearch-export-jobs-filter-identifier-forms#task#2 (representative; cluster also covers backupsearch-export-jobs-filter-identifier-forms#task#3, the second malformed identifier form).

=== Cause 2 of 2: the rejection of an unrecognized NextToken carries an emulator-authored message instead of the service's fixed malformed-token text ===
AWS: Smithy ListSearchResultExportJobs is @paginated (inputToken/outputToken NextToken, pageSize MaxResults, items ExportJobs); NextToken is a plain String @httpQuery member with no modeled constraint, and ValidationException (httpError 400) is a declared error. The message rule is therefore recorded behavior only: a NextToken the service did not issue is refused with HTTP 400 and the fixed text "Malformed Next Token. Please provide a valid Next Token". That whole string is fixed text - it has no variable part and in particular does not include the rejected token value.
Emulator: returns the correct status (400) but its own sentence, built from the request: "NextToken '<the token the client sent>' is not a pagination token this service issued" (the recorded run shows the probe's token substituted into it).
Why: ValidationException has no structured detail beyond its @required message, so the message is the entire machine- and human-readable content of the refusal. Pagination loops and test suites that match the service's fixed malformed-token text never match, and the emulator additionally reflects the client's token back into the response, which AWS does not do - so the string differs on every call and cannot be matched at all.
Fix: the ListSearchResultExportJobs pagination-token validation / error construction - emit the service's fixed malformed-next-token text for an unrecognized or undecodable NextToken instead of a per-request sentence.
Traces: backupsearch-export-jobs-filter-identifier-forms#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false; 36 steps total, 29 passed, 7 mismatched, 0 blocked/errored).

Cause 1 - fixture backupsearch-export-jobs-filter-identifier-forms, step task#2 (cluster member: task#3), operation ListSearchResultExportJobs, divergence key backupsearch|ListSearchResultExportJobs|status=400->200|keys=$.message,$headers.x-amzn-errortype,status:
  "status: expected 400, got 200"
  "$.message: missing (expected 'Invalid Job Id', got None)"
  "$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)"
Expected: HTTP 400, x-amzn-errortype ValidationException, body message "Invalid Job Id" (fixed text, no request-derived part). Actual: HTTP 200 with a normal list response and no error members.

Cause 2 - fixture backupsearch-export-jobs-filter-identifier-forms, step task#4, operation ListSearchResultExportJobs, divergence key backupsearch|ListSearchResultExportJobs|status=400->400|keys=$.message:
  "$.message: value_mismatch (expected 'Malformed Next Token. Please provide a valid Next Token', got \"NextToken 'sim2real-probe-bks-filter-215cf415' is not a pagination token this service issued\")"
Status matches (400/400); only the message diverges. The expected string is fixed text in full; the actual string embeds the token value from the request (the literal token shown is this run's probe value and is not part of any requirement).

## Steps to Reproduce
Boot the emulator and point a botocore backupsearch client (restJson1) at it.
1. (Cause 1, fixture backupsearch-export-jobs-filter-identifier-forms, steps task#2 and task#3) Call list_search_result_export_jobs(SearchJobIdentifier="<a malformed search-job identifier, e.g. not-a-valid-uuid>") - i.e. GET /export-search-jobs?SearchJobIdentifier=<malformed>. The emulator answers HTTP 200 with an ExportJobs list; AWS answers HTTP 400 with x-amzn-errortype ValidationException and message "Invalid Job Id". Repeat with the second malformed identifier form in the fixture (task#3) for the same result.
2. (Cause 2, fixture backupsearch-export-jobs-filter-identifier-forms, step task#4) Call list_search_result_export_jobs(NextToken="<any token this service never issued>") - i.e. GET /export-search-jobs?NextToken=<bogus>. Both return HTTP 400, but compare the body's message: AWS returns the fixed "Malformed Next Token. Please provide a valid Next Token", the emulator returns "NextToken '<the token you sent>' is not a pagination token this service issued".
