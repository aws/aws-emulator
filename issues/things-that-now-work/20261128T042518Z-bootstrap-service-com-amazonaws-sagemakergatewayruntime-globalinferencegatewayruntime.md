Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 14a20d4988acc078b454ec1aa206a20bf5810707

# Bootstrap the SageMaker Gateway Runtime (Global Inference Gateway Runtime) emulator

_Filed: 2026-11-28T04:25:18Z_
_Operation: service-com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime | Class: missing_

> _Note: this issue was identified against the code base at commit `14a20d4988acc078b454ec1aa206a20bf5810707`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of this crate is an untouched scaffold: `src/ops/invoke_global_inference_gateway.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("InvokeGlobalInferenceGateway"))`, the sibling streaming operation has no handler file at all (declined by the stub generator), and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). Nothing about this service works today.

This is a data-plane-only service: the model binds exactly two operations and both are invocations, so the happy path is "a registered gateway serves an OpenAI-shaped completion, and the invocation is recorded". Implement this FIRST SLICE, in order:

1. **Gateway registry bootstrap (state, not an operation).** Because this service models no CreateGateway, the gateway inventory must be materialised in `ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState` (seeded at first access and/or injectable by tests): a set of gateway ids, each with the models its targets serve and whether each model currently has capacity. The registry is what makes `ResourceNotFoundException` vs. `ServiceUnavailableException` vs. `200` a decision computed from state rather than a coin flip.
2. **InvokeGlobalInferenceGateway** — `POST /global-inference-gateway/{GatewayId}/invocations -> 200`. Inputs: `GatewayId` (@httpLabel, required), `ContentType` (@httpHeader(Content-Type), required), `Accept` (@httpHeader(Accept)), `CustomerIdentityToken` (@httpHeader(X-Amzn-SageMaker-Customer-Identity-Token)), `Body` (blob, @httpPayload, required). Output: `Body` (blob, @httpPayload). The handler must:
   - look `GatewayId` up in the registry and answer `ResourceNotFoundException` (404) when it is absent — the model says "No gateway with this ID exists in this account and Region, or no target of the gateway serves the model named in the request body";
   - parse `Body` as ONE JSON object and read only `model` and `stream` from it; a body that is not one JSON object, or that repeats the top-level `model` key with different values, is refused with a 400 validation error (`Scaffolded::validation(..)`); the parsed body must NOT be stored ("The gateway reads only `model` and `stream`, and does not store the body");
   - resolve `model` against that gateway's targets: unknown model -> `ResourceNotFoundException` (404); known model with no available target -> `ServiceUnavailableException` (503), per "A model that is served but has no available target right now returns ServiceUnavailableException instead";
   - on success, return the completion JSON computed from stored state — the resolved model id, a deterministic completion id drawn from the state's sequence counter, and usage counts derived from the request — never a canned blob and never the request echoed back;
   - record the invocation in state (gateway id, resolved model, request byte length, content type, whether a customer identity token was present, sequence number) so a second call returns a different completion id and the counters are observable.
3. **Content-type gate.** Accept `application/json` with optional parameters (e.g. `charset=utf-8`); refuse other content types, as the model states: "Pass `application/json` (parameters such as `charset=utf-8` are allowed) ... The Global Inference Gateway rejects other content types with a 415 error." This rule lives in ONE helper shared with the streaming operation.
4. **InvokeGlobalInferenceGatewayWithResponseStream** — `POST /global-inference-gateway/{GatewayId}/invocations-response-stream -> 200`, same inputs, output `Body` (ResponseStream, @httpPayload). It has NO handler file: hand-author `src/ops/invoke_global_inference_gateway_with_response_stream.rs` using the buffered sibling as the pattern, reusing the SAME registry lookup, body parse and content-type helper, and emit the event-stream frames for the completion chunks. Deliver this second, after step 2 passes.

No canned or echoed responses anywhere: both operations must compute their response (and their refusal) from the persisted registry and invocation log. Done = an integration test that invokes a seeded gateway twice and sees two distinct, well-formed completions plus two recorded invocations, and that gets 404 for an unknown gateway id, 404 for an unserved model, 503 for a model with no capacity, and 400 for a non-JSON body.

## State Model

In `src/state.rs`, on `ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState`:

- `gateways: BTreeMap<String /* GatewayId, ^[a-z0-9]+$ */, Gateway>`
- `Gateway { gateway_id: String, arn: String /* arn:aws:sagemaker:<region>:<account>:global-inference-gateway/<id> */, targets: BTreeMap<String /* model id */, ModelTarget>, created_at: SystemTime }`
- `ModelTarget { model: String, available: bool /* false -> ServiceUnavailableException */, component: Option<String> /* config | scheduler | model selector | model server, for error-prefix fidelity */ }`
- `invocations: Vec<InvocationRecord>` where `InvocationRecord { seq: u64, gateway_id: String, model: String, request_bytes: usize, content_type: String, accept: Option<String>, identity_token_present: bool, streaming: bool, at: SystemTime }`
- `next_completion_seq: u64` — source of the deterministic `chatcmpl-<seq>` id and of the per-gateway invocation count

