Issue-Source: trace-sweep-divergence
Issue-Trace: connect-contact-media-processing-start-refusals
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connect-contact-media-processing-start-refusals connect-start-contact-media-processing-failure-mode-first connect-current-metric-data-request-and-filter-refusals connect-get-current-metric-data-invalid-parameter-refusals connect-historical-metric-data-request-validation get-metric-data-historical-request-refusals connect-stop-contact-media-processing-refusals connect-test-case-execution-unknown-instance-and-parameter-refusals connect-test-case-execution-unknown-instance-refusals get-metric-data-v2-request-refusals
Issue-Shape-Id: com.amazonaws.connect#AmazonConnectService
Issue-Key: connect|StartContactMediaProcessing|4xx->4xx|$.message
Issue-Key: connect|StartContactMediaProcessing|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|GetCurrentMetricData|4xx->4xx|$.Message
Issue-Key: connect|GetCurrentMetricData|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|GetCurrentMetricData|4xx->4xx|$.message
Issue-Key: connect|GetMetricData|4xx->4xx|$.Message
Issue-Key: connect|GetMetricData|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|StopContactMediaProcessing|4xx->4xx|$.Message
Issue-Key: connect|StopContactMediaProcessing|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|ListTestCaseExecutions|4xx->4xx|$.message
Issue-Key: connect|ListTestCaseExecutions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|GetMetricData|4xx->4xx|$.message
Issue-Key: connect|GetMetricDataV2|4xx->4xx|$.Message
Issue-Key: connect|GetMetricDataV2|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 447941bda47a9e01bc13409633bd0ec832671375

# connect request-validation refusals answer an unmodeled ValidationException envelope instead of AWS's service-authored message

> _Note: this issue was identified against the code base at commit `447941bda47a9e01bc13409633bd0ec832671375`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartContactMediaProcessing

## Description
8 net-new causes, all one root fault: Amazon Connect refuses a malformed request with its OWN error shape and its OWN fixed message text, while the emulator answers a generic Smithy-framework constraint report. Every refusal in this bundle comes back with `x-amzn-errortype: ValidationException` and a body message of the form "1 validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: <rule>".

`ValidationException` is NOT a member of the Amazon Connect API at all. Smithy: none of GetCurrentMetricData, GetMetricData, GetMetricDataV2, StartContactMediaProcessing, StopContactMediaProcessing or ListTestCaseExecutions lists a ValidationException; each lists only InternalServiceException (500), InvalidParameterException (400), InvalidRequestException (400), ResourceNotFoundException (404) and ThrottlingException/LimitExceededException. So a client's `except connect.exceptions.InvalidRequestException` / `InvalidParameterException` handler never fires, and the SDK surfaces an unmodeled error it cannot type. That consequence is shared by all eight causes and is the thing to fix once: the request-validation path must raise the modelled Connect error shape with AWS's message, not the framework's constraint envelope.

The message texts below are AWS's FIXED sentences for their error class — they contain no per-run values at all except where noted, so they are reproducible verbatim. Related prior art in other services: '20261111T170613Z-dynamodb-non-constraint-validationexception-messages-are-wrapped-in.md' and '20261114T060602Z-cloudformation-cloudwatch-route53-route53-createhealthcheck-an-off-enum-insufficientdatahealthstatus-is-refused.md' report the same '1 validation error detected' envelope leaking in dynamodb and route53; neither covers connect.

