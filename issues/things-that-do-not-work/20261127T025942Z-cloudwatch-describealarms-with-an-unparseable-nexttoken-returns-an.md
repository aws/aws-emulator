Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.describe-alarms-invalid-next-token-is-ignored cloudwatch.describe-alarms-state-filter-and-invalid-next-token-ignored cloudwatch.op-contract-describeAlarms-dup
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.describe-alarms-invalid-next-token-is-ignored|run#10|AssertionError|assert token_names == baseline_names, (
Issue-Key: cloudwatch.describe-alarms-state-filter-and-invalid-next-token-ignored|run#18|AssertionError|assert names_token == {alarm_ok, alarm_in_alarm}, (
Issue-Key: cloudwatch.op-contract-describeAlarms-dup|run#18|AssertionError|assert names_token == {alarm_ok, alarm_in_alarm}, (
Issue-Commit: 34ec76825aeb60426f74f39ee1ee1124c6afb7e4

# cloudwatch: DescribeAlarms with an unparseable NextToken returns an empty page instead of the first page

> _Note: this issue was identified against the code base at commit `34ec76825aeb60426f74f39ee1ee1124c6afb7e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
This issue carries 1 net-new root cause in the cloudwatch (monitoring, 2010-08-01) emulator.

Cause 1 of 1: DescribeAlarms answers HTTP 200 with an EMPTY MetricAlarms list when the request carries a NextToken the service did not issue, instead of ignoring the unusable token and returning the same first page the token-less call returns.

AWS: Smithy (cloudwatch 2010-08-01) models DescribeAlarms as @paginated (inputToken NextToken / outputToken NextToken / pageSize MaxRecords) with input members AlarmNames (list, @length 0..100), StateValue (enum), MaxRecords (@range 1..100) and NextToken, and declares one error, InvalidNextToken (errorKind client, httpError 400, awsQueryError code InvalidNextToken / 400). The model therefore permits a refusal but does not require one, and the recorded behavior is that DescribeAlarms does NOT validate the token at all: a request whose NextToken is garbage succeeds with HTTP 200 and is answered exactly as if no NextToken had been sent — the first page, still scoped by the other filters (AlarmNames, StateValue, MaxRecords). This is the same "DescribeAlarms does not validate the token" behavior already noted for contrast in Cause 5 of the open issue 20261117T082319Z-cloudwatch-7-net-new-causes-putmetricalarm-accepts-threshold.md, which is about DescribeAnomalyDetectors' InvalidNextToken MESSAGE and does not carry these checks' replay keys. Recorded behavior only for the page content: when no 400 is raised, the response body must be the first page.

Emulator: the request is not refused either — no InvalidNextToken, HTTP 200 — but the response body is wrong: MetricAlarms comes back EMPTY. With AlarmNames naming alarms the same call returns without a token (verified as the baseline in each check), adding NextToken="not-a-real-token-xyz-123" / "garbage-not-a-real-token" yields set() instead of the baseline alarm set. The emulator evidently decodes the token into a resume position it cannot resolve and treats that as "past the end of the collection" rather than as "no token". The StateValue filtering in the same calls is correct (ALARM returns only the alarm SetAlarmState put in ALARM, OK only the OK one), so the defect is isolated to the token path.

Why: a client that sends a stale, truncated or foreign pagination token gets neither of AWS's two possible answers. It is not told the token is bad (no InvalidNextToken to retry on) and it is not given the page AWS would serve; it sees an authoritative, successful, EMPTY alarm list. Code that treats an empty DescribeAlarms page as "this alarm does not exist" will delete/recreate alarms, report a monitored resource as unmonitored, or stop a pagination loop having read nothing — all silently, with a 200 and no NextToken to signal more data.

Fix: DescribeAlarms' pagination/NextToken handling in the cloudwatch crate's DescribeAlarms handler (the token decode and resume-position computation), not its filtering or serializer: a NextToken that does not decode to a position this service issued must fall back to the start of the collection so the handler returns the first page, rather than resolving to an out-of-range offset that slices an empty result.

Traces: cloudwatch.describe-alarms-invalid-next-token-is-ignored, cloudwatch.describe-alarms-state-filter-and-invalid-next-token-ignored, cloudwatch.op-contract-describeAlarms-dup.

## Evidence
Cause 1 — all three checks create their own alarms with PutMetricAlarm (ActionsEnabled=false), confirm the token-less baseline returns them, then re-issue the same DescribeAlarms with a garbage NextToken.

- cloudwatch.describe-alarms-invalid-next-token-is-ignored: baseline DescribeAlarms(AlarmNames=[<alarm>], MaxRecords=5) returns {<alarm>}; DescribeAlarms(AlarmNames=[<alarm>], MaxRecords=5, NextToken='not-a-real-token-xyz-123') returns MetricAlarms=[] → "AssertionError: expected the garbage NextToken to be ignored and the same first page returned, got set() vs baseline {'sim2real-behav-cwprobe-alarm-...'}" (the alarm name is a per-run generated value, not part of the requirement).
- cloudwatch.describe-alarms-state-filter-and-invalid-next-token-ignored and cloudwatch.op-contract-describeAlarms-dup: DescribeAlarms(AlarmNames=[<ok-alarm>, <alarm-alarm>], StateValue='ALARM') and (…, StateValue='OK') both PASS and return exactly the matching alarm; DescribeAlarms(AlarmNames=[<ok-alarm>, <alarm-alarm>], NextToken='garbage-not-a-real-token') returns MetricAlarms=[] → "AssertionError: expected the garbage NextToken to be ignored and both alarms returned, got set()".

Expected in every case: HTTP 200 and the first page as scoped by the other filters (both named alarms, or the single named alarm). Actual: HTTP 200 with an empty MetricAlarms list. No InvalidNextToken was raised in either direction, so the divergence is the page CONTENT, not the status or an error code.

## Steps to Reproduce
1. PutMetricAlarm(AlarmName=A, Namespace=N, MetricName='CPUUtilization', Statistic='Average', Period=60, EvaluationPeriods=1, Threshold=80.0, ComparisonOperator='GreaterThanThreshold', ActionsEnabled=false).
2. DescribeAlarms(AlarmNames=[A], MaxRecords=5) — note the returned MetricAlarms (the baseline first page; it contains A).
3. DescribeAlarms(AlarmNames=[A], MaxRecords=5, NextToken='not-a-real-token-xyz-123') — observe HTTP 200 with MetricAlarms=[] instead of the same page as step 2.
4. Variant that also shows the filters are fine: PutMetricAlarm two alarms A_ok and A_alarm, SetAlarmState(A_ok, 'OK') and SetAlarmState(A_alarm, 'ALARM'); DescribeAlarms(AlarmNames=[A_ok, A_alarm], StateValue='ALARM') returns only A_alarm (correct), then DescribeAlarms(AlarmNames=[A_ok, A_alarm], NextToken='garbage-not-a-real-token') returns MetricAlarms=[] instead of both alarms.
