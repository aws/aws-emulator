Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: d6ab12dd13fc1c951c3823d9af85ad5bf611db1c

# Bootstrap the SageMaker Inference Router Runtime emulator

_Filed: 2026-11-28T04:39:45Z_
_Operation: service-com-amazonaws-sagemakerinferencerouterruntime-sagemakerinferencerouterruntime | Class: missing_

> _Note: this issue was identified against the code base at commit `d6ab12dd13fc1c951c3823d9af85ad5bf611db1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold answering HTTP 501 and holding no state (`src/ops/invoke_inference_router.rs:4`, `src/ops/invoke_inference_router_async.rs:4`). The service binds exactly TWO operations (OPERATIONS.md:3 "2 operations"), both runtime invocations, so the whole first slice is the invoke loop — there is no control plane here, and the routers/routing targets must therefore be registry state inside this crate that an invocation reads.

## Happy path to implement (in order)

1. **InvokeInferenceRouter** — `POST /inference-routers/{InferenceRouterName}/invocations` (200).
   Input: `InferenceRouterName` (@httpLabel, required, len 1..63, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`), `RoutingTargetName` (header `X-Amzn-SageMaker-Routing-Target-Name`, required), `Body` (@httpPayload blob, required, max 6291456), plus optional `ContentType`, `Accept`, `InferenceId`, `PrefixAwareId`, `CustomAttributes` headers.
   Output MUST be computed from the stored router registry, not canned: `Body` (the destination's echo/recorded response payload), `ContentType`, `InvokedRoutedResourceArn` (the ARN of the destination the router actually selected, taken from the stored routing target's destination list), `RoutingReason` (why that destination was chosen, recorded alongside the selection), `InvokedProductionVariant`, and `CustomAttributes` round-tripped from the request. The model states: "The response includes the destination that served the request and the reason that the router chose it." — so `InvokedRoutedResourceArn`/`RoutingReason` must come from a real selection over persisted destinations (e.g. round-robin), and two successive calls on a multi-destination target must be observably distributed, not a constant string.