Cause 1 of 8 — StartContactMediaProcessing, off-enum FailureMode (fixture connect-contact-media-processing-start-refusals, step task#3).
AWS: Smithy: StartContactMediaProcessing.FailureMode targets enum ContactMediaProcessingFailureMode (values DELIVER_UNPROCESSED_MESSAGE, DO_NOT_DELIVER_UNPROCESSED_MESSAGE); modelled errors include InvalidRequestException (400). Recorded: HTTP 400 with the fixed message `Invalid request body` and no framework envelope. Fixed text: the whole sentence; variable parts: none.
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException` and message "1 validation error detected: Value 'NOT_A_MODE' at 'failureMode' failed to satisfy constraint: Member must satisfy enum value set: [DELIVER_UNPROCESSED_MESSAGE, DO_NOT_DELIVER_UNPROCESSED_MESSAGE]".
Why: a client that keys off the modelled exception shape, or that matches on AWS's body-rejection text, sees an untyped error; the emulator also discloses the member path and rejected value where AWS discloses neither.
Fix: the operation's request validation / error serializer — emit the modelled Connect error with AWS's sentence.
Traces: connect-contact-media-processing-start-refusals#task#3 (also connect-start-contact-media-processing-failure-mode-first#task#2).

Cause 2 of 8 — GetCurrentMetricData, more than 2 Groupings (fixture connect-current-metric-data-request-and-filter-refusals, step task#3).
AWS: Smithy: GetCurrentMetricData.Groupings targets list Groupings with @length min 0 max 2. Recorded: HTTP 400, member `Message` = `You can specify a maximum of 2 groupings.` Fixed text: the whole sentence (the limit 2 is the modelled bound, not a per-run value).
Emulator: HTTP 400, `x-amzn-errortype: ValidationException`, message "1 validation error detected: Value '3 items' at 'groupings' failed to satisfy constraint: Member must have length less than or equal to 2".
Why: the over-grouping rule is one AWS documents with its own sentence; a client matching it, or typing the exception, does not recognise the emulator's refusal.
Fix: GetCurrentMetricData's validation.
Traces: connect-current-metric-data-request-and-filter-refusals#task#3.

Cause 3 of 8 — GetCurrentMetricData, off-enum Filters.Channels value (fixture connect-get-current-metric-data-invalid-parameter-refusals, step task#3).
AWS: Smithy: Channel is an enum with values VOICE, CHAT, TASK, EMAIL; GetCurrentMetricData lists InvalidRequestException/InvalidParameterException (400). Recorded: HTTP 400 with the fixed message `Invalid request body`. Fixed text: the whole sentence; variable parts: none.
Emulator: HTTP 400, `x-amzn-errortype: ValidationException`, message "1 validation error detected: Value 'FAX' at 'filters.channels.1.member' failed to satisfy constraint: Member must satisfy enum value set: [VOICE, CHAT, TASK, EMAIL]".
Why: as cause 1 — unmodeled error type, and a member path AWS does not expose.
Fix: GetCurrentMetricData's validation.
Traces: connect-get-current-metric-data-invalid-parameter-refusals#task#3.

Cause 4 of 8 — GetMetricData, more than 2 Groupings (fixture connect-historical-metric-data-request-validation, step task#7).
AWS: Smithy: GetMetricData.Groupings targets list Groupings with @length min 0 max 2. Recorded: HTTP 400, `Message` = `You can specify a maximum of 2 groupings.`
Emulator: HTTP 400, `x-amzn-errortype: ValidationException`, message "1 validation error detected: Value '3 items' at 'groupings' failed to satisfy constraint: Member must have length less than or equal to 2".
Why: as cause 2; the historical and current metric endpoints must share AWS's one sentence for this rule.
Fix: GetMetricData's validation.
Traces: connect-historical-metric-data-request-validation#task#7 (also get-metric-data-historical-request-refusals#task#7).

Cause 5 of 8 — StopContactMediaProcessing, over-long InstanceId (fixture connect-stop-contact-media-processing-refusals, step task#6).
AWS: Smithy: StopContactMediaProcessing.InstanceId targets InstanceId with @length min 1 max 100; modelled errors include InvalidRequestException (400). Recorded: HTTP 400 with the fixed message `Invalid request body`. Fixed text: the whole sentence; variable parts: none — in particular AWS does not echo the rejected id.
Emulator: HTTP 400, `x-amzn-errortype: ValidationException`, message "1 validation error detected: Value '<the 101-char instanceId, echoed in full>' at 'instanceId' failed to satisfy constraint: Member must have length less than or equal to 100".
Why: unmodeled error type, and the whole rejected identifier is reflected back into the error body where AWS returns a fixed sentence.
Fix: StopContactMediaProcessing's validation / error serializer.
Traces: connect-stop-contact-media-processing-refusals#task#6.

Cause 6 of 8 — ListTestCaseExecutions, MaxResults below the modelled minimum (fixture connect-test-case-execution-unknown-instance-and-parameter-refusals, step task#1).
AWS: Smithy: ListTestCaseExecutions.MaxResults targets MaxResult100 with @range min 1 max 100, bound @httpQuery "maxResults". Recorded: HTTP 400 with the service-authored fixed message `MaxResults must be at least 1`. Fixed text: the whole sentence (the bound 1 is the modelled minimum).
Emulator: HTTP 400, `x-amzn-errortype: ValidationException`, message "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
Why: this operation has its own sentence for the bound; a client matching it is not satisfied by the framework envelope, and the error stays untypeable.
Fix: ListTestCaseExecutions' validation.
Traces: connect-test-case-execution-unknown-instance-and-parameter-refusals#task#1 (also ...#task#3 and connect-test-case-execution-unknown-instance-refusals#task#3).

Cause 7 of 8 — GetMetricData, required HistoricalMetrics absent (fixture get-metric-data-historical-request-refusals, step task#2).
AWS: Smithy: GetMetricData.HistoricalMetrics is @required (list HistoricalMetrics). Recorded: HTTP 400 with the fixed message `Invalid request body`. Fixed text: the whole sentence; variable parts: none.
Emulator: HTTP 400, `x-amzn-errortype: ValidationException`, message "1 validation error detected: Value null at 'historicalMetrics' failed to satisfy constraint: Member must not be null".
Why: a missing required member is a body rejection in Connect, not a per-member constraint report; the client gets an unmodeled error shape either way.
Fix: GetMetricData's validation.
Traces: get-metric-data-historical-request-refusals#task#2.

Cause 8 of 8 — GetMetricDataV2, duplicate filter key (fixture get-metric-data-v2-request-refusals, step task#5).
AWS: Smithy: GetMetricDataV2.Filters targets FiltersV2List with @length min 1 max 5, member FilterV2 whose FilterKey doc states "There must be at least 1 key and a maximum 5 keys". Recorded: HTTP 400 with `Message` = `Invalid request with duplicate filter: QUEUE` — the duplicate-key rule is checked and reported in preference to the list bound. Fixed text: "Invalid request with duplicate filter: "; variable part: the duplicated FilterKey taken from the request (QUEUE in this recording).
Emulator: HTTP 400, `x-amzn-errortype: ValidationException`, message "1 validation error detected: Value '6 items' at 'filters' failed to satisfy constraint: Member must have length less than or equal to 5" — it never implements the duplicate-key rule at all and reports only the length bound.
Why: a client sending a repeated FilterKey is told its list is too long, so it trims the list instead of de-duplicating, and still gets refused; and the refusal is again untypeable.
Fix: GetMetricDataV2's validation — add the duplicate-FilterKey check ahead of the list-length bound and emit the modelled Connect error with the template above.
Traces: get-metric-data-v2-request-refusals#task#5 (also ...#task#7 and ...#task#8).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, /__admin/health 200). All eight clusters are verdict=mismatch, service=connect, stub_path empty, owned_by empty.

Shared header divergence on seven of the eight: `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')` — ValidationException is absent from com.amazonaws.connect#AmazonConnectService; the modelled 400s are InvalidParameterException and InvalidRequestException.

Per cause, expected vs actual message:
1. StartContactMediaProcessing — expected $.message 'Invalid request body'; got "1 validation error detected: Value 'NOT_A_MODE' at 'failureMode' failed to satisfy constraint: Member must satisfy enum value set: [DELIVER_UNPROCESSED_MESSAGE, DO_NOT_DELIVER_UNPROCESSED_MESSAGE]". Status 400 both sides.
2. GetCurrentMetricData — expected $.Message 'You can specify a maximum of 2 groupings.'; got "1 validation error detected: Value '3 items' at 'groupings' failed to satisfy constraint: Member must have length less than or equal to 2".
3. GetCurrentMetricData — expected $.message 'Invalid request body'; got "1 validation error detected: Value 'FAX' at 'filters.channels.1.member' failed to satisfy constraint: Member must satisfy enum value set: [VOICE, CHAT, TASK, EMAIL]".
4. GetMetricData — expected $.Message 'You can specify a maximum of 2 groupings.'; got the same '3 items'/'groupings' envelope as cause 2.
5. StopContactMediaProcessing — expected $.Message 'Invalid request body'; got "1 validation error detected: Value 'zzzz...z' (101 chars echoed) at 'instanceId' failed to satisfy constraint: Member must have length less than or equal to 100".
6. ListTestCaseExecutions — expected $.message 'MaxResults must be at least 1'; got "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
7. GetMetricData — expected $.message 'Invalid request body'; got "1 validation error detected: Value null at 'historicalMetrics' failed to satisfy constraint: Member must not be null".
8. GetMetricDataV2 — expected $.Message 'Invalid request with duplicate filter: QUEUE'; got "1 validation error detected: Value '6 items' at 'filters' failed to satisfy constraint: Member must have length less than or equal to 5".

Model citations: smithy_query operation=GetCurrentMetricData,GetMetricData,GetMetricDataV2,StartContactMediaProcessing,StopContactMediaProcessing,ListTestCaseExecutions and shape=InvalidParameterException,InvalidRequestException,Channel,ContactMediaProcessingFailureMode,Groupings,FiltersV2List,FilterV2 on services/service-com-amazonaws-connect-amazonconnectservice/smithy-model/com-amazonaws-connect-amazonconnectservice.json.

## Steps to Reproduce
1. Boot the connect emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Representative: replay fixture connect-contact-media-processing-start-refusals step task#3 — POST /contact/start-contact-media-processing with FailureMode='NOT_A_MODE'. Observe HTTP 400 with x-amzn-errortype: ValidationException and a '1 validation error detected: ...' message instead of the modelled Connect 400 error carrying 'Invalid request body'.
3. Replay each of the other bundled causes and compare status, x-amzn-errortype and the message member: connect-current-metric-data-request-and-filter-refusals#task#3 (3 Groupings), connect-get-current-metric-data-invalid-parameter-refusals#task#3 (Channels value 'FAX'), connect-historical-metric-data-request-validation#task#7 (3 Groupings), connect-stop-contact-media-processing-refusals#task#6 (101-char InstanceId), connect-test-case-execution-unknown-instance-and-parameter-refusals#task#1 (maxResults=0), get-metric-data-historical-request-refusals#task#2 (HistoricalMetrics omitted), get-metric-data-v2-request-refusals#task#5 (Filters repeating FilterKey QUEUE, 6 entries).
4. All eight must answer a modelled Connect error shape (InvalidRequestException / InvalidParameterException) with AWS's sentence for that error class; none may carry x-amzn-errortype: ValidationException or the '1 validation error detected' envelope.
