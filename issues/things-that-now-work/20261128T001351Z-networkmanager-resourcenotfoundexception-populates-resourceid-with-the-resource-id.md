Issue-Source: trace-sweep-divergence
Issue-Trace: networkmanager-error-execute-change-set-unknown-core-network
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkmanager-error-execute-change-set-unknown-core-network
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager|ExecuteCoreNetworkChangeSet|4xx->4xx|$.ResourceId
Issue-Commit: a301ce84f47538debe7c18f51774443720b7c560

# networkmanager ResourceNotFoundException populates ResourceId with the resource id instead of AWS's fixed text

> _Note: this issue was identified against the code base at commit `a301ce84f47538debe7c18f51774443720b7c560`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteCoreNetworkChangeSet

## Description
The emulator returns the right HTTP status and error shape for ExecuteCoreNetworkChangeSet against a core network that does not exist, but it fills the ResourceNotFoundException's ResourceId member with the requested core-network id instead of the value AWS puts there.

Cause 1 of 1: ResourceNotFoundException.ResourceId is wrong for ExecuteCoreNetworkChangeSet on an unknown core network.

AWS: Smithy (com.amazonaws.networkmanager): ExecuteCoreNetworkChangeSet POST /core-networks/{CoreNetworkId}/core-network-change-sets/{PolicyVersionId}/execute, error ResourceNotFoundException @error(client) @httpError(404) with required members Message, ResourceId, ResourceType (all ServerSideString) and optional Context; doc for ResourceId is only "The ID of the resource." The model does not state what the service actually puts in each member, so for the concrete payload this is recorded behavior only: the recording of AWS answers HTTP 404 with the ResourceNotFoundException and ResourceId carrying the fixed text "Core Network not found." — i.e. for this error class networkmanager emits that constant string in ResourceId, not a per-request identifier. The rest of the recorded envelope (status 404, error code ResourceNotFoundException, Message and ResourceType) matched, so only ResourceId is at issue. Hypothesis: the service populates ResourceId from the same not-found description it uses for the human-readable part of this error, rather than echoing the path label.

Emulator: replay of fixture networkmanager-error-execute-change-set-unknown-core-network step task#1 reports status 404 -> 404 with $.ResourceId: value_mismatch (expected 'Core Network not found.', got 'core-network-0fedcba9876543210'). The emulator echoes the CoreNetworkId path label from the request into ResourceId. The fixed text is the whole of the expected value here; the value the emulator substitutes is the request's CoreNetworkId, which is request-derived and therefore not what belongs in that member.

Why: a client that reads ResourceNotFoundException.ResourceId to branch or to log the failure sees a different kind of value against the emulator than against AWS — an identifier where AWS sends a fixed not-found description. Any equality check, regex or dispatch written against the real service's payload (and any SDK-level assertion in a test suite recorded off AWS) fails or mis-routes, even though the status and error code are correct, which makes the discrepancy silent until the field is inspected.

Fix: the ExecuteCoreNetworkChangeSet handler's not-found path in the networkmanager service crate — specifically where it constructs ResourceNotFoundException. It must populate ResourceId with the constant not-found text AWS uses for this error class rather than copying the CoreNetworkId path label; ResourceType and Message are already correct and must not change. If the construction is shared by other networkmanager lookups, scope the change to the member mapping for this error so other operations keep their recorded values.

Traces: networkmanager-error-execute-change-set-unknown-core-network#task#1 (the only member of this cluster).

## Evidence
Replay divergence (verdict=mismatch, divergence_key service=networkmanager|ExecuteCoreNetworkChangeSet|status=404->404|keys=$.ResourceId):
expected_status 404, actual_status 404 — status and error code agree.
detail: $.ResourceId: value_mismatch (expected 'Core Network not found.', got 'core-network-0fedcba9876543210').
Expected: ResourceNotFoundException body whose ResourceId member is the fixed text "Core Network not found.".
Actual: ResourceId is the CoreNetworkId taken verbatim from the request path (a per-request value, shown here as the fixture's core-network-0fedcba9876543210).
Smithy basis: ResourceNotFoundException @httpError(404), required Message/ResourceId/ResourceType; ExecuteCoreNetworkChangeSet lists ResourceNotFoundException among its errors.

## Steps to Reproduce
1. Boot the emulator from this checkout (networkmanager is bootstrapped).
2. Replay fixture networkmanager-error-execute-change-set-unknown-core-network, step task#1 — POST /core-networks/<core-network-id-that-does-not-exist>/core-network-change-sets/<policyVersionId>/execute.
3. Observe HTTP 404 with a ResourceNotFoundException body.
4. Inspect $.ResourceId: the emulator returns the core network id from the request URI; the recorded AWS response returns the fixed text "Core Network not found.".