Operations outside this slice: there are none — the service binds only these two — so anything deferred (full event-stream framing fidelity, `Retry-After` on ThrottlingException, BadGatewayException/GatewayTimeoutException message prefixes) stays unimplemented and is filed as individual gap issues later.

## Evidence

services/service-com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime/OPERATIONS.md:28-31 — "- **Route:** `POST /global-inference-gateway/{GatewayId}/invocations -> 200`" / "- **Input:** `GatewayId` (string, len 1..64, pattern ^[a-z0-9]+$, @httpLabel, required), `Accept` (string, @httpHeader(Accept)), `ContentType` (string, len 1..1024, @httpHeader(Content-Type), required), `CustomerIdentityToken` (string, @httpHeader(X-Amzn-SageMaker-Customer-Identity-Token)), `Body` (blob, len ..10485760, @httpPayload, required)" / "- **Output:** `Body` (blob, len ..10485760, @httpPayload)" / "- **Errors:** `AccessDeniedException (HTTP 403)`, `BadGatewayException (HTTP 502)`, `GatewayTimeoutException (HTTP 504)`, `InternalServerException (HTTP 500)`, `PayloadTooLargeException (HTTP 413)`, `ResourceNotFoundException (HTTP 404)`, `ServiceUnavailableException (HTTP 503)`, `ThrottlingException (HTTP 429)`".

OPERATIONS.md:35-37 — the streaming sibling: "- **Route:** `POST /global-inference-gateway/{GatewayId}/invocations-response-stream -> 200`" ... "- **Output:** `Body` (ResponseStream, @httpPayload)". It has no handler: src/ops/SKIPPED_STUBS.md:22 — "// SKIPPED: InvokeGlobalInferenceGatewayWithResponseStream — unsupported shape construct: operation 'InvokeGlobalInferenceGatewayWithResponseStream' has a @streaming event-stream output member, which needs wire framing the generator does not emit".

Model docs carried in the stub header (from the Smithy model): src/ops/invoke_global_inference_gateway.rs:85-93 — "`Body` (RequestPayloadBlob, len ..10485760) **required** — The inference request body: one OpenAI JSON object, at most 10 MiB. ... A body that is not one JSON object, or that repeats the top-level `model` key with different values, is rejected with a 400 validation error. ... The gateway reads only `model` and `stream`, and does not store the body."; :94-97 — "Pass `application/json` (parameters such as `charset=utf-8` are allowed) ... The Global Inference Gateway rejects other content types with a 415 error."; :142-145 — "`ResourceNotFoundException` (HTTP 404) — No gateway with this ID exists in this account and Region, or no target of the gateway serves the model named in the request body. A model that is served but has no available target right now returns ServiceUnavailableException instead."; :108-110 — "`Body` (ResponsePayloadBlob, len ..10485760) — The model server's OpenAI completion JSON, returned unchanged."

All-stubs proof: src/ops/invoke_global_inference_gateway.rs:4 — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`" and :249 — "Err(crate::scaffolded::Scaffolded::not_implemented(\"InvokeGlobalInferenceGateway\"))". No state: src/state.rs:8 — "// TODO: add service-specific state fields".

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /global-inference-gateway/gw1/invocations with `Content-Type: application/json`, `Accept: application/json` and body `{"model":"llama3-8b","messages":[{"role":"user","content":"hi"}]}` against a seeded gateway id `gw1` whose targets serve `llama3-8b`. Today: HTTP 501 NotImplemented. Expected: HTTP 200 with an OpenAI completion JSON whose `model` is `llama3-8b` and whose completion id comes from the state sequence.
3. Repeat the same POST. Expected: HTTP 200 with a DIFFERENT completion id, and two invocation records in state (the second call must not be a replay of a canned body).
4. POST the same request to /global-inference-gateway/nosuchgw/invocations. Expected: HTTP 404 ResourceNotFoundException because the gateway id is not in the registry.
5. POST body `{"model":"not-served"}` to `gw1`. Expected: HTTP 404 ResourceNotFoundException (no target serves the model).
6. Mark the `llama3-8b` target unavailable in state and POST again. Expected: HTTP 503 ServiceUnavailableException, not 404 and not 200.
7. POST body `not json` with `Content-Type: application/json`. Expected: HTTP 400 validation error; the invocation log must be unchanged.
8. POST with `Content-Type: text/plain`. Expected: refusal per the documented content-type gate, produced by the shared helper.
9. POST the same request to /global-inference-gateway/gw1/invocations-response-stream. Today: the route is unroutable (no handler file). Expected: HTTP 200 with streamed completion chunks, resolved through the SAME registry lookup and body parse as the buffered operation.
