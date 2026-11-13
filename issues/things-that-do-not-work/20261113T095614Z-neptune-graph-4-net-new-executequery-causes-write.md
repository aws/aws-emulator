Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune-graph.neptune-graph-execute-query-open-cypher-roundtrip neptune-graph.get-graph-summary-reflects-written-data neptune-graph.get-graph-summary-detailed-mode-includes-structures neptune-graph.get-graph-reflects-vector-search-configuration neptune-graph.neptune-graph-reset-graph-empties-data neptune-graph.neptune-graph-execute-query-parameterized-binding neptune-graph.concurrent-merge-no-duplicate-node neptune-graph.snapshot-create-and-restore-roundtrip neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable neptune-graph.stopped-graph-query-rejected
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph|ExecuteQuery|ValidationException
Issue-Key: neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable|run#8|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: neptune-graph.stopped-graph-query-rejected|run#8|AssertionError|assert code == "InternalServerErrorException", f"expected InternalServerErrorException on a STOPPED graph, got {code!r}"
Issue-Commit: 03e920c0d4c12b32c4ff6e56db2bffeedbd15a95

# neptune-graph: 4 net-new ExecuteQuery causes — write/mutation openCypher is refused as "not supported by this endpoint", `parameters` is never bound so `$name` is a syntax error, the parse-error message is not AWS's, and a STOPPED graph answers 404 instead of InternalServerErrorException

> _Note: this issue was identified against the code base at commit `03e920c0d4c12b32c4ff6e56db2bffeedbd15a95`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteQuery

## Description
ExecuteQuery is now implemented (read-only `RETURN 1 AS x` succeeds, 24/36 neptune-graph checks pass), so the open stub issue 20261023T175427Z-neptune-graph-executequery-stub.md no longer covers what follows. Four distinct causes remain, all in neptune-graph, all in ExecuteQuery's handler/engine. Smithy model consulted: services/service-com-amazonaws-neptunegraph-amazonneptunegraph/smithy-model/com-amazonaws-neptunegraph-amazonneptunegraph.json.

=== Cause 1 of 4: mutating openCypher (CREATE/MERGE/SET/DELETE) is refused instead of executed, so nothing can ever be written through the data plane ===
AWS: ExecuteQuery (POST /queries, hostPrefix "{graphIdentifier}.", input graphIdentifier/queryString(jsonName "query")/language(enum OPEN_CYPHER)/parameters/planCache/explainMode/queryTimeoutMilliseconds; output `payload` @httpPayload @streaming @required) runs READ AND WRITE openCypher on a Neptune Analytics graph. The operation's own modeled documentation names the three IAM actions it is authorized against — `neptune-graph:ReadDataViaQuery`, `neptune-graph:WriteDataViaQuery`, `neptune-graph:DeleteDataViaQuery` — i.e. mutation is part of this operation's contract, and `planCache`'s documentation states the plan cache "works for both READ and UPDATE queries". Recorded behavior: `CREATE (p:Person {name: 'Bob'})` on an AVAILABLE graph answers HTTP 200 with a payload whose `results` is the clause's own result set ([] for a bare CREATE), the created nodes/edges are then matchable by a subsequent MATCH in the same graph, and they are counted by GetGraphSummary. No ValidationException is produced for a well-formed write clause.
Emulator: every write clause is refused with HTTP 400 ValidationException, reason UNSUPPORTED_OPERATION, message "the CREATE clause is not supported by this endpoint: it runs read-only openCypher" (fixed text "the " + <clause> + " clause is not supported by this endpoint: it runs read-only openCypher", built in services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/execute_query.rs `query_refusal`, arm `crate::state::QueryFailure::Unsupported`). The handler's own module comment states "The engine's own subset is read-only openCypher". Because no data can be written, every check that writes and then reads fails at the first write, and GetGraphSummary/ResetGraph can only ever be observed over an empty graph.
Why: the whole write half of the Neptune Analytics data plane is unreachable. A client that loads a graph with openCypher, or that MERGEs on an upsert path, is told its well-formed query is unsupported — an UNSUPPORTED_OPERATION ValidationException is a permanent client fault, so the client stops rather than retrying, and no graph can ever hold data for the read paths (MATCH, GetGraphSummary, ResetGraph, snapshot/restore) to be exercised against.
Fix: ExecuteQuery's query engine and the state it writes (src/ops/execute_query.rs + crate::state::run_query and the `StoredGraph` trait it evaluates through) — the engine must evaluate mutating clauses against the graph's stored nodes/edges and commit them under the store's guard, instead of classifying them as QueryFailure::Unsupported; `QueryFailure::Unsupported` should then only cover clauses AWS itself does not accept.
Traces: neptune-graph.neptune-graph-execute-query-open-cypher-roundtrip, neptune-graph.get-graph-summary-reflects-written-data, neptune-graph.get-graph-summary-detailed-mode-includes-structures, neptune-graph.get-graph-reflects-vector-search-configuration, neptune-graph.neptune-graph-reset-graph-empties-data.

