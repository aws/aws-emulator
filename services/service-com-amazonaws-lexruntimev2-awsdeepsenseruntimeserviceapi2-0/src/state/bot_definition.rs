// SPDX-License-Identifier: CC0-1.0

//! What the bot's definition says about a dialog state a CALLER seeds.
//!
//! `PutSession` "sets the state of the session with the user. You can use this
//! to set the current intent, attributes, context, and dialog action" — so the
//! state it is handed is not something the runtime derived, it is something a
//! client asserted about a conversation with one bot. Two things follow, and
//! both are read off the built locale [`super::bot_model`] resolved:
//!
//! * the state has to name resources that bot declares, else it describes a
//!   dialog the bot could never be in ([`BotLocaleModel::check_session_state`]);
//! * the dialog it describes is reported as the bot's own, not as the caller
//!   spelled it ([`BotLocaleModel::completed_session_state`]): the active intent
//!   carries the slots that bot declares for it and the dialog members every
//!   reported intent carries, and the conversation is interpreted as being on
//!   that intent;
//! * the messages that state's dialog action sends are the ones the BOT
//!   authors for it, which is what a response reports when the request carries
//!   no messages of its own ([`BotLocaleModel::authored_messages`]).
//!
//! The turn's own side of the same model — matching an utterance, filling slots,
//! choosing the next dialog action — is [`super::nlu`]'s.

use crate::ops::types::{Intent, Interpretation, Message, SessionState};
use crate::state::bot_model::BotLocaleModel;
use crate::state::nlu::{active_intent, declared_slots, DIALOG_ACTION_ELICIT_SLOT};
use crate::state::DIALOG_ACTION_CLOSE;

/// `ConfirmIntent`: the next action is to ask the user to confirm the intent
/// (`DialogAction$type`, whose member set is `DIALOG_ACTION_TYPE_VALUES`).
pub const DIALOG_ACTION_CONFIRM_INTENT: &str = "ConfirmIntent";

/// Why a `sessionState` a caller supplied cannot be stored: it names something
/// the resolved locale does not define, so the dialog it describes is not one
/// this bot could ever be in.
#[derive(Debug, Clone, Copy)]
pub enum SeedRefused {
    /// `sessionState.intent.name` is not an intent of the locale.
    Intent,
    /// `sessionState.dialogAction.slotToElicit` is not a slot of that intent.
    Slot,
}

impl SeedRefused {
    /// The sentence Amazon Lex V2 answers with — one fixed sentence per member,
    /// naming which half of the seed is wrong.
    ///
    /// The intent's sentence is the one a recorded refusal carries. The slot's
    /// is stated in the same form for the sibling member, since no recording of
    /// that refusal is held here: a later recording would word this one, not the
    /// rule it refuses under.
    pub fn message(&self) -> String {
        match self {
            Self::Intent => "The intent is invalid.".to_string(),
            Self::Slot => "The slot is invalid.".to_string(),
        }
    }
}

