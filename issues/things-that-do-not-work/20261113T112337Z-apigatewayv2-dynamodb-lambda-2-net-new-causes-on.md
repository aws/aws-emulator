Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003|run#18|AssertionError|assert opcode == 0x8, (
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009|_ws_recv_exact#20|AssertionError|raise AssertionError(
Issue-Commit: 0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225

# apigatewayv2__dynamodb__lambda: 2 net-new causes on an upgraded WEBSOCKET stage — a binary client frame is echoed instead of refused with CLOSE 1003, and an oversize client message closes the socket with no CLOSE frame instead of CLOSE 1009

> _Note: this issue was identified against the code base at commit `0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
WebSocket frame dispatch (execute-api WEBSOCKET data plane)

## Description
Two distinct root causes in the WEBSOCKET data plane's frame handling, both reached now that the handshake upgrades.

Cause 1 of 2: a binary client frame is dispatched to the route integration instead of being refused with an RFC 6455 CLOSE. Failing check: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003.
AWS: recorded behavior only (the WEBSOCKET data plane is not modelled in Smithy). API Gateway v2 WebSocket APIs accept text frames only; a binary data frame (opcode 0x2) from the client is not routed to any integration — the gateway answers with a CLOSE frame (opcode 0x8) carrying status code 1003 ('unsupported data') and closes the connection. Nothing from the frame reaches the backend.
Emulator: the binary frame is routed to the $default integration and the backend's reply is written back as a text frame: the next frame the client reads has opcode 0x1 with a JSON body of the shape {"sha": "<sha256 of the bytes the handler saw>", "len": <byte count>} (one run: sha e9e20ac8e34fc66fdd87d70363085a7705b8750339974752be9166e0397d6885, len 25 — those values are that run's payload, the requirement is that no frame other than CLOSE 0x8/1003 is produced). No CLOSE frame and no close status is ever sent.
Why: a client that sends binary over a WebSocket API gets a successful round trip in the emulator and a refused, closed connection on AWS. Code that relies on the gateway to filter non-text payloads passes untrusted bytes into the backend, and client reconnect logic keyed on close code 1003 never runs.
Fix: the WEBSOCKET frame reader in the execute-api data plane — classify the frame opcode before route selection and answer a non-text data frame with CLOSE 1003 instead of invoking the route integration.

Cause 2 of 2: an oversize client message is not refused with CLOSE 1009 — the connection is dropped with no CLOSE frame at all. Failing check: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009.
AWS: recorded behavior only. A WebSocket API enforces a fixed, non-adjustable per-message ceiling of 128 KB on client-to-server messages (including a fragmented message whose assembled payload exceeds it). The gateway answers the offending message with a CLOSE frame (opcode 0x8) carrying status code 1009 ('message too big') and then closes; the integration is not invoked.
Emulator: the socket is closed without any frame being written — the client's next frame read ends immediately ('WebSocket closed after 0 of 1 expected bytes'), so no opcode and no close status are observable.
Why: the documented size ceiling is unobservable. A client cannot distinguish 'my message was too large' (1009, recoverable by chunking) from an abrupt transport failure, so retry/reconnect logic written against AWS's close code misbehaves, and the ceiling itself cannot be verified.
Fix: the same WEBSOCKET frame reader/limit enforcement path — accumulate fragments, compare the assembled payload against the 128 KB per-message ceiling, and emit CLOSE 1009 before closing rather than closing the TCP/TLS connection bare.

Related open issues that do NOT cover these checks (they describe a stage that never upgrades, or every frame answered with the gateway's 'Internal server error' frame): 20261103T042714Z-apigatewayv2-dynamodb-lambda-a-deployed-api-gateway-v2-websocket-stage-never.md, 20261029T180026Z-apigatewayv2-dynamodb-lambda-api-gateway-v2-websocket-oversize-frame-not-closed.md, 20261110T194854Z-apigatewayv2-dynamodb-lambda-every-frame-on-an-upgraded.md.

Traces: the two failing check ids above.

## Evidence
Cause 1 — wsduplex-client-binary-frame-rejected-close-1003: AssertionError: a binary client frame should be answered with an RFC-6455 CLOSE frame (opcode 0x8); got opcode 0x1 with payload b'{"sha": "e9e20ac8e34fc66fdd87d70363085a7705b8750339974752be9166e0397d6885", "len": 25}'. Expected: CLOSE frame, opcode 0x8, status code 1003, integration not invoked. Actual: text frame (0x1) carrying the backend's echo of the binary payload.

Cause 2 — wsduplex-oversize-client-message-rejected-close-1009: AssertionError: WebSocket closed after 0 of 1 expected bytes (raised while reading the first byte of the expected CLOSE frame header). Expected: CLOSE frame, opcode 0x8, status code 1009. Actual: connection closed with no frame written.

## Steps to Reproduce
1. Create a WEBSOCKET API (RouteSelectionExpression $request.body.action) with an AWS_PROXY $default integration and deploy a stage; complete an RFC 6455 upgrade.
2. Send a binary data frame (opcode 0x2) with any payload and read the next frame: AWS answers opcode 0x8 with close status 1003; the emulator answers opcode 0x1 with the integration's response.
3. On a fresh connection, send a single (or fragmented) text message whose assembled payload exceeds 128 KB and read the next frame: AWS answers opcode 0x8 with close status 1009; the emulator closes the socket with no frame.
