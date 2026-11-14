Issue-Source: trace-sweep-divergence
Issue-Trace: rds-engine-versions-pagination
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-engine-versions-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: f836b492e790c3b6e56806350eb24f389e34b620

# rds DescribeDBEngineVersions invalid-marker refusal echoes the marker and misquotes AWS wording

> _Note: this issue was identified against the code base at commit `f836b492e790c3b6e56806350eb24f389e34b620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
1 cause.

Cause 1 of 1: DescribeDBEngineVersions called with a Marker value the service cannot parse.

AWS: Smithy (com.amazonaws.rds#AmazonRDSv19) models DescribeDBEngineVersions as paginated with inputToken=Marker / outputToken=Marker / items=DBEngineVersions / pageSize=MaxRecords, and binds no operation-specific errors, so the refusal is a service-common one. Recorded behavior only for the refusal itself: HTTP 400 with an $.ErrorResponse.Error.Message of exactly 'Invalid marker. Please use the marker returned from a prior request or omit the marker parameter.' That whole sentence is FIXED text for this operation — there is no variable part at all: the rejected marker value is NOT echoed, and the article is 'a prior request'.

Emulator: returns the right status (400) and the right error code, but writes a different sentence: 'Invalid marker: <Marker>. Please use the marker returned from the prior request or omit the marker parameter.' It interpolates the offending Marker taken from the request after an added colon, and says 'the prior request' where AWS says 'a prior request'. (In the recorded step the request Marker was a literal placeholder value, so only the fixed-vs-variable structure of the sentence is the requirement, not that particular value.)

Note: this is the DescribeDBParameters wording applied to the wrong operation. The already-resolved issue 20261029T214632Z-rds-state-precondition-and-marker-refusals-are-worded.md asked for the echoing form ('Invalid marker: <Marker>. ... the prior request ...') for DescribeDBParameters, which is correct THERE; DescribeDBEngineVersions uses the non-echoing form, so a single shared marker-error string cannot serve both. The open issue 20261030T001542Z-rds-parameter-pagination-markers-are-raw-parameter-names.md is about the CONTENT of parameter-pagination markers and does not cover this operation's refusal wording.

Why: a client that matches RDS's documented/recorded invalid-marker sentence for DescribeDBEngineVersions — for example to distinguish an expired or malformed continuation token from any other 400 and restart the engine-version sweep from the first page — does not recognise the emulator's sentence, because it carries an extra ':<marker>' segment and a different article. Worse, the emulator reflects the caller-supplied marker back into the message, so a caller that logs or surfaces the error leaks the raw token in a place AWS keeps clean.

Fix: the DescribeDBEngineVersions handler's Marker validation message (the marker-decode error path of the RDS DescribeDBEngineVersions operation), which must carry its own fixed sentence rather than share the parameter-pagination one.

Traces: rds-engine-versions-pagination#task#4.

## Evidence
Replay cluster (verdict=mismatch, expected_status=400, actual_status=400 — only $.ErrorResponse.Error.Message diverged):

  divergence_key: service=rds|DescribeDBEngineVersions|status=400->400|keys=$.ErrorResponse.Error.Message
  fixture: rds-engine-versions-pagination, step task#4
  $.ErrorResponse.Error.Message: value_mismatch
    expected: 'Invalid marker. Please use the marker returned from a prior request or omit the marker parameter.'
    actual:   'Invalid marker: not-a-real-marker. Please use the marker returned from the prior request or omit the marker parameter.'

Fixed vs variable: for this operation AWS's message is entirely fixed text with no variable part. In the emulator's message, 'not-a-real-marker' is the Marker value echoed from the request (a per-request value, not part of any template), and 'the prior request' replaces AWS's 'a prior request'.

The cluster has affected_count 1 (rds-engine-versions-pagination#task#4), no stub_path and no owning open issue.

Smithy consulted: DescribeDBEngineVersions input member Marker (String), paginated {inputToken: Marker, outputToken: Marker, items: DBEngineVersions, pageSize: MaxRecords}, errors: [] (service-common errors only).

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the wire corpus for trace set simparity-wire-aws-v1.
3. Re-run fixture rds-engine-versions-pagination, step task#4: it calls DescribeDBEngineVersions with an unparseable Marker and compares $.ErrorResponse.Error.Message against the recorded response.
4. Expect HTTP 400 with Message exactly 'Invalid marker. Please use the marker returned from a prior request or omit the marker parameter.'; observe the emulator's 'Invalid marker: <marker>. Please use the marker returned from the prior request or omit the marker parameter.'
