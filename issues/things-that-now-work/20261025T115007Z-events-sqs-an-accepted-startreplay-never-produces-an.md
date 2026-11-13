Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window
Issue-Key: events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window|_collect#40|Timeout|resp = sqs.receive_message(
Issue-Commit: 1f7e5adccf9e411825240a72188af4d50262df19

# events__sqs: an accepted StartReplay never produces an observable redelivery of the in-window archived events to the destination bus's rule

> _Note: this issue was identified against the code base at commit `1f7e5adccf9e411825240a72188af4d50262df19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplay

## Description
Cause 1 of 1: a replay started on an ENABLED archive never results in the in-window archived events being redelivered to the destination bus's rule target, so the window selection, the FilterArns scoping and the `replay-name` stamp are all unobservable.

Context on what is already filed: the fixed issue 20261025T081516Z-events-sqs-eventbridge-archive-replay-is-a-501-scaffold-stub.md fixed StartReplay's 501 stub (the handler is now implemented at services/service-com-amazonaws-eventbridge-awsevents/src/ops/start_replay.rs), and the open issue 20261025T084539Z-events-sqs-archive-replay-is-unreachable-startreplay-is.md still describes this check as blocked by that stub. Neither describes the state this replay now reports: the call is accepted, and the redelivery is still never seen. The check is NOT marked OWNED or ALREADY FILED in the replay report, so it is filed here against its current signature.

AWS: Smithy (com.amazonaws.eventbridge#StartReplay) models required {ReplayName, EventSourceArn (ArchiveArn), EventStartTime, EventEndTime, Destination (ReplayDestination{Arn, FilterArns})} and output {ReplayArn, State, StateReason, ReplayStartTime}; DescribeReplay reports {State, EventLastReplayedTime, ReplayStartTime, ReplayEndTime}. The modeled documentation states "A replay processes events to replay based on the time in the event, and replays them using 1 minute intervals" and "You can use DescribeReplay to determine the progress of a replay". So: the replay settles at State=COMPLETED; EXACTLY the archived events whose own `time` falls inside [EventStartTime, EventEndTime] are re-emitted onto Destination.Arn (an archived event whose time is outside the window is NOT re-emitted); routing is confined to the rules named in Destination.FilterArns; each redelivered envelope carries a `replay-name` field equal to the ReplayName (which is what distinguishes a redelivery from a leftover live delivery) and otherwise carries the archived `detail`, `source` and `detail-type` unchanged.
Emulator: `Check timed out after 60.0s` — the check never reaches its redelivery assertions. Nothing is observed coming back out of the SQS target after the replay, so the three-event window selection (two in, one out) cannot be graded, and no envelope carrying `replay-name` is ever seen.
Why: archive + replay is EventBridge's documented recovery/backfill story. A consumer that requests a backfill gets an accepted StartReplay and then silently receives nothing, so a failed recovery is indistinguishable from a successful one; and because the `replay-name` stamp is never observed, the opt-out that stops a consumer re-processing history (`replay-name: [{exists: false}]`) cannot be relied on.
Fix: the replay re-emission path in the eventbridge crate — the StartReplay handler's selection/routing loop and the replay settling in crate::state (settle_replays), plus the rule->SQS target delivery branch it hands each replayed event to (crate::ops::put_events::route / deliver). The replay must drive DescribeReplay to COMPLETED and the redelivered envelope must carry `replay-name` AT MATCHING time, not after routing.
Traces: events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window.

## Evidence
events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window [mismatch]: `Error: Check timed out after 60.0s`.

Expected: one PutEvents of three entries (two with Time = now-1h, one with Time = now-2h) onto a custom bus with an ENABLED archive; after start_replay over a ten-minute band around the 1 h mark with Destination={'Arn': <bus arn>, 'FilterArns': [<rule arn>]}, describe_replay reaches State='COMPLETED' and the rule's SQS queue receives EXACTLY the two in-window events, each envelope carrying replay-name=<ReplayName>, detail equal to the archived detail, source and detail-type unchanged, and no third (out-of-window) event and no stranger messages.
Actual: no redelivery is observed at all; the check does not complete.

What the timeout does and does not establish: the harness caps each check at 60 s, and this check's own documented poll budgets are longer (300 s archive-settle, 180 s live-delivery drain, 1500 s archive-ingest, 400 s replay-settle), so the cap alone does not localise the stall to one call. What it does establish is that on this emulator no replay redelivery reaches the target queue inside the cap, whereas against real AWS the live-delivery and replay legs this check asserts on are the only thing standing between it and completion. StartReplay itself is no longer refused (the sibling events.start-replay-* checks now get real validation errors from it rather than NotImplemented), so the remaining gap is the re-emission/delivery leg, not the accept.

## Steps to Reproduce
1. events.create_event_bus(Name=B); create an SQS queue Q and events.put_rule on B with pattern {"source":[S]} and Q as its only target; drive a couple of warm-up PutEvents until Q delivers, to settle the delivery path BEFORE the archive exists.
2. events.create_archive(ArchiveName=A, EventSourceArn=<B's arn>); poll describe_archive until State='ENABLED'; drain Q.
3. One events.put_events call with three entries on B: two with Time=now-1h and one with Time=now-2h, all Source=S, DetailType='Replayable', distinct nonces in Detail. Assert FailedEntryCount==0.
4. Drain the three LIVE deliveries from Q and assert none carries a `replay-name` field.
5. Poll describe_archive until EventCount>=3 and SizeBytes>0; drain Q again.
6. events.start_replay(ReplayName=R, EventSourceArn=<A's arn>, EventStartTime=now-1h-5m, EventEndTime=now-1h+5m, Destination={'Arn': <B's arn>, 'FilterArns': [<rule arn>]}).
7. Poll describe_replay(R) for a terminal state -> expect 'COMPLETED'; then read Q -> expect exactly the two in-window nonces, each envelope carrying replay-name=R. Observed: nothing comes back and the check times out.

Replay events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window to confirm.
