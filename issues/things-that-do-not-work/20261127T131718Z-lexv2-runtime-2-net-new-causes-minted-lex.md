Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-runtime.get-session-interprets-an-intent-whose-slot-retries-ran-out-as-failed lexv2-runtime.get-session-reports-a-declined-confirmation-as-a-failed-intent lexv2-runtime.put-session-rejects-a-locale-the-bot-does-not-define lexv2-runtime.recognize-utterance-refuses-an-alias-the-bot-does-not-have lexv2-runtime.recognize-utterance-rejects-an-unsupported-request-content-type lexv2-runtime.spent-slot-retries-hand-the-turn-to-the-fallback-intent lexv2-runtime.recognize-text-refuses-session-state-naming-an-undefined-intent
Issue-Shape-Id: com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0
Issue-Key: lexv2-runtime.get-session-interprets-an-intent-whose-slot-retries-ran-out-as-failed|_serving_bot#2|AssertionError|assert re.fullmatch(r"[0-9A-Z]{10}", bot_id), (
Issue-Key: lexv2-runtime.get-session-reports-a-declined-confirmation-as-a-failed-intent|_serving_bot#2|AssertionError|assert re.fullmatch(r"[0-9A-Z]{10}", bot_id), (
Issue-Key: lexv2-runtime.put-session-rejects-a-locale-the-bot-does-not-define|_serving_bot#2|AssertionError|assert re.fullmatch(r"[0-9A-Z]{10}", bot_id), (
Issue-Key: lexv2-runtime.recognize-utterance-refuses-an-alias-the-bot-does-not-have|run#3|AssertionError|assert _ID.fullmatch(value), f"the {what} is {len(value)} chars, expected 10 of [0-9A-Z]"
Issue-Key: lexv2-runtime.recognize-utterance-rejects-an-unsupported-request-content-type|_serving_bot#2|AssertionError|assert re.fullmatch(r"[0-9A-Z]{10}", bot_id), (
Issue-Key: lexv2-runtime.spent-slot-retries-hand-the-turn-to-the-fallback-intent|_serving_bot#2|AssertionError|assert re.fullmatch(r"[0-9A-Z]{10}", bot_id), (
Issue-Key: lexv2-runtime.recognize-text-refuses-session-state-naming-an-undefined-intent|_refusal#6|AssertionError|raise AssertionError(f"the call was accepted but should have been refused: {result!r}")
Issue-Commit: 9c26f3bece60e98f7a80e23ef343cd438e1e94fa

# lexv2-runtime: 2 net-new causes — minted Lex ids contain lowercase letters so no bot/alias/intent id matches AWS's uppercase-alnum id space, and RecognizeText accepts a delegated sessionState naming an intent the locale does not define

> _Note: this issue was identified against the code base at commit `9c26f3bece60e98f7a80e23ef343cd438e1e94fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBot

## Description
Two independent root causes, both reached from the lexv2-runtime behavioral checks.

Cause 1 of 2: every Lex V2 id the emulator mints is drawn from a 62-character alphabet that includes lowercase letters, so the ids are not in AWS's id space.
AWS: recorded behavior only for the alphabet — the Smithy model constrains the id shape but not its case. Smithy (lexmodelsv2): shape `Id` is `string` with `@length(min:10,max:10)` and `@pattern("^[0-9a-zA-Z]+$")`; `CreateBot` output member `botId` targets it, and `RecognizeText`'s `botId` httpLabel targets `BotIdentifier` with the same length/pattern. The recordings show real Amazon Lex V2 always mints these ten-character identifiers (`botId`, `botAliasId`, `intentId`, `slotId`, `slotTypeId`) from the UPPERCASE-alphanumeric subset only — every check in this suite that creates a bot asserts `re.fullmatch(r"[0-9A-Z]{10}", bot_id)` against the live-recorded shape of the value, and that assertion is what the emulator fails. The model's wider pattern permits lowercase; AWS does not emit it.
Emulator: `CreateBot` returns a ten-character id that mixes cases, so the assertion fails with `AssertionError: CreateBot returned a bot id of 10 characters, expected 10 of [0-9A-Z]` (and, in the RecognizeUtterance addressing check, `the bot id is 10 chars, expected 10 of [0-9A-Z]`). The id generator is a single helper shared by every minter: `render_id` builds the value from `ALPHABET = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"`, and `mint_bot_id`, `mint_bot_alias_id`, `mint_intent_id`, `mint_slot_id` and `mint_slot_type_id` all draw from it, so EVERY Lex id the emulator hands out is affected, not just `botId`. The status is the modelled 202/200 and nothing else about the response diverges — only the character set of the minted value.
Why: a client that validates or normalizes Lex ids (case-sensitive comparisons against ids echoed back, regex guards built from observed AWS ids, ids embedded in uppercase-only identifiers, test fixtures that assert the AWS id shape) rejects or mis-handles an id the emulator hands out, and every runtime turn addressed with such an id is addressed with a value AWS would never have produced. In this suite it stops six lexv2-runtime data-plane checks before they take a single turn.
Fix: the id minting in the lexmodelsv2 state component — the shared `render_id` helper and the `mint_*` functions that call it must draw from the uppercase-alphanumeric subset (`[0-9A-Z]`) only, keeping the length at exactly 10 and keeping the collision re-draw. File and line citations under Traces.
Traces: lexv2-runtime.get-session-interprets-an-intent-whose-slot-retries-ran-out-as-failed, lexv2-runtime.get-session-reports-a-declined-confirmation-as-a-failed-intent, lexv2-runtime.put-session-rejects-a-locale-the-bot-does-not-define, lexv2-runtime.recognize-utterance-refuses-an-alias-the-bot-does-not-have, lexv2-runtime.recognize-utterance-rejects-an-unsupported-request-content-type, lexv2-runtime.spent-slot-retries-hand-the-turn-to-the-fallback-intent. Code: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/state.rs:1935-1952 (`render_id`, the 62-char `ALPHABET`), :1184-1192 (`mint_bot_id`), :1197-1205 (`mint_slot_type_id`), :1217-1225 (`mint_intent_id`), :1235-1243 (`mint_slot_id`), :1351-1359 (`mint_bot_alias_id`); services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_bot.rs:246 (`store.mint_bot_id()`).

