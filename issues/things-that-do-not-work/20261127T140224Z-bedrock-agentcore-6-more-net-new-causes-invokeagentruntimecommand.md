Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.runtime-command-missing-executable-rejected-as-runtime-client-error bedrock-agentcore.runtime-command-stream-orders-events-and-reports-exit-status bedrock-agentcore.runtime-command-timeout-and-length-bounds-rejected bedrock-agentcore.payment-instrument-balance-unknown-instrument-rejected bedrock-agentcore.harness-inline-tool-call-streams-content-block-start bedrock-agentcore.invoke-harness-final-tool-use-message-rejected-in-stream bedrock-agentcore.invoke-harness-stream-closes-text-block-before-metadata bedrock-agentcore.invoke-harness-stream-frames-each-message-start-to-stop bedrock-agentcore.invoke-harness-unknown-target-not-found-rejected bedrock-agentcore.retrieve-memory-records-top-k-not-max-results-sizes-the-one-page bedrock-agentcore.retrieve-memory-records-ranks-fresh-records-by-descending-score
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|InvokeAgentRuntimeCommand|UnknownService
Issue-Key: bedrock-agentcore.payment-instrument-balance-unknown-instrument-rejected|run#16|AssertionError|assert live_code == "ResourceNotFoundException", (
Issue-Key: bedrock-agentcore.harness-inline-tool-call-streams-content-block-start|run#7|AssertionError|raise AssertionError(f"the model answered without calling {_TOOL_NAME} in {_ATTEMPTS} turns")
Issue-Key: bedrock-agentcore.invoke-harness-final-tool-use-message-rejected-in-stream|run#10|AssertionError|assert str(sum(first)) in reply, f"control turn does not contain the sum {sum(first)}: {reply[-300:]!r}"
Issue-Key: bedrock-agentcore.invoke-harness-stream-closes-text-block-before-metadata|run#36|AssertionError|assert answer in reply, f"the streamed text deltas do not contain the sum {answer}: {reply[-300:]!r}"
Issue-Key: bedrock-agentcore.invoke-harness-stream-frames-each-message-start-to-stop|run#37|AssertionError|assert re.search(rf"(?<!\d){answer}(?!\d)", reply), (
Issue-Key: bedrock-agentcore.invoke-harness-unknown-target-not-found-rejected|_assert_harness_missing#19|AssertionError|assert message.startswith("Harness") and message.endswith(" not found."), (
Issue-Key: bedrock-agentcore.retrieve-memory-records-top-k-not-max-results-sizes-the-one-page|run#22|AssertionError|assert set(ids) == written, (
Issue-Key: bedrock-agentcore.retrieve-memory-records-ranks-fresh-records-by-descending-score|run#14|Timeout|hits = data.retrieve_memory_records(
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agentcore: 6 more net-new causes — InvokeAgentRuntimeCommand is unrouted (UnknownService), GetPaymentInstrumentBalance answers NotImplemented, harness turns serve a fixed notice, and RetrieveMemoryRecords mis-sizes and never settles

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeAgentRuntimeCommand

## Description
Six further net-new root causes in bedrock-agentcore, numbered with their own failing checks.

Cause 1 of 6: InvokeAgentRuntimeCommand is not routed at all.
AWS: Smithy: InvokeAgentRuntimeCommand is bound to the service as POST /runtimes/{agentRuntimeArn}/commands with a required streaming output (stream) and models AccessDeniedException 403, ResourceNotFoundException 404, RuntimeClientError 424, ValidationException 400. Recorded behavior: the command stream reports the ordered events and the exit status of the command, a missing executable is refused as RuntimeClientError, and timeout/length bounds are refused ValidationException.
Emulator: every call fails before reaching any handler with 404 UnknownService "no emulated service is registered for 'bedrock-agentcore'" — the command URI is not dispatched to the service.
Why: the entire runtime-command surface is unreachable, and the error a client sees (UnknownService) says the service does not exist rather than naming anything about the request. This is distinct from open issue 20261023T113357Z-bedrock-agentcore-invokeagentruntime-stub.md, which is the separate InvokeAgentRuntime operation.
Fix: route POST /runtimes/{agentRuntimeArn}/commands to the InvokeAgentRuntimeCommand handler in the service's request router (the same place /runtimes/{arn}/invocations is bound).
Traces: bedrock-agentcore.runtime-command-missing-executable-rejected-as-runtime-client-error, bedrock-agentcore.runtime-command-stream-orders-events-and-reports-exit-status, bedrock-agentcore.runtime-command-timeout-and-length-bounds-rejected.

