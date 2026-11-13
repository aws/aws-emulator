Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-aqua-configuration-modify-transition
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-aqua-configuration-modify-transition
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|ModifyAquaConfiguration|2xx->5xx|$.ModifyAquaConfigurationResponse
Issue-Key: redshift|ModifyAquaConfiguration|2xx->5xx|status
Issue-Commit: b037d5392f1efc291497c72edeaad9a6f7e9925d

# Redshift ModifyAquaConfiguration answers HTTP 500 instead of returning the retired AquaConfiguration

> _Note: this issue was identified against the code base at commit `b037d5392f1efc291497c72edeaad9a6f7e9925d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyAquaConfiguration

## Description
Cause 1 of 1: ModifyAquaConfiguration on an existing cluster fails with a server error instead of succeeding.

AWS: ModifyAquaConfiguration is a retired operation that still answers successfully. Smithy: ModifyAquaConfiguration input ModifyAquaInputMessage {ClusterIdentifier (required), AquaConfigurationStatus (enum)}, output ModifyAquaOutputMessage {AquaConfiguration}; AquaConfiguration has members AquaStatus and AquaConfigurationStatus, both documented 'This field is retired. Amazon Redshift automatically determines whether to use AQUA (Advanced Query Accelerator).' The only modeled errors are ClusterNotFoundFault (ClusterNotFound, 404), InvalidClusterStateFault (InvalidClusterState, 400) and UnsupportedOperationFault (UnsupportedOperation, 400) — none of them a 5xx. The recorded response for a call against an existing, available cluster is HTTP 200 carrying ModifyAquaConfigurationResponse.ModifyAquaConfigurationResult.AquaConfiguration with AquaConfigurationStatus and AquaStatus, plus ResponseMetadata.RequestId. The recording shows the retired semantics: whatever AquaConfigurationStatus the request asks for, the returned AquaConfiguration reports the server-determined pair (in the recording AquaConfigurationStatus 'auto' with AquaStatus 'disabled'), because Redshift now decides AQUA use itself.

Emulator: the request is answered with HTTP 500 and no ModifyAquaConfigurationResponse element at all — the whole response body is absent, so a client gets a transport/internal-failure shape rather than either a success or a modeled query-protocol ErrorResponse.

Why: a 500 is retried by every AWS SDK's default retry policy and then surfaced as an internal-service exception, so a client cannot distinguish 'AQUA setting applied' from 'service broken'. Any workflow that calls ModifyAquaConfiguration as part of cluster setup (the recorded transition does exactly that, then re-reads the cluster) aborts, and the follow-up DescribeClusters cannot observe the resulting AQUA state. Nothing in the model permits a 5xx for this operation.

Fix: the ModifyAquaConfiguration handler in the redshift service crate (services/service-com-amazonaws-redshift-redshiftserviceversion20121201) — it must resolve ClusterIdentifier from cluster state, refuse an unknown identifier with ClusterNotFoundFault (404 ClusterNotFound) rather than erroring, and otherwise serialize a 200 ModifyAquaOutputMessage whose AquaConfiguration carries the server-determined AquaConfigurationStatus/AquaStatus pair for the retired feature; it must never surface an unhandled internal failure.

Related open issues that do NOT carry this fixture step's replay key: '20261104T045922Z-redshift-modifyaquaconfiguration-stub.md' (unimplemented scaffold stub) and '20261113T150542Z-redshift-6-net-new-behavioral-causes-modifyaquaconfiguration-refuses.md' (ModifyAquaConfiguration refuses with InternalFailure). The replay reports this cluster as unowned, so it is filed here with its own replay key.

Traces: redshift-aqua-configuration-modify-transition#task#4.

## Evidence
Expected (recorded AWS): HTTP 200 with body $.ModifyAquaConfigurationResponse = {'ModifyAquaConfigurationResult': {'AquaConfiguration': {'AquaConfigurationStatus': 'auto', 'AquaStatus': 'disabled'}}, 'ResponseMetadata': {'RequestId': <per-run id>}}.
Actual (emulator): HTTP 500; $.ModifyAquaConfigurationResponse missing entirely (got None).
Replay detail: "status: expected 200, got 500" / "$.ModifyAquaConfigurationResponse: missing".
Fixed vs variable: the 200 status, the ModifyAquaConfigurationResponse/ModifyAquaConfigurationResult/AquaConfiguration element nesting and the AquaStatus/AquaConfigurationStatus member names are fixed requirements; the RequestId value is per-run and is not asserted.

## Steps to Reproduce
1. Replay fixture redshift-aqua-configuration-modify-transition against the emulator.
2. Step task#1-task#2 create a cluster and wait for it to become available.
3. Step task#4 issues the Query-protocol POST Action=ModifyAquaConfiguration with ClusterIdentifier=<the created cluster> and AquaConfigurationStatus set.
4. Observe HTTP 500 with no ModifyAquaConfigurationResponse body, where AWS returns HTTP 200 with ModifyAquaConfigurationResult.AquaConfiguration.
