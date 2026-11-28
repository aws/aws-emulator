Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-list-invocations-request-validation
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-list-invocations-request-validation bedrock-agent-runtime-put-invocation-step-validation-errors bedrock-agent-runtime-rerank-number-of-results-bounded-by-source-count
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|ListInvocations|2xx->2xx|$.nextToken
Issue-Key: bedrock-agent-runtime|ListInvocationSteps|2xx->2xx|$.invocationStepSummaries[*]
Issue-Key: bedrock-agent-runtime|ListInvocationSteps|2xx->2xx|$.invocationStepSummaries[*].invocationStepId
Issue-Key: bedrock-agent-runtime|Rerank|2xx->2xx|$.results[*].index
Issue-Commit: 2bdcb6d32c1ef40e42e13d397b84071513486a08

# bedrock-agent-runtime success responses diverge: 4 net-new causes — ListInvocations omits nextToken, invocation-step writes do not overwrite by id, Rerank result indices wrong

> _Note: this issue was identified against the code base at commit `2bdcb6d32c1ef40e42e13d397b84071513486a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInvocations

## Description
Four divergences in which the emulator answers 200 but the body is not the body AWS returns: a pagination token is dropped, session invocation-step state is accumulated instead of replaced, and Rerank's result `index` does not identify the source document it scored. All four are in bedrock-agent-runtime and all four must be closed before this issue is submitted.

Cause 1 of 4: ListInvocations never returns a nextToken, so a truncated page looks like the last page.
AWS: Smithy: ListInvocations is @paginated{inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: invocationSummaries}; input `maxResults` is an Integer with @range 1..1000 and default 10; output `nextToken` is a NextToken with @length 1..2048 and pattern ^\S*$. AWS returns `nextToken` whenever more invocations remain beyond the requested page size; the token's value is opaque and environment-specific, so only its presence is a requirement.
Emulator: 200 with `invocationSummaries` but `$.nextToken` absent where the recording has a token.
Why: every AWS paginator stops when `nextToken` is absent. A client therefore silently receives only the first page of a session's invocations and treats it as the complete set — a correctness bug with no error to catch.
Fix: ListInvocations' handler — honour maxResults and emit an opaque, resumable `nextToken` whenever results are truncated (and accept it on the next request).
Traces: bedrock-agent-runtime-list-invocations-request-validation#task#8.

Cause 2 of 4: PutInvocationStep with an explicit invocationStepId appends a new step instead of replacing the existing one, and the id it stores is not the id supplied.
AWS: Smithy: PutInvocationStep is @idempotent, takes an optional `invocationStepId` (Uuid pattern), and returns the required `invocationStepId`; ListInvocationSteps returns InvocationStepSummary{sessionId, invocationId, invocationStepId, invocationStepTime}. Because the operation is idempotent and the caller may name the step, putting the same `invocationStepId` twice updates that one step; the step the caller named is the id that must come back from a subsequent ListInvocationSteps, and the list holds exactly one entry per distinct step id.
Emulator: after the recorded sequence of puts, ListInvocationSteps returns 200 with the first summary carrying a server-generated id instead of the caller-supplied one, plus two additional summaries the recording does not have — i.e. each put created a fresh step and the supplied id was discarded. (The specific uuids are per-run values, not requirements; the requirement is one entry per supplied id, carrying that id.)
Why: the operation's @idempotent contract is broken. A client that retries a put (the documented safe behaviour) silently duplicates session history, and a client that reads back a step by the id it chose cannot find it, so conversation history reconstructed from the emulator does not match AWS.
Fix: PutInvocationStep's state layer — key steps by `invocationStepId` within (session, invocation), upsert on a supplied id, and return/store that id; ListInvocationSteps then reports one summary per step.
Traces: bedrock-agent-runtime-put-invocation-step-validation-errors#task#8.

Cause 3 of 4: Rerank's `index` is the position in the result list, not the original index of the scored source, when results are reordered.
AWS: Smithy: RerankResult.index is a required Integer with @range 0..1000 documented as "The original index of the document from the input sources array", and results are ordered by descending relevanceScore. So when the most relevant document is not the first source, the first result's `index` is that source's position in the request's `sources` list, not 0.
Emulator: 200 with results whose `index` values are the output positions (first result index 0, second index 1) where the recording has them swapped — the emulator either does not reorder by score or renumbers after reordering.
Why: `index` is the only link from a result back to the document the caller submitted. If it is the output position, the caller attributes each relevance score to the wrong document, which corrupts every downstream use of reranking (citation, filtering, context assembly) while the response still looks well-formed.
Fix: Rerank's handler/serializer — carry each source's original index through scoring and sorting, and emit that original index in RerankResult.index.
Traces: bedrock-agent-runtime-rerank-number-of-results-bounded-by-source-count#task#3.

Cause 4 of 4: the same index defect when numberOfResults truncates the result list.
AWS: Smithy: BedrockRerankingConfiguration.numberOfResults is an Integer with @range 1..1000, "The number of results to return after reranking" — truncation happens after ranking, so a single returned result is the highest-scoring source and its `index` is that source's original position in `sources`.
Emulator: 200 with the single result's `index` reported as 0, i.e. the first source rather than the top-scoring one, so truncation is applied to the request order instead of the ranked order (or the index is renumbered to the output position).
Why: with numberOfResults < source count the caller keeps the wrong document entirely: it receives a score for a document AWS did not select, and nothing in the response reveals the substitution.
Fix: Rerank's handler — rank first, truncate to numberOfResults afterwards, and preserve the original source index on each surviving result.
Traces: bedrock-agent-runtime-rerank-number-of-results-bounded-by-source-count#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (65 fixtures, 302 steps, 267 passed, score 88.4%).

Cause 1 — ListInvocations, fixture bedrock-agent-runtime-list-invocations-request-validation step task#8: status 200 as expected, but $.nextToken missing (expected '<shaped-not-asserted>', got None).

Cause 2 — ListInvocationSteps, fixture bedrock-agent-runtime-put-invocation-step-validation-errors step task#8: status 200 as expected, but $.invocationStepSummaries[0].invocationStepId value_mismatch (expected '5e1f0c2a-7b3d-4e8f-9a6b-0019e551be01', got a freshly generated uuid) and $.invocationStepSummaries[1] / [2] extra (expected None, got two further summaries for the same sessionId/invocationId, one of them carrying the expected id '5e1f0c2a-...'). The uuids are per-run values; the requirement is one summary per supplied invocationStepId, carrying the supplied id.

Cause 3 — Rerank, fixture bedrock-agent-runtime-rerank-number-of-results-bounded-by-source-count step task#3: $.results[0].index expected 1, got 0; $.results[1].index expected 0, got 1.

Cause 4 — Rerank, same fixture step task#4: $.results[0].index expected 1, got 0 (single result after numberOfResults truncation).

## Steps to Reproduce
1. Clone the repo and build/boot the bedrock-agent-runtime emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Inspect the representative step bedrock-agent-runtime-list-invocations-request-validation#task#8 and each step in trace_ids:
   - ListInvocations on a session with more invocations than maxResults -> expect a nextToken in the 200 body.
   - PutInvocationStep twice with the same explicit invocationStepId, then ListInvocationSteps -> expect one summary per distinct step id, each carrying the caller-supplied id, with no extra appended steps.
   - Rerank where the most relevant source is not the first -> expect results ordered by descending relevanceScore with each result's index naming its original position in sources.
   - Rerank with numberOfResults smaller than the source count -> expect the top-scoring source(s), with original source indices.
4. All four must pass before this issue is closed.