Cause 2 of 6: GetPaymentInstrumentBalance answers NotImplemented.
AWS: Smithy: GetPaymentInstrumentBalance (POST /payments/getPaymentInstrumentBalance) models ResourceNotFoundException (404), AccessDeniedException (403) and ValidationException (400). Recorded behavior: a balance read naming a paymentInstrumentId that does not exist under a live payment manager is refused ResourceNotFoundException; once the manager is deleted the same read is refused AccessDeniedException 'Payment manager not found' (fixed text).
Emulator: the call answers error code 'NotImplemented'.
Why: 'NotImplemented' is not a modeled error for this operation, so a client's ResourceNotFoundException/AccessDeniedException handling never runs and the refusal cannot be told apart from an outage.
Fix: the GetPaymentInstrumentBalance handler (lookup + the two refusals) in the payments data plane.
Traces: bedrock-agentcore.payment-instrument-balance-unknown-instrument-rejected.

Cause 3 of 6: a harness turn streams a fixed 'no language model runs in this emulator' notice instead of the turn's answer, so no tool is ever called.
AWS: recorded behavior only. The recording shows InvokeHarness streaming the model's reply, which contains the value computed by the harness turn (e.g. the server-computed sum appears verbatim in the text deltas), and the model issuing a tool-use request for the registered inline tool before answering.
Emulator: the stream carries the constant text 'No language model runs in this emulator. <modelId> generated no text for this request. The request carried N message(s) and M input token(s).', so the sum never appears and 'the model answered without calling parcel_weight in 3 turns'.
Why: every harness scenario that depends on the turn's content — streamed deltas, tool-use framing, inline tool calls — reads a fixed notice, so no client can exercise the harness. Open issue 20261124T000000Z-bedrock-agentcore-invokeharness-streams-a-fixed-notice-instead-of-the-turns-answer.md already describes this behavior and owns one check; the four checks here are not carried by it.
Fix: the InvokeHarness turn execution / model-reply generation in the harness data-plane handler.
Traces: bedrock-agentcore.harness-inline-tool-call-streams-content-block-start, bedrock-agentcore.invoke-harness-final-tool-use-message-rejected-in-stream, bedrock-agentcore.invoke-harness-stream-closes-text-block-before-metadata, bedrock-agentcore.invoke-harness-stream-frames-each-message-start-to-stop.

Cause 4 of 6: InvokeHarness refuses an unknown target by naming the qualifier instead of reporting the harness not found.
AWS: recorded behavior only: invoking a harness ARN with a qualifier the harness does not have is refused with a message stating the harness was not found (the resource the caller named), not one about endpoints or agents.
Emulator: the refusal message is "No endpoint or agent found with qualifier '<qualifier>' for agent '<harness ARN>'" — agent/endpoint wording for a harness resource.
Why: a client that keys on the refusal text (or reports it to a user) is told the wrong resource kind was missing.
Fix: the not-found message rendering in the InvokeHarness handler.
Traces: bedrock-agentcore.invoke-harness-unknown-target-not-found-rejected.