=== Cause 2 of 4: the request's `parameters` are never bound, so a parameterized query fails as a syntax error on `$` ===
AWS: Smithy: ExecuteQueryInput.parameters targets DocumentValuedMap (map<String, Document>), documented "The data parameters the query can use in JSON format. For example: {\"name\": \"john\", \"age\": 20}". A `$name` reference in queryString is bound to the matching entry of `parameters` as DATA, not spliced into the query text: the query parses, the value is matched/stored opaquely (including a value containing the openCypher string delimiter, e.g. "O'Brien"), and a parameter set that does not match returns an empty result set rather than an error.
Emulator: `parameters` is accepted by the deserializer and then dropped — src/ops/execute_query.rs passes only `input.query_string` to `crate::state::run_query(&data, &text, deadline)` and never reads `input.parameters`. The `$` therefore reaches the lexer as query text and the call is refused with HTTP 400 ValidationException, reason MALFORMED_QUERY, message "Invalid input '$': unexpected character `$` (line 1, column 25 (offset: 24))" — the position varies with where the first `$` sits in the submitted text; the "unexpected character `$`" classification is the defect.
Why: a client that does the correct thing — keep user data out of the query text and send it in `parameters` — cannot run any query at all, and is told its query is malformed when it is not. The only way to get a value into a query against the emulator is to string-format it into the text, which is exactly the injection-prone pattern `parameters` exists to prevent, so applications validated against the emulator will ship the unsafe form.
Fix: ExecuteQuery's handler plus the query engine (src/ops/execute_query.rs and crate::state::run_query) — thread `input.parameters` into the evaluation and bind `$<name>` references to those Document values as data; an unbound `$<name>` should be refused as a missing-parameter ValidationException, never as an unexpected `$` character.
Traces: neptune-graph.neptune-graph-execute-query-parameterized-binding, neptune-graph.concurrent-merge-no-duplicate-node, neptune-graph.snapshot-create-and-restore-roundtrip.

