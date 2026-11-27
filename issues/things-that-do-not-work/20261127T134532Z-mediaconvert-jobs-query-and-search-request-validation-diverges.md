Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-job-and-query-id-refusals
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-job-and-query-id-refusals mediaconvert-jobs-query-filter-validation mediaconvert-jobs-query-request-validation mediaconvert-search-jobs-request-validation
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|CancelJob|4xx->4xx|$.errorType
Issue-Key: mediaconvert|CancelJob|4xx->4xx|$.httpStatus
Issue-Key: mediaconvert|CancelJob|4xx->4xx|$.message
Issue-Key: mediaconvert|CancelJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|CancelJob|4xx->4xx|status
Issue-Key: mediaconvert|StartJobsQuery|4xx->4xx|$.message
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$.accountId
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$.errorType
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$.httpStatus
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$.message
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$.operation
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$.requestId
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$.settingsValidationErrorsJsonBlob
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|StartJobsQuery|4xx->2xx|status
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.errorType
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.httpStatus
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.message
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|status
Issue-Key: mediaconvert|SearchJobs|4xx->4xx|$.message
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$.accountId
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$.errorType
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$.httpStatus
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$.message
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$.operation
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$.requestId
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$.settingsValidationErrorsJsonBlob
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|SearchJobs|4xx->2xx|status
Issue-Commit: 692629b6d1ae5ae78e012290c3136f29b80245ac

# MediaConvert jobs-query and search request validation diverges: wrong error class, wrong message text, and missing rejection

