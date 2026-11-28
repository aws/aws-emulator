Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 24df4b5bb34709ed7c873e3f6d0f18f4590f38dd

# PutSession clears a session's active contexts when the request carries no activeContexts member

_Filed: 2026-11-25T05:42:00Z_
_Operation: PutSession | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `24df4b5bb34709ed7c873e3f6d0f18f4590f38dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutSession` writes a caller-seeded dialog state through `Session::replace_session_state`, which reads the supplied `activeContexts` as `supplied.active_contexts.clone().unwrap_or_default()` and then unconditionally calls `set_active_contexts(...)`. An ABSENT member and an EMPTY list therefore do the same thing: both wipe every active context the session already holds. The model states those two cases differently — an absent list keeps the session's current contexts, an empty list clears them — and the crate's sibling write path (`merge_session_state`, the one `RecognizeText`/`RecognizeUtterance` reach through `open_turn`) implements exactly that distinction with `if let Some(contexts) = supplied.active_contexts`. The same `replace_session_state` already honours "absent means keep the stored value" for the neighbouring `originatingRequestId` member two lines further down, so the two members of one function disagree about what an omitted member means.

Consequence: a conversation that set contexts (through a turn, or through an earlier PutSession) and is then handed a PutSession that only sets, say, a `dialogAction` loses the contexts and their remaining time/turn budgets, and a later `GetSession` reports `activeContexts: []` for contexts that should still be live.

## Evidence

Smithy — `smithy_query(query="doc", shape="ActiveContext", member="contextAttributes")`:
  "A list of contexts active for the request. A context can be activated when a previous intent is fulfilled, or by including the context in the request. **If you don't specify a list of contexts, Amazon Lex V2 will use the current list of contexts for the session. If you specify an empty list, all contexts for the session are cleared.**"

The PutSession path collapses the two cases — src/state.rs:687 and :701 (reached from src/ops/put_session.rs:353 `session.replace_session_state(input.session_state, now, &locale);`):
  687: `let active_contexts = supplied.active_contexts.clone().unwrap_or_default();`
  701: `self.set_active_contexts(active_contexts, now);`
and `set_active_contexts` replaces the whole vector — src/state.rs:586-595: `self.active_contexts = contexts.into_iter().map(...).collect();`

The SIBLING path in the same module keeps them when the member is absent — src/state.rs:731-733 (`merge_session_state`, called at src/state.rs:397-399 from `open_turn`):
  731: `if let Some(contexts) = supplied.active_contexts {`
  732: `    self.set_active_contexts(contexts, now);`
  733: `}`

And the SAME function already treats an omitted member as "keep" for the next member — src/state.rs:688-690:
  `if let Some(originating_request_id) = supplied.originating_request_id.clone() { self.originating_request_id = originating_request_id; }`
with its doc at src/state.rs:678-680: "The `originatingRequestId` is the SESSION's, not one turn's: ... so a request that carries none leaves the one the session already holds in place."

Test coverage: nothing pins the clearing. tests/all/always_reported_members.rs:349-359 tables `("a seed carrying no activeContexts member", None, 0)`, `("a seed carrying the empty list", Some(json!([])), 0)` etc., but every row seeds a FRESH session id (`let path = bot.session_path(&format!("context-{index}"));`, line 364), so no row performs a second PutSession over a session that already holds a context. tests/all/session_lifecycle.rs:601-676 sets contexts once with PutSession and then only drives `RecognizeText` turns.

## Steps to Reproduce

1. Build a bot locale through the Lex Models V2 control plane mounted on the same engine (as tests/all/harness.rs `built_bot` does).
2. POST /bots/{botId}/botAliases/{botAliasId}/botLocales/en_US/sessions/ctx-1 with body {"sessionState": {"dialogAction": {"type": "Delegate"}, "activeContexts": [{"name": "checkout", "timeToLive": {"timeToLiveInSeconds": 86400, "turnsToLive": 20}, "contextAttributes": {"step": "payment"}}]}}.
3. GET the same session path — `sessionState.activeContexts` holds the one context (expected).
4. POST the same session path again with body {"sessionState": {"dialogAction": {"type": "Delegate"}}} — i.e. carrying NO activeContexts member.
5. GET the session path once more: `sessionState.activeContexts` is now `[]`. Per `ActiveContext$contextAttributes` ("If you don't specify a list of contexts, Amazon Lex V2 will use the current list of contexts for the session") the `checkout` context should still be reported, and only a request that sends `"activeContexts": []` should clear it — which is how the sibling RecognizeText/RecognizeUtterance path already behaves (state.rs:731-733).