=== Cause 3 of 4: a genuine openCypher syntax error is refused with the emulator's own parser wording, not AWS's parse-error message ===
AWS: Smithy models ValidationException @httpError 400 for ExecuteQuery; the message is the openCypher parser's own parse error, of the form "Invalid input '<offending-token>': expected <the parser's alternatives list> (line <L>, column <C> (offset: <O>))". Fixed text: "Invalid input '", "': expected ", " (line ", ", column ", " (offset: ", "))". Variable parts, all derived from the submitted queryString: the offending token, the list of alternatives the grammar admits at that position, and the 1-based line/column plus 0-based offset. For a node pattern missing its closing parenthesis the recorded alternatives list is "whitespace, comment, COPY, node labels, a property map, ')', MapLiteral or a relationship pattern".
Emulator: the status (400) and code (ValidationException) are now right, but the message is the emulator's own shorter wording — "Invalid input 'R': expected `)` to close a node pattern (line 1, column 10 (offset: 9))" — which replaces AWS's alternatives list with a hand-written explanation and uses backticks where AWS uses a quoted token list. Built by crate::state::syntax_refusal from ::cypher_parser::CypherError::Syntax (src/ops/execute_query.rs `query_refusal`, MALFORMED_QUERY arm).
Why: the parse-error message is the only machine- and human-readable description of WHERE a query is wrong; tooling and tests that match on it (and users reading it) get text that exists nowhere in AWS. Note this cause is the surviving half of the already-open 20261104T060310Z-neptune-graph-executequery-answers-notimplemented-501-for-a-malformed-opencypher.md: its 501→400/ValidationException part is fixed, the message requirement it recorded is not. Close them together.
Fix: the syntax-refusal message construction (crate::state::syntax_refusal and the `cypher-parser` crate's Syntax error) — emit the expected-alternatives list the grammar admits at the failing position in AWS's wording and punctuation, with line/column/offset computed from the request's query text.
Traces: neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable.

=== Cause 4 of 4: ExecuteQuery against a STOPPED graph is refused as a missing resource (404) instead of InternalServerErrorException ===
AWS: recorded behavior only — the wire error code for this path, "InternalServerErrorException", is NOT a shape this service's model declares (the model's server fault is InternalServerException, @httpError 500); the recording shows the service emitting "InternalServerErrorException" with message "Request cannot be processed as graph is unavailable" (that whole sentence is fixed text and interpolates nothing from the request or state) when ExecuteQuery is called on a graph whose status is STOPPED. The graph still EXISTS — GetGraph answers for it and StartGraph brings it back — so the refusal is a server-side "not serving right now", not a 404 and not the control plane's ConflictException. After StartGraph returns the graph to AVAILABLE, the identical query succeeds again.
Emulator: answers ResourceNotFoundException (HTTP 404, the graph-not-found message) for the same request. src/ops/execute_query.rs folds "not serving" into "not found": `store.settled_graph(...).filter(|graph| graph.status.is_serving())` and a single `else` arm returning `crate::state::graph_not_found(&input.graph_identifier)`, so a graph the Region does not hold and a graph that is merely STOPPED are reported identically.
Why: a client cannot tell a deleted graph from a stopped one. 404 reads as "this graph is gone, stop and clean up"; the real answer is a retryable server fault that resolves by starting the graph. Callers that key cleanup or alarms on 404 will discard a live graph's configuration, and retry policies that back off on 5xx never engage.
Fix: ExecuteQuery's handler (src/ops/execute_query.rs, the `settled_graph`/`is_serving` lookup) — split the two facts: a graph this Region does not hold stays ResourceNotFoundException, while a graph that exists but is not serving is refused with the InternalServerErrorException wire code and AWS's fixed sentence; keep the graph untouched so StartGraph followed by the same query succeeds.
Traces: neptune-graph.stopped-graph-query-rejected.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): 24/39 passed (61.5%). neptune-graph 24/36, 12 failed; 2 of those are OWNED by 20261108T135214Z-neptune-graph-2-net-new-causes-startimporttask-never.md and the 3 cloudformation__* failures are OWNED by the two 20261109T115129Z CFN issues — the 10 remaining failures are the ones clustered here.

