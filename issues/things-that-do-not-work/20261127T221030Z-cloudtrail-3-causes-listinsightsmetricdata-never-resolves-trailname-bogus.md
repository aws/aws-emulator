Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.list-insights-metric-data-refuses-deleted-trail cloudtrail.list-insights-metric-data-rejects-a-trail-name-only-while-no-such-trail-exists cloudtrail.list-insights-metric-data-unissued-next-token-rejected cloudtrail.lookup-events-next-token-with-other-lookup-attribute-rejected
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.list-insights-metric-data-refuses-deleted-trail|_refusal#21|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded with {answer!r}")
Issue-Key: cloudtrail.list-insights-metric-data-rejects-a-trail-name-only-while-no-such-trail-exists|_refusal#7|AssertionError|raise AssertionError(f"expected a refusal, got a success: {answer!r}")
Issue-Key: cloudtrail.list-insights-metric-data-unissued-next-token-rejected|run#10|AssertionError|assert got == expected, (
Issue-Key: cloudtrail.lookup-events-next-token-with-other-lookup-attribute-rejected|run#15|AssertionError|assert got == ("InvalidNextTokenException", _MISMATCH), (
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudtrail: 3 causes — ListInsightsMetricData never resolves TrailName (bogus/deleted trail answered 200), and both pagination-token refusals carry non-AWS messages

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInsightsMetricData

## Description
Three net-new root causes in the CloudTrail Insights-metric and LookupEvents read paths. All three are grounded in failing behavioral checks listed under `### cloudtrail`.

Cause 1 of 3: ListInsightsMetricData never resolves TrailName, so a name/ARN that names no trail is served instead of refused.
AWS:      ListInsightsMetricData takes an optional `TrailName` (Smithy: ListInsightsMetricDataRequest.TrailName -> String; modelled errors InvalidParameterException (client, 400), InvalidTrailNameException, OperationNotPermittedException, UnsupportedOperationException). Recorded behavior: when `TrailName` names no trail in the account/Region — before any trail of that name is created, and again after the trail is deleted, whether given as a bare name or as the full `arn:aws:cloudtrail:<region>:<account>:trail/<name>` — the call is refused with HTTP 400, Error.Code `InvalidParameterException` and Error.Message of the form `The TrailName <trail-name-or-arn> is not a valid trail name or ARN.` Fixed text: `The TrailName ` ... ` is not a valid trail name or ARN.`; the single variable part is the `TrailName` value copied from the request. When `TrailName` DOES name a live trail, the 200 response echoes that trail's ARN in the `TrailARN` output member (Smithy: ListInsightsMetricDataResponse.TrailARN).
Emulator: the TrailName is ignored entirely. Every call is answered HTTP 200 with a body that echoes only EventSource/EventName/InsightType and empty `Timestamps`/`Values`, e.g. `{'EventSource': 's3.amazonaws.com', 'EventName': 'PutObject', 'InsightType': 'ApiCallRateInsight', 'Timestamps': [], 'Values': []}` for a deleted trail, and `{'EventSource': 'cloudtrail.amazonaws.com', 'EventName': 'PutEventSelectors', ...}` for a name no trail ever held. No refusal is produced for either the never-existed or the deleted trail, and `TrailARN` is absent even on the live-trail call, so the response cannot be joined back to a trail.
Why:      a client cannot distinguish "this trail has no Insights data points" from "this trail does not exist". Monitoring code that typos a trail name, or that keeps polling a trail someone deleted, reads an empty series as a healthy zero-activity window forever instead of surfacing the InvalidParameterException AWS raises. Code that keys results off the echoed `TrailARN` gets `None`.
Fix:      the ListInsightsMetricData handler — resolve `TrailName` (bare name or trail ARN) against trail state BEFORE serving metric data, refuse an unresolvable one with InvalidParameterException/400 and the message template above, and populate the `TrailARN` output member from the resolved trail on success.
Traces:   cloudtrail.list-insights-metric-data-refuses-deleted-trail, cloudtrail.list-insights-metric-data-rejects-a-trail-name-only-while-no-such-trail-exists.

Cause 2 of 3: ListInsightsMetricData's refusal of an un-issued NextToken uses an emulator-authored message.
AWS:      recorded behavior — a `NextToken` the service never issued is refused with Error.Code `InvalidParameterException` and Error.Message `NextToken could not be deserialized: <next-token>`. Fixed text: `NextToken could not be deserialized: `; the variable part is the NextToken value copied verbatim from the request. (Smithy: ListInsightsMetricDataRequest.NextToken, InsightsMetricNextToken @length 1..5000; InvalidParameterException is the modelled client error.) The classification is right in the emulator; the wording is not.
Emulator: refuses with the correct code `InvalidParameterException` but the message `NextToken `<next-token>` was not issued by this service for a Period of 60 seconds.` — wording that exists nowhere in AWS, back-quotes the token, and leaks the request's `Period` value into an error class that has nothing to do with it.
Why:      clients and support tooling that match on CloudTrail's documented pagination-failure text (or that log it for triage) see a message no AWS runbook recognises, and the spurious `Period` clause invites the wrong diagnosis (a period mismatch) for a plain forged-cursor failure.
Fix:      the ListInsightsMetricData handler's NextToken validation — emit AWS's fixed text with the request's token interpolated, and drop the Period clause.
Traces:   cloudtrail.list-insights-metric-data-unissued-next-token-rejected.

