Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-graph-host-prefixed-graph-ops-nonexistent-graph-auth-rejected
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-graph-host-prefixed-graph-ops-nonexistent-graph-auth-rejected
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph|ExecuteQuery|4xx->4xx|$.code
Issue-Key: neptune-graph|ExecuteQuery|4xx->4xx|$.message
Issue-Key: neptune-graph|ExecuteQuery|4xx->4xx|$.requestId
Issue-Key: neptune-graph|ExecuteQuery|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: neptune-graph|ExecuteQuery|4xx->4xx|status
Issue-Commit: 923c323f54672fefacf2668b8aa66ab28d8eb234

# ExecuteQuery on an unresolvable graph host prefix answers 404 ResourceNotFoundException instead of the 403 AccessDeniedException data-plane refusal

> _Note: this issue was identified against the code base at commit `923c323f54672fefacf2668b8aa66ab28d8eb234`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteQuery

## Description
Wire replay of trace set simparity-wire-aws-v1 against this checkout left 5 diverging steps on neptune-graph. FOUR of them are already owned by the open issue 20261029T201135Z-neptune-graph-3-net-new-wire-causes-host.md (its replay key covers GetGraphSummary task#1, GetQuery task#3, CancelQuery task#4 of this same fixture, and CreateGraphUsingImportTask). The FIFTH — ExecuteQuery, step task#2 of the same fixture — is NOT covered by that issue: its Issue-Key list names GetGraphSummary, GetQuery, CancelQuery, CancelImportTask, CancelExportTask and CreateGraphUsingImportTask and does NOT name ExecuteQuery, and the replay reports it with an EMPTY owned_by. It is filed here so the wire corpus re-runs it. This issue carries exactly ONE cause. Smithy model consulted: services/service-com-amazonaws-neptunegraph-amazonneptunegraph/smithy-model/com-amazonaws-neptunegraph-amazonneptunegraph.json.

=== Cause 1 of 1: ExecuteQuery must refuse a graphIdentifier that resolves to no serving graph as an AUTHENTICATION failure (403 AccessDeniedException, data-plane envelope), not as a control-plane 404 ResourceNotFoundException ===

AWS: ExecuteQuery is bound to the graph's OWN data endpoint — Smithy: @endpoint hostPrefix "{graphIdentifier}.", with graphIdentifier @hostLabel + @httpHeader "graphIdentifier", required, pattern ^g-[a-z0-9]{10}$; http POST /queries, success 200 with a streaming httpPayload. Its modeled error set is AccessDeniedException @httpError 403 ("Raised in case of an authentication or authorization failure"), ConflictException 409, InternalServerException 500, ThrottlingException 429, UnprocessableException 422, ValidationException 400. ResourceNotFoundException is NOT in ExecuteQuery's modeled error set at all — unlike its host-prefixed siblings GetQuery and CancelQuery, which do model it @httpError 404. So for this operation a 404 ResourceNotFoundException is not merely the wrong refusal, it is a response shape the operation's contract does not permit, and a generated SDK client has no modeled branch to deserialize it into.

What the service does instead, from the recording (recorded behavior only for the envelope's extra members — the model's AccessDeniedException declares only `message`): when graphIdentifier names a graph the account does not hold, the request never reaches a query, because there is no graph data endpoint at that host prefix for the signature to be accepted against. The call is refused with HTTP 403 and the Neptune Analytics data-plane error envelope, whose JSON body carries `code`, `requestId` and `message`. For this error class the fixed text of `message` is exactly "Authentication Failed" — the whole message, with no variable part — and `code` is the fixed string "AccessDeniedException". The ONE variable part of the body is `requestId`, a per-request id the service generates; its value is never part of the requirement. The refusal does not depend on the query text, the language, or any other member of the request: it is taken on the host prefix alone, which is why it is identical across ExecuteQuery and the other operations on that endpoint.

Emulator: ExecuteQuery resolves the graph out of the control-plane store INSIDE the handler and reports a miss as the control-plane not-found refusal. services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/execute_query.rs widens ResourceNotFoundException into the operation's error channel on purpose (`#[smithy_macros::operation_error(..., extra_errors = ["ResourceNotFoundException"])]`, lines 119-132, with the module doc at lines 89-92 asserting the 404 is the right report), then at the store lookup does `store.settled_graph(&input.graph_identifier, &region, now).filter(|g| g.status.is_serving())` and on None returns `ExecuteQueryErrorModeled::ResourceNotFoundException(crate::state::graph_not_found(&input.graph_identifier))`. Replay therefore gets HTTP 404 with header x-amzn-errortype: ResourceNotFoundException, NO `code` member, NO `requestId` member, and message "No graph found for customer [<account>] with graph id [<graphIdentifier>]" where AWS says exactly "Authentication Failed". The status, the error code, the error message and the body SHAPE all diverge, and the emulator's answer is outside the operation's modeled error set. (The handler's earlier `crate::state::query_denial` check does produce a 403 AccessDeniedException, but only for a policy denial on an EXISTING graph, and it carries only `message`, not the code/requestId envelope.)

Why: a client that reaches a graph data endpoint for a graph it does not own, mistyped, or that has already been deleted must distinguish "your credentials or this endpoint are not accepted" (403 — not retryable, re-resolve the endpoint or fix credentials) from "the resource is gone" (404 — stop polling). Here every such ExecuteQuery is reported as a plain missing resource, so retry and failover logic keyed on the status class takes the wrong branch; worse, because ExecuteQuery does not model ResourceNotFoundException, an SDK client deserializes an unmodeled error and surfaces it as a generic/unknown service failure rather than either condition. The data-plane envelope that Neptune Analytics tooling parses is absent too: handling keyed on `code` sees no error code at all, and support correlation by `requestId` is impossible.

