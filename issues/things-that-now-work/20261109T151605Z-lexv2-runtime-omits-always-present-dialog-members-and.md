Issue-Source: trace-sweep-divergence
Issue-Trace: lex-runtime-v2-session-and-recognition-roundtrip
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-runtime-v2-session-and-recognition-roundtrip lexv2-runtime-recognize-text-fills-slot-and-fulfills-intent
Issue-Shape-Id: com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0
Issue-Key: lexv2-runtime|GetSession|2xx->2xx|$.interpretations[*]
Issue-Key: lexv2-runtime|GetSession|2xx->2xx|$.sessionState.activeContexts
Issue-Key: lexv2-runtime|GetSession|2xx->2xx|$.sessionState.intent.confirmationState
Issue-Key: lexv2-runtime|GetSession|2xx->2xx|$.sessionState.intent.slots
Issue-Key: lexv2-runtime|RecognizeText|2xx->2xx|$.interpretations[*]
Issue-Key: lexv2-runtime|RecognizeText|2xx->2xx|$.interpretations[*].intent.confirmationState
Issue-Key: lexv2-runtime|RecognizeText|2xx->2xx|$.requestAttributes
Issue-Key: lexv2-runtime|RecognizeText|2xx->2xx|$.sessionState.intent.confirmationState
Issue-Key: lexv2-runtime|GetSession|2xx->2xx|$.interpretations[*].intent.confirmationState
Issue-Commit: 3d0a54f64a8eacec16ca43902aa47fe37888ae5e

# lexv2-runtime omits always-present dialog members and the fallback interpretation from session and turn responses

