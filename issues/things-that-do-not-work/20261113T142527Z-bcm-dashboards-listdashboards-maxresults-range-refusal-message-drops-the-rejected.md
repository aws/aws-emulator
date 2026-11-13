Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.list-dashboards-max-results-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards.list-dashboards-max-results-out-of-range-rejected|run#4|AssertionError|assert message == (
Issue-Commit: 8519584554abb240da771f66a4fe7c47c54f612d

# ListDashboards maxResults @range refusal message drops the rejected-value clause AWS includes

> _Note: this issue was identified against the code base at commit `8519584554abb240da771f66a4fe7c47c54f612d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDashboards

## Description
REOPENS 20261111T084409Z-bcm-dashboards-listdashboards-maxresults-range-refusal-message-interpolates.md: its fix left the call refused. That issue asked the @range formatter to STOP emitting the "Value '<submitted>' " clause, and the formatter now omits it; the behavioral check for this same refusal grades the message WITH that clause, so the refusal ListDashboards now returns for an out-of-range maxResults still does not match. This is not a new defect, it is the same refusal message re-diverging in the other direction, and the two recordings of this one message must be reconciled rather than flipped again.

ONE root cause (Cause 1 of 1).

=== Cause 1 of 1: the integer @range ValidationException message omits the rejected value, so the refusal text diverges ===
AWS: Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) - ListDashboards.maxResults targets MaxResults, integer, @default 20, @range min 1 max 100; modelled errors AccessDeniedException (403), InternalServerException (500), ThrottlingException (429), ValidationException (400). A maxResults above the modelled maximum is a client constraint violation, so the call is refused with HTTP 400 and error code ValidationException. The model fixes the status, the error shape and the bound; the message WORDING is recorded behavior only, and the behavioral recording for this operation is AWS's standard constraint sentence WITH the rejected value: fixed text "<n> validation error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>". Its variable parts are the violation count, the rejected value taken from the request, the member name taken from the request (`maxResults`) and the ceiling taken from the member's modelled @range maximum (100). A request inside the range is NOT refused: maxResults=100 succeeds and returns a `dashboards` key (the check's positive control).
Emulator: the status and the error code are right - HTTP 400, ValidationException - but the message is "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100": the "Value '<rejected value>' " clause is missing between "Value" and "at 'maxResults'". Everything else in the sentence matches.
Why: status and code are correct, so a client branching only on those is unaffected; anything reading the message diverges. A client, log matcher or test suite that parses the rejected value out of a ValidationException - the standard AWS constraint template puts it there - reads a sentence with the value position empty and cannot report which input was refused, and a human copying this emulator's text gets the wrong template for the next range check.
Fix: the shared boundary constraint validator that enforces @range before the handler runs (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/list_dashboards.rs documents at its module head that a maxResults outside 1..100 is refused by the boundary validator before `handle` runs). Its integer-@range message formatter must render the rejected value in the value position; the status (400) and error shape (ValidationException) already match and need no change. No change belongs in the ListDashboards handler, its pagination projection or its serializer. Note that this same formatter's output is asserted in BOTH directions by the two corpora cited above - reconcile the two recordings for this member before re-flipping the clause, and record which recording is authoritative, so the next pass does not oscillate.
Traces: bcm-dashboards.list-dashboards-max-results-out-of-range-rejected

## Evidence
Suite: simparity-behavioral-aws-v1 - 24/29 passed; bcm-dashboards 24/26 (92%), 2 failed.

[mismatch] bcm-dashboards.list-dashboards-max-results-out-of-range-rejected
  Call: list_dashboards(maxResults=101).
  Code assertion PASSED: ValidationException (HTTP 400) - the call is correctly refused.
  Message assertion FAILED (check_bcm_dashboards_list_dashboards_max_results_out_of_range_rejected.py line 19):
    Expected: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
    Actual:   "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
  Message breakdown - fixed text: "1 validation error detected: Value '" + value + "' at '" + member + "' failed to satisfy constraint: Member must have value less than or equal to " + bound. Variable parts: the rejected value (101 in this run, whatever the request submits in general), the member name `maxResults` from the request, and the bound 100 from the member's modelled @range maximum. The emulator's message omits the value clause entirely.
  Positive control in the same check: list_dashboards(maxResults=100) succeeds and returns a `dashboards` key - that part passed, so only the refusal text diverges.

Prior history for this exact message: fixed issue 20261111T084409Z-bcm-dashboards-listdashboards-maxresults-range-refusal-message-interpolates.md (corpus simparity-wire-aws-v1, fixture bcm-dashboards-list-dashboards-max-results-above-range-rejected#task#1) recorded the message WITHOUT the value clause and its fix removed the clause; this behavioral check records it WITH the clause.

Smithy consulted (smithy_query query="operation", operation="ListDashboards"): input {maxResults -> MaxResults integer, default 20, range 1..100; nextToken -> NextPageToken @length 1..8192 @pattern ^[\S\s]*$}, output {dashboards required, nextToken}, errors AccessDeniedException 403 / InternalServerException 500 / ThrottlingException 429 / ValidationException 400, @readonly, @paginated(pageSize=maxResults, items=dashboards).

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore bcm-dashboards client at it.
2. Call list_dashboards(maxResults=101). Both AWS and the emulator refuse with HTTP 400 / ValidationException.
3. Compare the `message` field: the behavioral recording has "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; the emulator omits the "Value '101' " clause.
4. Call list_dashboards(maxResults=100) and confirm it still succeeds and returns a `dashboards` key (positive control, currently passing).
5. Read services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/list_dashboards.rs - the module doc confirms the 1..100 check is applied by the shared boundary validator before `handle` runs, so the message comes from that validator's @range formatter.
6. Reconcile the two recordings of this message (wire fixture bcm-dashboards-list-dashboards-max-results-above-range-rejected#task#1 vs this behavioral check) before changing the formatter, then re-replay check bcm-dashboards.list-dashboards-max-results-out-of-range-rejected AND that wire fixture step together.