Cause 2 of 2: RecognizeText does not refuse a caller-supplied `sessionState` whose `intent.name` is not an intent of the addressed locale.
AWS: Smithy: `RecognizeText` models `ValidationException` with `httpError 400` (errorKind client), documented as "One of the input parameters in your request isn't valid". The recording shows AWS refusing a `sessionState` of `{"dialogAction":{"type":"Delegate"},"intent":{"name":"<undefined intent>"}}` with HTTP 400, error code `ValidationException` and the message `Invalid Request: The intent name <intentName> isn't valid. Provide a valid intent and try the request again.` — the fixed text is `Invalid Request: The intent name ` … ` isn't valid. Provide a valid intent and try the request again.` and the only variable part is `<intentName>`, taken verbatim from the request's `sessionState.intent.name`. The refusal must also open no session: a `GetSession` on the same session id afterwards is `ResourceNotFoundException`.
Emulator: the turn is ACCEPTED with HTTP 200. The handler resolves the bot/alias/locale and then hands the caller's `session_state` straight to `crate::state::take_turn` without applying the locale check the crate already owns, so the undeclared intent name is dropped and the utterance is instead interpreted normally: the replay observed `200` with `sessionState.intent` = the locale's own intent in state `Fulfilled`, `dialogAction.type` `Close`, closing message served, `interpretations` listing that intent plus `FallbackIntent` — and the session is created, so the follow-on `GetSession` finds one. No `ValidationException` is raised at all.
Why: a client that relies on Lex rejecting a delegated dialog state it cannot honour gets a silent, WRONG success: a turn it expected refused instead fulfils some other intent and persists a session, so the client's error path is never taken and it acts on a reply for an intent it did not delegate to.
Fix: `RecognizeText`'s input validation in the lexruntimev2 crate — apply the crate's existing `BotLocaleModel::check_session_state` to `input.session_state` after the locale is resolved and before the turn is taken (the sibling `PutSession` already does exactly this), and raise `ValidationException` (HTTP 400) with the message wording above so nothing is stored and no session is opened. Note: the open issue 20261125T114912Z-lex-runtime-v2-recognizetext-validation.md describes the same missing `check_session_state` call on the turn path; it was raised from the static surface and does NOT carry this behavioral check's replay key, and it pins the PutSession wording ("The intent is invalid.") rather than the RecognizeText wording AWS actually returns for this case, which is itself part of the requirement here.
Traces: lexv2-runtime.recognize-text-refuses-session-state-naming-an-undefined-intent. Code: services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0/src/ops/recognize_text.rs:258-289 (resolves `built_locale`, then calls `take_turn` with no `check_session_state`); the sibling that does check it: src/ops/put_session.rs:299-301; the helper: src/state/bot_definition.rs:62-70.

