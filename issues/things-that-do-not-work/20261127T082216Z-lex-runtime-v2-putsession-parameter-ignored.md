Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: b0742486eb968c338848501643305914f4d73195

# A seeded sessionState.intent.slots entry whose key differs in case from the declared slot name is silently dropped, although the same spelling is accepted as that slot by dialogAction.slotToElicit in the same request

_Filed: 2026-11-27T08:22:16Z_
_Operation: PutSession | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `b0742486eb968c338848501643305914f4d73195`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutSession resolves a seeded dialog state against the built locale twice, through two slot lookups that disagree about case:

* the slot a dialog action elicits is looked up with `ModelIntent::slot`, which compares names case-INSENSITIVELY — so `dialogAction.slotToElicit: "size"` is accepted as the locale's declared slot `Size`, passes `check_session_state`, and even gets that slot's authored prompt played back in the `x-amz-lex-messages` header;
* the slot MAP the caller seeded is completed by `declared_slots`, which looks each declared slot up in the caller's map by EXACT string — so `slots: {"size": {...}}` matches nothing, the value the caller supplied is never stored, and the session reads back as `{"Size": null}`.

The request is therefore answered 200 while one half of it is interpreted as naming the declared slot and the other half is discarded as naming nothing. The supplied slot value is neither stored nor refused: it is silently lost, and a later GetSession reports the slot as unfilled, so the dialog will elicit a value the caller already supplied.

This is the crate's own normalisation being applied on one path and skipped on the other: the turn path (`nlu::interpret`) looks the slot up case-insensitively and then re-keys the map entry under the DECLARED spelling (`slots.insert(slot.name.clone(), ...)`) before it is stored, which is exactly the step the seed path omits.

## Evidence

Case-insensitive lookup, with its own doc stating the rule — services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0/src/state/bot_model.rs:166-174:
```
    /// The slot of this intent that name declares. A sample utterance names a
    /// slot by its name, which is unique within the intent and compared without
    /// regard to case, as a bot author writes it in prose.
    pub fn slot(&self, name: &str) -> Option<&ModelSlot> {
        self.slots
            .iter()
            .find(|slot| slot.name.eq_ignore_ascii_case(name))
    }
```

The seed path keys the caller's map EXACTLY — src/state/nlu.rs:426-435 (`declared_slots`):
```
    authored
        .slots
        .iter()
        .map(|slot| {
            // Keyed by the slot's own name, which is what this dialog writes
            // and what a caller seeding the session names the slot by.
            let value = filled.get(&slot.name).cloned().unwrap_or_else(unfilled);
            (slot.name.clone(), value)
        })
```
The comment ASSERTS the caller spells the slot canonically; the sibling lookup quoted above does not require that, so a spelling the crate accepts for `slotToElicit` is dropped here.

The turn path does normalise before storing — src/state/nlu.rs:287-291:
```
        for (slot_name, words) in &captures {
            if let Some(slot) = authored.slot(slot_name) {
                if let Some(value) = resolve_slot_value(slot.slot_type.as_ref(), words) {
                    slots.insert(slot.name.clone(), filled(value));
```

The seed path, which reaches `declared_slots` with the caller's own keys — src/state/bot_definition.rs:114-120:
```
        let filled = seeded.slots.take().unwrap_or_default();
        let slots = match self.intent(&seeded.name) {
            Some(authored) => declared_slots(authored, &filled),
```
and its validation, which never looks at the slot map at all while accepting a case-differing `slotToElicit` — src/state/bot_definition.rs:75-82:
```
        let elicited = state
            .dialog_action
            .as_ref()
            .filter(|action| action.r#type == DIALOG_ACTION_ELICIT_SLOT)
            .and_then(|action| action.slot_to_elicit.as_deref());
        if elicited.is_some_and(|slot| intent.slot(slot).is_none()) {
            return Err(SeedRefused::Slot);
        }
```

The handler that drives both — src/ops/put_session.rs:299-301 and :353:
```
        locale
            .check_session_state(&input.session_state)
            .map_err(seed_refused)?;
...
            session.replace_session_state(input.session_state, now, &locale);
```

Test coverage: tests/all/always_reported_members.rs:233-333 (`a_seeded_dialog_is_read_back_completed_against_the_locale`) tables seeded slot maps, including "a seed naming a slot the intent does not declare" (`Crust`), but no row spells a DECLARED slot in a different case, so this path is unpinned. The fixture's declared slot is `SLOT = "Size"` (tests/all/harness.rs:50).

## Steps to Reproduce

1. Build a bot locale through the Lex Models V2 control plane declaring intent `OrderPizza` with one required, prompted slot named `Size` (the sequence in tests/all/harness.rs `create_intent_with_slot`).
2. POST /bots/{botId}/botAliases/TSTALIASID/botLocales/en_US/sessions/seed with body {"sessionState":{"dialogAction":{"type":"ElicitSlot","slotToElicit":"size"},"intent":{"name":"OrderPizza","state":"InProgress","slots":{"size":{"value":{"interpretedValue":"large"}}}}}}
3. Observe HTTP 200, and that the `x-amz-lex-messages` header carries the slot's authored prompt — proof that `"size"` was resolved to the declared slot `Size` for the dialog action.
4. GET the same session path and decode `sessionState.intent.slots`: it is {"Size": null}. The value supplied under `"size"` was neither stored nor refused.
5. Contrast with a turn: POST .../text with {"text":"I want to order a pizza"} then an answer for the slot — the turn path re-keys the filled slot under the declared spelling before storing it (src/state/nlu.rs:290).
