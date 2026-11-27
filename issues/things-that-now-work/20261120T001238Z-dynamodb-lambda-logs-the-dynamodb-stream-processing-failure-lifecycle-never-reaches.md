Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle
Issue-Key: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle|_reap_log_group#10|Timeout|return
Issue-Commit: 7ddb4188dd9af5c3d292129d1524d0998b9f045b

# The DynamoDB-stream processing-failure lifecycle never reaches a verdict: the check exhausts its whole budget waiting on the stream event source mapping

> _Resolution: the cause as REPORTED — the hypothesis that "a DynamoDB-stream event source
> mapping created with StartingPosition LATEST never starts delivering records, so the check's
> arming loop ... never observes a delivery" — is not present at this commit, measured on the
> ASSEMBLED emulator binary over its own signed HTTPS wire: a key written straight after the
> create, while the mapping still reports `Creating`, is handed to the handler inside a second;
> the whole declared estate (two streamed tables, the role, the function, two LATEST mappings)
> settles `CREATE_COMPLETE` in 3.6 s and is destroyed to a terminal status 6.2 s after
> `DeleteStack`, so deploy + arm both streams + destroy costs 10.5 s in all, with 3.3 ms per
> API call and a 0.6 s boot. Each span inside that total is one the project's own
> transition rule requires (a created table reaching `ACTIVE`, a created function reaching
> `Active`, a deleted mapping and table leaving their non-terminal states), one per dependency
> level, so there is nothing in it to shorten. `core/router/tests/all/`
> `ddb_stream_lambda_processing_failure_lifecycle.rs` now pins what the report says AWS does for
> the beats no neighbouring suite covered: a batch the handler cannot process is handed over the
> first time plus exactly the retries its mapping's `MaximumRetryAttempts` allows (none, one,
> three) and no more, the record written BEHIND it is then handed over, and the estate is
> destroyed to `DELETE_COMPLETE` with the mapping and the function gone from Lambda and the table
> from DynamoDB — whether nothing, a processed record or a given-up record ran through it. What
> remains of the recorded failure is the harness's own WALL CLOCK: this six-phase scenario, whose
> client-side poll helper sleeps 3 s between probes and 20 s between arming writes, does not
> finish inside the 60 s cap, and this report itself records the whole lifecycle as "recorded
> behavior only" with no AWS-published latency for any step — so no AWS rule fixes it, and the
> replay of this check keeps timing out until a later change makes the whole scenario fit the cap.
> Two earlier reports of the same replay key reached the same conclusion from the other end
> (`20261117T213136Z-dynamodb-lambda-logs-the-two-stream-onyx-stream.md`)._

> _Note: this issue was identified against the code base at commit `7ddb4188dd9af5c3d292129d1524d0998b9f045b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the deploy/diagnose lifecycle over a DynamoDB-stream-triggered function never completes — the replay spends its entire budget inside the check and is killed, so no beat of the scenario is graded.

AWS: recorded behavior only — the suite's recording of this scenario completes within the budget. The scenario deploys a stack with two DynamoDB tables with streams, an execution role and a function, creates stream event source mappings whose StartingPosition is LATEST, waits for a mapping to begin reading at the shard tip (its own arming poll, re-writing a fresh key each round), then re-probes the handler's behaviour on NewImage/OldImage. Against AWS every one of those steps settles in seconds to a minute; no step is documented to block.

Emulator: the replay reports only "Check timed out after 60.0s" — the check never returned, so no assertion in it was ever reached and no status/error/response can be quoted. Hypothesis: a DynamoDB-stream event source mapping created with StartingPosition LATEST never starts delivering records, so the check's arming loop (which re-writes a new key every 20 s and waits for the handler to observe it) never observes a delivery and simply spins until the budget ends.

Why: a client that wires a DynamoDB stream to a function and waits for the first delivery before proceeding hangs indefinitely instead of seeing records; every downstream diagnosis of the handler's OldImage/NewImage behaviour is unreachable.

Fix: the DynamoDB-stream event source mapping poller (its arming/StartingPosition=LATEST handling and record delivery), and whatever step of the stack deploy/record-write path keeps the scenario from progressing.

Traces: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle. This issue is grounded only in that timeout; the failing check is the whole of the evidence, and a fixer should re-replay it to see which beat blocks.

## Evidence
Error: Check timed out after 60.0s

Expected (AWS): the lifecycle runs its deploy, setup, pre_invoke, solve, cleanup and destroy phases to a verdict; the stream mapping arms and the handler observes a written key.
Actual (emulator): the check produced no assertion at all before the replay budget expired — the only recorded datum is the timeout. The check's own internal deadlines (600 s for stack create/delete, 420 s for mapping arming) are never reached.

## Steps to Reproduce
1. Deploy the onyx-stream-handler stack (two DynamoDB tables with streams, an IAM role, a function, two stream event source mappings).
2. Create/confirm a stream event source mapping with StartingPosition LATEST and poll for the first delivery by writing a fresh item key every 20 s and watching the function's log group.
3. Observe that no delivery is ever observed and the scenario never progresses past its arming loop.