> _Note: this issue was identified against the code base at commit `692629b6d1ae5ae78e012290c3136f29b80245ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobsQuery

## Description
Six distinct request-validation / error-reporting causes in the emulator's MediaConvert jobs, jobsQueries and search surface. All six share one fault family: the emulator does not apply MediaConvert's request-parameter validation, so a malformed or unresolvable parameter is either reported with the wrong exception class, reported with wording MediaConvert never uses, or not reported at all. All six MUST be closed before this issue is submitted.

Cause 1 of 6: CancelJob with a malformed job id answers NotFoundException/404 instead of BadRequestException/400.
AWS: Smithy: CancelJob (DELETE /2017-08-29/jobs/{Id}), input member Id is an @httpLabel @required __string with no pattern in the model, errors include BadRequestException (httpError 400) and NotFoundException (httpError 404). The model does not state the id syntax, so the syntax rule is recorded behavior only: MediaConvert first checks the id against the job-id shape '\d{13}-\w{6}' and, when the supplied id cannot be a job id at all, rejects the REQUEST rather than reporting a missing resource — status 400, errorType 'BadRequestException', httpStatus 400, message "You specified an invalid value for id. The resource ID must match the format '\d{13}-\w{6}'." In that message the fixed text is "You specified an invalid value for ", then the member name taken from the request binding (id), then ". The resource ID must match the format '\d{13}-\w{6}'."; the only variable part is the member name, and the quoted format literal is fixed text. The x-amzn-errortype header carries the same exception name.
Emulator: answers 404 with errorType 'NotFoundException', httpStatus 404, message 'The resource you specified in id was not found.' and header x-amzn-errortype: NotFoundException — it skips the syntax check and goes straight to a state lookup.
Why: a client cannot distinguish a client-side malformed-identifier bug (400, not retryable, fix the call) from a job that has already aged out or never existed (404, often benign). SDK error handling keyed on BadRequestException/400 never fires, and retry/ignore logic for NotFoundException swallows a real programming error.
Fix: CancelJob's input validation, ahead of the job lookup in its handler — validate the {Id} label's syntax and raise BadRequestException before any state read.
Traces: mediaconvert-job-and-query-id-refusals#task#1.

Cause 2 of 6: the invalid-nextToken rejection uses the emulator's own wording instead of MediaConvert's.
AWS: Smithy: StartJobsQuery (POST /2017-08-29/jobsQueries), input member NextToken (jsonName nextToken) targets __string; errors include BadRequestException (httpError 400). The model does not state the opaque-token rule, so the wording is recorded behavior only: MediaConvert rejects a nextToken it did not issue with 400 BadRequestException and message 'You specified an invalid value for nextToken. Remove the nextToken field and resubmit your request.' The whole sentence is fixed text apart from the member name (nextToken) taken from the request; no token value, id or count appears in it.
Emulator: returns the right status (400) and the right exception class, but the message is 'nextToken is not a token this service issued.' — wording MediaConvert never emits.
Why: the message is part of the API contract: tests, log scrapers and user-facing error surfaces that match MediaConvert's sentence (and its instruction to remove the field) see an unrecognised string, so an emulator-based test suite passes against text that will not appear in production.
Fix: the shared pagination-token validation used by the jobsQueries/search handlers — the message template emitted with BadRequestException.
Traces: mediaconvert-job-and-query-id-refusals#task#4 (cluster representative), plus the same cluster's other members mediaconvert-jobs-query-filter-validation#task#3/#task#6/#task#7 and mediaconvert-jobs-query-request-validation#task#4/#task#5/#task#6/#task#7/#task#9/#task#10.

Cause 3 of 6: StartJobsQuery ACCEPTS a FilterList that repeats the same filter key, returning 201 instead of rejecting it.
AWS: Smithy: StartJobsQuery input member FilterList targets __listOfJobsQueryFilter; JobsQueryFilter has Key (enum JobsQueryFilterKey) and Values (list), and its documentation states "Provide one or more JobsQueryFilter objects, each containing a Key with an associated Values array" — one object per key. The rejection itself is recorded behavior only: a FilterList containing the same Key twice is refused with status 400, errorType 'BadRequestException', httpStatus 400, operation 'StartJobsQuery', and message 'You specified the same filter multiple times, which is not allowed. To resolve: Specify each filter only once and include all filter values in the associated Values array. Then resubmit your request.' That message is entirely fixed text; the surrounding body also carries accountId, requestId and an (empty) settingsValidationErrorsJsonBlob, as every MediaConvert error body does.
Emulator: accepts the request and answers 201 with a query id (the operation's modelled success code), emitting no errorType/httpStatus/message at all.
Why: the client gets a query id for a query MediaConvert would never have run, then polls GetJobsQueryResults and receives results computed under different filter semantics than production. The validation gap is silent, so the defect only surfaces as wrong result sets later.
Fix: StartJobsQuery's input validation in its handler — reject duplicate JobsQueryFilter.Key values before the query is created in state.
Traces: mediaconvert-jobs-query-filter-validation#task#5.

Cause 4 of 6: GetJobsQueryResults with an over-long id answers NotFoundException/404 instead of BadRequestException/400 reporting the length.
AWS: Smithy: GetJobsQueryResults (GET /2017-08-29/jobsQueries/{Id}), Id is an @httpLabel @required __string with no length constraint in the model; errors include BadRequestException (400) and NotFoundException (404). The length bound is recorded behavior only: MediaConvert caps the resource id at 80 characters and rejects a longer one as a bad REQUEST — status 400, errorType 'BadRequestException', httpStatus 400, message 'The resource ID in the value you entered for id is too long: length=<actual length>. The maximum length is <limit>.' Fixed text: 'The resource ID in the value you entered for ', ' is too long: length=', '. The maximum length is ', '.'; variable parts: the member name from the request binding (id), the supplied value's actual length, and the service's maximum (80). The recorded step shows the 81-character case rendering as length=81 / maximum 80 — that 81 is the request's value, not part of the template.
Emulator: answers 404 with errorType 'NotFoundException', httpStatus 404, message 'The resource you specified in id was not found.' and x-amzn-errortype: NotFoundException.
Why: same confusion as Cause 1, and worse for pagination loops: a client that treats 404 as "query expired" will restart the query instead of fixing the id it is constructing, and never learns a length limit exists.
Fix: GetJobsQueryResults' input validation in its handler — bound-check the {Id} label before the state lookup, and share the check with the other {Id}-bound jobsQueries/jobs operations.
Traces: mediaconvert-jobs-query-request-validation#task#12.

Cause 5 of 6: SearchJobs' maxResults range rejection uses the generic Smithy constraint sentence instead of MediaConvert's message.
AWS: Smithy: SearchJobs (GET /2017-08-29/search), input member MaxResults is @httpQuery("maxResults") targeting __integerMin1Max20 (@range min 1, max 20); errors include BadRequestException (400). MediaConvert reports an out-of-range value with 400 BadRequestException and message "You specified an invalid value for maxResults: '<value>'. Valid values are from <min> to <max>." Fixed text: 'You specified an invalid value for ', ": '", "'. Valid values are from ", ' to ', '.'; variable parts: the member name from the request (maxResults), the value the request supplied, and the modelled range bounds (1 and 20). The recorded '21' is the request's value filling that template, not part of it.
Emulator: status and exception class are right, but the message is "Value '21' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20" — the generic constraint-violation sentence of a different AWS error dialect, which MediaConvert does not use.
Why: callers and test suites that match MediaConvert's sentence (or parse the 'Valid values are from X to Y' bounds out of it) get an unparseable message; the emulator trains clients against wording production never returns.
Fix: SearchJobs' parameter validation / its validation-error serializer — replace the generic range-constraint renderer with MediaConvert's message template for this operation's query parameters.
Traces: mediaconvert-search-jobs-request-validation#task#3 (cluster representative), plus the same cluster's other members mediaconvert-search-jobs-request-validation#task#4/#task#5/#task#6/#task#7.

Cause 6 of 6: SearchJobs with a queue that does not exist answers 200 with an empty/OK result instead of NotFoundException/404.
AWS: Smithy: SearchJobs input member Queue is @httpQuery("queue") __string; errors include NotFoundException (httpError 404, doc "The resource you requested doesn't exist"). The resolution behavior is recorded behavior only: MediaConvert resolves the queue named in the request before searching and, when no such queue exists, fails the call — status 404, errorType 'NotFoundException', httpStatus 404, operation 'SearchJobs', message 'The resource you specified in queue was not found.' Fixed text: 'The resource you specified in ', ' was not found.'; the single variable part is the member name taken from the request (queue). The body also carries accountId, requestId and an empty settingsValidationErrorsJsonBlob.
Emulator: answers 200 with no error members at all — it filters jobs by the queue string without ever resolving it, so an unknown queue is indistinguishable from a queue holding no jobs.
Why: this converts a hard error into a plausible empty page. A client that lists or monitors jobs per queue sees 'no jobs' for a misspelled or deleted queue and silently reports a healthy, idle queue; dashboards and alarms built on that search never fire.
Fix: SearchJobs' handler — resolve the queue parameter against queue state and raise NotFoundException before filtering.
Traces: mediaconvert-search-jobs-request-validation#task#8.

Relation to open issues: the open MediaConvert issues cover different ground — 20261022T040904Z (queue/job control-plane fidelity: CreateQueue idempotency, CreateJob role, default SYSTEM queue), 20261024T051247Z (jobs terminate in ERROR), 20261121T060913Z (GetJob omits job warnings) and 20261126T031447Z (no EventBridge job-state-change event). None of them owns a jobsQueries/search request-validation step, and the two divergences already owned by open issues (GetJob warnings, s3 ListObjectsV2 output key/checksum) are excluded here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (14 fixtures, 111 steps, 90 passed, 21 mismatched, score 81.1%). The six causes, expected -> actual:

Cause 1 — CancelJob, mediaconvert-job-and-query-id-refusals#task#1: expected 400 / errorType 'BadRequestException' / httpStatus 400 / message "You specified an invalid value for id. The resource ID must match the format '\d{13}-\w{6}'."; actual 404 / errorType 'NotFoundException' / httpStatus 404 / message 'The resource you specified in id was not found.' / header x-amzn-errortype: NotFoundException (expected the BadRequestException name).

Cause 2 — StartJobsQuery, mediaconvert-job-and-query-id-refusals#task#4 (+9 affected steps): status matches at 400; $.message expected 'You specified an invalid value for nextToken. Remove the nextToken field and resubmit your request.', actual 'nextToken is not a token this service issued.'

Cause 3 — StartJobsQuery, mediaconvert-jobs-query-filter-validation#task#5: expected 400 with errorType 'BadRequestException', httpStatus 400, accountId, requestId, operation 'StartJobsQuery', settingsValidationErrorsJsonBlob '' and message 'You specified the same filter multiple times, which is not allowed. To resolve: Specify each filter only once and include all filter values in the associated Values array. Then resubmit your request.'; actual 201 with every one of those members missing (None).

Cause 4 — GetJobsQueryResults, mediaconvert-jobs-query-request-validation#task#12: expected 400 / 'BadRequestException' / httpStatus 400 / message 'The resource ID in the value you entered for id is too long: length=81. The maximum length is 80.'; actual 404 / 'NotFoundException' / httpStatus 404 / message 'The resource you specified in id was not found.' / x-amzn-errortype: NotFoundException.

Cause 5 — SearchJobs, mediaconvert-search-jobs-request-validation#task#3 (+4 affected steps): status matches at 400; $.message expected "You specified an invalid value for maxResults: '21'. Valid values are from 1 to 20.", actual "Value '21' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20".

Cause 6 — SearchJobs, mediaconvert-search-jobs-request-validation#task#8: expected 404 / 'NotFoundException' / httpStatus 404 / operation 'SearchJobs' / message 'The resource you specified in queue was not found.'; actual 200 with all error members missing (None).

Model grounding (services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json): CancelJob DELETE /2017-08-29/jobs/{Id} (success 202) with BadRequestException 400 and NotFoundException 404; GetJobsQueryResults GET /2017-08-29/jobsQueries/{Id}; StartJobsQuery POST /2017-08-29/jobsQueries (success 201), FilterList -> __listOfJobsQueryFilter, MaxResults -> __integerMin1Max20 @range 1..20; SearchJobs GET /2017-08-29/search with Queue/MaxResults as httpQuery and MaxResults -> __integerMin1Max20; JobsQueryFilter doc: "Provide one or more JobsQueryFilter objects, each containing a Key with an associated Values array".

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus), then per cause:

1. Cause 1: DELETE /2017-08-29/jobs/not-a-valid-id (an id that cannot match '\d{13}-\w{6}'). Observe 404 NotFoundException instead of 400 BadRequestException naming the required id format. Fixture mediaconvert-job-and-query-id-refusals, step task#1.
2. Cause 2: POST /2017-08-29/jobsQueries with a nextToken the service never issued. Observe 400 BadRequestException whose message is 'nextToken is not a token this service issued.' instead of MediaConvert's 'You specified an invalid value for nextToken. Remove the nextToken field and resubmit your request.' Fixture mediaconvert-job-and-query-id-refusals, step task#4.
3. Cause 3: POST /2017-08-29/jobsQueries with a filterList that lists the same key twice (e.g. two objects with key 'status'). Observe 201 plus a query id instead of 400 BadRequestException about specifying the same filter multiple times. Fixture mediaconvert-jobs-query-filter-validation, step task#5.
4. Cause 4: GET /2017-08-29/jobsQueries/<81-character id>. Observe 404 NotFoundException instead of 400 BadRequestException reporting length=81 against the 80-character maximum. Fixture mediaconvert-jobs-query-request-validation, step task#12.
5. Cause 5: GET /2017-08-29/search?maxResults=21. Observe the generic "Value '21' at 'maxResults' failed to satisfy constraint..." message instead of MediaConvert's "You specified an invalid value for maxResults: '21'. Valid values are from 1 to 20." Fixture mediaconvert-search-jobs-request-validation, step task#3.
6. Cause 6: GET /2017-08-29/search?queue=<name of a queue that does not exist>. Observe 200 with an empty job list instead of 404 NotFoundException 'The resource you specified in queue was not found.' Fixture mediaconvert-search-jobs-request-validation, step task#8.
