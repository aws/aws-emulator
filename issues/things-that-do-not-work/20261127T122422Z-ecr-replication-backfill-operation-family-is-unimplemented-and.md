Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-replication-backfill-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-replication-backfill-lifecycle
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|StartReplicationBackfill|2xx->5xx|$.destination
Issue-Key: ecr|StartReplicationBackfill|2xx->5xx|$.registryId
Issue-Key: ecr|StartReplicationBackfill|2xx->5xx|$.replicationBackfillId
Issue-Key: ecr|StartReplicationBackfill|2xx->5xx|$.startedAt
Issue-Key: ecr|StartReplicationBackfill|2xx->5xx|$.status
Issue-Key: ecr|StartReplicationBackfill|2xx->5xx|status
Issue-Key: ecr|ListReplicationBackfills|2xx->5xx|$.backfills
Issue-Key: ecr|ListReplicationBackfills|2xx->5xx|$.registryId
Issue-Key: ecr|ListReplicationBackfills|2xx->5xx|status
Issue-Key: ecr|DescribeReplicationBackfillResults|4xx->5xx|$.__type
Issue-Key: ecr|DescribeReplicationBackfillResults|4xx->5xx|$.message
Issue-Key: ecr|DescribeReplicationBackfillResults|4xx->5xx|status
Issue-Key: ecr|StopReplicationBackfill|4xx->5xx|$.__type
Issue-Key: ecr|StopReplicationBackfill|4xx->5xx|$.message
Issue-Key: ecr|StopReplicationBackfill|4xx->5xx|status
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ecr replication-backfill operation family is unimplemented and answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplicationBackfill

## Description
ECR's replication-backfill control surface — start a backfill against a configured replication destination, list backfills, describe one backfill's results, stop one — is not served by the emulator at all. Every one of the four operations answers HTTP 501 with __type 'NotImplemented' and the emulator's own prose, including the two calls whose recorded outcome is a modelled 400 client error. These operations are NOT present in the checked-out Smithy model for ECR (smithy_query reports 'operation not found' for all four, suggesting only PutReplicationConfiguration / DescribeImageReplicationStatus), so their contract here is recorded behavior only and is taken from the recorded responses. The replay report shows an EMPTY stub_path for all four, so no unimplemented-operation issue has been filed for them. 4 net-new causes, one per operation in the family.

