Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.channel-lifecycle kafka.channel-duplicate-name-conflict kafka.channel-delete-nonexistent-rejected kafka.cluster-delete-and-broker-ops-rejected-while-creating kafka.reboot-broker-server-orchestrates-without-full-outage kafka.reboot-broker-state-transition
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.channel-lifecycle|run#16|AssertionError|assert observed == expected, f"observed={observed}"
Issue-Key: kafka.channel-duplicate-name-conflict|run#15|AssertionError|assert del_code == _NOT_FOUND, f"DeleteChannel: expected NotFoundException, got {del_code}"
Issue-Key: kafka.channel-delete-nonexistent-rejected|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: kafka.cluster-delete-and-broker-ops-rejected-while-creating|run#6|AssertionError|assert refusals == expected, f"expected refusals {expected}, got {refusals}"
Issue-Key: kafka.reboot-broker-server-orchestrates-without-full-outage|run#14|AssertionError|assert topics_calls_ok >= 3, (
Issue-Key: kafka.reboot-broker-state-transition|_wait_operation_state#9|Timeout|time.sleep(interval_s)
Issue-Commit: 347b4a2ff1d577fa6445525d6d30457661a1c2dd

# MSK channel refusals invert NotFound/BadRequest, two cluster-state guard messages are reworded, and the RebootBroker lifecycle is unobservable (6 causes)

