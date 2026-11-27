Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__lambda.gateway-lambda-target-non-json-payload-relayed-as-content-string bedrock-agentcore__lambda.gateway-lambda-target-response-over-six-mb-rejected bedrock-agentcore__lambda.gateway-lambda-tool-call-missing-required-argument-rejected bedrock-agentcore__lambda.gateway-streaming-response-interceptor-sees-is-streaming-response bedrock-agentcore__lambda.harness-hook-decision-outside-allow-deny-rejected-for-failure-mode bedrock-agentcore__lambda.harness-lambda-hook-events-report-applied-decisions bedrock-agentcore__lambda.harness-same-event-lambda-hooks-all-fire-and-any-deny-stops bedrock-agentcore__lambda.harness-before-tool-call-lambda-deny-skips-tool-and-loop-continues
Issue-Key: bedrock-agentcore__lambda.gateway-lambda-target-non-json-payload-relayed-as-content-string|run#24|AssertionError|assert (status, answer) == (200, wrapped), (
Issue-Key: bedrock-agentcore__lambda.gateway-lambda-target-response-over-six-mb-rejected|run#43|AssertionError|assert large == expected, (
Issue-Key: bedrock-agentcore__lambda.gateway-lambda-tool-call-missing-required-argument-rejected|run#27|AssertionError|assert got == want, f"the incomplete tools/call answered {got!r}, expected {want!r}"
Issue-Key: bedrock-agentcore__lambda.gateway-streaming-response-interceptor-sees-is-streaming-response|run#24|AssertionError|assert media == "text/event-stream", f"the SSE-accepting call was answered as {media!r}"
Issue-Key: bedrock-agentcore__lambda.harness-hook-decision-outside-allow-deny-rejected-for-failure-mode|_graded_legs#20|AssertionError|assert [(h.get("name"), h.get("decision"), h.get("reason")) for h in held_hooks] == [
Issue-Key: bedrock-agentcore__lambda.harness-lambda-hook-events-report-applied-decisions|_invoke#40|AssertionError|assert sorted(by_name) == ["hard_stop", "plain_allow", "strict_unreadable", "unreadable"], (
Issue-Key: bedrock-agentcore__lambda.harness-same-event-lambda-hooks-all-fire-and-any-deny-stops|attempt#10|AssertionError|raise AssertionError("CreateHarness accepted two hooks sharing one name")
Issue-Key: bedrock-agentcore__lambda.harness-before-tool-call-lambda-deny-skips-tool-and-loop-continues|run#31|AssertionError|assert asked, (
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agentcore__lambda: 7 more causes — Lambda-target payload and limit handling, unvalidated tool arguments, no SSE for MCP, and harness Lambda hooks that never fire

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeHarness

## Description
Seven further net-new root causes in the Lambda-backed AgentCore surfaces, each numbered with its own failing checks.

Cause 1 of 7: a non-JSON Lambda target payload is relayed as raw text instead of being wrapped.
AWS: recorded behavior only: when the target function returns a payload that is not JSON (e.g. the bare token NaN), the gateway answers the tools/call with one text content item holding {"content":"<payload>"} and isError false.
Emulator: the content item holds the raw payload text ('NaN') with no wrapper.
Why: a client parsing tool content as the documented JSON envelope gets a value it cannot parse.
Fix: the Lambda-target response-to-MCP-content conversion in the gateway data plane.
Traces: bedrock-agentcore__lambda.gateway-lambda-target-non-json-payload-relayed-as-content-string.

Cause 2 of 7: an over-limit Lambda response leaks the runtime's error text instead of AWS's generic tool error.
AWS: recorded behavior only: when the target function's response exceeds Lambda's payload limit, the tools/call answers isError true with exactly one text item 'An internal error occurred. Please retry later.' (fixed text, no values).
Emulator: the text is 'Unhandled: {"errorType":"Function.ResponseSizeTooLarge","errorMessage":"Response payload size exceeded maximum allowed payload size (6291556 bytes)."}'.
Why: the gateway leaks the target's internal failure detail, and clients matching the documented sentence do not recognise the error.
Fix: error normalisation in the gateway's Lambda-target invocation path.
Traces: bedrock-agentcore__lambda.gateway-lambda-target-response-over-six-mb-rejected.

Cause 3 of 7: tools/call arguments are not validated against the tool's input schema.
AWS: recorded behavior only: a tools/call missing a required argument is answered isError true with the text "ValidationException - Parameter validation failed: Invalid request parameters:\n- Missing required field(s): '<member>'" (fixed text; the member name comes from the tool schema) and the target function is not invoked.
Emulator: the call succeeds — the function is invoked with the incomplete arguments and its result is returned (isError false).
Why: a target function receives arguments its schema forbids, so the schema the gateway advertises is not enforced anywhere.
Fix: argument validation against the target's tool schema before invoking, in the gateway tools/call dispatcher.
Traces: bedrock-agentcore__lambda.gateway-lambda-tool-call-missing-required-argument-rejected.

