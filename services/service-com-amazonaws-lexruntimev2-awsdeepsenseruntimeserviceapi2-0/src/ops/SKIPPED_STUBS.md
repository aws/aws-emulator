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

// SKIPPED: StartConversation — unsupported shape construct: operation 'StartConversation' has a @streaming event-stream input member, which needs wire framing the generator does not emit

## Hand-authored since

`StartConversation` is now written by hand in `ops/start_conversation.rs`, with its
shapes in `ops/types/start_conversation_types.rs` and the `requestEventStream` in that
file's `unsupported = [..]` list. `src/service.rs` routes a request to it through
`Service::override_operation` behind `service_common::event_stream::EventStreamRoute`,
because the GENERATED `ErasedOperation` shim of a `@streaming`-output operation declines
the request. It is IMPLEMENTED: the request's labels, its `conversationMode` header and
every event payload are validated against the model, the bot locale the conversation is
held against is resolved in the control plane that owns it, each input event takes the
same `crate::state::take_turn` turn the two `Recognize*` operations take, and the
response is the `StartConversationResponseEventStream`'s own events — framed through
`service_common::event_stream::frame_event`, with the stream's in-band exception members
for what the conversation cannot serve.

Note that an operation listed above is unwired only until a file exists for it in
`ops/`: `build.rs` scans `src/ops/*.rs` and puts every `//! @op <Name>` header it finds
in the generated `operations(..)` list, which is what puts the operation's route in the
service's routing table — and therefore what makes a request to it reach this service at
all.