Cause 3 of 3: LookupEvents' InvalidNextTokenException for a token replayed against different LookupAttributes carries a non-AWS message.
AWS:      Smithy: LookupEvents models InvalidNextTokenException (client, httpError 400). Recorded behavior — when a NextToken issued for one `LookupAttributes` set (e.g. EventName=DescribeTrails) is sent back with a different one (EventName=GetTrailStatus), the call is refused with Error.Code `InvalidNextTokenException` and Error.Message `Your pagination token does not match your request`. That message is entirely fixed text; it has no variable part.
Emulator: refuses with the correct code `InvalidNextTokenException` but the message `Can't decrypt marker` — internal-implementation wording describing a cursor-decryption failure rather than AWS's statement that the token does not match the request.
Why:      a client cannot tell a token/request mismatch (retryable by restarting the scan with the new attributes) from a corrupt or expired cursor, and any assertion or log-matching on AWS's text fails.
Fix:      the LookupEvents handler's NextToken validation — when the token's bound lookup attributes differ from the request's, raise InvalidNextTokenException with AWS's fixed text.
Traces:   cloudtrail.lookup-events-next-token-with-other-lookup-attribute-rejected.

No open issue covers these: the open CloudTrail issues concern UpdateTrail/PutInsightSelectors IAM resolution (20261125T063514Z-...), audit-record attribution (20261031T171713Z-...) and UpdateTrail concurrency/CloudFormation (20261023T135552Z-...), none of which touch TrailName resolution in ListInsightsMetricData or pagination-token message text.

## Evidence
Cause 1 — cloudtrail.list-insights-metric-data-refuses-deleted-trail:
  Expected: ClientError, Error.Code 'InvalidParameterException', Error.Message 'The TrailName <name> is not a valid trail name or ARN.'
  Actual:   HTTP 200, body {'EventSource': 's3.amazonaws.com', 'EventName': 'PutObject', 'InsightType': 'ApiCallRateInsight', 'Timestamps': [], 'Values': []}
  Assertion: "expected a refusal, but the call succeeded with {...}"

Cause 1 — cloudtrail.list-insights-metric-data-rejects-a-trail-name-only-while-no-such-trail-exists:
  Expected: 'InvalidParameterException' + message starting 'The TrailName ' and ending ' is not a valid trail name or ARN.' for (a) before the trail exists, (b) after deletion by name, (c) after deletion by ARN; and TrailARN echoed for a live trail.
  Actual:   HTTP 200, body {'EventSource': 'cloudtrail.amazonaws.com', 'EventName': 'PutEventSelectors', 'InsightType': 'ApiCallRateInsight', 'Timestamps': [], 'Values': []} — no TrailARN member at all.
  Assertion: "expected a refusal, got a success: {...}"

Cause 2 — cloudtrail.list-insights-metric-data-unissued-next-token-rejected:
  Expected: ('InvalidParameterException', 'NextToken could not be deserialized: <token>')
  Actual:   ('InvalidParameterException', 'NextToken `<token>` was not issued by this service for a Period of 60 seconds.')
  Assertion: "a NextToken never issued was refused as ('InvalidParameterException', 'NextToken `Zm9yZ2VkLWN1cnNvcg` was not issued by this service for a Period of 60 seconds.')"

Cause 3 — cloudtrail.lookup-events-next-token-with-other-lookup-attribute-rejected:
  Expected: ('InvalidNextTokenException', 'Your pagination token does not match your request')
  Actual:   ('InvalidNextTokenException', "Can't decrypt marker")
  Assertion: "a token sent with another EventName was refused as ('InvalidNextTokenException', \"Can't decrypt marker\")"

## Steps to Reproduce
Cause 1:
1. ListInsightsMetricData(TrailName="<a name no trail holds>", EventSource="cloudtrail.amazonaws.com", EventName="PutEventSelectors", InsightType="ApiCallRateInsight") -> expect InvalidParameterException/400 'The TrailName <name> is not a valid trail name or ARN.'; emulator answers 200 with empty Timestamps/Values.
2. CreateTrail(Name=<name>, S3BucketName=<bucket>); ListInsightsMetricData(TrailName=<name>, ...) -> expect 200 whose TrailARN is arn:aws:cloudtrail:<region>:<account>:trail/<name>; emulator omits TrailARN.
3. DeleteTrail(Name=<name>); repeat step 1 with the bare name and again with the full trail ARN -> both must be refused as in step 1.

Cause 2:
4. ListInsightsMetricData(EventSource="cloudtrail.amazonaws.com", EventName="GetTrailStatus", InsightType="ApiCallRateInsight", StartTime=T-4m, EndTime=T, Period=60, DataType="FillWithZeros", MaxResults=2) to obtain a real NextToken, then re-issue the same query with NextToken="Zm9yZ2VkLWN1cnNvcg" -> expect ('InvalidParameterException', 'NextToken could not be deserialized: Zm9yZ2VkLWN1cnNvcg').

Cause 3:
5. LookupEvents(StartTime=.., EndTime=.., LookupAttributes=[{EventName: DescribeTrails}], MaxResults=1) to obtain a NextToken.
6. LookupEvents(same window, LookupAttributes=[{EventName: GetTrailStatus}], MaxResults=1, NextToken=<that token>) -> expect ('InvalidNextTokenException', 'Your pagination token does not match your request').