2. **Unknown router / routing target must error, not succeed** — an invocation naming an `InferenceRouterName` or `RoutingTargetName` that is not in the registry must fail with a modelled error (`ValidationException` 400 for a name that does not exist, `ModelNotReadyException` 429 for a destination recorded as not ready). Both are modelled on this operation (OPERATIONS.md:31). A 200 for an unknown router proves the handler is not reading state.
3. **InvokeInferenceRouterAsync** — `POST /inference-routers/{InferenceRouterName}/async-invocations` (202).
   Input: `InferenceRouterName` (required label), `RoutingTargetName` (required header), `InputLocation` (required header `X-Amzn-SageMaker-InputLocation`, pattern `^(https|s3)://([^/]+)/?(.*)$`), plus `ContentType`, `Accept`, `CustomAttributes`, `InferenceId`, `S3OutputPathExtension`, `Filename`, `RequestTTLSeconds` (60..21600), `InvocationTimeoutSeconds` (1..3600).
   Output: `InferenceId` (the id the request was accepted under — the caller-supplied `InferenceId` when given, otherwise a server-generated one that is STORED), `OutputLocation` (derived from the routing target's configured S3 output path plus `S3OutputPathExtension`/`Filename`), `FailureLocation`. The handler must INSERT an async-invocation record keyed by that `InferenceId` before returning 202; a random id with no stored record is a bootstrap failure.
4. **Async invocation state is observable** — a second async submit reusing the same `InferenceId` must see the stored record (idempotent accept returning the same `OutputLocation`), and the recorded destination selection must come from the same registry the sync path uses (ONE shared selection helper, not two copies).

All four steps must read and write the single shared engine store via `crate::state::service_state(&*self.engine)` (README.md:90) — never `engine::InMemoryEngine::new()` inside a handler (README.md:98). No canned or echoed-only responses: `InvokedRoutedResourceArn`, `RoutingReason`, `InferenceId` and `OutputLocation` must all be derivable from persisted state.

## State Model

In `src/state.rs`, on `ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState`:

- `routers: HashMap<String /* InferenceRouterName */, InferenceRouter>`
  - `InferenceRouter { name: String, arn: String, routing_targets: HashMap<String /* RoutingTargetName */, RoutingTarget> }`
- `RoutingTarget { name: String, destinations: Vec<Destination>, next_destination: usize /* round-robin cursor */, s3_output_path: String, s3_failure_path: String, max_time_in_backlog_seconds: Option<i32> }`
- `Destination { routed_resource_arn: String, production_variant: Option<String>, ready: bool, canned_response: Option<Vec<u8>> /* what a sync invoke returns for this destination */, content_type: Option<String> }`
- `sync_invocations: Vec<SyncInvocationRecord>` — `{ inference_id: Option<String>, router: String, routing_target: String, chosen_destination_arn: String, routing_reason: String, request_body_len: usize, content_type: Option<String>, custom_attributes: Option<String>, received_at: SystemTime }`
- `async_invocations: HashMap<String /* InferenceId */, AsyncInvocationRecord>` — `{ inference_id: String, router: String, routing_target: String, chosen_destination_arn: String, input_location: String, output_location: String, failure_location: String, request_ttl_seconds: Option<i32>, invocation_timeout_seconds: Option<i32>, s3_output_path_extension: Option<String>, filename: Option<String>, status: AsyncStatus /* Accepted | InProgress | Completed | Failed */, submitted_at: SystemTime }`
- a seeding/registration path so a test can populate `routers` (routers and routing targets have no create operation in this service), and ONE shared `fn select_destination(&mut RoutingTarget) -> (arn, reason)` used by both handlers so the selection rule is not implemented twice.

Anything beyond this slice (retry-on-retryable-error across a second destination, backlog/TTL expiry moving a request, error-topic notification, `InternalDependencyException`/`ModelError` surfaces) stays 501 and will be filed as individual gap issues later.

## Evidence

Both handlers are unmodified scaffolds: `services/.../src/ops/invoke_inference_router.rs:4` and `services/.../src/ops/invoke_inference_router_async.rs:4` both carry `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and`.

Contract, from `services/.../OPERATIONS.md:3`: "2 operations from `smithy-model/com-amazonaws-sagemakerinferencerouterruntime-sagemakerinferencerouterruntime.json`."

OPERATIONS.md:28-31 (InvokeInferenceRouter): "**Route:** `POST /inference-routers/{InferenceRouterName}/invocations`" / "**Input:** `InferenceRouterName` (string, @httpLabel, required), `RoutingTargetName` (string, @httpHeader(X-Amzn-SageMaker-Routing-Target-Name), required), ... `Body` (blob, len ..6291456, @httpPayload, required)" / "**Output:** `Body` (blob...), `ContentType`..., `InvokedRoutedResourceArn` (string, @httpHeader(X-Amzn-SageMaker-Invoked-Routed-Resource-Arn)), `RoutingReason` (string, @httpHeader(X-Amzn-SageMaker-Invoked-Routing-Reason)), `InvokedProductionVariant`..., `CustomAttributes`..." / "**Errors:** `AccessDeniedException (HTTP 403)`, `InternalDependencyException (HTTP 530)`, ... `ModelError (HTTP 424)`, `ModelNotReadyException (HTTP 429)`, ... `ValidationException (HTTP 400)`".

Smithy doc for `InvokeInferenceRouter`: "Sends a real-time inference request to an inference router. In the request, you name the routing target, and the router sends the request to one of the routing target's destinations. The response includes the destination that served the request and the reason that the router chose it."

OPERATIONS.md:35-37 (InvokeInferenceRouterAsync): "**Route:** `POST /inference-routers/{InferenceRouterName}/async-invocations -> 202`" / "`InputLocation` (string, @httpHeader(X-Amzn-SageMaker-InputLocation), required)" / "**Output:** `InferenceId` (string), `OutputLocation` (string, @httpHeader(X-Amzn-SageMaker-OutputLocation)), `FailureLocation` (string, @httpHeader(X-Amzn-SageMaker-FailureLocation))".

Smithy doc for `InvokeInferenceRouterAsync`: "The router returns as soon as it accepts the request, and sends the request to one of the destinations of the routing target. The destination reads the input from Amazon S3, and the inference response is written to the Amazon S3 output path that the routing target specifies."

State access contract, `services/.../README.md:90`: "let Some(state) = crate::state::service_state(&*self.engine) else {" and README.md:98: "**Never write `engine::InMemoryEngine::new()` inside a handler.**".

Test coverage today: only `tests/all/state_roundtrip.rs` (README.md:22, "compiled example of the state API") — no invocation test exists.

## Steps to Reproduce

1. Start the emulator and seed one router `my-router` with routing target `my-target` holding two destinations (distinct `routed_resource_arn`s, both ready).
2. POST /inference-routers/my-router/invocations with header `X-Amzn-SageMaker-Routing-Target-Name: my-target`, `Content-Type: application/json`, body `{"input":"hello"}` — expect 200 with a body, `X-Amzn-SageMaker-Invoked-Routed-Resource-Arn` equal to one of the two seeded destination ARNs and a non-empty `X-Amzn-SageMaker-Invoked-Routing-Reason`. Today: HTTP 501 NotImplemented.
3. Repeat the same invocation and assert the two calls together covered BOTH seeded destination ARNs (selection is read/advanced in persisted state, not a constant).
4. POST /inference-routers/no-such-router/invocations with the same headers — expect a modelled 400 ValidationException, NOT a 200.
5. POST /inference-routers/my-router/async-invocations with `X-Amzn-SageMaker-Routing-Target-Name: my-target`, `X-Amzn-SageMaker-InputLocation: s3://bucket/in.json`, `X-Amzn-SageMaker-Inference-Id: req-1` — expect 202 with `InferenceId == "req-1"` in the JSON body and `X-Amzn-SageMaker-OutputLocation` derived from the seeded routing target's S3 output path. Today: HTTP 501.
6. Re-submit the identical async request with `X-Amzn-SageMaker-Inference-Id: req-1` and assert the returned `OutputLocation`/`FailureLocation` match the first response exactly (served from the stored async-invocation record).
7. Submit an async request with no `X-Amzn-SageMaker-Inference-Id` and assert the generated `InferenceId` is present in the service's stored async-invocation map (a generated id with no persisted record is a failure).
