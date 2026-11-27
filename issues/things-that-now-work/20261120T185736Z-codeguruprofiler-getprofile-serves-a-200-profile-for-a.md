Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.get-profile-no-aggregated-data-yet
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.get-profile-no-aggregated-data-yet|_error_code#16|AssertionError|raise AssertionError("expected a rejection, but the call SUCCEEDED")
Issue-Commit: 597db9eebaefdf1df0a6fca4ca797f56db8217fd

# codeguruprofiler: GetProfile serves a 200 profile for a just-posted, not-yet-aggregated window instead of refusing with ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `597db9eebaefdf1df0a6fca4ca797f56db8217fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProfile

## Description
One net-new root cause in the codeguruprofiler emulator, grounded in a failing behavioral check of the simparity-behavioral-aws-v1 replay.

Already-filed work that does NOT cover this cause: the open issue 20261111T051323Z-codeguruprofiler-getprofile-never-makes-a-posted-agent-profile.md concerns the opposite direction — GetProfile never serving a 200 for a CLOSED, UTC-aligned PT5M window a PostAgentProfile populated, so that check exhausts its poll budget. The cause filed here is that GetProfile answers 200 IMMEDIATELY, for a submission AWS has not aggregated yet, on the un-closed window around 'now'; the refusal that should still be in force is gone. Neither issue's replay key covers the other's check.

Cause 1 of 1: GetProfile does not keep refusing a window whose only agent submission has not been aggregated yet
AWS: Smithy GetProfile — GET /profilingGroups/{profilingGroupName}/profile, success 200 with the required `profile` (@httpPayload blob, target AggregatedProfile) plus the required Content-Type header; modeled errors are ValidationException (400), ResourceNotFoundException (404), ThrottlingException (429), InternalServerException (500). PostAgentProfile — POST /profilingGroups/{profilingGroupName}/agentProfile, success 204 with an empty output. The AggregatedProfile blob has no modeled schema, so the timing rule is recorded behavior only: aggregation is asynchronous and is NOT complete when PostAgentProfile returns, so a GetProfile for the window covering a submission that has only just been accepted still answers 404 ResourceNotFoundException with the message `No profiles found for the requested time range!` — fixed text, no variable parts. The recording shows this both before any PostAgentProfile call and immediately after one that has already set profilingStatus.latestAgentProfileReportedAt on DescribeProfilingGroup: a reported agent profile is not by itself a retrievable aggregated profile.
Emulator: the first GetProfile(profilingGroupName=N, period='PT5M') — before any submission — is refused correctly (404 ResourceNotFoundException, `No profiles found for the requested time range!`; that assertion in the check passes). After PostAgentProfile(contentType='application/json') for a window ending at the current instant, the SAME GetProfile call SUCCEEDS: the check's `_error_code` helper raises `AssertionError: expected a rejection, but the call SUCCEEDED`, i.e. the emulator returned a 2xx profile payload. The emulator therefore treats 'an agent profile was posted' as 'an aggregated profile exists', with no aggregation state between the two.
Why: a client cannot tell 'submitted' from 'aggregated'. Agent-side and console-side callers poll GetProfile and treat a 404 as 'still aggregating'; an emulator that answers 200 on the first poll hands them a profile document for a window AWS would never serve yet, so code that validates the aggregated document (window bounds, aggregatedMetadata.numReportedAgents) is exercised against synthesised content, and retry/backoff paths around the documented 404 are never exercised at all. Combined with the opposite behavior filed for closed windows, the data plane's retrievability rule is inverted relative to AWS.
Fix: GetProfile's handler and the aggregation state behind PostAgentProfile — an accepted agent submission must be recorded as pending aggregation and must NOT make a window retrievable on its own; GetProfile must keep answering 404 ResourceNotFoundException with the exact message `No profiles found for the requested time range!` until that window's aggregated profile actually exists. File/line citations belong under Traces.
Traces: codeguruprofiler.get-profile-no-aggregated-data-yet.

## Evidence
Cause 1 — GetProfile(profilingGroupName=N, period='PT5M') immediately after PostAgentProfile for the window [now-60s, now]:
  expected: 404 ResourceNotFoundException, Error.Code 'ResourceNotFoundException', Error.Message exactly 'No profiles found for the requested time range!' (entirely fixed text; no request- or state-derived parts)
  actual:   the call returns success (a 2xx with a profile payload). The check's helper reports `AssertionError: expected a rejection, but the call SUCCEEDED` at check_codeguruprofiler_get_profile_no_aggregated_data_yet.py line 78, reached from line 62 (the post-submission GetProfile), verdict `[mismatch] codeguruprofiler.get-profile-no-aggregated-data-yet`
  note: the pre-submission GetProfile in the same check (line 29) was refused with exactly the expected code and message, so the refusal path exists; what diverges is that PostAgentProfile immediately satisfies it
  note: DescribeProfilingGroup assertions around the submission pass — profilingStatus.latestAgentProfileReportedAt is absent before PostAgentProfile and populated after — so the emulator does track the submission; it just also treats it as an aggregated profile

## Steps to Reproduce
1. Boot the emulator and point a botocore codeguruprofiler client at it.
2. CreateProfilingGroup(profilingGroupName=N, agentOrchestrationConfig={'profilingEnabled': True}).
3. GetProfile(profilingGroupName=N, period='PT5M') — refused 404 ResourceNotFoundException 'No profiles found for the requested time range!' (correct).
4. DescribeProfilingGroup(profilingGroupName=N) — profilingStatus.latestAgentProfileReportedAt is absent.
5. PostAgentProfile(profilingGroupName=N, agentProfile=json.dumps({'start': now_ms-60000, 'end': now_ms, 'callgraph': {'frameId':'ALL','runnableCount':1,'children':{}}}).encode(), contentType='application/json').
6. DescribeProfilingGroup(profilingGroupName=N) — latestAgentProfileReportedAt is now set.
7. GetProfile(profilingGroupName=N, period='PT5M') again: AWS still refuses with the same 404/message; the emulator returns a profile instead.
8. Or replay the check listed in trace_ids.