Cause 1 — write clauses refused (5 checks, identical error):
  ExecuteQuery(graphIdentifier=<g-...>, queryString="CREATE (p:Person {name: 'Bob'})", language="OPEN_CYPHER") on an AVAILABLE graph
    expected: HTTP 200, payload JSON with "results" (the clause's result set), and the node visible to a later MATCH / counted by GetGraphSummary
    actual:   botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the ExecuteQuery operation: the CREATE clause is not supported by this endpoint: it runs read-only openCypher   (HTTP 400, reason UNSUPPORTED_OPERATION)
  Same error aborts: neptune-graph-execute-query-open-cypher-roundtrip, get-graph-summary-reflects-written-data, get-graph-summary-detailed-mode-includes-structures, get-graph-reflects-vector-search-configuration, neptune-graph-reset-graph-empties-data.
  Source: src/ops/execute_query.rs lines ~416-424 (`QueryFailure::Unsupported` arm of `query_refusal`); module comment lines 11-13 ("The engine's own subset is read-only openCypher").

Cause 2 — `parameters` ignored (3 checks):
  ExecuteQuery(queryString="CREATE (p:Person {name: $name, age: $age})", parameters={"name": "Bob", "age": 25}, language="OPEN_CYPHER")
    expected: parameters bound as data; the node is created and then matched by "MATCH (p:Person {name: $name, age: $age}) RETURN p.name AS name, p.age AS age" with the same parameters -> results [{"name": "Bob", "age": 25}]; a non-matching parameter set -> results []; a value containing a quote ("O'Brien") stored and matched intact
    actual:   ValidationException: Invalid input '$': unexpected character `$` (line 1, column 25 (offset: 24))
    also:     ValidationException: Invalid input '$': unexpected character `$` (line 1, column 23 (offset: 22))  — concurrent-merge-no-duplicate-node and snapshot-create-and-restore-roundtrip, whose first query also references a `$` parameter
  Source: src/ops/execute_query.rs line 284 (`let text = input.query_string.clone();`) and line 289 (`crate::state::run_query(&data, &text, deadline)`) — `input.parameters` is never read anywhere in the handler.

Cause 3 — parse-error message wording ([mismatch] neptune-graph-execute-query-invalid-syntax-unprocessable):
  ExecuteQuery(queryString="MATCH (n RETURN n", language="OPEN_CYPHER")
    expected message: "Invalid input 'R': expected whitespace, comment, COPY, node labels, a property map, ')', MapLiteral or a relationship pattern (line 1, column 10 (offset: 9))"
    actual message:   "Invalid input 'R': expected `)` to close a node pattern (line 1, column 10 (offset: 9))"
    AssertionError: expected message '...expected whitespace, comment, COPY, node labels, a property map, \')\', MapLiteral or a relationship pattern...', got '...expected `)` to close a node pattern...'
  (Status 400 and code ValidationException now match; only the message diverges. Fixed text = "Invalid input '<tok>': expected <alternatives> (line <L>, column <C> (offset: <O>))"; <tok>, <alternatives>, <L>, <C>, <O> are derived from the submitted query text, never constants.)
  Source: src/ops/execute_query.rs lines ~386-392 -> crate::state::syntax_refusal.

Cause 4 — STOPPED graph ([mismatch] neptune-graph.stopped-graph-query-rejected):
  CreateGraph -> AVAILABLE; ExecuteQuery("RETURN 1 AS x") -> 200, results [{"x": 1}]  (positive control PASSES)
  StopGraph -> poll GetGraph until status STOPPED; ExecuteQuery("RETURN 1 AS x")
    expected: error code "InternalServerErrorException", message "Request cannot be processed as graph is unavailable"
    actual:   error code "ResourceNotFoundException" (HTTP 404, graph-not-found message)
    AssertionError: expected InternalServerErrorException on a STOPPED graph, got 'ResourceNotFoundException'
  Source: src/ops/execute_query.rs lines ~245-257 — `settled_graph(...).filter(|g| g.status.is_serving())` with a single `graph_not_found` else-arm.

## Steps to Reproduce
Boot the emulator from this checkout and point a botocore neptune-graph client at it. Queries use a client with retries total_max_attempts=1 (ExecuteQuery answers an SDK retry with UnprocessableException "Retry for SDK query requests is suppressed").

Common setup: ng.create_graph(graphName=<unique>, provisionedMemory=16, deletionProtection=False, replicaCount=0, publicConnectivity=True); poll get_graph until status == 'AVAILABLE'.

Cause 1:
1. ng.execute_query(graphIdentifier=<id>, queryString="CREATE (p:Person {name: 'Bob', age: 25})", language="OPEN_CYPHER").
2. Observe HTTP 400 ValidationException "the CREATE clause is not supported by this endpoint: it runs read-only openCypher"; expected HTTP 200 with a results payload and the node persisted.
3. ng.execute_query(..., queryString="MATCH (p:Person) RETURN p.name AS name") and ng.get_graph_summary(graphIdentifier=<id>, mode='DETAILED') — both can only ever report an empty graph.

Cause 2:
1. ng.execute_query(graphIdentifier=<id>, queryString="MATCH (p:Person {name: $name, age: $age}) RETURN p.name AS name, p.age AS age", parameters={"name": "Bob", "age": 25}, language="OPEN_CYPHER").
2. Observe HTTP 400 ValidationException "Invalid input '$': unexpected character `$` (line 1, column <col> (offset: <col-1>))"; expected the query to parse with $name/$age bound to the `parameters` values (result set, or [] when nothing matches).
3. Repeat with parameters={"name": "O'Brien"} — the value must be treated as opaque data, never spliced into the query text.

Cause 3:
1. ng.execute_query(graphIdentifier=<id>, queryString="MATCH (n RETURN n", language="OPEN_CYPHER").
2. Observe message "Invalid input 'R': expected `)` to close a node pattern (line 1, column 10 (offset: 9))"; expected "Invalid input 'R': expected whitespace, comment, COPY, node labels, a property map, ')', MapLiteral or a relationship pattern (line 1, column 10 (offset: 9))".

Cause 4:
1. ng.execute_query(graphIdentifier=<id>, queryString="RETURN 1 AS x", language="OPEN_CYPHER") -> 200 with results [{"x": 1}] (baseline).
2. ng.stop_graph(graphIdentifier=<id>); poll get_graph until status == 'STOPPED'.
3. Repeat step 1. Observe ResourceNotFoundException (404); expected error code "InternalServerErrorException" with message "Request cannot be processed as graph is unavailable".
4. ng.start_graph(graphIdentifier=<id>); poll until 'AVAILABLE'; repeat step 1 and confirm it succeeds again.