## Evidence
Cause 1 — expected vs actual:
Expected (recorded AWS): `CreateBot` -> `botId` matching `^[0-9A-Z]{10}$` (ten characters, digits and UPPERCASE letters only). Smithy `Id`: `{"type":"string","length":{"min":10,"max":10},"pattern":"^[0-9a-zA-Z]+$"}`.
Actual (emulator): a ten-character id that contains lowercase letters, failing the checks' id guard:
  `AssertionError: CreateBot returned a bot id of 10 characters, expected 10 of [0-9A-Z]` (check_lexv2_runtime_get_session_interprets_an_intent_whose_slot_retries_ran_out_as_failed.py:104, and identically at :104/:114/:116 in the declined-confirmation, put-session-locale, unsupported-content-type and spent-slot-retries checks)
  `AssertionError: the bot id is 10 chars, expected 10 of [0-9A-Z]` (check_lexv2_runtime_recognize_utterance_refuses_an_alias_the_bot_does_not_have.py:152, guard `_ID = re.compile(r"[0-9A-Z]{10}")`)
Source of the divergence — services/.../lexmodelsv2.../src/state.rs:1942:
  `const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";`
consumed by `render_id` (:1941-1952) and therefore by `mint_bot_id`/`mint_bot_alias_id`/`mint_intent_id`/`mint_slot_id`/`mint_slot_type_id`.

Cause 2 — expected vs actual:
Expected (recorded AWS), RecognizeText with `sessionState={"dialogAction":{"type":"Delegate"},"intent":{"name":"<undefined>"}}`:
  HTTP 400, `Error.Code` = `ValidationException`,
  `Error.Message` = `Invalid Request: The intent name <undefined> isn't valid. Provide a valid intent and try the request again.`
  and no session opened (subsequent GetSession -> `ResourceNotFoundException`, message `The session for user <sessionId> doesn't exist or has expired.`).
Actual (emulator): HTTP 200 with a fulfilled turn for the locale's own intent, e.g.
  `{'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'messages': [{'content': 'Your room is booked.', 'contentType': 'PlainText'}], 'sessionState': {'dialogAction': {'type': 'Close'}, 'intent': {'name': 'BookRoom', 'slots': {}, 'state': 'Fulfilled', 'confirmationState': 'None'}, 'activeContexts': [], 'sessionAttributes': {}, 'originatingRequestId': '<uuid>'}, 'interpretations': [{'nluConfidence': {'score': 1.0}, 'intent': {'name': 'BookRoom', ...}, 'interpretationSource': 'Lex'}, {'intent': {'name': 'FallbackIntent', 'slots': {}}, 'interpretationSource': 'Lex'}], 'requestAttributes': {}, 'sessionId': '<session>'}`
  raising `AssertionError: the call was accepted but should have been refused` at check_lexv2_runtime_recognize_text_refuses_session_state_naming_an_undefined_intent.py:28. (The uuid, session id and host-specific values above are per-run; only the status, the absence of the error and the fulfilled-intent shape are the divergence.)
Smithy: `RecognizeText` errors include `{"shape":"ValidationException","errorKind":"client","httpError":400}`; http binding `POST /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}/text`.

## Steps to Reproduce
Cause 1:
1. Call lexv2-models `CreateBot` with a valid `botName`, `roleArn`, `dataPrivacy={childDirected:false}` and `idleSessionTTLInSeconds=300`.
2. Read `botId` from the response and test it against `^[0-9A-Z]{10}$`. The emulator returns ten characters that include lowercase letters, so the test fails; real Lex V2 ids always pass it.
3. Repeat with `CreateBotAlias`, `CreateIntent`, `CreateSlot` and `CreateSlotType` — `botAliasId`, `intentId`, `slotId` and `slotTypeId` come from the same `render_id` helper and show the same lowercase characters.

Cause 2:
1. Create a bot, create the `en_US` locale, create one slotless intent (e.g. `BookRoom`) with sample utterances and an `intentClosingSetting`, and `BuildBotLocale` until the locale is `Built`; take the DRAFT alias id from `ListBotAliases`.
2. Call lexv2-runtime `RecognizeText` on that bot/alias/locale with a fresh `sessionId`, `text="book a room"` and `sessionState={"dialogAction": {"type": "Delegate"}, "intent": {"name": "CancelRoom"}}` — a name the locale does not define.
3. Expected: HTTP 400 `ValidationException` "Invalid Request: The intent name CancelRoom isn't valid. Provide a valid intent and try the request again." Observed: HTTP 200 with `sessionState.intent.name` = `BookRoom`, `state` = `Fulfilled`.
4. Call `GetSession` on the same `sessionId`: expected `ResourceNotFoundException` (the refused turn opens no session); observed a stored session, because the turn was accepted.
