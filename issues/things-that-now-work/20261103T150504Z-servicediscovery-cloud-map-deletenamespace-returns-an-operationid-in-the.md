Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service
Issue-Step: task#13
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: servicediscovery|DeleteNamespace|2xx->2xx|$.OperationId
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# Cloud Map DeleteNamespace returns an OperationId in the wrong format

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNamespace

## Description
One cause.

Cause 1 of 1: DeleteNamespace's OperationId does not match the shape Cloud Map issues. AWS: the operation id is a 32-character lowercase base32-style token followed by a '-' and a short suffix segment (e.g. '<32 chars>-<8 chars>'); the recorded response's value has that two-segment form, and GetOperation is addressed by it. This is recorded behavior only — Smithy types DeleteNamespaceResponse.OperationId as a plain String with no pattern — so the requirement is the recorded format, not any single run's value. Emulator: a single 32-character lowercase alphanumeric token with no second segment. Why: the id is an opaque handle, but clients and tests that validate its format, or that split it, reject the emulator's value; and the difference is observable wherever the id is logged or round-tripped. Fix: the servicediscovery operation state's id generation (the same generator GetOperation resolves against), not DeleteNamespace's serializer. Traces: ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 (cluster has empty owned_by and empty stub_path), status 200 on both sides: '$.OperationId: value_mismatch (expected <shaped-not-asserted>, got "tftiyvfumtv6xbhl0gyet1fu0l0ddyrq")' — the emulator's single-segment 32-character token fails the recorded two-segment operation-id shape.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service, step task#13 — DeleteNamespace on a Cloud Map namespace; compare $.OperationId's format against the recorded response.
