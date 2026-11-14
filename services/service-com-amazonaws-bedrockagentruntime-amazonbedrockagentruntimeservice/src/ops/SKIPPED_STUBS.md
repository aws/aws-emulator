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

// SKIPPED: AgenticRetrieveStream — unsupported shape construct: operation 'AgenticRetrieveStream' has a @streaming event-stream output member, which needs wire framing the generator does not emit
// SKIPPED: InvokeFlow — unsupported shape construct: operation 'InvokeFlow' has a @streaming event-stream output member, which needs wire framing the generator does not emit
// SKIPPED: InvokeAgent — unsupported shape construct: operation 'InvokeAgent' has a @streaming event-stream output member, which needs wire framing the generator does not emit
// SKIPPED: InvokeInlineAgent — unsupported shape construct: operation 'InvokeInlineAgent' has a @streaming event-stream output member, which needs wire framing the generator does not emit
// SKIPPED: OptimizePrompt — unsupported shape construct: operation 'OptimizePrompt' has a @streaming event-stream output member, which needs wire framing the generator does not emit
// SKIPPED: RetrieveAndGenerateStream — unsupported shape construct: operation 'RetrieveAndGenerateStream' has a @streaming event-stream output member, which needs wire framing the generator does not emit

## Hand-authored since

`InvokeInlineAgent` is now written by hand in `ops/invoke_inline_agent.rs`, with its shapes
in `ops/types/invoke_inline_agent_types.rs` and the `completion` event stream in that file's
`unsupported = [..]` list. `src/service.rs` routes a request to it through
`Service::override_operation` behind `service_common::event_stream::EventStreamRoute`,
because the GENERATED `ErasedOperation` shim of a `@streaming`-output operation declines the
request. It is IMPLEMENTED: its request is validated against the model, the model the inline
agent orchestrates with is resolved in the plane that owns the catalogue, the conversation
its `sessionId` addresses is this plane's own store, and its answer is the
`InlineAgentResponseStream`'s `chunk` event, framed through
`service_common::event_stream::frame_event`.

`OptimizePrompt` is written by hand in `ops/optimize_prompt.rs`, with its shapes in
`ops/types/optimize_prompt_types.rs`, and is routed the same way. It is IMPLEMENTED:
its request is validated against the model and its answer is the
`OptimizedPromptStream`'s two events, framed through
`service_common::event_stream::frame_event`.

`InvokeAgent`, `InvokeFlow`, `AgenticRetrieveStream` and `RetrieveAndGenerateStream` are
written by hand too — `ops/invoke_agent.rs`, `ops/invoke_flow.rs`,
`ops/agentic_retrieve_stream.rs` and `ops/retrieve_and_generate_stream.rs`, with their
shapes in the `_types` module beside each — and routed the same way. All four are
IMPLEMENTED, so every streaming operation of this service is now served by a real
handler.

Note that an operation listed above is unwired only until a file exists for it in
`ops/`: `build.rs` scans `src/ops/*.rs` and puts every `//! @op <Name>` header it finds in
the generated `operations(..)` list, which is what puts the operation's route in the
service's routing table — and therefore what makes a request to it reach this service at
all.

