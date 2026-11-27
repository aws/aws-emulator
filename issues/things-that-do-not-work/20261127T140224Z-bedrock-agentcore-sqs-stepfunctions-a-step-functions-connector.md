Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__sqs__stepfunctions.gateway-states-send-task-success-invalid-output-rejected-then-resumes
Issue-Key: bedrock-agentcore__sqs__stepfunctions.gateway-states-send-task-success-invalid-output-rejected-then-resumes|_rpc#17|AssertionError|assert "result" in reply, f"MCP {method} returned no result: {reply!r}"
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agentcore__sqs__stepfunctions: a Step Functions connector gateway target advertises no tools, so the SendTaskSuccess callback tool cannot be called

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayTarget

## Description
Cause 1 of 1: a gateway target whose targetConfiguration names the built-in Step Functions connector (source connectorId 'states') contributes no tools to the gateway's MCP endpoint, so the callback scenario cannot issue its tool call.

AWS: recorded behavior only (the connector projection is not stated in the model). The recording shows a connector target being projected into tools/list as one tool per configured operation, named '<targetName>___<OperationName>' (here '<target>___StartExecution'), and tools/call on that name invoking the Step Functions operation — including the SendTaskSuccess path that resumes a waiting execution and refuses an invalid output first.

Emulator: tools/call answers the JSON-RPC error {'code': -32602, 'message': 'Invalid params: this gateway serves no tool named <target>___StartExecution'} — the connector's tools are never advertised or dispatched.

Why: an agent wired to Step Functions through a connector target can call nothing, so no execution is started, no task token is delivered to the queue, and no callback can be completed.

Fix: the connector-target tools/list projection and tools/call dispatch in the gateway data plane (the same component that projects Lambda targets).

Traces: the check below; the same root cause is reported for this service's sibling heading in the bedrock-agentcore__stepfunctions issue filed alongside this one.

## Evidence
Expected: tools/call '<target>___StartExecution' starts the pinned state machine and returns its result.
Actual: `AssertionError: MCP tools/call returned no result: {'jsonrpc': '2.0', 'id': 1, 'error': {'code': -32602, 'message': 'Invalid params: this gateway serves no tool named wfc8da7a892593___StartExecution'}}`.

## Steps to Reproduce
1. Create a Standard state machine with a .waitForTaskToken task that writes the token to an SQS queue.
2. CreateGateway (MCP) and CreateGatewayTarget with targetConfiguration.mcp.connector.source.connectorId='states' and a configuration for StartExecution (and the callback operations).
3. Send a signed tools/list — no connector tools appear — and tools/call '<target>___StartExecution', which answers JSON-RPC -32602.
