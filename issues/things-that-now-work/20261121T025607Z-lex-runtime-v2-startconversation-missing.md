Issue-Severity: MEDIUM
Issue-Gap-Class: missing
Issue-Commit: be68e03a3b164dfd55295662738d8b5c0a02662c

# StartConversation has no handler file at all: the modeled streaming conversation operation is unwired

_Filed: 2026-11-21T02:56:07Z_
_Operation: StartConversation | Class: missing_

> _Note: this issue was identified against the code base at commit `be68e03a3b164dfd55295662738d8b5c0a02662c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service model binds six operations; this crate has a handler file for five of them. `StartConversation` has NO handler file under `src/ops/` — not a scaffold that answers 501 and not an implementation — so it is not in the service's operation allow-list and is served by the framework's unroutable-operation path instead of by this service. Every other operation of the model is wired and implemented, so the whole bidirectional-streaming half of the Lex V2 runtime (`POST /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}/conversation`, input `requestEventStream`, output `responseEventStream`) is unreachable, including the `conversationMode` (AUDIO|TEXT) header the model binds. The crate already holds everything a text conversation needs — `crate::state::take_turn` drives one turn for both `RecognizeText` and `RecognizeUtterance` — but nothing calls it from a conversation stream.

## Evidence

Model binds it: smithy-model service shape lists `{"target":"com.amazonaws.lexruntimev2#StartConversation"}` among the six operations, and OPERATIONS.md:61-66 renders it — "## StartConversation / - **Route:** `POST /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}/conversation -> 200` / - **Input:** ... `conversationMode` (ConversationMode, one of: AUDIO | TEXT, @httpHeader(x-amz-lex-conversation-mode)), `requestEventStream` (StartConversationRequestEventStream, @httpPayload, required)".
No handler exists: `glob("services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0/**/*.rs")` returns `src/ops/delete_session.rs`, `src/ops/get_session.rs`, `src/ops/put_session.rs`, `src/ops/recognize_text.rs`, `src/ops/recognize_utterance.rs` and no `src/ops/start_conversation.rs`.
The crate admits it: src/ops/SKIPPED_STUBS.md:21 — "// SKIPPED: StartConversation — unsupported shape construct: operation 'StartConversation' has a @streaming event-stream input member, which needs wire framing the generator does not emit", and :11-14 "a scaffolded op has a compile-checked typed skeleton and is in the service's `operations(...)` allow-list; an op listed here has neither, and is answered by the framework's unroutable-operation path instead."
src/service.rs:54-56 — "as does the model's sixth operation, `StartConversation`, whose event-stream input leaves it unwired (see `ops/SKIPPED_STUBS.md`), so no request of it reaches this seam at all."
No test exercises it: `grep StartConversation` under `tests/` returns no match.

## Steps to Reproduce

1. Create and build a bot locale through the Lex Models V2 control plane mounted on the same engine (as tests/all/harness.rs `built_bot` does).
2. POST /bots/{botId}/botAliases/{botAliasId}/botLocales/en_US/sessions/s1/conversation with `x-amz-lex-conversation-mode: TEXT` and an event-stream payload.
3. Observe the request is not routed to any operation of this service (no handler file exists), so it is answered by the framework's unroutable-operation path rather than by a StartConversation handler — while the five sibling session operations on the same four path labels are served.