> _Note: this issue was identified against the code base at commit `347b4a2ff1d577fa6445525d6d30457661a1c2dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChannel

## Description
SIX distinct root causes in the kafka (MSK) service, all grounded in behavioral checks that the suite did not mark ALREADY FILED or OWNED. All six must be closed.

Cause 1 of 6 — DescribeChannel and DeleteChannel answer their own channel-ARN syntax 400 instead of resolving the cluster first.
AWS: Smithy kafka#DescribeChannel (GET /v1/clusters/{ClusterArn}/channels/{ChannelArn}) and kafka#DeleteChannel (DELETE /v1/clusters/{ClusterArn}/channels/{ChannelArn}) both model BadRequestException (400) and NotFoundException (404); both take ClusterArn and ChannelArn as @httpLabel members. Recorded behavior: when the ClusterArn is well formed but names no cluster, EVERY channel operation — CreateChannel, DescribeChannel, UpdateChannel, DeleteChannel, ListChannels — is refused identically with Error.Code 'NotFoundException' and Error.Message "The requested resource doesn\u2019t exist." (entirely fixed text, U+2019 apostrophe, no variable part). Cluster resolution is the FIRST gate; the shape of the ChannelArn is not consulted when the cluster does not exist.
Emulator: CreateChannel and ListChannels do answer NotFoundException, but DescribeChannel and DeleteChannel answer Error.Code 'BadRequestException' with a message of its own invention whose fixed text is " is not a valid Amazon MSK channel ARN." prefixed by the request's ChannelArn (one replay showed the ARN of that run followed by that sentence) — i.e. a per-handler ARN-shape validator runs ahead of the cluster lookup, so one contract is split into two codes depending on which channel operation is called.
Why: a client that treats 404 as 'the cluster/channel is gone, stop retrying and clean up' and 400 as 'my request is malformed, fix the code' is routed to the wrong remedy, and the same ghost cluster yields two different codes across the five channel operations, so no uniform error handling can be written.
Fix: in the kafka service's channel handlers (services/service-com-amazonaws-kafka-kafka/src/ops/describe_channel.rs and delete_channel.rs) resolve the path ClusterArn before parsing/validating ChannelArn, and return the shared fixed NotFoundException when the cluster does not exist — ideally via the one resolver CreateChannel/ListChannels already use. Related but not covering: 20261029T175822Z-kafka-msk-arn-handling-malformed-and-cross-region-arns.md describes this gate-order defect for MALFORMED cluster ARNs on other operations; here the cluster ARN is well formed and these check ids are not in that filing.
Traces: kafka.channel-lifecycle (observed {'CreateChannel': 'NotFoundException', 'DescribeChannel': 'BadRequestException', 'UpdateChannel': 'NotImplemented', 'DeleteChannel': 'BadRequestException', 'ListChannels': 'NotFoundException'} where all five must be NotFoundException), kafka.channel-duplicate-name-conflict (DeleteChannel leg).

Cause 2 of 6 — DeleteChannel on a REAL cluster with a never-created channel answers NotFound instead of the link-ARN BadRequest.
AWS: same modeled errors as Cause 1. Recorded behavior: once the ClusterArn resolves to an existing (ACTIVE, Express-broker) cluster, a well-formed ChannelArn that was never created is NOT a not-found; it is refused with Error.Code 'BadRequestException' and Error.Message "The specified link ARN isn\u2019t valid. Specify a valid link ARN, and try again." — entirely fixed text (U+2019 apostrophe), no value from the request interpolated. So the two gates are ordered: cluster missing -> NotFoundException (Cause 1), cluster present but channel unknown -> BadRequestException with this message.
Emulator: Error.Code 'NotFoundException' — the exact inverse of Cause 1's defect, confirming the two gates are swapped rather than merely mistyped. ListChannels on the cluster is empty before and after, so only the refusal diverges.
Why: a client deleting a channel cannot distinguish 'the cluster is gone' (give up) from 'this link ARN is not one of this cluster's channels' (a bad identifier in its own request); idempotent-delete logic keyed on 404 will silently treat a bad ARN as already-deleted.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/delete_channel.rs — after the cluster resolves, an unknown-but-well-formed ChannelArn must produce BadRequestException with the fixed link-ARN message, not NotFoundException.
Traces: kafka.channel-delete-nonexistent-rejected.

Cause 3 of 6 — ListNodes' CREATING-state refusal message is reworded.
AWS: Smithy kafka#ListNodes (GET /v1/clusters/{ClusterArn}/nodes) models BadRequestException (400). Recorded behavior for a cluster still in CREATING: HTTP 400, Error.Code 'BadRequestException', Error.Message "You can't list the nodes for a cluster that is in the CREATING state." The only variable part is the state name, taken from the cluster's current state; the rest ("You can't list the nodes for a cluster that is in the " ... " state.") is fixed text.
Emulator: the same code but Error.Message "You can't list nodes for a cluster in CREATING state." — a paraphrase ('list nodes' for 'list the nodes', dropping 'that is in the').
Why: the message is part of the contract; a client or test asserting on, logging, or pattern-matching MSK's state-guard text does not match, and operators comparing emulator output to AWS see a different string for the same refusal.
Fix: the state-guard message in services/service-com-amazonaws-kafka-kafka/src/ops/list_nodes.rs (or the shared cluster-state guard it calls) must emit AWS's fixed wording with the current state interpolated.
Traces: kafka.cluster-delete-and-broker-ops-rejected-while-creating.

Cause 4 of 6 — RebootBroker's state guard emits a per-operation sentence where MSK emits the generic allowed-states sentence.
AWS: Smithy kafka#RebootBroker (PUT /v1/clusters/{ClusterArn}/reboot-broker) models BadRequestException (400). Recorded behavior for a cluster in CREATING: HTTP 400, Error.Code 'BadRequestException', Error.Message 'This operation is only valid for resources that are in one of the following states :[ACTIVE]' — fixed text, with the bracketed list naming the states the operation permits (here exactly ACTIVE); note the space before the colon is AWS's own.
Emulator: Error.Message "You can't reboot brokers for a cluster in CREATING state." — a wholly different sentence modelled on the other state guards, so RebootBroker is wrongly made to look like DeleteCluster/ListNodes-style refusals. (DeleteCluster's and GetBootstrapBrokers' messages in the same replay matched AWS, so this is specific to RebootBroker's guard.)
Why: same as Cause 3, and worse here because the AWS message is the generic allowed-states form a client parses to learn WHICH states would be acceptable; the emulator's text carries no such list.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/reboot_broker.rs — use the allowed-states refusal form with the permitted state set, not a hand-written per-operation sentence.
Traces: kafka.cluster-delete-and-broker-ops-rejected-while-creating.

Cause 5 of 6 — the ClusterOperation a RebootBroker starts is not observably in progress.
AWS: Smithy kafka#RebootBroker returns ClusterOperationArn, polled with kafka#DescribeClusterOperation (GET /v1/operations/{ClusterOperationArn}) whose ClusterOperationInfo.OperationState walks REBOOT_IN_PROGRESS -> REBOOT_COMPLETE. Recorded behavior: a reboot of one broker of a multi-broker cluster is a rolling operation that stays non-terminal for a sustained window — the cluster keeps serving, ListNodes keeps reporting the full broker inventory and ListTopics keeps answering throughout, so a client polling at a few seconds' cadence observes the in-progress state on several consecutive samples.
Emulator: the operation is terminal on the second sample of a 2 s-cadence poll loop — the whole reboot window spans fewer than 3 samples, so 'the cluster stays reachable throughout the reboot' cannot be observed at all (AssertionError: "ListTopics was only exercised 2 time(s) across the reboot window"). Hypothesis: the operation is promoted to its terminal state on the first read after the write rather than after any elapsed work, so its duration is one poll interval by construction.
Why: a client that drains or rebalances around a rolling broker reboot, or a test that asserts no full outage, has no window in which to act or observe; emulator-backed rehearsals of reboot procedures prove nothing.
Fix: the cluster-operation lifecycle in the kafka service's state layer (the RebootBroker path in services/service-com-amazonaws-kafka-kafka/src/ops/reboot_broker.rs plus the operation-state progression it registers) must keep the operation non-terminal for a wall-clock window that outlasts several client polls, with the broker inventory intact and the data-plane-facing reads answering during it.
Traces: kafka.reboot-broker-server-orchestrates-without-full-outage.

Cause 6 of 6 — the RebootBroker lifecycle sequence never reaches a verdict.
AWS: the sequence a client runs is CreateCluster (PROVISIONED, 2 brokers) -> poll DescribeClusterV2 until ACTIVE -> ListNodes -> RebootBroker -> poll DescribeClusterOperation until REBOOT_COMPLETE -> poll DescribeClusterV2 until ACTIVE again. Each transition is genuinely asynchronous and each is reported truthfully, so a poller at an AWS-appropriate cadence (20 s for the cluster, 15 s for the operation) completes the sequence in as many intervals as there are real transitions.
Emulator: the check reports only 'Check timed out after 60.0s' with no frame, no HTTP status and no error code — nothing was refused and no assertion was evaluated. Hypothesis (not stated by the replay output): the same clock-owed settling already recorded for other MSK transitions applies to this path too, so the FIRST probe after each write always observes the non-terminal value and the caller is charged a full poll interval per transition, exhausting the ceiling before RebootBroker's terminal state is ever read. Note this is the same underlying settling model as Cause 5 seen from the other side: with a 2 s cadence the window is too short to observe, with a 15-20 s cadence it costs a full interval per step.
Why: the whole RebootBroker contract (the operation walking to REBOOT_COMPLETE rather than UPDATE_COMPLETE, and the cluster returning to ACTIVE) is unverifiable against the emulator, and real clients polling at AWS cadences pay waits for work the emulator had already finished.
Fix: make these transitions settle on the write path / complete-on-return in the kafka state layer so a first probe reflects reality, while honouring Cause 5's requirement that the reboot operation have a real observable window; then re-replay. Related but not covering: 20261104T171616Z-kafka-msk-asynchronous-transitions-are-clock-owed-so-an.md records this symptom for 14 other checks and does not carry kafka.reboot-broker-state-transition.
Traces: kafka.reboot-broker-state-transition.

## Evidence
Suite simparity-behavioral-aws-v1, kafka 105/215 passed. Every failure below is verdict 'mismatch' and is NOT marked ALREADY FILED or OWNED by the report.

Cause 1 — kafka.channel-lifecycle:
  ListChannels on a real ACTIVE Express cluster succeeds (positive control), then all five channel ops are called with a well-formed but never-created ClusterArn ('arn:aws:kafka:<region>:<account>:cluster/<name>/<uuid>-2') and a ghost ChannelArn.
  expected {'CreateChannel','DescribeChannel','UpdateChannel','DeleteChannel','ListChannels'} -> all 'NotFoundException'
  actual   AssertionError: observed={'CreateChannel': 'NotFoundException', 'DescribeChannel': 'BadRequestException', 'UpdateChannel': 'NotImplemented', 'DeleteChannel': 'BadRequestException', 'ListChannels': 'NotFoundException'}
  (UpdateChannel's 'NotImplemented' is the separately filed stub for services/service-com-amazonaws-kafka-kafka/src/ops/update_channel.rs and is not part of this issue; DescribeChannel and DeleteChannel are.)
Cause 1 — kafka.channel-duplicate-name-conflict (CreateChannel, its retry and ListChannels all correctly answered NotFoundException; the DeleteChannel leg failed):
  delete_channel(ChannelArn='arn:aws:kafka:<region>:<account>:channel/<name>/<uuid>-5', ClusterArn=<well-formed nonexistent cluster>)
  expected ('NotFoundException', 'The requested resource doesn\u2019t exist.')
  actual   AssertionError: DeleteChannel: expected NotFoundException, got ('BadRequestException', '<the request ChannelArn> is not a valid Amazon MSK channel ARN.')

Cause 2 — kafka.channel-delete-nonexistent-rejected:
  a real ACTIVE Express cluster is created, ListChannels returns [] (positive control), then delete_channel(ChannelArn='arn:aws:kafka:<region>:<account>:channel/<cluster-name>/<never-created-name>/<uuid>', ClusterArn=<that real cluster>)
  expected Error.Code 'BadRequestException', Error.Message 'The specified link ARN isn\u2019t valid. Specify a valid link ARN, and try again.'
  actual   AssertionError: expected BadRequestException, got 'NotFoundException'

Causes 3 and 4 — kafka.cluster-delete-and-broker-ops-rejected-while-creating, four state guards probed on a cluster still in CREATING:
  expected {'DeleteCluster': ('BadRequestException', "You can't delete cluster in CREATING state."), 'ListNodes': ('BadRequestException', "You can't list the nodes for a cluster that is in the CREATING state."), 'GetBootstrapBrokers': ('BadRequestException', "You can't get bootstrap broker nodes for a cluster in CREATING state."), 'RebootBroker': ('BadRequestException', 'This operation is only valid for resources that are in one of the following states :[ACTIVE]')}
  actual   {'ListNodes': ('BadRequestException', "You can't list nodes for a cluster in CREATING state."), 'GetBootstrapBrokers': ('BadRequestException', "You can't get bootstrap broker nodes for a cluster in CREATING state."), 'RebootBroker': ('BadRequestException', "You can't reboot brokers for a cluster in CREATING state."), 'DeleteCluster': ('BadRequestException', "You can't delete cluster in CREATING state.")}
  i.e. DeleteCluster and GetBootstrapBrokers match; only ListNodes (Cause 3) and RebootBroker (Cause 4) diverge, in message text only — the code is right in all four.

Cause 5 — kafka.reboot-broker-server-orchestrates-without-full-outage:
  2-broker PROVISIONED cluster, reboot_broker(BrokerIds=[one id]), then a loop at 2 s cadence calling DescribeClusterOperation + ListNodes + ListTopics until the operation state ends in COMPLETE/FAILED.
  expected at least 3 samples inside the reboot window (the inventory never shrinking and ListTopics answering throughout)
  actual   AssertionError: ListTopics was only exercised 2 time(s) across the reboot window, too few samples to support a 'stays reachable throughout' claim

Cause 6 — kafka.reboot-broker-state-transition:
  create 2-broker PROVISIONED cluster -> DescribeClusterV2 until ACTIVE (20 s cadence) -> ListNodes -> RebootBroker -> DescribeClusterOperation until REBOOT_COMPLETE (15 s cadence) -> DescribeClusterV2 until ACTIVE.
  expected OperationState 'REBOOT_COMPLETE' and the cluster back ACTIVE
  actual   Error: Check timed out after 60.0s — no frame, no status, no error code; no assertion was evaluated.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: build a well-formed but never-created ClusterArn 'arn:aws:kafka:<region>:<account>:cluster/<name>/<uuid>-2'. Call CreateChannel, DescribeChannel, UpdateChannel, DeleteChannel and ListChannels against it (any ghost ChannelArn). Every one must be HTTP 404 Error.Code 'NotFoundException' with Message "The requested resource doesn\u2019t exist.". Observe DescribeChannel and DeleteChannel answering 400 'BadRequestException' with '<ChannelArn> is not a valid Amazon MSK channel ARN.'.
3. Cause 2: CreateClusterV2 an Express-broker cluster across 3 distinct-AZ subnets and wait for ACTIVE; ListChannels must be []. Then DeleteChannel with ClusterArn=<that cluster> and a well-formed, never-created ChannelArn -> expect HTTP 400 'BadRequestException', Message 'The specified link ARN isn\u2019t valid. Specify a valid link ARN, and try again.'. Observe 'NotFoundException'. ListChannels must still be [].
4. Causes 3 and 4: CreateCluster a 2-broker PROVISIONED cluster and, while State is CREATING, call ListNodes -> expect Message "You can't list the nodes for a cluster that is in the CREATING state."; call RebootBroker -> expect Message 'This operation is only valid for resources that are in one of the following states :[ACTIVE]'. Both must be 400 'BadRequestException'. Compare the exact strings; DeleteCluster and GetBootstrapBrokers already match and must keep matching.
5. Cause 5: with a 2-broker cluster ACTIVE, RebootBroker one broker and poll DescribeClusterOperation + ListNodes + ListTopics every 2 s. The operation must report its in-progress state on at least 3 consecutive samples, the broker-node count must never shrink, ListTopics must answer every time, the operation must end in REBOOT_COMPLETE and the cluster return to ACTIVE.
6. Cause 6: run the same sequence with AWS cadences (DescribeClusterV2 every 20 s until ACTIVE, DescribeClusterOperation every 15 s until REBOOT_COMPLETE, then ACTIVE again) and assert the whole sequence reaches a verdict inside the 60 s check ceiling; measure whether the first probe after each write already reflects the settled value.
7. Re-replay every check id in trace_ids.
