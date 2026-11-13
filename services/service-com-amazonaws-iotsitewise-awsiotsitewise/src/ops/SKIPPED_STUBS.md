# Skipped operation stubs

The scaffold op-stub generator declined to emit a stub for the operations
below. Every remaining decline class is one the generator cannot TYPE: a
Unit/absent input, a `@streaming` member (event-stream union or streaming
blob), a member whose target is a shape kind that has no Rust type, or a
malformed model. A stub that does not compile would break the tenant build,
so each is left modeled-but-unwired — hand-author it in `ops/<op>.rs`, using
a rendered sibling as the pattern.

Note the difference from a SCAFFOLDED operation. Both answer HTTP 501, but a
scaffolded op has a compile-checked typed skeleton and is in the service's
`operations(...)` allow-list; an op listed here has neither, and is answered
by the framework's unroutable-operation path instead.

An output that is a SCALAR, list or map is declined: only a structure or a
union can be declared as `{Op}Output`. A `@required` union member and a
`@required`-member reference cycle are NOT decline reasons — a scaffolded
handler returns an error, so it never has to construct a value.

// SKIPPED: InvokeAssistant — unsupported shape construct: operation 'InvokeAssistant' has a @streaming event-stream output member, which needs wire framing the generator does not emit

The census above records what the GENERATOR did, so each line stays as written.
`InvokeAssistant` has since been hand-authored in `ops/invoke_assistant.rs`: it frames
its `ResponseStream` through `service_common::event_stream`, reads its own request, and
is routed by `Service::override_operation` (the generated shim for a `@streaming`-output
operation declines the request, so it cannot serve one). It therefore has no
`impl Operation`; its shapes are hand-authored in `types/invoke_assistant_types.rs`.