Fix: the graph data-plane front door — the host-prefix dispatch that routes {graphIdentifier}. requests to the host-prefixed operations — must refuse an unresolvable or non-serving graph host prefix with the 403 AccessDeniedException "Authentication Failed" refusal, serialized in the data-plane envelope (code/requestId/message), BEFORE the per-handler store lookup in src/ops/execute_query.rs is reached; `extra_errors = ["ResourceNotFoundException"]` and the 404 branch in that handler should then be removed, since the shape is not in the operation's modeled error set. crate::state::graph_not_found must remain the refusal only for control-plane operations addressed by graphIdentifier in the request body or URI. The same front-door refusal is what cause 1 of the open issue 20261029T201135Z-neptune-graph-3-net-new-wire-causes-host.md asks for on GetGraphSummary/GetQuery/CancelQuery; ExecuteQuery must be fixed with them and is filed separately only because that issue's replay pointer does not run this step.

Traces: neptune-graph-host-prefixed-graph-ops-nonexistent-graph-auth-rejected#task#2 (ExecuteQuery, expected 403, got 404; $.code missing where 'AccessDeniedException' expected; $.requestId missing; $.message value_mismatch where 'Authentication Failed' expected; x-amzn-errortype shape_mismatch, got 'ResourceNotFoundException').

## Evidence
Replay summary (trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, emulator_detail "/__admin/health answered 200 and the :45621 data listener accepts"): 24 steps total, 18 passed, 6 mismatched, 0 errored, 0 blocked, score 75.0%, 5 divergence clusters — all five on service neptune-graph, all with empty stub_path. Four carry owned_by = [20261029T201135Z-neptune-graph-3-net-new-wire-causes-host.md]; the ExecuteQuery cluster carries owned_by = [] and is the one filed here.

Diverging cluster, expected vs actual (divergence_key service=neptune-graph|ExecuteQuery|status=403->404|keys=$.code,$.message,$.requestId,$headers.x-amzn-errortype,status):
- fixture neptune-graph-host-prefixed-graph-ops-nonexistent-graph-auth-rejected, step task#2, operation ExecuteQuery, verdict mismatch
- status: expected 403, got 201/404 -> got 404
- $.requestId: missing (expected a shaped, non-asserted value)
- $.code: missing (expected 'AccessDeniedException')
- $.message: value_mismatch — expected 'Authentication Failed', got 'No graph found for customer [000000000000] with graph id [g-9f4a0b7944]'
- $headers.x-amzn-errortype: shape_mismatch — got 'ResourceNotFoundException'

Fixed vs variable text: the requirement is the 403 status, the fixed code string 'AccessDeniedException', the fixed message 'Authentication Failed' (the entire message — it has no variable part), and the presence of the code/requestId/message envelope. The account '000000000000', the graph id 'g-9f4a0b7944' in the emulator's message, and the requestId AWS returns are per-run values and are NOT part of the requirement.

Source evidence in the checkout (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/):
- src/ops/execute_query.rs:119-132 — `#[smithy_macros::operation_error("com.amazonaws.neptunegraph#ExecuteQuery", extra_errors = ["ResourceNotFoundException"])]`, widening in a shape the operation does not model.
- src/ops/execute_query.rs:245-257 — `let serving = store.settled_graph(&input.graph_identifier, &region, now).filter(|graph| graph.status.is_serving()); let Some(graph) = serving else { return Err(ExecuteQueryErrorModeled::ResourceNotFoundException(crate::state::graph_not_found(&input.graph_identifier)).into()) };` — the control-plane not-found refusal answered from a host-prefixed data-plane operation. Nothing on this path emits a 403 or the code/requestId envelope.
- src/ops/execute_query.rs:204-206 — the only 403 path, `crate::state::query_denial(...)` -> AccessDeniedException, is a policy denial on an existing graph and carries `message` only.
- src/ops/execute_query.rs:89-92 — module doc asserting the 404 is correct for this operation, which is the assumption to reverse.
- Smithy ExecuteQuery: endpoint hostPrefix "{graphIdentifier}."; graphIdentifier hostLabel+httpHeader, required, pattern ^g-[a-z0-9]{10}$; errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ThrottlingException(429), UnprocessableException(422), ValidationException(400) — ResourceNotFoundException absent. Contrast Smithy GetQuery, whose error set DOES include ResourceNotFoundException(404). Smithy AccessDeniedException: structure, client error, httpError 403, member `message` required, doc "Raised in case of an authentication or authorization failure."

## Steps to Reproduce
Boot the emulator from this checkout and point a neptune-graph client at it, addressing the data plane on the {graphId}. host prefix.

1. Ensure NO graph with that id exists in the account/Region (fresh emulator state is enough).
2. Call ExecuteQuery against the graph's data endpoint: ng.execute_query(graphIdentifier='g-9f4a0b7944', queryString='MATCH (n) RETURN n LIMIT 1', language='OPEN_CYPHER') — with explainMode omitted, so the handler's explain refusal is not what answers.
3. AWS answers HTTP 403 with the data-plane error envelope: body members code='AccessDeniedException', requestId=<per-request id>, message='Authentication Failed'.
4. The emulator answers HTTP 404 with header x-amzn-errortype: ResourceNotFoundException, body message 'No graph found for customer [<account>] with graph id [g-9f4a0b7944]', and no code or requestId members — an error shape ExecuteQuery does not even model.

This is fixture neptune-graph-host-prefixed-graph-ops-nonexistent-graph-auth-rejected step task#2. Steps task#1 (GetGraphSummary), task#3 (GetQuery) and task#4 (CancelQuery) of the SAME fixture diverge identically and are owned by 20261029T201135Z-neptune-graph-3-net-new-wire-causes-host.md; re-replay this fixture in full to confirm the front-door fix closes all four.
