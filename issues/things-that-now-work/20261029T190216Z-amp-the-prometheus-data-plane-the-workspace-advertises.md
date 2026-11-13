Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amp.remote-write-query-round-trip
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: amp.remote-write-query-round-trip|run#13|AssertionError|assert status in (200, 202, 204), f"remote_write failed: HTTP {status}: {body!r}"
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# amp: the Prometheus data plane the workspace advertises is unrouted — a signed remote_write to prometheusEndpoint answers 501 NotImplemented, and the advertised endpoint's path already contains /api/v1 so the composed URL doubles it

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWorkspace (prometheusEndpoint data plane: POST /api/v1/remote_write, GET /api/v1/query)

## Description
ONE net-new root cause: the amp emulator serves the control plane but not the Prometheus data plane that a workspace's own prometheusEndpoint advertises, and the endpoint it advertises has a path shape that does not compose with the standard data-plane paths. amp IS served (bootstrap resolved, 15/22 amp checks pass), so this is a fidelity defect, not a bootstrap gap.

Cause 1 of 1: the workspace's advertised Prometheus endpoint accepts no remote_write/query traffic.
  AWS: Smithy: WorkspaceDescription.prometheusEndpoint is documented as "The Prometheus endpoint available for this workspace. For example, https://aps-workspaces.<region>.amazonaws.com/workspaces/ws-example1-1234-abcd-5678-ef90abcd1234/api/v1/" — i.e. the URL at which THIS workspace's native Prometheus HTTP API is actually served. Recorded behavior: taking that advertised endpoint and appending the standard Prometheus data-plane paths, a SigV4-signed (signing name from the amp service model) POST <endpoint>/api/v1/remote_write carrying a snappy-compressed protobuf WriteRequest (Content-Encoding: snappy, Content-Type: application/x-protobuf, X-Prometheus-Remote-Write-Version: 0.1.0) is accepted with HTTP 200, 202 or 204, and a subsequent signed GET <endpoint>/api/v1/query?query=<metric> returns HTTP 200 with a Prometheus-shaped body {"status":"success","data":{"result":[{...,"value":[<ts>,"<value>"]}]}} whose value is the sample just written.
  Emulator: the POST is answered HTTP 501 with body {"__type":"NotImplemented","message":"operation POST /workspaces/<workspaceId>/api/v1/api/v1/remote_write is not implemented in this emulator"}. Two facts are visible in that one response: (a) no route exists for the workspace's Prometheus data plane at all — neither remote_write nor query is served, so the round trip can never happen; and (b) the request path contains 'api/v1/api/v1', because the prometheusEndpoint the emulator returned already ends in '/api/v1' and the client appended the standard '/api/v1/remote_write' to it. Against the recorded service the same composition reaches the data plane, so the emulator's advertised endpoint path shape and its routing disagree with the service's.
  Why: every real consumer of an AMP workspace is a data-plane client — a Prometheus agent's remote_write target, an OTEL/ADOT exporter, a Grafana datasource, any PromQL query caller. All of them read prometheusEndpoint and append the Prometheus paths to it. Against this emulator each of them gets 501 NotImplemented, so a workspace is write-only-in-name: nothing can be ingested and nothing can be queried back, and no test of an ingestion/alerting pipeline can run against the emulator even though the control plane fully succeeds.
  Fix: the amp service must route the workspace Prometheus data plane (at minimum POST .../api/v1/remote_write accepting a snappy protobuf WriteRequest and answering 200/202/204, and GET .../api/v1/query answering the Prometheus instant-query JSON from the written samples, which requires per-workspace sample state), and the endpoint construction that fills WorkspaceDescription.prometheusEndpoint (the DescribeWorkspace/CreateWorkspace/ListWorkspaces response builder) must agree with those routes so that appending '/api/v1/remote_write' to the advertised endpoint — the way every Prometheus client composes it — lands on the implemented route instead of a doubled '/api/v1/api/v1' path.
  Traces: amp.remote-write-query-round-trip

Note on an existing issue: open issue 20261022T110707Z-amp-3-fidelity-divergences-prometheusendpoint-announces-the-real.md lists this check among its trace ids for a DIFFERENT, earlier failure — prometheusEndpoint then named the real AWS host 'aps-workspaces.<region>.amazonaws.com' and the run was stopped by the egress guard before any data-plane call. The endpoint now resolves to the endpoint under test, so the host cause no longer reproduces; the failure has moved to the data plane itself (501) and to the endpoint's path shape, neither of which that issue describes.

## Evidence
amp.remote-write-query-round-trip [mismatch]
  AssertionError: remote_write failed: HTTP 501: b'{"__type":"NotImplemented","message":"operation POST /workspaces/ws-d4a986a2-8b58-1154-6773-0d9b2464959d/api/v1/api/v1/remote_write is not implemented in this emulator"}'
  Expected: POST <prometheusEndpoint>/api/v1/remote_write (SigV4-signed, snappy-compressed protobuf WriteRequest) -> HTTP 200/202/204; then GET <prometheusEndpoint>/api/v1/query?query=<metric> -> HTTP 200 with {"status":"success"} and data.result[0].value[1] equal to the written sample value.
  Actual: HTTP 501 NotImplemented on the write; the query leg never ran. The echoed path shows the advertised endpoint already ends in '/api/v1' ('/workspaces/<id>/api/v1' + '/api/v1/remote_write' -> '/api/v1/api/v1/remote_write').
  (The control plane in this same check succeeded: create_workspace, describe_workspace reaching ACTIVE, and a non-empty prometheusEndpoint pointing at the endpoint under test.)

## Steps to Reproduce
1. amp.create_workspace(alias=<unique>) and poll amp.describe_workspace(workspaceId=...) until workspace.status.statusCode == 'ACTIVE'.
2. Read endpoint = describe_workspace(...)['workspace']['prometheusEndpoint'] — note the path already ends with '/api/v1'.
3. Build a Prometheus WriteRequest protobuf for one timeseries ({__name__: <metric>, job: <any>}) with one sample (value 42.0, timestamp now in ms), snappy-compress it, and SigV4-sign a POST to endpoint.rstrip('/') + '/api/v1/remote_write' with headers Content-Encoding: snappy, Content-Type: application/x-protobuf, X-Prometheus-Remote-Write-Version: 0.1.0.
4. Observe HTTP 501 with __type NotImplemented naming 'POST /workspaces/<id>/api/v1/api/v1/remote_write', instead of 200/202/204.
5. Also signed-GET endpoint.rstrip('/') + '/api/v1/query?query=<metric>' — it likewise never returns the Prometheus instant-query JSON containing the written sample.
