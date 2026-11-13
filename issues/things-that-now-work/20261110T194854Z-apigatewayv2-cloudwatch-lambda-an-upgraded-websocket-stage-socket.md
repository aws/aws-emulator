Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes|_recv_exact#19|AssertionError|raise AssertionError(f"WebSocket closed after {len(buf)} of {n} expected bytes")
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__cloudwatch__lambda: an upgraded WebSocket stage socket is closed with no frame ever sent, so connect/message/reply/push metrics stay empty

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
ONE net-new root cause on the apigatewayv2__cloudwatch__lambda service token.

=== Cause 1 of 1: the WebSocket stage now completes the handshake but then closes the upgraded socket without delivering any frame, so the client's read returns 0 bytes and no stage metric series is produced ===

AWS: recorded behavior only for the data plane (the metric surface is CloudWatch GetMetricStatistics/GetMetricData over AWS/ApiGateway). Recorded: after a 101 upgrade the socket STAYS OPEN for the life of the connection; frames the client sends are dispatched to the selected route's integration, a two-way route's reply and an out-of-band PostToConnection push arrive back as TEXT frames, and only an explicit close (client close frame, DeleteConnection, or idle/limit timeout) closes it — with a WebSocket close frame, not a bare TCP teardown. The stage then publishes AWS/ApiGateway ConnectCount/MessageCount/IntegrationError-style series under {ApiId, Stage}.

Emulator: the handshake no longer 500s — it upgrades — but the first frame the client waits for never arrives and the socket is closed underneath it: `AssertionError: WebSocket closed after 0 of 2 expected bytes` raised from the check's `_recv_exact(sock, 2)`, i.e. the read of the 2-byte WebSocket frame header returned EOF. No reply frame, no push, and therefore no datapoint in any of the stage's metric series.

Why: a WebSocket client that connects successfully and is then dropped mid-session cannot send or receive anything, and the session is torn down without a close frame so the client cannot even learn a close code/reason. Everything behind the session — route dispatch, two-way replies, server pushes — is unreachable, and monitoring built on the stage's AWS/ApiGateway series never sees traffic because none is ever counted.

Fix: the apigatewayv2 WEBSOCKET execute-api session layer that owns the socket after the upgrade (frame read/write loop, route dispatch, keeping the connection registered until an explicit close) and the stage metric emission hooked off it; a session that must end has to send a WebSocket close frame with a code rather than dropping the TCP connection.

Related, does NOT cover this check: 20261109T154749Z-apigatewayv2-cloudwatch-lambda-2-net-new-causes-the.md lists this same check id under a DIFFERENT failure mode (its Cause 2: 'every WebSocket stage answers the RFC 6455 handshake 500', key `_connect#...|AssertionError|stage ... never upgraded a WebSocket handshake`). The replay reports no owner for the failure seen now — the handshake upgrades and the post-upgrade session is what breaks — so this is a net-new cause behind it.

Traces: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes.

## Evidence
Suite: simparity-behavioral-aws-v1. apigatewayv2__cloudwatch__lambda 0/1 (0%), 1 failed, not marked ALREADY FILED or OWNED.

[mismatch] apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
  File "check_...obs_apigatewayv2_1_websocket_stage_counts_connects_frames_replies_and_pushes.py", line 364, in _recv_frame -> b1, b2 = _recv_exact(sock, 2)
  File "...", line 356, in _recv_exact -> raise AssertionError(f"WebSocket closed after {len(buf)} of {n} expected bytes")
  AssertionError: WebSocket closed after 0 of 2 expected bytes

Expected (recorded AWS): the upgraded socket stays open; the client's three text frames are dispatched ($default on one stage, a two-way 'reply' route and a push request on the other), the reply/push arrive as TEXT frames, and the stage's AWS/ApiGateway series under {ApiId, Stage} gain ConnectCount/MessageCount datapoints.
Actual (emulator): the socket reaches EOF before the 2-byte frame header of the first expected frame — closed with no WebSocket close frame and no data — so no frame and no metric datapoint is ever produced.

## Steps to Reproduce
1. CreateApi(ProtocolType='WEBSOCKET') with AWS_PROXY Lambda integrations on $connect/$default and a two-way 'reply' route (CreateRoute + CreateRouteResponse), grant lambda:InvokeFunction to apigateway.amazonaws.com, deploy and create the stages the check uses.
2. Open an RFC 6455 handshake to a stage URL — it upgrades (101).
3. Send a text frame and read the next frame: the read returns EOF ('WebSocket closed after 0 of 2 expected bytes') instead of the integration's reply or the PostToConnection push.
4. Query CloudWatch for AWS/ApiGateway ConnectCount/MessageCount under {ApiId, Stage}: no datapoints.
5. After fixing, re-replay apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes.