Cause 4 of 7: the gateway MCP endpoint never answers as an event stream.
AWS: recorded behavior only: a tools/call whose Accept includes text/event-stream is answered with Content-Type text/event-stream, and a response interceptor sees isStreamingResponse true for it.
Emulator: the SSE-accepting call is answered as 'application/json'.
Why: MCP clients that negotiate SSE (the streamable-HTTP transport) cannot stream from the gateway, and the streaming flag an interceptor branches on is never set.
Fix: content negotiation and SSE framing in the gateway MCP endpoint's response path.
Traces: bedrock-agentcore__lambda.gateway-streaming-response-interceptor-sees-is-streaming-response.

Cause 5 of 7: harness Lambda hooks never fire and the stream carries no hookEvent.
AWS: recorded behavior only: a harness with Lambda hooks reports each applied decision as a hookEvent in the InvokeHarness stream (the allow/deny decision, the hook name and the event it was attached to), and a valid deny surfaces as exactly one hookEvent.
Emulator: hookEvents is [] and the stream's event kinds are only ['messageStart', 'contentBlockDelta', 'contentBlockStop', 'messageStop', 'metadata'].
Why: a client cannot observe or audit hook decisions, and a deny that should be visible (and should stop the turn) is invisible.
Fix: hook invocation and hookEvent emission in the InvokeHarness streaming handler.
Traces: bedrock-agentcore__lambda.harness-hook-decision-outside-allow-deny-rejected-for-failure-mode, bedrock-agentcore__lambda.harness-lambda-hook-events-report-applied-decisions.

Cause 6 of 7: CreateHarness accepts two hooks sharing one name.
AWS: recorded behavior only: CreateHarness refuses a hook list that repeats a hook name, so the harness is not created.
Emulator: the harness is created with both hooks ('CreateHarness accepted two hooks sharing one name').
Why: which hook a decision came from is undefined, and a client's duplicate registration is never reported.
Fix: duplicate-name validation over the hook list in the CreateHarness handler.
Traces: bedrock-agentcore__lambda.harness-same-event-lambda-hooks-all-fire-and-any-deny-stops.

Cause 7 of 7: a harness turn serves the fixed no-model notice, so the before_tool_call hook never has a tool call to decide on.
AWS: recorded behavior only: the harness turn requests the registered tool, and the before_tool_call hook's deny then skips that tool while the loop continues.
Emulator: the turn ends with stop reason 'end_turn' and no tool request — 'the model made no lookup_code request, so the before_tool_call hook had no call to decide'. Open issue 20261124T000000Z-bedrock-agentcore-invokeharness-streams-a-fixed-notice-instead-of-the-turns-answer.md describes the same fixed-notice behavior but does not carry this check.
Why: hook-mediated tool control cannot be exercised at all.
Fix: the InvokeHarness turn execution (tool-use generation) in the harness data-plane handler.
Traces: bedrock-agentcore__lambda.harness-before-tool-call-lambda-deny-skips-tool-and-loop-continues.

## Evidence
Cause 1 — expected {'content': [{'type': 'text', 'text': '{"content":"NaN"}'}]}; actual {'content': [{'type': 'text', 'text': 'NaN'}]}.
Cause 2 — expected one text item 'An internal error occurred. Please retry later.' with isError true; actual 'Unhandled: {"errorType":"Function.ResponseSizeTooLarge","errorMessage":"Response payload size exceeded maximum allowed payload size (6291556 bytes)."}'.
Cause 3 — expected (True, [{'type': 'text', 'text': "ValidationException - Parameter validation failed: Invalid request parameters:\n- Missing required field(s): 'right'"}]); actual (False, [{'type': 'text', 'text': '{"total":5,"mirrored":"…"}'}]).
Cause 4 — actual: 'the SSE-accepting call was answered as application/json'.
Cause 5 — actual `hookEvents named []; stream event kinds were ['messageStart', 'contentBlockDelta', 'contentBlockStop', 'messageStop', 'metadata']` and 'the valid deny should surface as one hookEvent, got []'.
Cause 6 — actual 'CreateHarness accepted two hooks sharing one name'.
Cause 7 — actual "stop reasons ['end_turn'], event kinds ['messageStart', 'contentBlockDelta', 'contentBlockStop', 'messageStop', 'metadata']" with no tool request.

## Steps to Reproduce
Cause 1: Lambda target returning the bare token NaN; tools/call it and read the content item.
Cause 2: Lambda target returning a >6 MB payload; tools/call it.
Cause 3: tools/call a Lambda-target tool omitting a required argument of its schema.
Cause 4: tools/call with Accept: application/json, text/event-stream and inspect the response Content-Type.
Cause 5/6/7: CreateHarness with Lambda hooks (one pair sharing a name; one before_tool_call hook that denies), then InvokeHarness and read the stream's hookEvents and tool-use events.
