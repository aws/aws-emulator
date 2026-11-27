Issue-Source: trace-sweep-divergence
Issue-Trace: grafana-workspace-endpoint-is-a-bare-host
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: grafana-workspace-endpoint-is-a-bare-host
Issue-Shape-Id: com.amazonaws.grafana#AWSGrafanaControlPlane
Issue-Key: grafana|DescribeWorkspace|2xx->2xx|$.workspace.endpoint
Issue-Commit: 481a4a04672d6cfe412cdf6a905c6af373954dc8

# Grafana workspace endpoint announces an unreachable amazonaws.com host

> _Note: this issue was identified against the code base at commit `481a4a04672d6cfe412cdf6a905c6af373954dc8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWorkspace

## Description
There is 1 cause in this issue.

Cause 1 of 1: DescribeWorkspace announces a workspace endpoint host the emulator is not reached on (fixture grafana-workspace-endpoint-is-a-bare-host, step task#3).
AWS: Smithy: DescribeWorkspace @http(method: GET, uri: "/workspaces/{workspaceId}", code: 200), @readonly; input workspaceId @httpLabel required, pattern ^g-[0-9a-f]{10}$; output workspace: WorkspaceDescription required. WorkspaceDescription$endpoint is documented as "The URL that users can use to access the Grafana console in the workspace" — i.e. the ADDRESS at which that workspace's Grafana console and native HTTP API actually answer, which a client splices into a request of its own (https://<endpoint>/api/...) or uses where only a host name will do (DNS, SNI, a Host header, an allow-list). On real AWS that address is reachable, and it is shaped as the bare host <workspaceId>.grafana-workspace.<region>.amazonaws.com. The requirement the model states is that the member carries the workspace's SERVING address; the literal amazonaws.com host from the recording is the value that address takes in a real account, not part of the contract. Status 200 and the response shape otherwise match.
Emulator: returns 200 with workspace.endpoint set to a hard-coded public AWS host composed from the workspace id and region (state.rs workspace_endpoint(): format!("{id}.{WORKSPACE_ENDPOINT_LABEL}.{region}.{AWS_ENDPOINT_SUFFIX}")), which is NOT an address this emulator is reached on — replay reports "$.workspace.endpoint: host_not_own". The emulator serves its data plane on the listener the request arrived at (the replay harness reached it on the :43969 data listener), and announces an unrelated amazonaws.com name with no port instead.
Why: a client that does what the member's documentation tells it to do — take endpoint and talk to the workspace's Grafana API at it — is sent off the emulator entirely. It either resolves a real AWS name (reaching production or nothing) or fails name resolution; it never reaches the emulator's own workspace API, which crate::service::workspace_api is written to claim. Every workspace-console / native-API flow built on the announced endpoint is therefore broken against the emulator while the control-plane call looks successful, and the failure surfaces far from the call that caused it.
Fix: the state projection that composes the announced endpoint — state::workspace_endpoint and the CreateWorkspace path that stores Workspace.endpoint from it (reported by to_description/to_summary) — must announce the address the emulator is actually reached on (the authority of the inbound request, including its port, so the same host the control-plane call arrived at is the host the workspace API is offered at), instead of a fixed <id>.grafana-workspace.<region>.amazonaws.com string. The shape requirement from the recording (a bare host, no scheme and no path) stays; what must change is that the announced host be the emulator's own.
Traces: grafana-workspace-endpoint-is-a-bare-host#task#3.

No open issue covers this (list_existing_issues reports no grafana issues at all).

## Evidence
Replay of fixture grafana-workspace-endpoint-is-a-bare-host (trace set simparity-wire-aws-v1), 1 mismatch cluster:

Step task#3, DescribeWorkspace: expected status 200, actual status 200 — detail "$.workspace.endpoint: host_not_own (expected 'g-0000000001.grafana-workspace.us-west-2.amazonaws.com', got 'g-0000000001.grafana-workspace.us-west-2.amazonaws.com')". divergence_key service=grafana|DescribeWorkspace|status=200->200|keys=$.workspace.endpoint. The verdict is host_not_own, not a value mismatch: the string is shaped like the recording but names a host that is not the one the emulator is reached on (the harness reached the emulator on its :43969 data listener per emulator_detail "/__admin/health answered 200 and the :43969 data listener accepts").

Expected: workspace.endpoint is a bare host (no scheme, no path) that resolves to the emulator serving the response, so https://<endpoint>/api/... reaches the emulated workspace API.
Actual: workspace.endpoint is the fixed public host <workspaceId>.grafana-workspace.<region>.amazonaws.com, which the emulator does not answer on.

Source: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/state.rs:849-851 (workspace_endpoint composes WORKSPACE_ENDPOINT_LABEL + AWS_ENDPOINT_SUFFIX) and state.rs:584 / state.rs:622 (to_description / to_summary echo the stored endpoint); services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/describe_workspace.rs:172-174 returns workspace.to_description().

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture grafana-workspace-endpoint-is-a-bare-host from trace set simparity-wire-aws-v1.
2. The fixture creates a workspace, then step task#3 issues GET /workspaces/{workspaceId} (DescribeWorkspace).
3. Read $.workspace.endpoint from the 200 response: it is <workspaceId>.grafana-workspace.<region>.amazonaws.com.
4. Compare that host with the address the request was sent to (the emulator's data listener): they differ, so the announced endpoint is not an address the emulator serves. Attempting https://<endpoint>/api/... from the same client does not reach the emulator's workspace API.
5. The step is reported as a mismatch on $.workspace.endpoint with verdict host_not_own.