impl BotLocaleModel {
    /// Refuse a `sessionState` a caller seeded that names something this locale
    /// does not define: the intent it names has to be an intent of the locale,
    /// and the slot its dialog action elicits has to be a slot of that intent.
    ///
    /// A state that names NO intent names nothing to check: which slots exist is
    /// a property of an intent, so an elicited slot beside no intent names
    /// nothing the locale could contradict — it is the next turn, whose intent
    /// the bot decides, that would have to make sense of it.
    pub fn check_session_state(&self, state: &SessionState) -> Result<(), SeedRefused> {
        let Some(named) = state.intent.as_ref() else {
            return Ok(());
        };
        let intent = self.intent(&named.name).ok_or(SeedRefused::Intent)?;
        let elicited = state
            .dialog_action
            .as_ref()
            .filter(|action| action.r#type == DIALOG_ACTION_ELICIT_SLOT)
            .and_then(|action| action.slot_to_elicit.as_deref());
        if elicited.is_some_and(|slot| intent.slot(slot).is_none()) {
            return Err(SeedRefused::Slot);
        }
        Ok(())
    }

    /// The dialog state this locale reports for the one a caller SEEDED, and the
    /// interpretations that go with it.
    ///
    /// A seeded state is a client's assertion about a conversation with this bot
    /// (`PutSession` "sets the state of the session with the user"), so what is
    /// stored — and read back — is that dialog as the BOT has it, completed
    /// against the locale exactly as a turn's own dialog is:
    ///
    /// * the active intent names "all of the slots for the intent", the ones the
    ///   locale declares for it, each carrying the value the seed filled or the
    ///   documented null ([`declared_slots`]), and carries the dialog members
    ///   every reported active intent carries ([`active_intent`]);
    /// * the conversation is on that intent, so it is reported as the
    ///   interpretation of the session — "an object containing information about
    ///   an intent that Amazon Lex V2 determined might satisfy the user's
    ///   utterance" (`Interpretation`). Nothing was said to the bot yet, so there
    ///   is no confidence to report for it and no service that interpreted an
    ///   input.
    ///
    /// A state that seeds NO intent leaves the conversation on none, and there is
    /// then nothing to interpret: the list is empty.
    pub fn completed_session_state(
        &self,
        mut state: SessionState,
    ) -> (SessionState, Vec<Interpretation>) {
        let Some(mut seeded) = state.intent.take() else {
            return (state, Vec::new());
        };
        let filled = seeded.slots.take().unwrap_or_default();
        let slots = match self.intent(&seeded.name) {
            Some(authored) => declared_slots(authored, &filled),
            // An intent the locale does not declare has no slots here to name,
            // so what the seed carried for it stands.
            None => filled,
        };
        let intent = active_intent(Intent {
            slots: Some(slots),
            ..seeded
        });
        let interpretations = vec![Interpretation {
            nlu_confidence: None,
            sentiment_response: None,
            intent: Some(intent.clone()),
            interpretation_source: None,
        }];
        state.intent = Some(intent);
        (state, interpretations)
    }

    /// The messages the BOT authors for the dialog state `state` describes,
    /// "ordered ... the order that the messages are defined in the bot".
    ///
    /// A dialog state the bot has a message for is answered with that message:
    /// eliciting a slot plays the slot's own `promptSpecification`, confirming
    /// an intent plays its `intentConfirmationSetting`, and closing one plays
    /// its `intentClosingSetting.closingResponse`. Every other dialog action has
    /// no message authored for it on the bot, so there is nothing to say and
    /// nothing is invented.
    pub fn authored_messages(&self, state: &SessionState) -> Vec<Message> {
        let Some(action) = state.dialog_action.as_ref() else {
            return Vec::new();
        };
        let Some(intent) = state
            .intent
            .as_ref()
            .and_then(|named| self.intent(&named.name))
        else {
            return Vec::new();
        };
        match action.r#type.as_str() {
            DIALOG_ACTION_ELICIT_SLOT => action
                .slot_to_elicit
                .as_deref()
                .and_then(|name| intent.slot(name))
                .map(|slot| slot.prompt_messages.clone())
                .unwrap_or_default(),
            DIALOG_ACTION_CONFIRM_INTENT => intent.confirmation_messages.clone(),
            DIALOG_ACTION_CLOSE => intent.closing_messages.clone(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    //! What a resolved locale says about a dialog state a caller seeded, over a
    //! table of states. The resolution itself is driven end to end over HTTP in
    //! `tests/all/bot_addressing.rs`, against a bot the control plane built.

    use super::*;
    use crate::ops::types::{DialogAction, Intent};
    use crate::state::bot_model::{ModelIntent, ModelSlot, MESSAGE_CONTENT_TYPE_PLAIN_TEXT};

    /// A locale declaring one intent with one prompted slot, a confirmation
    /// prompt and a closing response.
    fn locale() -> BotLocaleModel {
        BotLocaleModel {
            nlu_intent_confidence_threshold: 0.4,
            intents: vec![ModelIntent {
                name: "OrderShake".to_string(),
                // An intent authored from scratch, so it is based on no built-in
                // intent of Amazon Lex's.
                parent_intent_signature: None,
                sample_utterances: vec!["I would like a shake".to_string()],
                slots: vec![ModelSlot {
                    name: "Flavor".to_string(),
                    required: true,
                    prompt_messages: vec![plain("What flavor would you like?")],
                    slot_type: None,
                }],
                confirmation_messages: vec![plain("Shall I order it?")],
                closing_messages: vec![plain("Your order is placed")],
                fulfillment_by_code_hook: false,
            }],
        }
    }

    fn plain(content: &str) -> Message {
        Message {
            content: Some(content.to_string()),
            content_type: MESSAGE_CONTENT_TYPE_PLAIN_TEXT.to_string(),
            image_response_card: None,
        }
    }

    fn seed(action: Option<(&str, Option<&str>)>, intent: Option<&str>) -> SessionState {
        SessionState {
            dialog_action: action.map(|(kind, slot)| DialogAction {
                r#type: kind.to_string(),
                slot_to_elicit: slot.map(str::to_string),
                slot_elicitation_style: None,
                sub_slot_to_elicit: None,
            }),
            intent: intent.map(|name| Intent {
                name: name.to_string(),
                ..Intent::default()
            }),
            ..SessionState::default()
        }
    }

    /// RULE: a seeded session state names resources the locale defines; one that
    /// names an intent, or an elicited slot, it does not is refused.
    #[test]
    fn a_seed_is_checked_against_the_locale() {
        // (case, seed, the refusal owed — None where the seed is accepted)
        let rows: Vec<(&str, SessionState, Option<String>)> = vec![
            ("no intent named", seed(Some(("Delegate", None)), None), None),
            (
                "an intent the locale declares",
                seed(Some((DIALOG_ACTION_CLOSE, None)), Some("OrderShake")),
                None,
            ),
            (
                "an intent the locale does not declare",
                seed(Some((DIALOG_ACTION_CLOSE, None)), Some("NoSuchIntent")),
                Some(SeedRefused::Intent.message()),
            ),
            (
                "an intent differing from a declared one only in case",
                seed(Some((DIALOG_ACTION_CLOSE, None)), Some("ordershake")),
                Some(SeedRefused::Intent.message()),
            ),
            (
                "eliciting a slot of that intent",
                seed(
                    Some((DIALOG_ACTION_ELICIT_SLOT, Some("Flavor"))),
                    Some("OrderShake"),
                ),
                None,
            ),
            (
                "eliciting a slot the intent does not declare",
                seed(
                    Some((DIALOG_ACTION_ELICIT_SLOT, Some("Size"))),
                    Some("OrderShake"),
                ),
                Some(SeedRefused::Slot.message()),
            ),
            (
                "a slot named by a dialog action that elicits nothing",
                seed(Some(("Delegate", Some("Size"))), Some("OrderShake")),
                None,
            ),
            (
                "an elicited slot beside no intent, which names no slot to check",
                seed(Some((DIALOG_ACTION_ELICIT_SLOT, Some("Size"))), None),
                None,
            ),
        ];

        for (case, state, expected) in rows {
            let actual = locale()
                .check_session_state(&state)
                .err()
                .map(|refused| refused.message());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the messages of a dialog state are the ones the bot authored for
    /// that state, and nothing is invented for a state it authored none for.
    #[test]
    fn the_messages_of_a_dialog_state_are_the_bots_own() {
        // (case, seed, the contents owed)
        let rows: Vec<(&str, SessionState, Vec<&str>)> = vec![
            (
                "eliciting a prompted slot",
                seed(
                    Some((DIALOG_ACTION_ELICIT_SLOT, Some("Flavor"))),
                    Some("OrderShake"),
                ),
                vec!["What flavor would you like?"],
            ),
            (
                "confirming the intent",
                seed(
                    Some((DIALOG_ACTION_CONFIRM_INTENT, None)),
                    Some("OrderShake"),
                ),
                vec!["Shall I order it?"],
            ),
            (
                "closing the intent",
                seed(Some((DIALOG_ACTION_CLOSE, None)), Some("OrderShake")),
                vec!["Your order is placed"],
            ),
            (
                "delegating the next step",
                seed(Some(("Delegate", None)), Some("OrderShake")),
                vec![],
            ),
            (
                "eliciting an intent, with none named",
                seed(Some(("ElicitIntent", None)), None),
                vec![],
            ),
            (
                "eliciting a slot of an intent the locale does not declare",
                seed(
                    Some((DIALOG_ACTION_ELICIT_SLOT, Some("Flavor"))),
                    Some("NoSuchIntent"),
                ),
                vec![],
            ),
        ];

        for (case, state, expected) in rows {
            let messages = locale().authored_messages(&state);
            let actual: Vec<&str> = messages
                .iter()
                .filter_map(|message| message.content.as_deref())
                .collect();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