> _Note: this issue was identified against the code base at commit `3d0a54f64a8eacec16ca43902aa47fe37888ae5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSession

## Description
Five net-new causes in lexv2-runtime, all in the shape of a 200 response: members AWS always reports are omitted when they are empty or unset, and the fallback interpretation is never reported at all. Every affected step returned the right status (200) and the right scalar values; what diverges is which members exist on the wire. A fixer must close all five.

Related open issue which does NOT cover this: 20261107T161252Z-lexv2-runtime-recognizetext-reports-intent-slots-holding-only-filled-slots.md. That one is about the CONTENTS of Intent.slots on a RecognizeText turn (declared-but-unfilled slots missing from a map that is present); it landed for the turn path (src/state/nlu.rs::declared_slots now seeds the declared slots). Cause 5 below is a different path — a session seeded by PutSession, read back by GetSession, where the slots member is ABSENT altogether — and none of the other four causes concern slots at all. None of these clusters are owned by an open issue in the replay report.

Cause 1 of 5: Intent.confirmationState is omitted instead of being reported as "None".
AWS: Smithy com.amazonaws.lexruntimev2#Intent$confirmationState targets the ConfirmationState enum (values Confirmed, Denied, None), documented "Indicates whether the intent has been Confirmed, Denied, or None if the confirmation stage has not yet been reached." So an intent whose confirmation stage has not been reached is reported with the member PRESENT and set to the enum value "None" — on the Intent inside sessionState and on the Intent inside each interpretation. The recordings show exactly that for every reported Intent of a dialog that has no confirmation prompt.
Emulator: the member is absent from both sessionState.intent and interpretations[0].intent on every affected step ($.sessionState.intent.confirmationState: missing (expected 'None', got None); $.interpretations[0].intent.confirmationState: missing (expected 'None', got None)). In src/state/nlu.rs:262 the field is built as `confirmation_state: continued.and_then(|intent| intent.confirmation_state.clone())`, so it is None (and therefore omitted) unless a caller already put one in the session; the alternative interpretations built at src/state/nlu.rs:285-292 hard-code `confirmation_state: None`.
Why: a client reading intent["confirmationState"] to decide whether it is in a confirmation turn gets a KeyError/absent member instead of the explicit "None", and cannot distinguish "confirmation stage not yet reached" from "this service does not report confirmation at all". Generated SDK models that type the member as a non-optional enum fail to deserialize.
Fix: the turn component that assembles the Intent in the runtime crate (src/state/nlu.rs, both the turn's own intent and the alternative interpretations) must default confirmationState to the enum value "None" whenever no confirmation has been recorded, and the session store must carry that value so the GetSession serializer reports it identically.
Traces: lex-runtime-v2-session-and-recognition-roundtrip#task#15 (GetSession), #task#16 (RecognizeText), #task#17 (GetSession), lexv2-runtime-recognize-text-fills-slot-and-fulfills-intent#task#12 and #task#13.

Cause 2 of 5: sessionState.activeContexts is omitted when the session has no active context, instead of being reported as the empty list.
AWS: Smithy SessionState$activeContexts targets ActiveContextsList (list, length 0..20) — the model admits a list of length 0, and the recorded behavior (recorded behavior only; the model does not state presence) is that AWS reports `"activeContexts": []` on the session state of a conversation that has set no context.
Emulator: the member is absent ($.sessionState.activeContexts: missing (expected [], got None)). src/state.rs:630, in Session::visible_session_state, computes `state.active_contexts = (!live.is_empty()).then_some(live);` — an empty live set is turned into None, so the member is dropped rather than serialized as [].
Why: a client that iterates sessionState["activeContexts"] to decide which follow-up intents are eligible must special-case an absent member against the emulator but not against AWS; code that reads the member directly raises instead of looping zero times.
Fix: the session-state projection in the runtime crate's state component (src/state.rs, Session::visible_session_state) must always report the live-context list, empty included, rather than suppressing the empty case.
Traces: lex-runtime-v2-session-and-recognition-roundtrip#task#15 (GetSession), #task#17 (GetSession).

Cause 3 of 5: RecognizeText omits requestAttributes when the session has none, instead of reporting the empty map.
AWS: Smithy RecognizeTextResponse$requestAttributes targets StringMap (map). Recorded behavior only: AWS answers a turn that carried no request attributes with `"requestAttributes": {}` — the member present and empty.
Emulator: the member is absent ($.requestAttributes: missing (expected {}, got None)). src/ops/recognize_text.rs:305-306 builds it as `request_attributes: (!turn.request_attributes.is_empty()).then_some(turn.request_attributes)`, which drops the member for an empty map.
Why: a client that reads response["requestAttributes"] to carry attributes forward across turns gets an absent member rather than an empty map, so attribute-threading code raises or silently skips on the first turn.
Fix: the RecognizeText response serializer in the runtime crate (src/ops/recognize_text.rs, the output construction) must always report the accumulated request-attribute map, empty included.
Traces: lex-runtime-v2-session-and-recognition-roundtrip#task#16 (RecognizeText), lexv2-runtime-recognize-text-fills-slot-and-fulfills-intent#task#12.

Cause 4 of 5: the fallback intent is never reported as an alternative interpretation, so the interpretations list is short by one entry.
AWS: Smithy Interpretation is documented "An object containing information about an intent that Amazon Lex V2 determined might satisfy the user's utterance. The intents are ordered by the confidence score", and Interpretation$nluConfidence is documented as "Determines the threshold where Amazon Lex V2 will insert the AMAZON.FallbackIntent ... when returning alternative intents in a response" — i.e. the fallback intent is one of the alternatives Lex inserts into the list. The recording shows the entry AWS adds after the recognized intent: an interpretation whose intent names the locale's fallback intent with an empty slots map and whose interpretationSource is "Lex", carrying no nluConfidence. The intent NAME is a value from the authored locale (the fallback intent every locale carries); the presence of the entry, its empty slots map and its interpretationSource are the rule.
Emulator: the entry does not exist ($.interpretations[1]: missing (expected {'intent': {'name': 'FallbackIntent', 'slots': {}}, 'interpretationSource': 'Lex'}, got None)). src/state/nlu.rs:132-137 builds the candidate list only from intents that declare sample utterances and the module comment at :123-126 states the fallback intent "is not scored but inserted by the threshold rule below" — but the insertion only ever replaces the RECOGNIZED intent (:155-164); nothing adds the fallback intent to the alternatives emitted at :275-296.
Why: a client that inspects the interpretations list to see whether Lex considered the fallback — to score confidence, to log alternates, or to drive a disambiguation prompt — sees one fewer alternative than AWS reports and concludes the fallback was never a candidate. Code indexing interpretations[1] raises.
Fix: the interpretation assembly in the runtime crate (src/state/nlu.rs::interpret, where the alternative interpretations are built) must include the locale's fallback intent as an alternative interpretation — empty slots map, interpretationSource "Lex", no nluConfidence — alongside the scored intents, within the modelled list bound of 5.
Traces: lex-runtime-v2-session-and-recognition-roundtrip#task#16 (RecognizeText), lexv2-runtime-recognize-text-fills-slot-and-fulfills-intent#task#12.

Cause 5 of 5: GetSession on a session seeded by PutSession reports no interpretations and no slots map — the seeded dialog state is never normalized against the built locale.
AWS: Smithy GetSessionResponse$interpretations targets Interpretations (list 0..5) and Intent$slots targets Slots, documented "A map of all of the slots for the intent. The name of the slot maps to the value of the slot. If a slot has not been filled, the value is null." Recorded behavior: after a PutSession that seeds an active intent, GetSession on the same botId/botAliasId/localeId/sessionId reports an interpretation for that active intent — the intent's name, its declared slots keyed with null for each unfilled one, its state and confirmationState "None" — and the same slots map under sessionState.intent. The slot and intent NAMES are values from the authored locale; the rule is that the reported intent is completed against the built locale rather than echoed verbatim.
Emulator: GetSession after a PutSession-seeded session reports neither ($.interpretations[0]: missing (expected {'intent': {'confirmationState': 'None', 'name': '<intent>', 'slots': {'<declaredSlot>': None}, 'state': 'InProgress'}}, got None); $.sessionState.intent.slots: missing (expected {'<declaredSlot>': None}, got None)). src/state.rs:663-674, Session::replace_session_state, stores the supplied SessionState as-is and never fills the intent's declared slots, and nothing on that path populates Session::interpretations (initialised empty at src/state.rs:568 and written only by a turn at :436); src/ops/get_session.rs:190-195 then reports `session.interpretations` and `session.visible_session_state(now)` verbatim, so the read is empty.
Why: a client that seeds a dialog with PutSession and then reads it back — a resumed conversation, a handoff between front ends — sees an active intent with no slot map and no interpretation, so it cannot tell which slots are still open or which intent Lex considers active, and any loop over interpretations finds nothing even though the session is mid-dialog.
Fix: the session component in the runtime crate must normalize a seeded dialog state against the built locale when PutSession stores it (src/state.rs, Session::replace_session_state): complete the active intent's slots map with every slot the locale declares for that intent (null for unfilled) and record the interpretation for the active intent, so GetSession's serializer (src/ops/get_session.rs) reports the same shape a turn would.
Traces: lex-runtime-v2-session-and-recognition-roundtrip#task#15 (GetSession).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 85.7%, 7 mismatched steps of 49, all in lexv2-runtime). Three divergence clusters, all status-matched (expected 200 / actual 200) with member-level mismatches only.

Cluster A — fixture lex-runtime-v2-session-and-recognition-roundtrip, step task#15, operation GetSession, verdict mismatch (expected 200 / actual 200), affected: lex-runtime-v2-session-and-recognition-roundtrip#task#15:
  $.interpretations[0]: missing (expected {'intent': {'confirmationState': 'None', 'name': 'OrderShake', 'slots': {'Flavor': None}, 'state': 'InProgress'}}, got None)
  $.sessionState.activeContexts: missing (expected [], got None)
  $.sessionState.intent.confirmationState: missing (expected 'None', got None)
  $.sessionState.intent.slots: missing (expected {'Flavor': None}, got None)

Cluster B — fixture lex-runtime-v2-session-and-recognition-roundtrip, step task#16, operation RecognizeText, verdict mismatch (expected 200 / actual 200), affected: lex-runtime-v2-session-and-recognition-roundtrip#task#16, lexv2-runtime-recognize-text-fills-slot-and-fulfills-intent#task#12:
  $.interpretations[0].intent.confirmationState: missing (expected 'None', got None)
  $.interpretations[1]: missing (expected {'intent': {'name': 'FallbackIntent', 'slots': {}}, 'interpretationSource': 'Lex'}, got None)
  $.requestAttributes: missing (expected {}, got None)
  $.sessionState.intent.confirmationState: missing (expected 'None', got None)

Cluster C — fixture lex-runtime-v2-session-and-recognition-roundtrip, step task#17, operation GetSession, verdict mismatch (expected 200 / actual 200), affected: lex-runtime-v2-session-and-recognition-roundtrip#task#17, lexv2-runtime-recognize-text-fills-slot-and-fulfills-intent#task#13:
  $.interpretations[0].intent.confirmationState: missing (expected 'None', got None)
  $.sessionState.activeContexts: missing (expected [], got None)
  $.sessionState.intent.confirmationState: missing (expected 'None', got None)

The intent name OrderShake, the slot name Flavor and the session/bot identifiers are values from the fixture's authored locale and request, not part of the requirement; the requirements are the member presence and the enum/empty-collection values.

Source evidence (services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0):
  src/state/nlu.rs:262 — confirmationState taken only from a continued intent, so omitted by default (Cause 1).
  src/state/nlu.rs:285-292 — alternative interpretations hard-code confirmation_state: None (Cause 1).
  src/state.rs:630 — `state.active_contexts = (!live.is_empty()).then_some(live);` drops the empty list (Cause 2).
  src/ops/recognize_text.rs:305-306 — `(!turn.request_attributes.is_empty()).then_some(...)` drops the empty map (Cause 3).
  src/state/nlu.rs:132-137, :275-296 — candidate list excludes the fallback intent, so it is never an alternative (Cause 4).
  src/state.rs:568, :436, :663-674 and src/ops/get_session.rs:190-195 — PutSession stores the seeded state verbatim and sets no interpretations; GetSession reports them verbatim (Cause 5).

Smithy cited: com.amazonaws.lexruntimev2#Intent$confirmationState -> ConfirmationState enum [Confirmed, Denied, None] ("... or None if the confirmation stage has not yet been reached"); Intent$slots -> Slots ("If a slot has not been filled, the value is null"); SessionState$activeContexts -> ActiveContextsList (list, length 0..20); RecognizeTextResponse$requestAttributes -> StringMap; GetSessionResponse$interpretations -> Interpretations (list, length 0..5); Interpretation$nluConfidence (threshold at which AMAZON.FallbackIntent is inserted among the alternative intents).

## Steps to Reproduce
1. Boot the emulator from this checkout. Using lexv2-models, create a bot with an en_US locale that declares one intent with sample utterances and one Required slot over a custom slot type, plus a closing response; build the locale and note the DRAFT test alias id.
2. PutSession(botId, botAliasId, localeId=en_US, sessionId=<fresh>) seeding sessionState with that intent active (state InProgress, dialogAction ElicitSlot on the declared slot).
3. GetSession on the same four labels. Emulator: no interpretations member entry at all, sessionState.intent has no slots member and no confirmationState, sessionState has no activeContexts member. AWS: interpretations[0] carries the active intent with confirmationState "None", state InProgress and slots = {"<declaredSlot>": null}; sessionState.intent carries the same slots map and confirmationState "None"; sessionState.activeContexts = [].
4. RecognizeText(botId, botAliasId, localeId=en_US, sessionId=<same>, text=<an utterance naming the intent>) with no requestAttributes. Emulator: response has no requestAttributes member, interpretations holds only the recognized intent (no fallback entry), and neither the sessionState intent nor the interpretation intent carries confirmationState. AWS: requestAttributes = {}, interpretations[1] = {"intent": {"name": "<the locale's fallback intent>", "slots": {}}, "interpretationSource": "Lex"}, and every reported Intent carries confirmationState "None".
5. GetSession again. Emulator: interpretations[0].intent and sessionState.intent still lack confirmationState and sessionState still lacks activeContexts. AWS: both carry confirmationState "None" and activeContexts = [].
