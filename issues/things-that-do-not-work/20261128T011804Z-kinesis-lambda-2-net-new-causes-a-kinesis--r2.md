Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda.kinesis-esm-splits-a-backlog-larger-than-six-mb-into-several-invokes kinesis__lambda.kinesis-esm-counts-the-base64-encoded-size-against-the-six-mib-invoke-limit kinesis__lambda.kinesis-esm-exists-filter-passes-only-records-carrying-the-leaf
Issue-Key: kinesis__lambda.kinesis-esm-splits-a-backlog-larger-than-six-mb-into-several-invokes|_poll#8|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting until {what}; last={last!r}")
Issue-Key: kinesis__lambda.kinesis-esm-counts-the-base64-encoded-size-against-the-six-mib-invoke-limit|wait_for#14|Timeout|time.sleep(5)
Issue-Key: kinesis__lambda.kinesis-esm-exists-filter-passes-only-records-carrying-the-leaf|run#31|AssertionError|assert seen == _EXPECTED, (
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# kinesis__lambda: 2 net-new causes — a Kinesis event-source mapping never invokes the function for ~900 KB records, and an {"exists": true} filter matches an OBJECT-valued field

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two independent root causes in Lambda's Kinesis event-source-mapping poller, both grounded in failing behavioral checks. Neither check is marked ALREADY FILED or OWNED in the report.

=== Cause 1 of 2: a backlog of large records is never delivered — the mapping makes no invocation at all ===
AWS: recorded behavior only (the 6 MB synchronous invoke payload limit is a runtime property of the poller, not a modelled constraint; CreateEventSourceMapping models BatchSize but nothing about payload size). Recorded behavior: with BatchSize=100 and StartingPosition=TRIM_HORIZON over eight ~900 KB records (~7.2 MB, and more once base64-encoded), the mapping reaches Enabled and every one of the eight records reaches the function, split across MORE THAN ONE invocation, because a batch is never allowed to exceed the synchronous payload limit; the companion check additionally grades that the BASE64-encoded size is what counts against that limit.
Emulator: no invocation happens at all. The poller delivers nothing for the whole polling window — the function's log shows zero batches after 43s of polling (the check's own budget), and the base64-size companion check reaches no verdict inside its 60s budget for the same reason. Smaller-record Kinesis mappings in the same suite do deliver, so the mapping itself is created and Enabled; it is the large-record batch path that produces no invoke.
Why: any consumer of multi-hundred-KB Kinesis records (the common aggregated/compressed payload case) silently processes nothing against the emulator — no error, no DLQ, no log — and a client cannot distinguish "not yet" from "never".
Fix: the batch-assembly step of the Kinesis poller in the event-source-mapping component — bound a batch by the base64-encoded payload size and emit as many invocations as the backlog needs, instead of producing no invocation when the first batch exceeds the limit.
Traces: kinesis__lambda.kinesis-esm-splits-a-backlog-larger-than-six-mb-into-several-invokes, kinesis__lambda.kinesis-esm-counts-the-base64-encoded-size-against-the-six-mib-invoke-limit.

=== Cause 2 of 2: an {"exists": true} filter matches a field whose value is an OBJECT ===
AWS: recorded behavior only (FilterCriteria.Filters[].Pattern is an opaque string in the model; the rule is AWS' documented event-filtering semantics, https://docs.aws.amazon.com/lambda/latest/dg/invocation-eventfiltering.html). Recorded behavior: with the pattern {"data": {"owner": [{"exists": true}]}} on a Kinesis mapping, a record is passed to the function only when the field is present as a LEAF value — falsy leaves included (a string, and 0, both match) — while a record whose field is ABSENT and a record whose field holds an intermediate OBJECT (e.g. {"owner": {"name": "team-a"}}) are both dropped and never invoke the function.
Emulator: the object-valued record is passed through: the function saw the object-valued record in addition to the three leaf records. Absent fields are dropped correctly, so only the intermediate-object case is mis-evaluated.
Why: filtering is the mapping's only way to keep records away from the function; a leaked record means the function is invoked (and billed, and may fail) for payloads AWS filters out, and a client's filter-based routing delivers to the wrong consumer.
Fix: the exists operator in the event-filtering pattern matcher used by the ESM poller — exists:true must match only non-object leaf values at the pattern's path.
Traces: kinesis__lambda.kinesis-esm-exists-filter-passes-only-records-carrying-the-leaf.

## Evidence
Cause 1 — kinesis__lambda.kinesis-esm-splits-a-backlog-larger-than-six-mb-into-several-invokes: after writing eight ~900 KB records to a 1-shard stream and creating the mapping (BatchSize=100, TRIM_HORIZON), the check fails with "timed out after 43.40491979400031s waiting until all eight records reach the function; last=[]" — zero batches logged, so neither the "all eight delivered" nor the ">= 2 invocations" assertion was reachable. kinesis__lambda.kinesis-esm-counts-the-base64-encoded-size-against-the-six-mib-invoke-limit fails with "Check timed out after 60.0s" on the same large-record delivery path (no verdict reached).

Cause 2 — kinesis__lambda.kinesis-esm-exists-filter-passes-only-records-carrying-the-leaf: records written in order were {owner:"team-a"} (hit-string), {team:"team-a"} (miss-absent), {owner:{name:"team-a"}} (miss-intermediate), {owner:0} (hit-zero), {owner:"team-b"} (hit-last) under Pattern {"data": {"owner": [{"exists": true}]}} (GetEventSourceMapping echoed the pattern unchanged). The function saw ['hit-last', 'hit-string', 'hit-zero', 'miss-intermediate']; expected exactly ['hit-last', 'hit-string', 'hit-zero'] (leaked: ['miss-intermediate']).

## Steps to Reproduce
1. Cause 1: create a 1-shard PROVISIONED stream, PutRecord eight records of ~900 KB each, create a Lambda function plus an execution role with the kinesis read actions, then create_event_source_mapping(EventSourceArn=<stream>, FunctionName=<fn>, StartingPosition='TRIM_HORIZON', BatchSize=100); wait for State=Enabled and poll the function's output — expect all eight records delivered across >= 2 invocations, observe no invocation at all.
2. Cause 2: create a source stream and a report stream, a function that reports the labels of the records it receives, and create_event_source_mapping(..., BatchSize=10, StartingPosition='TRIM_HORIZON', FilterCriteria={'Filters': [{'Pattern': '{"data": {"owner": [{"exists": true}]}}'}]}); PutRecord the five bodies above in order and read what the function reports — expect only the three leaf-valued records, observe the {"owner": {...}} record leaking through.
3. Re-replay every check id in trace_ids.
