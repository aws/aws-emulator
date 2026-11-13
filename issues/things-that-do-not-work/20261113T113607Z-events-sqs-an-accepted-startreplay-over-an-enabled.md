Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window
Issue-Key: events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Commit: b3586cbe985cc2412e94e4f6c5b37df3b09bc671

# events__sqs: an accepted StartReplay over an ENABLED archive still re-emits nothing, so the window selection and the replay-name stamp are unobservable

> _Note: this issue was identified against the code base at commit `b3586cbe985cc2412e94e4f6c5b37df3b09bc671`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplay

## Description
One net-new root cause (Cause 1 of 1).

Already filed and NOT attributed to this failure by the replay report: 20261025T115007Z-events-sqs-an-accepted-startreplay-never-produces-an.md (open) names this same check id, and 20261027T000839Z-events-sqs-an-accepted-eventbridge-replay-still-re.md (open) describes the same missing re-emission path for two sibling replay checks. Neither was matched to this check's current replay key, so the failure is re-filed here against its current signature; a fixer closing this one should close those together.

Cause 1 of 1: a replay started on an ENABLED archive never redelivers the in-window archived events to the destination bus's rule target. Failing check: events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window.
AWS: Smithy (com.amazonaws.eventbridge#StartReplay) models required {ReplayName, EventSourceArn (an ArchiveArn), EventStartTime, EventEndTime, Destination(ReplayDestination{Arn, FilterArns})} with output {ReplayArn, State, StateReason, ReplayStartTime}, and DescribeReplay reporting {State, EventLastReplayedTime, ReplayStartTime, ReplayEndTime}; the modeled documentation states that "A replay processes events to replay based on the time in the event" and that DescribeReplay is how progress is determined. So: DescribeReplay settles at State=COMPLETED; exactly those archived events whose own envelope time lies inside [EventStartTime, EventEndTime] are re-emitted onto Destination.Arn (an event archived with a time outside the window is NOT re-emitted); re-emission is confined to the rules named in Destination.FilterArns; and each redelivered envelope carries a 'replay-name' field equal to the ReplayName while a live delivery carries no such key, with detail, source and detail-type unchanged from the archived event.
Emulator: nothing comes back. The check publishes three entries on a custom bus (two with Time=now-1h, one with Time=now-2h), observes all three delivered LIVE with no 'replay-name' key, drains the queue, starts a replay over a ten-minute band around the 1 h mark with FilterArns=[the rule arn] — and no redelivery is ever observed on the rule's SQS queue; the check is cut off at the harness budget (`Error: Check timed out after 60.0s`) before it can grade the window selection or the stamp.
Why: archive + replay is EventBridge's documented backfill/recovery story. A consumer that asks for a bounded backfill gets an accepted StartReplay and then silence, so a failed recovery is indistinguishable from a successful one; and because 'replay-name' is never seen, the documented opt-out that stops a consumer re-processing history ('replay-name': [{'exists': false}]) cannot be relied on.
Fix: the replay re-emission path in the eventbridge service crate — the StartReplay handler's window selection over the archive, the replay progression/settling that drives DescribeReplay to COMPLETED, the restriction of candidate rules to Destination.FilterArns, the attachment of 'replay-name' to the envelope BEFORE pattern evaluation, and the rule→SQS target delivery branch each re-emitted event is handed to.
Traces: the check above.

## Evidence
Expected (AWS): after one PutEvents of three entries on a custom bus with an ENABLED archive (two entries Time=now-1h, one Time=now-2h, FailedEntryCount 0), all three delivered live with no 'replay-name' key; then start_replay(EventStartTime=now-1h-5m, EventEndTime=now-1h+5m, Destination={'Arn': <bus arn>, 'FilterArns': [<rule arn>]}) reaches DescribeReplay State='COMPLETED' and the rule's SQS queue receives EXACTLY the two in-window events, each envelope carrying replay-name == the ReplayName, detail equal to what was archived, source and detail-type unchanged, with no third (out-of-window) event and no stranger messages.
Actual (emulator):
  [mismatch] events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window
  Error: Check timed out after 60.0s
What the timeout does and does not establish: the harness caps this check at 60 s while its own poll budgets are far longer (300 s archive-settle, 180 s live-delivery drain, 1500 s archive-ingest, 400 s replay-settle), so the cap does not localise the stall to one call; what it establishes is that no replay redelivery reaches the target queue on this emulator. Generated bus/rule/archive/replay names, the nonces, the account id and the timestamps are per-run values and are not part of the requirement; the COMPLETED settle, the window selection, the confinement to FilterArns and the presence of 'replay-name' are.

## Steps to Reproduce
1. events.create_event_bus(Name=B); create an SQS queue Q and events.put_rule on B with EventPattern {"source":[S]} and Q as its only target; drive warm-up PutEvents until Q delivers twice, to settle the delivery path before the archive exists.
2. events.create_archive(ArchiveName=A, EventSourceArn=<B's arn>); poll describe_archive until State='ENABLED'; drain Q.
3. One events.put_events with three entries on B: two with Time=now-1h, one with Time=now-2h, all Source=S, DetailType='Replayable', distinct nonces in Detail; assert FailedEntryCount==0; drain the three live deliveries and confirm none carries a 'replay-name' key.
4. Poll describe_archive until EventCount>=3 and SizeBytes>0; drain Q.
5. events.start_replay(ReplayName=R, EventSourceArn=<A's arn>, EventStartTime=now-1h-5m, EventEndTime=now-1h+5m, Destination={'Arn': <B's arn>, 'FilterArns': [<rule arn>]}); poll describe_replay(R) for a terminal state -> expect 'COMPLETED'.
6. Read Q: expect exactly the two in-window nonces, each envelope carrying replay-name == R and the archived detail/source/detail-type. Observed: nothing arrives and the check times out.
Replay the check id in trace_ids to confirm a fix.