Cause 1 of 4: StartReplicationBackfill returns 501 instead of starting a backfill.
  AWS: recorded behavior only (operation absent from the on-disk ECR model). The recording shows HTTP 200 with a body carrying `registryId` (the caller's registry, the 12-digit account id), `destination` as an object of {region, registryId} echoing the replication destination the backfill targets, a server-minted `replicationBackfillId`, a `startedAt` timestamp and a `status`. The id, timestamp and the concrete region/account values are per-run; the required SHAPE is those five members, with destination echoing the requested destination and registryId being the registry the call was made against.
  Emulator: HTTP 501, body {__type: 'NotImplemented', message: 'operation StartReplicationBackfill is not implemented in this emulator'}; all five response members are absent.
  Why: a client cannot start a replication backfill at all. Because the response is 501 with a non-modelled error type rather than a modelled ECR exception, SDKs surface it as an unknown/server error instead of anything retryable or actionable, and nothing downstream can obtain the replicationBackfillId that the rest of the family keys on — the whole lifecycle is unreachable from its first call.
  Fix: register the operation in the ECR service's request dispatch and add its handler plus the backfill records it must keep in the service's state (a backfill needs an id, destination, status and timestamps that the list/describe/stop operations read back).
  Traces: ecr-replication-backfill-lifecycle#task#2.

Cause 2 of 4: ListReplicationBackfills returns 501 instead of listing the registry's backfills.
  AWS: recorded behavior only. The recording shows HTTP 200 with `registryId` and `backfills` — a list whose entries carry `destination` {region, registryId}, `replicationBackfillId`, `startedAt`, `endedAt`, `status` (the recorded value for a finished backfill is 'COMPLETE'), and a `progress` object of {totalReplicationCount, completedReplicationCount, failedReplicationCount, failureCodes}. The requirement is that a backfill started earlier in the session is listed back with the id it was given and a terminal status with populated progress counters; the specific counts and timestamps are per-run.
  Emulator: HTTP 501, body {__type: 'NotImplemented', message: 'operation ListReplicationBackfills is not implemented in this emulator'}; `backfills` and `registryId` absent.
  Why: a client has no way to enumerate or poll backfills, so it can neither discover an in-flight backfill nor observe one reaching a terminal status — any workflow that waits for a backfill to complete hangs or fails.
  Fix: the same dispatch registration plus a handler that projects the backfill records held in the ECR service's state, including the per-backfill progress counters.
  Traces: ecr-replication-backfill-lifecycle#task#4.

Cause 3 of 4: DescribeReplicationBackfillResults answers 501 NotImplemented where an unknown backfill id must be refused with a modelled 400 ReplicationBackfillNotFoundException.
  AWS: recorded behavior only. The recording shows HTTP 400 with __type 'ReplicationBackfillNotFoundException' and message "The replication backfill with id '<replicationBackfillId>' does not exist in the registry with id '<registryId>'". Fixed text: "The replication backfill with id '", "' does not exist in the registry with id '" and the closing "'". Variable parts: the backfill id taken from the REQUEST and the registry id taken from the caller's registry in the emulator's state — the recorded run's all-zero id and 000000000000 account are that run's values filling the template, not part of it.
  Emulator: HTTP 501 with __type 'NotImplemented' and message 'operation DescribeReplicationBackfillResults is not implemented in this emulator' — wrong status class (5xx server fault instead of 4xx client error), wrong error code, and emulator-authored prose.
  Why: a client cannot read a backfill's results, and the negative path is misreported as a server fault: SDK retry policies retry a 501-shaped failure and error handlers keyed on ReplicationBackfillNotFoundException never fire, so "this backfill does not exist" is indistinguishable from "the service is broken".
  Fix: the same dispatch registration plus a handler whose lookup miss raises the modelled ReplicationBackfillNotFoundException at 400, with the message template above interpolated from the request's id and the registry in state.
  Traces: ecr-replication-backfill-lifecycle#task#6.

Cause 4 of 4: StopReplicationBackfill answers 501 NotImplemented where an unknown backfill id must be refused with a modelled 400 ReplicationBackfillNotFoundException.
  AWS: recorded behavior only. The recording shows the same refusal as cause 3 — HTTP 400, __type 'ReplicationBackfillNotFoundException', message "The replication backfill with id '<replicationBackfillId>' does not exist in the registry with id '<registryId>'" with the same fixed text and the same two request/state-derived values.
  Emulator: HTTP 501, {__type: 'NotImplemented', message: 'operation StopReplicationBackfill is not implemented in this emulator'}.
  Why: a backfill cannot be cancelled, and as in cause 3 the not-found path is reported as a server fault, so a client cannot tell a bad id from an outage and will retry a request that can never succeed.
  Fix: the same dispatch registration plus a handler that transitions a known backfill to its stopped state and raises the modelled ReplicationBackfillNotFoundException at 400 for an id absent from the registry's backfill records.
  Traces: ecr-replication-backfill-lifecycle#task#7.

No open issue covers this family — a keyword search for 'backfill' over the open issues returns nothing, and the open ECR replication-adjacent issues concern repositoryUri hosts and pull-through-cache rules, not replication backfills.

## Evidence
All four causes come from fixture ecr-replication-backfill-lifecycle; each is its own divergence cluster (affected_count 1), stub_path empty and owned_by empty for all four.

Cause 1 — StartReplicationBackfill, step task#2:
  status: expected 200, got 501
  $.registryId: missing (expected '000000000000')
  $.destination: missing (expected {'region': 'us-east-1', 'registryId': '000000000000'})
  $.replicationBackfillId: missing (expected a shaped, non-asserted id)
  $.startedAt: missing (expected a shaped, non-asserted timestamp)
  $.status: missing (expected a shaped, non-asserted status)

Cause 2 — ListReplicationBackfills, step task#4:
  status: expected 200, got 501
  $.registryId: missing (expected '000000000000')
  $.backfills: missing (expected [{'destination': {'region': 'us-east-1', 'registryId': '000000000000'}, 'replicationBackfillId': <the id minted at task#2>, 'startedAt': shaped, 'endedAt': shaped, 'status': 'COMPLETE', 'progress': {'totalReplicationCount': 0, 'completedReplicationCount': 0, 'failedReplicationCount': 0, 'failureCodes': []}}])

Cause 3 — DescribeReplicationBackfillResults, step task#6:
  status: expected 400, got 501
  $.__type: expected 'ReplicationBackfillNotFoundException', got 'NotImplemented'
  $.message: expected "The replication backfill with id '<id>' does not exist in the registry with id '<registryId>'" (recorded run: id '00000000-0000-0000-0000-000000000000', registry '000000000000'), got 'operation DescribeReplicationBackfillResults is not implemented in this emulator'

Cause 4 — StopReplicationBackfill, step task#7:
  status: expected 400, got 501
  $.__type: expected 'ReplicationBackfillNotFoundException', got 'NotImplemented'
  $.message: expected "The replication backfill with id '<id>' does not exist in the registry with id '<registryId>'" (same template, same recorded values), got 'operation StopReplicationBackfill is not implemented in this emulator'

Model evidence: smithy_query query=operation against services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json reports "operation 'StartReplicationBackfill' not found", and likewise for ListReplicationBackfills, DescribeReplicationBackfillResults and StopReplicationBackfill — hence 'recorded behavior only' for all four contracts.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus (replay_wire_corpus).
2. Replay fixture ecr-replication-backfill-lifecycle. It configures ECR replication to a destination registry/region, then exercises the backfill lifecycle.
3. Cause 1 — step task#2 calls StartReplicationBackfill for that destination. Observe HTTP 501 with __type 'NotImplemented' instead of 200 carrying registryId, destination, replicationBackfillId, startedAt and status.
4. Cause 2 — step task#4 calls ListReplicationBackfills for the registry. Observe HTTP 501 instead of 200 carrying registryId and a backfills list containing the backfill started at task#2 with its progress counters and terminal status.
5. Cause 3 — step task#6 calls DescribeReplicationBackfillResults with a backfill id that does not exist in the registry. Observe HTTP 501 / __type 'NotImplemented' instead of HTTP 400 / __type 'ReplicationBackfillNotFoundException' whose message names the requested id and the registry id.
6. Cause 4 — step task#7 calls StopReplicationBackfill with the same nonexistent id. Observe the same 501 instead of the same modelled 400 refusal.
7. Re-replay ecr-replication-backfill-lifecycle after the fix; all four steps (task#2, task#4, task#6, task#7) must pass before this issue is closed.
