Issue-Source: trace-sweep-divergence
Issue-Trace: xray-put-trace-segments-empty-batch-rejected
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-put-trace-segments-empty-batch-rejected xray-put-telemetry-records-empty-and-length-limits-rejected xray-statistics-reads-window-and-group-mistakes xray-put-segment-is-found-by-trace-summaries-and-batch-get
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|PutTraceSegments|4xx->2xx|$.Message
Issue-Key: xray|PutTraceSegments|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|PutTraceSegments|4xx->2xx|status
Issue-Key: xray|PutTelemetryRecords|4xx->2xx|$
Issue-Key: xray|PutTelemetryRecords|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|PutTelemetryRecords|4xx->2xx|status
Issue-Key: xray|GetTimeSeriesServiceStatistics|2xx->4xx|$.ContainsOldGroupVersions
Issue-Key: xray|GetTimeSeriesServiceStatistics|2xx->4xx|$.TimeSeriesServiceStatistics
Issue-Key: xray|GetTimeSeriesServiceStatistics|2xx->4xx|status
Issue-Key: xray|GetTimeSeriesServiceStatistics|4xx->4xx|$.Message
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.ApproximateTime
Issue-Commit: c6826042d875b94d633b71de8e29643d34bc36fa

# xray trace ingest and statistics reads: empty-batch refusals missing, expression/window refusals wrong, ApproximateTime snapped to the hour grid

