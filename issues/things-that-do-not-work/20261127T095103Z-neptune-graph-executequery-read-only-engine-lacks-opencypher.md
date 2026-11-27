Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune-graph.cancel-query-never-issued-query-id-rejected neptune-graph.execute-query-past-its-timeout-rejected neptune-graph.execute-query-write-overtaken-by-committed-write-conflict-rejected
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph.cancel-query-never-issued-query-id-rejected|run#22|AssertionError|assert outcome.get("error") == want_end, f"the cancelled read's ExecuteQuery ended with {outcome!r}, expected {want_end!r}"
Issue-Key: neptune-graph.execute-query-past-its-timeout-rejected|run#10|AssertionError|assert got == want, f"a query past its 50 ms timeout answered {got!r}, expected {want!r}"
Issue-Key: neptune-graph.execute-query-write-overtaken-by-committed-write-conflict-rejected|run#18|AssertionError|assert error == want, f"the overtaken long write ended with {error!r}, expected a conflict or {want!r}"
Issue-Commit: ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7

# neptune-graph ExecuteQuery: read-only engine lacks openCypher range(), and queries that end at their deadline are refused with emulator-invented messages / an internal 100000-value ceiling instead of QUERY_TIMEOUT "Operation terminated (deadline exceeded)"

> _Note: this issue was identified against the code base at commit `ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteQuery

## Description
Three distinct root causes in the neptune-graph data plane, all reached through ExecuteQuery. They are independent and each must be closed separately. (The open issue 20261023T175427Z-neptune-graph-executequery-stub.md asks for ExecuteQuery to exist at all; it is implemented now and these are fidelity defects inside it, not that stub. The open issue 20261124T075822Z-neptune-graph-executequery-serves-a-graph-still-in.md is about CREATING-state admission and covers none of the causes below.)

Cause 1 of 3 — the read-only openCypher engine does not know the built-in list function `range()`.
AWS: `range(start, end [, step])` is a standard openCypher list function and Neptune Analytics evaluates it in read queries; `UNWIND range(1, 1000000) AS x ... RETURN count(*)` is accepted and runs (to completion, to its queryTimeoutMilliseconds, or until CancelQuery stops it). Smithy says nothing about query-language coverage (recorded behavior only): the recordings show the call accepted and the query running, never a 400. ExecuteQuery's modeled ValidationException (HTTP 400, reason ILLEGAL_ARGUMENT) exists for a query asking for something that cannot be evaluated — an unknown function — which `range` is not.
Emulator: the request is refused immediately with HTTP 400 ValidationException, reason `ILLEGAL_ARGUMENT`, message `unknown function `range``. The fixed text is "unknown function" and the variable part is the function name taken from the query text. Note the same `range(...)` call DOES evaluate when the query carries an updating clause or `$name` parameters (cause 3's trace reaches the engine's value ceiling, so it computed the range), i.e. only the read-only path (`crate::state::evaluate_query` -> the `cypher-parser` read engine) is missing the function.
Why: every read query that generates rows with `range()` — a very common openCypher idiom, and the only portable way to drive a long-running read — fails with a 400 instead of running. A client cannot exercise cancellation, timeouts, or any row-generating read; worse, it is told its query is invalid, so it "fixes" a query AWS accepts.
Fix: the read-only openCypher evaluator's function table (`crate::state::evaluate_query`, read path), which must support `range` with the same arity and semantics as the updating engine already does. Nothing in the ExecuteQuery handler's error mapping changes for this cause.
Traces: neptune-graph.cancel-query-never-issued-query-id-rejected (the long read fails at once with this 400, so the cancelled query's ExecuteQuery ends with ('ValidationException', 'unknown function `range`', 'ILLEGAL_ARGUMENT')); neptune-graph.execute-query-past-its-timeout-rejected (the 50 ms query is refused with the same 400 before any deadline is reached).

Cause 2 of 3 — the messages for a query stopped at its deadline and for a query stopped by CancelQuery are the emulator's own prose, not AWS's.
AWS: a query that outruns its deadline is refused with HTTP 422 UnprocessableException, reason `QUERY_TIMEOUT`, message exactly `Operation terminated (deadline exceeded)`; a query a CancelQuery ended answers HTTP 400 ValidationException, reason `QUERY_CANCELLED`, message exactly `Operation terminated (cancelled by user)`. Both messages are entirely fixed text — there is no request- or state-derived part in them. Smithy: ExecuteQuery errors UnprocessableException (422, required `reason` of UnprocessableExceptionReason incl. QUERY_TIMEOUT) and ValidationException (400, `reason` of ValidationExceptionReason incl. QUERY_CANCELLED); the wording itself is recorded behavior only.
Emulator: the status codes and reasons are right but both messages are substituted. The timeout path answers `the query did not complete within the time it was given` (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/execute_query.rs:433 on the wait path and :585 on the engine-reported path), and the cancellation path answers `crate::state::QUERY_CANCELLED_MESSAGE` (same file, :602).
Why: these messages are how a client distinguishes a cancelled query from a deadline-exceeded one when it logs or surfaces the failure, and retry/alerting logic that matches AWS's sentence never matches. The divergence is silent — the code and reason look correct.
Fix: the ExecuteQuery handler's error mapping (`query_refusal`) and the constant the state module holds for the cancellation sentence; both message literals must be AWS's fixed text verbatim.
Traces: neptune-graph.execute-query-past-its-timeout-rejected (expects ('UnprocessableException', 'Operation terminated (deadline exceeded)', 'QUERY_TIMEOUT')); neptune-graph.execute-query-write-overtaken-by-committed-write-conflict-rejected (same expectation for the overtaken write); neptune-graph.cancel-query-never-issued-query-id-rejected (expects ('ValidationException', 'Operation terminated (cancelled by user)', 'QUERY_CANCELLED') from the cancelled read).

Cause 3 of 3 — a long-running write is refused by an emulator-only value ceiling (INTERNAL_LIMIT_EXCEEDED) instead of running to its deadline or losing a write conflict.
AWS: a write query that generates a very large intermediate result either runs until its queryTimeoutMilliseconds and is refused with UnprocessableException/`QUERY_TIMEOUT` `Operation terminated (deadline exceeded)`, or — when another committed write touched the same property — is refused with ConflictException (HTTP 409) reason `CONCURRENT_MODIFICATION`, message `Operation failed due to conflicting concurrent operations (please retry), <n> transactions are currently rolling back.` (fixed text with the live rolling-back transaction count from service state as its only variable part). Smithy: ExecuteQuery declares ConflictException (409) and UnprocessableException (422, reason incl. QUERY_TIMEOUT and INTERNAL_LIMIT_EXCEEDED); AWS reserves the internal-limit reason for real service limits (partition/memory/storage), not for a row count a correct query generates.
Emulator: the write is refused with HTTP 422 UnprocessableException, reason `INTERNAL_LIMIT_EXCEEDED`, message `the query builds a value of 100001 and this endpoint builds at most 100000` — fixed text "the query builds a value of" / "and this endpoint builds at most", with the reached count and the emulator's own ceiling as values. It fires long before the request's 300000 ms timeout and before any conflict is detected, so neither the deadline nor the concurrency outcome is ever observable. The mapping is `crate::state::QueryFailure::OverLimit` -> INTERNAL_LIMIT_EXCEEDED at services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/execute_query.rs:566-570; the ceiling itself is imposed by the updating engine in `crate::state`.
Why: a client's long or generative write gets a 422 that AWS would never send, with an error class that implies the graph hit a capacity limit. Write-conflict and timeout semantics — the whole point of queryTimeoutMilliseconds and of ConflictException retry loops — cannot be exercised, and a retry on INTERNAL_LIMIT_EXCEEDED is pointless, so clients that retry conflicts will not retry here.
Fix: the updating openCypher engine's work accounting in `crate::state` (it must not refuse a well-formed query for exceeding a self-chosen intermediate-value count while the request's own deadline has not expired) and the ExecuteQuery handler's precedence between deadline/conflict detection and the internal-limit refusal.
Traces: neptune-graph.execute-query-write-overtaken-by-committed-write-conflict-rejected.

## Evidence
Suite: 37/43 passed; neptune-graph 36/40 with 4 failed (one of those is OWNED by an existing issue and excluded here).

Cause 1:
- neptune-graph.cancel-query-never-issued-query-id-rejected:
  AssertionError: the cancelled read's ExecuteQuery ended with {'error': ('ValidationException', 'unknown function `range`', 'ILLEGAL_ARGUMENT')}, expected ('ValidationException', 'Operation terminated (cancelled by user)', 'QUERY_CANCELLED')
  Query sent: "UNWIND range(1, 1000000) AS x UNWIND range(1, 1000000) AS y RETURN count(*) AS total_<tag>" with queryTimeoutMilliseconds=240000.
- neptune-graph.execute-query-past-its-timeout-rejected:
  AssertionError: a query past its 50 ms timeout answered ('ValidationException', 'unknown function `range`', 'ILLEGAL_ARGUMENT'), expected ('UnprocessableException', 'Operation terminated (deadline exceeded)', 'QUERY_TIMEOUT')
  Query sent: "UNWIND range(1, 1000000) AS a UNWIND range(1, 1000000) AS b RETURN count(*) AS rows_<tag>" with queryTimeoutMilliseconds=50.
  Expected: HTTP 422, code UnprocessableException, reason QUERY_TIMEOUT, message "Operation terminated (deadline exceeded)".
  Actual:   HTTP 400, code ValidationException, reason ILLEGAL_ARGUMENT, message "unknown function `range`".

Cause 2 (message wording, same two expectations plus the write check):
- Expected cancellation ending: ValidationException / QUERY_CANCELLED / "Operation terminated (cancelled by user)".
- Expected timeout ending: UnprocessableException / QUERY_TIMEOUT / "Operation terminated (deadline exceeded)".
- Emulator literals, from the handler: "the query did not complete within the time it was given" (src/ops/execute_query.rs:433 and :585) and crate::state::QUERY_CANCELLED_MESSAGE (src/ops/execute_query.rs:602).

Cause 3:
- neptune-graph.execute-query-write-overtaken-by-committed-write-conflict-rejected:
  AssertionError: the overtaken long write ended with ('UnprocessableException', 'the query builds a value of 100001 and this endpoint builds at most 100000', 'INTERNAL_LIMIT_EXCEEDED'), expected a conflict or ('UnprocessableException', 'Operation terminated (deadline exceeded)', 'QUERY_TIMEOUT')
  Long write sent: "MATCH (n:Counter {k: $k}) SET n.v = 1 WITH n UNWIND range(1, 1000000) AS a UNWIND range(1, 1000000) AS b WITH n, count(*) AS rows RETURN n.v AS v" with queryTimeoutMilliseconds=300000, overtaken by "MATCH (n:Counter {k: $k}) SET n.v = $v RETURN n.v AS v".
  Accepted AWS endings: ConflictException / CONCURRENT_MODIFICATION / "Operation failed due to conflicting concurrent operations (please retry), <n> transactions are currently rolling back." OR UnprocessableException / QUERY_TIMEOUT / "Operation terminated (deadline exceeded)".
  Actual: UnprocessableException / INTERNAL_LIMIT_EXCEEDED, raised well inside the 300 s deadline.
  Note this same query evaluated range() successfully (it counted to 100001), which is what shows cause 1 is confined to the read-only engine.

## Steps to Reproduce
1. Boot the emulator and create a graph with CreateGraph (provisionedMemory=16, replicaCount=0, publicConnectivity=true) and wait for status AVAILABLE.
2. Cause 1: call ExecuteQuery(language=OPEN_CYPHER, queryString="UNWIND range(1, 10) AS a RETURN count(*) AS rows"). Observe HTTP 400 ValidationException reason ILLEGAL_ARGUMENT message "unknown function `range`"; AWS returns [{"rows": 10}].
3. Cause 2: run any query that outruns its deadline, e.g. ExecuteQuery(queryString=<a query that cannot finish>, queryTimeoutMilliseconds=50). Observe HTTP 422 / QUERY_TIMEOUT with message "the query did not complete within the time it was given" instead of "Operation terminated (deadline exceeded)". Then start a long query, find its id with ListQueries(state="RUNNING"), call CancelQuery on that id, and observe the cancelled ExecuteQuery's message differ from "Operation terminated (cancelled by user)" (reason QUERY_CANCELLED).
4. Cause 3: seed a node with "CREATE (n:Counter {k: $k, v: 0}) RETURN n.v AS v"; start, in the background, "MATCH (n:Counter {k: $k}) SET n.v = 1 WITH n UNWIND range(1, 1000000) AS a UNWIND range(1, 1000000) AS b WITH n, count(*) AS rows RETURN n.v AS v" with queryTimeoutMilliseconds=300000; while it runs, commit "MATCH (n:Counter {k: $k}) SET n.v = $v RETURN n.v AS v" with v=2. Observe the long write refused with UnprocessableException / INTERNAL_LIMIT_EXCEEDED "the query builds a value of 100001 and this endpoint builds at most 100000" seconds in, instead of ConflictException / CONCURRENT_MODIFICATION or UnprocessableException / QUERY_TIMEOUT at its 300 s deadline.
5. Re-replay: neptune-graph.cancel-query-never-issued-query-id-rejected, neptune-graph.execute-query-past-its-timeout-rejected, neptune-graph.execute-query-write-overtaken-by-committed-write-conflict-rejected.