Cause 5 of 6: RetrieveMemoryRecords sizes its page by maxResults instead of searchCriteria.topK.
AWS: Smithy: RetrieveMemoryRecords input has searchCriteria (required) whose topK member is @range 1..100 default 10 and is documented as 'The maximum number of top-scoring memory records to return. This value is used for semantic search ranking', while maxResults (@range 1..100, default 100) is the @paginated pageSize. Recorded behavior: topK=5 with maxResults=2 returns all 5 matching records in one page.
Emulator: the response carries only 2 records — the last two — so maxResults truncated the result set and topK was ignored.
Why: a client asking for the top K matches receives fewer (and, because the truncation is applied after ranking, the wrong) records.
Fix: apply topK to the ranked result set and maxResults only as the page size in the RetrieveMemoryRecords handler.
Traces: bedrock-agentcore.retrieve-memory-records-top-k-not-max-results-sizes-the-one-page.

Cause 6 of 6: records just written by BatchCreateMemoryRecords never become retrievable, so the ranking scenario reaches no verdict.
AWS: recorded behavior only: records accepted by BatchCreateMemoryRecords are returned by RetrieveMemoryRecords for a query equal to one of their texts, in descending score order, with the verbatim match first and scored above every other record.
Emulator: the check polls RetrieveMemoryRecords for the three records it just stored and never sees all of them; it is killed at the suite's 60s budget with no assertion reached. Hypothesis: RetrieveMemoryRecords does not index/return the records written through BatchCreateMemoryRecords in the same namespace (the ranking and score ordering are therefore also unverified).
Why: semantic retrieval of freshly written records — the primary read path of AgentCore memory — never answers, so a client polling for its own writes hangs.
Fix: the retrieval index/scoring the RetrieveMemoryRecords handler reads, and its agreement with the BatchCreateMemoryRecords write path.
Traces: bedrock-agentcore.retrieve-memory-records-ranks-fresh-records-by-descending-score.

## Evidence
Cause 1 — actual: `botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the InvokeAgentRuntimeCommand operation: no emulated service is registered for 'bedrock-agentcore'` (all three checks). Expected: the command's event stream, RuntimeClientError for a missing executable, ValidationException for the bounds.
Cause 2 — expected ResourceNotFoundException (and AccessDeniedException 'Payment manager not found' after the manager is deleted); actual `a balance read of an unknown instrument under a live manager answered 'NotImplemented'`.
Cause 3 — expected the server-computed sum in the streamed text (e.g. '14023', '9745', '77') and a tool-use request for the inline tool; actual the constant 'No language model runs in this emulator. us.amazon.nova-micro-v1:0 generated no text for this request. The request carried 1 message(s) and 21 input token(s).'
Cause 4 — actual message "No endpoint or agent found with qualifier 'nosuchendpoint' for agent 'arn:aws:bedrock-agentcore:…:harness/…'", expected one saying the harness is not found.
Cause 5 — expected all 5 stored records for topK=5 with maxResults=2; actual the last 2 ids only.
Cause 6 — `Error: Check timed out after 60.0s` while polling RetrieveMemoryRecords for the 3 records BatchCreateMemoryRecords had accepted.

## Steps to Reproduce
Cause 1: call InvokeAgentRuntimeCommand (POST /runtimes/{agentRuntimeArn}/commands) on any agent runtime.
Cause 2: CreatePaymentManager, wait until it is live, then GetPaymentInstrumentBalance with a paymentInstrumentId that was never created.
Cause 3: CreateHarness with an inline tool, InvokeHarness with a prompt whose answer the harness computes, and read the streamed text deltas.
Cause 4: InvokeHarness on an existing harness ARN with qualifier='nosuchendpoint'.
Cause 5: store 5 records in one namespace, then RetrieveMemoryRecords with searchCriteria={'searchQuery': …, 'topK': 5} and maxResults=2.
Cause 6: BatchCreateMemoryRecords with 3 records in a fresh namespace, then poll RetrieveMemoryRecords with one record's text as searchQuery.