> _Note: this issue was identified against the code base at commit `c6826042d875b94d633b71de8e29643d34bc36fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTraceSegments

## Description
FIVE distinct net-new root causes in the xray service, all found by replaying the wire fixture corpus simparity-wire-aws-v1 (24 fixtures, 135 steps, 115 passed). xray is bootstrapped and answering, so these are fidelity defects, not a bootstrap gap. None of the five clusters is marked with a stub_path or an owned_by in the replay report.

RELATED, NOT COVERING: 20261127T143255Z-xray-6-net-new-behavioral-causes-puttracesegments-skips.md covers PER-DOCUMENT PutTraceSegments validation (MissingEndTime/InvalidId/entity-level ParseError wording/per-trace LimitExceeded) and GetTraceGraph — it does not cover the WHOLE-REQUEST empty-batch refusal in Cause 1. 20261022T142349Z-xray-x-ray-group-and-trace-segment-destination-responses.md owns the group-response clusters (InsightsConfiguration, Default group ARN) and 20261123T064210Z-xray-batchgettraces-re-serializes-the-stored-segment-document--r2.md owns the BatchGetTraces document round-trip; both were skipped here as owned. 20261025T043615Z-xray-3-net-new-behavioral-root-causes-getservicegraph.md covers GetTraceSummaries ResponseTimeRoot derivation, not the ApproximateTime label of Cause 5.

Cause 1 of 5: PutTraceSegments accepts an EMPTY TraceSegmentDocuments list with HTTP 200 instead of refusing the request.
AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models PutTraceSegments as POST /TraceSegments, success 200, input TraceSegmentDocuments (required, TraceSegmentDocumentList, no @length), errors InvalidRequestException (HTTP 400, "The request is missing required parameters or has invalid parameters") and ThrottledException (429). The model states no emptiness rule, so for the rule itself this is recorded behavior: a call whose TraceSegmentDocuments list is empty is refused with HTTP 400, the InvalidRequestException error shape (an x-amzn-errortype header is present) and body member Message = "Segments can not be empty" — entirely FIXED text, interpolating nothing from the request or from state.
Emulator: the call answers HTTP 200 with an empty body; no Message and no x-amzn-errortype are produced. The handler bounds the batch only from ABOVE (crate::state::MAX_SEGMENT_DOCUMENTS_PER_CALL) and has no lower bound, so a zero-document batch walks the (empty) parse loop and returns an empty UnprocessedTraceSegments.
Why: a client whose buffer flushed nothing — the common instrumentation bug of sending an empty batch — is told the submission succeeded. Against AWS that call is a 400 the SDK raises as InvalidRequestException, so error handling and retry/alarm paths that exist only on that refusal are dead code against the emulator, and an agent health check that asserts the refusal cannot pass.
Fix: the request-level validation in the PutTraceSegments handler (services/service-com-amazonaws-xray-awsxray/src/ops/put_trace_segments.rs, beside the existing MAX_SEGMENT_DOCUMENTS_PER_CALL upper-bound check) — refuse an empty list with InvalidRequestException and that message before any parsing or ingest.
Traces: xray-put-trace-segments-empty-batch-rejected#task#2.

Cause 2 of 5: PutTelemetryRecords accepts an EMPTY TelemetryRecords list with HTTP 200 instead of refusing the request.
AWS: Smithy models PutTelemetryRecords as POST /TelemetryRecords, success 200, input TelemetryRecords (required, TelemetryRecordList, no @length) plus EC2InstanceId/Hostname/ResourceARN, errors InvalidRequestException (400) and ThrottledException (429); output is empty. The emptiness rule is recorded behavior only: a call whose TelemetryRecords list is empty is refused with HTTP 400, the InvalidRequestException error shape (x-amzn-errortype present) and body member Message = "Telemetry records cannot be empty" — entirely fixed text, nothing interpolated. The same fixture's other steps (the per-call length limits) PASS, so only the empty case is missing.
Emulator: HTTP 200 with an empty body — no Message, no x-amzn-errortype. The handler checks only the upper bound (crate::state::MAX_TELEMETRY_RECORDS_PER_CALL) and the per-record negative counters, so an empty list is recorded as a source with no records.
Why: the X-Ray daemon reports its own health through this call; a daemon (or a test of one) that submits nothing is told it succeeded, and the client-side classification that depends on the 400 never runs. It also writes a telemetry source into the store that AWS would never have created.
Fix: the request-level validation in the PutTelemetryRecords handler (services/service-com-amazonaws-xray-awsxray/src/ops/put_telemetry_records.rs, beside the MAX_TELEMETRY_RECORDS_PER_CALL check), refusing an empty list with InvalidRequestException and that message before record_telemetry is called.
Traces: xray-put-telemetry-records-empty-and-length-limits-rejected#task#2.

Cause 3 of 5: GetTimeSeriesServiceStatistics REFUSES a well-formed request whose expression uses the service() function with a quoted service name, instead of answering with an empty aggregation.
AWS: Smithy models GetTimeSeriesServiceStatistics as POST /TimeSeriesServiceStatistics, success 200, input StartTime/EndTime (required) + GroupName (len 1..32) / GroupARN (len 1..400) / EntitySelectorExpression (len 1..500, documented "A filter expression defining entities that will be aggregated for statistics. Supports ID, service, and edge functions") / Period / ForecastStatistics, output TimeSeriesServiceStatistics + ContainsOldGroupVersions (Boolean, default false, required), errors InvalidRequestException (400) and ThrottledException (429). Recorded behavior: a request over a forward window scoped by an X-Ray expression of the form service("<service-name>") is ANSWERED with HTTP 200, TimeSeriesServiceStatistics = [] and ContainsOldGroupVersions = false when no stored traffic matches — selecting nothing is an empty aggregation, not a refusal.
Emulator: the call answers HTTP 400; neither ContainsOldGroupVersions nor TimeSeriesServiceStatistics is present in the body. The refusal is the crate's expression-parse failure, worded as in Cause 4 and rejecting the expression at the opening double quote of the service name — the quoted-string argument of the service() function is not accepted by the one parser this crate reads the language with (crate::state::filter_expression::parse_entity_selector / crate::state::filter), even though the same quoted form is accepted elsewhere (a group's FilterExpression service("nonexistent-service") is stored and echoed verbatim by the group operations).
Why: every service-scoped statistics query — the shape a dashboard or a per-service latency panel sends — fails with a 400 instead of returning numbers (or an honest empty series), so a client cannot distinguish "no traffic in this window" from "your expression is malformed", and a well-formed query it took straight from the AWS docs is rejected.
Fix: the expression parser this read uses and the handler's use of it (crate::state::filter_expression::parse_entity_selector and crate::state::filter, reached from services/service-com-amazonaws-xray-awsxray/src/ops/get_time_series_service_statistics.rs) — accept the service()/edge()/id() functions with a quoted string argument, and let an expression that names nothing recorded aggregate to an empty list with ContainsOldGroupVersions false rather than a refusal.
Traces: xray-statistics-reads-window-and-group-mistakes#task#1.

Cause 4 of 5: GetTimeSeriesServiceStatistics refuses an INVERTED window with the expression-parse message instead of the window message, because the expression is read before the window is checked.
AWS: the operation models StartTime and EndTime as required timestamps and InvalidRequestException (400) for "missing required parameters or has invalid parameters"; the message text is recorded behavior: a request whose EndTime precedes its StartTime is refused with HTTP 400 and Message = "End time cannot be earlier than start time" — entirely fixed text interpolating no bound from the request. The window mistake takes precedence over anything the expression says.
Emulator: HTTP 400 is right but the Message is the parse failure of the request's expression, 'Error Location: line 1, position 9 : Invalid input symbol: \'"s\'' — different fixed text ("Error Location: line ", ", position ", " : Invalid input symbol: ") plus per-request values (the line, the character offset and the offending source fragment, here the opening quote of the quoted service name). The handler parses EntitySelectorExpression (and compiles any group filter) BEFORE it calls crate::state::validate_window, so the window check in services/service-com-amazonaws-xray-awsxray/src/ops/get_time_series_service_statistics.rs is never reached for such a request.
Why: the error message is a requirement. A caller that mixed up its window bounds is told its expression is unparseable, so the actual mistake is undiagnosable, and any client or test matching AWS's window wording sees an unknown refusal. Ordering matters independently of Cause 3: AWS validates the window first whatever the expression says.
Fix: the GetTimeSeriesServiceStatistics handler's validation order — run the crate's one window check (crate::state::validate_window, whose inverted-window sentence is already the shared wording) before parsing EntitySelectorExpression or compiling a group's filter.
Traces: xray-statistics-reads-window-and-group-mistakes#task#2.

Cause 5 of 5: GetTraceSummaries labels the page with the start of the absolute HOUR bucket instead of the start time of the page of results.
AWS: Smithy models GetTraceSummaries output ApproximateTime (Timestamp), documented as "The start time of this page of results." Recorded behavior: for a minutes-wide window served in a single page, ApproximateTime is a time INSIDE the requested window — the start of the results this page covers, i.e. a value derived from the request's StartTime, not aligned to any clock grid and never later than the window.
Emulator: ApproximateTime is the start of the absolute hour bucket the served step closes in, clipped forward to the start of that step (crate::state::WindowPage::approximate_time / crate::state::span_approximate_time), so the label is an hour boundary the request never named; in the replayed step it came back as an hour-aligned instant later than the window start the recording reports, and for a window ending inside the current hour that boundary can even be ahead of the moment the response was built.
Why: this is a request-derived label, not a clock reading, so a client that uses ApproximateTime to advance its own paging cursor or to stamp the page it just read walks onto an hour grid the query never used: it re-queries or skips whole minutes of traces, and a label later than the window it asked for makes the page look like it covers time the client has not yet requested.
Fix: the ApproximateTime derivation for this read — crate::state::WindowPage::approximate_time and crate::state::span_approximate_time, as used by services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs — label a page by the start of the results it actually served (the step/window start the request named) rather than by an absolute hour bucket.
Traces: xray-put-segment-is-found-by-trace-summaries-and-batch-get#task#2.

## Evidence
Replay summary (trace_set_id simparity-wire-aws-v1): emulator_up true, bootstrap_signal false, steps_total 135, steps_passed 115, steps_mismatched 20, score_pct 85.2, divergence_count 11 — all eleven on service "xray", of which six are owned by existing issues and are not filed here.

Cause 1 — fixture xray-put-trace-segments-empty-batch-rejected, step task#2, operation PutTraceSegments, verdict mismatch, expected_status 400, actual_status 200:
  status: expected 400, got 200
  $.Message: missing (expected 'Segments can not be empty', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
The step submits PutTraceSegments with TraceSegmentDocuments = []. Message is entirely fixed text; nothing in it comes from the request.

Cause 2 — fixture xray-put-telemetry-records-empty-and-length-limits-rejected, step task#2, operation PutTelemetryRecords, verdict mismatch, expected_status 400, actual_status 200:
  status: expected 400, got 200
  $: empty_body (expected {'Message': 'Telemetry records cannot be empty'}, got {})
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
The step submits PutTelemetryRecords with TelemetryRecords = []. The remaining steps of this fixture (the per-call length limits) pass, so only the empty-list refusal is absent.

Cause 3 — fixture xray-statistics-reads-window-and-group-mistakes, step task#1, operation GetTimeSeriesServiceStatistics, verdict mismatch, expected_status 200, actual_status 400:
  status: expected 200, got 400
  $.ContainsOldGroupVersions: missing (expected False, got None)
  $.TimeSeriesServiceStatistics: missing (expected [], got None)
AWS answered the well-formed, service-scoped query with an empty aggregation; the emulator refused it with 400.

Cause 4 — same fixture, step task#2, operation GetTimeSeriesServiceStatistics, verdict mismatch, expected_status 400, actual_status 400:
  $.Message: value_mismatch (expected 'End time cannot be earlier than start time', got 'Error Location: line 1, position 9 : Invalid input symbol: '\"s'')
Status and error class match; only the message diverges. The expected message is 100% fixed text. In the observed message, 'Error Location: line ', ', position ' and ' : Invalid input symbol: ' are the emulator's fixed text and '1', '9' and '\"s' are per-request values (line, offset, and the source fragment at that offset — the opening quote of the expression's quoted service name).

Cause 5 — fixture xray-put-segment-is-found-by-trace-summaries-and-batch-get, step task#2, operation GetTraceSummaries, verdict mismatch, expected_status 200, actual_status 200:
  $.ApproximateTime: value_mismatch (expected 1795808863, got 1795809600)
The recorded value lies inside the requested window and is not aligned to any grid; the observed value is exactly hour-aligned (1795809600 = 498836 x 3600), which is the absolute-hour-bucket label the handler's own documentation describes. The literal epoch seconds are per-run values filling the slot; the requirement is that the label be the start of the page of results the request's window named, never an hour boundary and never later than that window's start.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (it builds and boots the emulator from the checkout); confirm emulator_up true and bootstrap_signal false.
2. Cause 1: POST /TraceSegments (xray.put_trace_segments) with TraceSegmentDocuments=[] — observe HTTP 200 with an empty body instead of HTTP 400, x-amzn-errortype naming InvalidRequestException and Message 'Segments can not be empty'.
3. Cause 2: POST /TelemetryRecords (xray.put_telemetry_records) with TelemetryRecords=[] (and any/no EC2InstanceId/Hostname/ResourceARN) — observe HTTP 200 with an empty body instead of HTTP 400 with Message 'Telemetry records cannot be empty'.
4. Cause 3: POST /TimeSeriesServiceStatistics (xray.get_time_series_service_statistics) with StartTime=now-15m, EndTime=now and an expression scoping the aggregation to one service by name, of the form service("<some-service-name>") (EntitySelectorExpression, or a GroupName whose group carries that FilterExpression) — observe HTTP 400 carrying the 'Error Location: line 1, position 9 : Invalid input symbol: ...' parse failure instead of HTTP 200 with TimeSeriesServiceStatistics=[] and ContainsOldGroupVersions=false.
5. Cause 4: repeat step 4 with the window inverted (EndTime earlier than StartTime) — observe the same expression-parse Message instead of 'End time cannot be earlier than start time'; i.e. the expression is read before the window order is checked.
6. Cause 5: xray.put_trace_segments one complete segment, then xray.get_trace_summaries(StartTime=<segment start - a few minutes>, EndTime=<now>) — observe ApproximateTime coming back as an exact hour boundary (a multiple of 3600, possibly later than StartTime and ahead of the response's own build time) instead of the start time of the page of results the window named.
7. Re-replay the five fixture#step bindings in Issue-Trace / Issue-Trace-Ids to confirm each cause is closed: xray-put-trace-segments-empty-batch-rejected#task#2, xray-put-telemetry-records-empty-and-length-limits-rejected#task#2, xray-statistics-reads-window-and-group-mistakes#task#1, xray-statistics-reads-window-and-group-mistakes#task#2, xray-put-segment-is-found-by-trace-summaries-and-batch-get#task#2.
