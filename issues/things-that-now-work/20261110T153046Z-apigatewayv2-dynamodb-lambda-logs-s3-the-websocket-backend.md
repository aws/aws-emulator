Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle
Issue-Key: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle|_ws_connect#9|Timeout|status, ws, head = _ws_open(url)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigatewayv2__dynamodb__lambda__logs__s3: the WebSocket backend-response lifecycle check reaches no verdict — the scenario never completes within the run budget (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: the end-to-end WebSocket debugging lifecycle (deploy, setup, pre_invoke sessions, solve, cleanup, destroy) never progresses to a verdict.
AWS: each phase's calls return within seconds — the stack deploys, the agent sessions open RFC 6455 connections to the deployed WebSocket stage and are routed (register_ui, register, agent_connected, heartbeat; an unrouted 'ping' is dropped), and the solve trajectory's control-plane reads (GetIntegrations, GetFunctionConfiguration, GetFunction, GetObject, FilterLogEvents) all answer. Recorded behavior only for the scenario's own derived facts.
Emulator: the check produces no verdict at all — 'Error: Check timed out after 60.0s'. No phase ever reports completion.
Why: a WebSocket-backed application cannot be deployed and exercised against the emulator, and the diagnostic reads a customer would use to debug it are never reached.
Fix: the apigatewayv2 WEBSOCKET execute-api data plane (handshake and route dispatch) together with whichever phase blocks first in the deploy path; the handshake is the first thing the scenario depends on.
Traces: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: the phases deploy/setup/pre_invoke/solve/cleanup/destroy each complete and the check reaches a pass/fail verdict. Actual: the run is killed at the harness cap with no phase verdict recorded.

## Steps to Reproduce
Replay apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle: deploy the stack, run the pre_invoke agent sessions that open WebSocket connections to the deployed stage and send register/heartbeat/ping actions, then run the solve-trajectory reads.
