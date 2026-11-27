Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.batch-get-frame-metric-data-batch-matching-no-samples-still-answered codeguruprofiler.batch-get-frame-metric-data-invalid-metric-type-refuses-whole-batch codeguruprofiler.batch-get-frame-metric-data-mixed-batch-answers-every-entry-in-order
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.batch-get-frame-metric-data-batch-matching-no-samples-still-answered|run#15|AssertionError|assert control[0] is not None and abs(control[0] - 0.4) < 1e-9, (
Issue-Key: codeguruprofiler.batch-get-frame-metric-data-invalid-metric-type-refuses-whole-batch|run#16|AssertionError|assert share is not None and abs(share - _FRAME_RUNNABLE / _TOTAL_RUNNABLE) < 1e-9, (
Issue-Key: codeguruprofiler.batch-get-frame-metric-data-mixed-batch-answers-every-entry-in-order|run#19|AssertionError|assert len(steps) == 5 and gaps == {300.0}, (
Issue-Commit: ab3264a14a30dede061951437467343aaef14e5b

# codeguruprofiler: BatchGetFrameMetricData computes AggregatedRelativeTotalTime against the wrong denominator and drops the last time step of the requested range

> _Note: this issue was identified against the code base at commit `ab3264a14a30dede061951437467343aaef14e5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetFrameMetricData

## Description
Two distinct root causes in the emulator's BatchGetFrameMetricData implementation for CodeGuru Profiler. Both were found by replaying behavioral checks against a profiling group seeded with one posted agent profile (PostAgentProfile) whose aggregation GetProfile then served.

Cause 1 of 2: AggregatedRelativeTotalTime divides by root-plus-descendant sample counts instead of the root's own count.
AWS: Smithy: BatchGetFrameMetricData (POST /profilingGroups/{profilingGroupName}/frames/-/metrics, 200), output member frameMetricData -> FrameMetricDatum{frameMetric, values}; FrameMetric.type is the enum MetricType whose only value is AggregatedRelativeTotalTime. AWS documents AggregatedRelativeTotalTime as the percentage/share of the total time the whole profile spent, i.e. the frame's sample count in the requested thread state divided by the ROOT frame's sample count in that same thread state for the aggregation window. Recorded behavior: a profile with callgraph.counts {RUNNABLE: 50} and child frame 'media.Transcoder.encode' {RUNNABLE: 20} answers that frame 20/50 = 0.4; a profile with root {RUNNABLE: 60} and children 45 and 15 answers the 45 frame 45/60 = 0.75 and the 15 frame 15/60 = 0.25.
Emulator: it returns the frame's count divided by (root count + the summed counts of the children/descendants). For root 50 / child 20 it answered 0.2857142857142857 (= 20/70) instead of 0.4; for root 60 / children 45+15 it answered 0.375 (= 45/120) instead of 0.75. The status is 200 and the response shape is correct — only the values are wrong, and the error scales with how much of the tree is sampled.
Why: every value a client reads from frameMetricData is a wrong share, always smaller than the real one, and the values no longer sum the way a call-tree share must (a child can never exceed its root). Dashboards, regression gates and 'frame X takes N% of total time' alarms built on the emulator compute figures that cannot be reproduced on AWS.
Fix: the frame-metric aggregation that computes a frame's relative total time for BatchGetFrameMetricData — the denominator must be the root frame's own sample count for the requested thread state in that window, not the subtree total.
Traces: codeguruprofiler.batch-get-frame-metric-data-batch-matching-no-samples-still-answered (expected 0.4, got 0.2857142857142857); codeguruprofiler.batch-get-frame-metric-data-invalid-metric-type-refuses-whole-batch (expected 0.75, got 0.375 — this check's control call fails on the same cause before it ever reaches its refusal assertion).

Cause 2 of 2: endTimes enumerates the requested range half-open, losing the step at endTime.
AWS: Smithy: BatchGetFrameMetricData input startTime/endTime (httpQuery, date-time) and targetResolution (AggregationPeriod: PT5M | PT1H | P1D); output endTimes is a required ListOfTimestamps of TimestampStructure{value}, and every FrameMetricDatum.values list is parallel to it. Recorded behavior: for a range of 20 minutes at targetResolution PT5M AWS returns 5 time steps spaced 300 s apart — the enumeration is inclusive of both the start and the end boundary (floor((end-start)/resolution)+1 steps).
Emulator: it returns only 4 steps for the same 20-minute, PT5M request — the steps run from startTime up to but excluding endTime, so the final boundary step is missing (observed: 4 steps 300 s apart, the step stamped at endTime absent). Each frameMetricData entry's values list is correspondingly one element short.
Why: a client that computes its own time axis from startTime/endTime/resolution, or zips values against an expected step count, misaligns or index-errors against the emulator; the last bucket of every query silently disappears, so the most recent window — the one callers poll for — is never returned.
Fix: the time-step enumeration in the BatchGetFrameMetricData handler that builds endTimes (and the per-step value vectors) must include the step at endTime.
Traces: codeguruprofiler.batch-get-frame-metric-data-mixed-batch-answers-every-entry-in-order.

Note: the two cloudformation__codeguruprofiler failures in the same replay are unrelated and already owned by 20261109T163828Z-cloudformation-codeguruprofiler-aws-codeguruprofiler-profilinggroup-is-not-a.md.

## Evidence
Cause 1 — AggregatedRelativeTotalTime denominator

Check codeguruprofiler.batch-get-frame-metric-data-batch-matching-no-samples-still-answered
Seed: PostAgentProfile with callgraph {counts: {RUNNABLE: 50}, children: {'media.Transcoder.encode': {counts: {RUNNABLE: 20}}}} over one closed 5-minute window; GetProfile(period=PT5M) confirmed the aggregate.
Request: BatchGetFrameMetricData(targetResolution=PT5M, frameMetrics=[{frameName: 'media.Transcoder.encode', type: 'AggregatedRelativeTotalTime', threadStates: ['RUNNABLE']}])
Expected (AWS): frameMetricData[0].values[0] == 0.4  (20 / 50)
Actual (emulator): frameMetricData[0].values == [0.2857142857142857]  (20 / (50+20))
Assertion: "expected the sampled frame in its recorded state to score 20/50 = 0.4, got [0.2857142857142857]"

Check codeguruprofiler.batch-get-frame-metric-data-invalid-metric-type-refuses-whole-batch
Seed: callgraph {counts: {RUNNABLE: 60}, children: {'ledger.Posting.apply': {counts: {RUNNABLE: 45}}, 'ledger.Posting.audit': {counts: {RUNNABLE: 15}}}}
Request: BatchGetFrameMetricData(targetResolution=PT5M, frameMetrics=[{frameName: 'ledger.Posting.apply', type: 'AggregatedRelativeTotalTime', threadStates: ['RUNNABLE']}])
Expected (AWS): 45 / 60 = 0.75
Actual (emulator): [{'frameMetric': {'frameName': 'ledger.Posting.apply', 'threadStates': ['RUNNABLE'], 'type': 'AggregatedRelativeTotalTime'}, 'values': [0.375]}]  (45 / (60+45+15) = 45/120)
Assertion: "expected the valid entry alone to be answered 0.75, got ... 'values': [0.375]"
The divergence is a pure ratio defect: 0.4 -> 20/70 and 0.75 -> 45/120 both equal frame_count / (root_count + sum(child_counts)).

Cause 2 — endTimes step enumeration

Check codeguruprofiler.batch-get-frame-metric-data-mixed-batch-answers-every-entry-in-order
Request: BatchGetFrameMetricData(startTime = W - 10min, endTime = W + 10min, targetResolution='PT5M', frameMetrics=[3 entries]) where W is the start of the posted, closed 5-minute window.
Expected (AWS): 5 entries in endTimes, consecutive gaps all 300.0 s, spanning startTime..endTime inclusive; each frameMetricData[i].values has 5 elements.
Actual (emulator): 4 entries — ['2026-11-27 10:20:00+00:00', '2026-11-27 10:25:00+00:00', '2026-11-27 10:30:00+00:00', '2026-11-27 10:35:00+00:00'] — the step at the endTime boundary (10:40) is absent. (The concrete stamps are this run's clock; the requirement is the count/spacing: floor((endTime-startTime)/resolution)+1 steps, both boundaries included.)
Assertion: "expected 5 time steps 300 s apart over a 20-minute range, got [...4 stamps...]"
Status is 200 and resolution is correctly 'PT5M'; only the step list is short.

## Steps to Reproduce
Cause 1:
1. CreateProfilingGroup(profilingGroupName=G); ConfigureAgent(profilingGroupName=G, fleetInstanceId=any).
2. PostAgentProfile(profilingGroupName=G, contentType='application/json', agentProfile=JSON of {start: T0, end: T0+300000, callgraph: {counts: {RUNNABLE: 60}, children: {'ledger.Posting.apply': {counts: {RUNNABLE: 45}}, 'ledger.Posting.audit': {counts: {RUNNABLE: 15}}}}} where T0 is a 5-minute-aligned millisecond timestamp of a closed window.
3. Poll GetProfile(profilingGroupName=G, period='PT5M', accept='application/json') until it succeeds.
4. BatchGetFrameMetricData(profilingGroupName=G, startTime=T0, endTime=T0+5min, targetResolution='PT5M', frameMetrics=[{frameName:'ledger.Posting.apply', type:'AggregatedRelativeTotalTime', threadStates:['RUNNABLE']}]).
   -> emulator answers values [0.375]; AWS answers [0.75] (45/60). Same with root 50 / single child 20: emulator 0.2857142857142857, AWS 0.4.

Cause 2:
1. Same setup through step 3 with a single posted window starting at W.
2. BatchGetFrameMetricData(profilingGroupName=G, startTime=W-10min, endTime=W+10min, targetResolution='PT5M', frameMetrics=[any entry]).
   -> emulator returns len(endTimes) == 4 and 4 values per datum; AWS returns 5, the fifth stamped at endTime.
