// SPDX-License-Identifier: CC0-1.0

//! One turn of the conversation: what the built locale makes of an utterance,
//! and the dialog step that leaves the user.
//!
//! This is the half of a turn that is about the BOT rather than about the
//! session store: given the locale [`crate::state::bot_model`] read off the
//! control plane, the words the user said, and the dialog state the session is
//! in, it answers with the intent of the turn, the slots it filled, the next
//! dialog action and the messages to send. `RecognizeText` and
//! `RecognizeUtterance` are two encodings of the same turn, so both reach it
//! through [`crate::state::take_turn`] and neither holds a second copy of the
//! dialog.
//!
//! The rules it implements, each from the modelled contract:
//!
//! 1. An utterance is matched against the sample utterances the locale's intents
//!    declare; the best match at or above the locale's own
//!    `nluIntentConfidenceThreshold` is the intent of the turn, and when nothing
//!    matches the conversation stays on the intent it is already on, or falls
//!    back to the locale's own fallback intent
//!    ([`crate::state::bot_model::BotLocaleModel::fallback_intent`]) — with the
//!    locale's own intents reported beside it at the scores they did reach,
//!    nought included.
//! 2. A sample utterance names a slot of its intent in braces, and the words in
//!    that position are what the user said for the slot; the answer to a slot the
//!    previous turn elicited is the whole utterance. Either way the words are
//!    resolved against the slot type's own vocabulary
//!    ([`crate::state::bot_model::resolve_slot_value`]), and a word that resolves
//!    to nothing leaves the slot unfilled.
//! 3. While a required slot of the intent has no value the turn ends on
//!    `ElicitSlot` for that slot, with the prompt authored on it and the intent
//!    `InProgress`; once every required slot has one the dialog closes, and the
//!    intent is `Fulfilled` where the bot completed it itself — it has a closing
//!    response to convey that it did and no fulfilment code hook to hand the
//!    intent to
//!    ([`crate::state::bot_model::ModelIntent::bot_completes_fulfillment`]) —
//!    and `ReadyForFulfillment` where the fulfilment is the caller's.
//! 4. Every turn reports every member a client drives a dialog off:
//!    `Intent.state`; `Intent.slots` — the map of the intent's slots, which
//!    names EVERY slot the recognized intent declares with a null value for each
//!    one that has none ("a map of all of the slots for the intent … if a slot
//!    has not been filled, the value is null", `Intent$slots`) and is the empty
//!    map for an intent that declares none; and `Intent.confirmationState`,
//!    which is [`CONFIRMATION_STATE_NONE`] until the dialog records a
//!    confirmation ([`active_intent`]).
//! 5. The alternatives a turn reports are the intents that "might satisfy the
//!    user's utterance" ordered by their confidence, and the locale's fallback
//!    intent is INSERTED among the alternatives of the RESPONSE —
//!    `Interpretation$nluConfidence` "determines the threshold where Amazon Lex
//!    V2 will insert the `AMAZON.FallbackIntent` … when returning alternative
//!    intents in a response" — so it is reported by the recognition that
//!    answers the utterance and not by a later read of the session, which
//!    reports what the recognition itself made of it
//!    ([`with_inserted`]).

use super::bot_model::{resolve_slot_value, BotLocaleModel, ModelIntent};
use super::{DIALOG_ACTION_CLOSE, INTERPRETATION_SOURCE, MAX_UTTERANCE_CHARS};
use crate::ops::types::{
    ConfidenceScore, DialogAction, Intent, Interpretation, Message, SessionState, Slot, Value,
};
use std::collections::HashMap;

/// The most words one utterance is split into.
///
/// A word costs at least one character, so an utterance within
/// [`MAX_UTTERANCE_CHARS`] — the bound both encodings of a turn apply before
/// they get here — can never reach this many: the cap is what makes the
/// allocation bounded AT the split, whatever path reached it, rather than only
/// upstream of it.
pub const MAX_UTTERANCE_WORDS: usize = MAX_UTTERANCE_CHARS;

/// The most work one turn spends matching, counted in word comparisons.
///
/// A turn scans every intent of the locale and every sample utterance each one
/// declares, and both are grown by whoever AUTHORED the bot rather than by this
/// request, so the scan carries a budget: each sample costs at most its own
/// words times the words that were said, and once the budget is spent the turn
/// stops looking. Stopping can only leave the turn with a match no better than
/// the best it had already found — in the limit the fallback intent — never with
/// a wrong one.
///
/// The figure is far past any bot a client drives (the control plane admits
/// 1,000 intents in a locale, and a locale of 100 intents declaring 20 sample
/// utterances of 8 words each is matched in full against the longest utterance
/// the model allows), so a realistic bot is never cut short; what it bounds is
/// the pathological one.
pub const MAX_MATCH_WORK: usize = 10_000_000;

/// `ElicitSlot`: "the next action is to elicit a slot value from the user"
/// (`DialogAction$type`).
pub const DIALOG_ACTION_ELICIT_SLOT: &str = "ElicitSlot";

/// `InProgress`: "the bot is in the middle of eliciting the slot values that are
/// necessary to fulfill the intent" (`Intent$state`).
pub const INTENT_STATE_IN_PROGRESS: &str = "InProgress";

/// `ReadyForFulfillment`: "the bot has elicited all the slot values for the
/// intent and is ready to fulfill the intent" (`Intent$state`) — the state of an
/// intent whose fulfilment the bot does not perform itself.
pub const INTENT_STATE_READY_FOR_FULFILLMENT: &str = "ReadyForFulfillment";

/// `Fulfilled`: "the bot has completed fulfillment of the intent"
/// (`Intent$state`) — where an intent whose dialog the bot closed with its own
/// closing response lands, that response being the statement Amazon Lex conveys
/// "when the intent is successfully fulfilled"
/// (`IntentClosingSetting$closingResponse`).
pub const INTENT_STATE_FULFILLED: &str = "Fulfilled";

/// The most interpretations one turn reports: the modelled `Interpretations`
/// list is `len 0..5`.
pub const MAX_INTERPRETATIONS: usize = 5;

/// `None`: "indicates whether the intent has been `Confirmed`, `Denied`, or
/// `None` if the confirmation stage has not yet been reached"
/// (`Intent$confirmationState`, whose member set is
/// `CONFIRMATION_STATE_VALUES`) — so an intent whose dialog has not reached its
/// confirmation stage is reported with this value rather than with the member
/// left out.
pub const CONFIRMATION_STATE_NONE: &str = "None";

/// The intent a DIALOG is on, as this service reports it: the two members every
/// reported active intent carries beside its name and state are filled in, so a
/// client can read either without first testing whether it is there.
///
/// * `slots` is "a map of all of the slots for the intent" (`Intent$slots`), the
///   empty map where the dialog knows of none;
/// * `confirmationState` is [`CONFIRMATION_STATE_NONE`] until a confirmation has
///   been recorded for the intent, which is what "`None` if the confirmation
///   stage has not yet been reached" states.
///
/// An intent Amazon Lex V2 merely CONSIDERED is not a dialog's intent: an
/// alternative interpretation names the intent and the slots it would fill and
/// carries neither of these dialog members, so this is applied to the active
/// intent only.
pub fn active_intent(intent: Intent) -> Intent {
    Intent {
        slots: Some(intent.slots.unwrap_or_default()),
        confirmation_state: Some(
            intent
                .confirmation_state
                .unwrap_or_else(|| CONFIRMATION_STATE_NONE.to_string()),
        ),
        ..intent
    }
}

/// One alternative interpretation: an intent that "might satisfy the user's
/// utterance" (`Interpretation`), at the confidence it reached — none at all for
/// an intent Amazon Lex V2 INSERTS among the alternatives rather than scores.
fn alternative(name: String, score: Option<f64>) -> Interpretation {
    Interpretation {
        nlu_confidence: score.map(|score| ConfidenceScore { score }),
        sentiment_response: None,
        intent: Some(Intent {
            name,
            // An alternative is an intent the utterance MIGHT have satisfied: no
            // dialog is on it, so nothing is filled or being elicited for it and
            // the map it reports is empty.
            slots: Some(HashMap::new()),
            state: None,
            confirmation_state: None,
        }),
        interpretation_source: Some(INTERPRETATION_SOURCE.to_string()),
    }
}

/// What one turn made of the utterance: everything it leaves on the session,
/// and the entry only the RESPONSE to a recognition carries.
#[derive(Debug, Clone)]
pub struct TurnPlan {
    /// The intent the conversation is on after the turn, with the slots filled
    /// so far and the state the dialog reached.
    pub intent: Intent,
    /// What the bot does next.
    pub dialog_action: DialogAction,
    /// The messages this turn sends the user — a slot's prompt, or the intent's
    /// closing response. Empty when the bot has nothing authored to say.
    pub messages: Vec<Message>,
    /// What the RECOGNITION made of the utterance: the intents it scored that
    /// might satisfy it, "ordered by the confidence score" (`Interpretation`),
    /// the turn's own first. This is what the session holds, so it is also what
    /// a later `GetSession` reports.
    pub interpretations: Vec<Interpretation>,
    /// The locale's fallback intent, which Amazon Lex V2 INSERTS among the
    /// alternative intents of the response it answers a recognition with
    /// (`Interpretation$nluConfidence`: "determines the threshold where Amazon
    /// Lex V2 will insert the `AMAZON.FallbackIntent` … when returning
    /// alternative intents in a response") — so it belongs to the response and
    /// not to the session's own interpretations. `None` for a turn that is
    /// already ON the fallback intent, which reports it as its own
    /// interpretation.
    pub inserted_fallback: Option<Interpretation>,
}

/// The interpretations a RECOGNITION answers with: the ones it produced, plus
/// the entry Amazon Lex V2 inserts among the alternative intents of a response.
///
/// An intent the utterance scored against is already reported, with the score it
/// reached, so it is not reported a second time.
pub fn with_inserted(
    mut interpretations: Vec<Interpretation>,
    inserted: Option<Interpretation>,
) -> Vec<Interpretation> {
    let Some(inserted) = inserted else {
        return interpretations;
    };
    let named = |entry: &Interpretation| {
        entry
            .intent
            .as_ref()
            .zip(inserted.intent.as_ref())
            .is_some_and(|(held, fallback)| held.name == fallback.name)
    };
    if !interpretations.iter().any(named) && interpretations.len() < MAX_INTERPRETATIONS {
        interpretations.push(inserted);
    }
    interpretations
}

/// Interpret `text` against `model` for a conversation in `state`.
pub fn interpret(model: &BotLocaleModel, text: &str, state: &SessionState) -> TurnPlan {
    let said = tokens(text);

    // Every intent of the locale an utterance CAN be matched against, scored
    // against this one, best first — within the work one turn may spend on the
    // scan. An intent that declares no sample utterance is left out: there is
    // nothing to match it on, which is why the fallback intent the control plane
    // adds to a locale is not scored but inserted by the threshold rule below.
    //
    // An intent that scored 0.00 STAYS: "ranges between 0.00 and 1.00"
    // (`ConfidenceScore$score`), so nought is a score the bot's intent earned
    // and reporting it is what tells a fallback the threshold caused from a bot
    // that matches nothing at all.
    let mut budget = MAX_MATCH_WORK;
    let mut scored: Vec<(&ModelIntent, Matched)> = model
        .intents
        .iter()
        .filter(|intent| !intent.sample_utterances.is_empty())
        .map(|intent| (intent, best_match(intent, &said, &mut budget)))
        .collect();
    scored.sort_by(|(left, left_match), (right, right_match)| {
        right_match
            .score
            .total_cmp(&left_match.score)
            .then_with(|| left.name.cmp(&right.name))
    });
    // "The specified confidence threshold for inserting the
    // AMAZON.FallbackIntent": a best match below the locale's own threshold is
    // not a match at all, and neither is one that matched nothing of the
    // utterance, however low the threshold is set.
    let recognized = scored.first().filter(|(_, matched)| {
        matched.score > 0.0 && matched.score >= model.nlu_intent_confidence_threshold
    });

    // The intent of this turn: the one the utterance recognised, else the one
    // the conversation is already on (an answer to a slot elicitation names no
    // intent of its own), else the fallback intent.
    let fallback_name = model.fallback_intent_name();
    let (name, captures) = match recognized {
        Some((intent, matched)) => (intent.name.clone(), matched.captures.clone()),
        None => (
            state
                .intent
                .as_ref()
                .map_or_else(|| fallback_name.clone(), |intent| intent.name.clone()),
            Vec::new(),
        ),
    };
    let continued = state.intent.as_ref().filter(|intent| intent.name == name);

    // The slots the conversation has already filled for this intent survive the
    // turn; a turn that moved to another intent starts from none.
    let mut slots: HashMap<String, Slot> = continued
        .and_then(|intent| intent.slots.clone())
        .unwrap_or_default();

    let authored = model.intent(&name);
    if let Some(authored) = authored {
        // Rule 2, from the utterance's own shape: the words a sample utterance
        // put in the slot's position.
        for (slot_name, words) in &captures {
            if let Some(slot) = authored.slot(slot_name) {
                if let Some(value) = resolve_slot_value(slot.slot_type.as_ref(), words) {
                    slots.insert(slot.name.clone(), filled(value));
                }
            }
        }
        // Rule 2, from the dialog: an utterance that named no intent of its own
        // is the answer to the slot the previous turn asked for, so the whole of
        // it is what the user said for that slot.
        if recognized.is_none() {
            if let Some(slot) = eliciting(state).and_then(|slot| authored.slot(&slot)) {
                if let Some(value) = resolve_slot_value(slot.slot_type.as_ref(), text) {
                    slots.insert(slot.name.clone(), filled(value));
                }
            }
        }
    }

    // Rule 3: the dialog step this turn ends on.
    let mut messages = Vec::new();
    let (intent_state, dialog_action) = match authored {
        Some(authored) => match authored
            .slots
            .iter()
            .find(|slot| slot.required && !has_value(&slots, &slot.name))
        {
            Some(unfilled) => {
                messages = unfilled.prompt_messages.clone();
                (
                    Some(INTENT_STATE_IN_PROGRESS.to_string()),
                    DialogAction {
                        r#type: DIALOG_ACTION_ELICIT_SLOT.to_string(),
                        slot_to_elicit: Some(unfilled.name.clone()),
                        slot_elicitation_style: None,
                        sub_slot_to_elicit: None,
                    },
                )
            }
            None => {
                messages = authored.closing_messages.clone();
                // Nothing is left to elicit. The bot has COMPLETED the intent
                // where it has a closing response to convey that it has and no
                // fulfilment code hook to hand the intent to, and is ready for
                // the client to fulfil it in every other case.
                let state = if authored.bot_completes_fulfillment() {
                    INTENT_STATE_FULFILLED
                } else {
                    INTENT_STATE_READY_FOR_FULFILLMENT
                };
                (Some(state.to_string()), close())
            }
        },
        // An intent the locale does not declare — a name a caller put in the
        // session itself — has no slots here to elicit, so this turn elicits
        // nothing, the dialog closes, and the state the session carries for it
        // stands; with none carried, there is nothing left to elicit for it
        // either.
        None => (
            continued
                .and_then(|intent| intent.state.clone())
                .or_else(|| Some(INTENT_STATE_READY_FOR_FULFILLMENT.to_string())),
            close(),
        ),
    };

    // Rule 4: the map names every slot the intent declares, each carrying what
    // the conversation has filled for it or nothing at all. An intent the locale
    // does not declare has no slots to name, so the map is the one the session
    // carries for it.
    let slots = match authored {
        Some(authored) => declared_slots(authored, &slots),
        None => slots,
    };

    // "A map of all of the slots for the intent. The name of the slot maps to
    // the value of the slot. If a slot has not been filled, the value is null"
    // (`Intent$slots`): the map is reported on every turn, naming every slot the
    // intent declares, so a client can index it without first testing whether
    // the member is there and can tell a slot that is declared and open from one
    // the intent does not have. `confirmationState` is reported the same way,
    // and both are filled in by [`active_intent`].
    let intent = active_intent(Intent {
        name,
        slots: Some(slots),
        state: intent_state,
        confirmation_state: continued.and_then(|intent| intent.confirmation_state.clone()),
    });

    // The fallback intent is one of the alternatives Amazon Lex V2 reports:
    // `Interpretation$nluConfidence` "determines the threshold where Amazon Lex
    // V2 will insert the AMAZON.FallbackIntent … when returning alternative
    // intents in a response", so the locale's own fallback intent is INSERTED
    // into the response rather than scored against the utterance — which is why
    // it carries no confidence, and why it is not part of what the RECOGNITION
    // made of the utterance and a later read of the session reports. A turn that
    // is already ON the fallback reports it as its own interpretation, so there
    // is nothing to insert beside it.
    let inserted_fallback = (intent.name != fallback_name).then(|| alternative(fallback_name, None));

    // What the recognition made of the utterance: its own interpretation first,
    // then the other intents it scored, best first. The room the inserted entry
    // needs is left for it, so the response it joins still holds no more than
    // the modelled list does.
    let mut interpretations = vec![Interpretation {
        nlu_confidence: recognized.map(|(_, matched)| ConfidenceScore {
            score: matched.score,
        }),
        sentiment_response: None,
        intent: Some(intent.clone()),
        interpretation_source: Some(INTERPRETATION_SOURCE.to_string()),
    }];
    let room = MAX_INTERPRETATIONS - 1 - usize::from(inserted_fallback.is_some());
    for (candidate, matched) in scored
        .iter()
        .filter(|(candidate, _)| candidate.name != intent.name)
        .take(room)
    {
        interpretations.push(alternative(candidate.name.clone(), Some(matched.score)));
    }

    TurnPlan {
        intent,
        dialog_action,
        messages,
        interpretations,
        inserted_fallback,
    }
}

/// The slot map an intent reports: one entry per slot the intent DECLARES,
/// carrying the value the conversation has filled for it, and an unfilled slot
/// ([`unfilled`]) for every slot it has none for.
///
/// "A map of all of the slots for the intent. The name of the slot maps to the
/// value of the slot. If a slot has not been filled, the value is null"
/// (`Intent$slots`) — so the map is keyed by the intent's own slots whatever the
/// dialog reached, and an intent that declares none reports an empty map. A name
/// the intent does not declare is not a slot of it, so a stray entry a caller
/// put in the session is not reported back.
pub(crate) fn declared_slots(
    authored: &ModelIntent,
    filled: &HashMap<String, Slot>,
) -> HashMap<String, Slot> {
    authored
        .slots
        .iter()
        .map(|slot| {
            // Keyed by the slot's own name, which is what this dialog writes
            // and what a caller seeding the session names the slot by.
            let value = filled.get(&slot.name).cloned().unwrap_or_else(unfilled);
            (slot.name.clone(), value)
        })
        .collect()
}

/// A slot the intent declares and the conversation has no value for: the entry
/// whose wire form is the documented `null` (`Intent$slots`), which is what a
/// slot holding nothing at all serializes as.
fn unfilled() -> Slot {
    Slot::default()
}

/// `Close`, the dialog action of a turn that elicits nothing further.
fn close() -> DialogAction {
    DialogAction {
        r#type: DIALOG_ACTION_CLOSE.to_string(),
        slot_to_elicit: None,
        slot_elicitation_style: None,
        sub_slot_to_elicit: None,
    }
}

/// A slot holding one value: "when the value is `Scalar`, it indicates that the
/// `value` field contains a single value" (`Slot$shape`), which is what a slot
/// filled from one answer holds.
fn filled(value: Value) -> Slot {
    Slot {
        value: Some(value),
        shape: Some(SHAPE_SCALAR.to_string()),
        values: None,
        sub_slots: None,
    }
}

/// `Scalar`, from the modelled `Shape` enum (`SHAPE_VALUES`).
const SHAPE_SCALAR: &str = "Scalar";

/// Does the session hold a value for that slot? A slot the map names without a
/// value is not filled: "if a slot has not been filled, the value is null"
/// (`Intent$slots`).
fn has_value(slots: &HashMap<String, Slot>, name: &str) -> bool {
    slots
        .get(name)
        .is_some_and(|slot| slot.value.is_some() || slot.values.is_some())
}

/// The slot the conversation is waiting for an answer to: the one the dialog
/// state elicits, which is the member that carries "the name of the slot that
/// should be elicited from the user".
fn eliciting(state: &SessionState) -> Option<String> {
    let action = state.dialog_action.as_ref()?;
    if action.r#type != DIALOG_ACTION_ELICIT_SLOT {
        return None;
    }
    action.slot_to_elicit.clone()
}

/// One word of an utterance: what the user wrote, and the form it is compared
/// in.
#[derive(Debug, Clone)]
struct Token {
    /// The word as the user wrote it, with the punctuation around it dropped —
    /// this is what a slot value reports as its `originalValue`.
    said: String,
    /// The same word folded for comparison, since neither case nor the
    /// punctuation around a word changes what was said.
    folded: String,
}

/// The words of an utterance, at most [`MAX_UTTERANCE_WORDS`] of them.
fn tokens(text: &str) -> Vec<Token> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .take(MAX_UTTERANCE_WORDS)
        .map(|word| Token {
            said: word.to_string(),
            folded: word.to_lowercase(),
        })
        .collect()
}

/// One token of a sample utterance: a word the user has to say, or a reference
/// to a slot of the intent.
#[derive(Debug, Clone)]
enum PatternToken {
    Word(String),
    Slot(String),
}

/// The pattern a sample utterance states: its words, with every slot reference
/// `{SlotName}` standing for the words the user says for that slot.
fn pattern(sample: &str) -> Vec<PatternToken> {
    sample
        .split_whitespace()
        .filter_map(|raw| match slot_reference(raw) {
            Some(name) => Some(PatternToken::Slot(name)),
            None => {
                let word = raw.trim_matches(|c: char| !c.is_alphanumeric());
                (!word.is_empty()).then(|| PatternToken::Word(word.to_lowercase()))
            }
        })
        .collect()
}

/// The slot a sample-utterance token references, as a bot author writes it:
/// "I want a {PizzaSize} pizza" names the intent's `PizzaSize` slot in the
/// position its value is said in.
fn slot_reference(raw: &str) -> Option<String> {
    let open = raw.find('{')?;
    let close = raw.rfind('}')?;
    let name = raw.get(open + 1..close)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// How well an utterance matched, and what it said for the intent's slots.
#[derive(Debug, Clone, Default)]
struct Matched {
    /// The confidence, between 0.00 and 1.00 as the modelled `ConfidenceScore`
    /// states.
    score: f64,
    /// What the utterance said for each slot a sample utterance referenced, as
    /// (slot name, the words said).
    captures: Vec<(String, String)>,
}

/// The best this intent does on the utterance: the highest-scoring of its sample
/// utterances, with what that one captured.
///
/// `budget` is the work the whole turn has left ([`MAX_MATCH_WORK`]): each
/// sample is charged what it can cost before it is matched, and the scan stops
/// once there is nothing left to spend.
fn best_match(intent: &ModelIntent, said: &[Token], budget: &mut usize) -> Matched {
    let mut best = Matched::default();
    for sample in &intent.sample_utterances {
        let pattern = pattern(sample);
        // What matching this sample can cost at worst: every word of the pattern
        // compared against every word that was said, plus the step of looking at
        // the sample at all.
        let cost = pattern.len().saturating_mul(said.len()).saturating_add(1);
        if cost > *budget {
            break;
        }
        *budget -= cost;
        let matched = match_pattern(&pattern, said);
        if matched.score > best.score {
            best = matched;
        }
    }
    best
}

/// Match one sample utterance against what the user said.
///
/// An utterance the pattern accounts for in full — every word of the pattern
/// said in order, and every word of the utterance either one of those or part of
/// a slot value — is that sample utterance said, and scores 1.00. Anything else
/// scores by how much of the two the words they share account for, so a longer
/// utterance that only brushes past the sample scores low and the locale's
/// confidence threshold decides whether it is a match at all.
fn match_pattern(pattern: &[PatternToken], said: &[Token]) -> Matched {
    if let Some(captures) = spans(pattern, said) {
        return Matched {
            score: 1.0,
            captures,
        };
    }
    Matched {
        score: overlap(pattern, said),
        captures: Vec::new(),
    }
}

/// The words the utterance said for each slot the pattern references, when the
/// pattern accounts for the whole utterance; `None` when it does not.
///
/// A slot's words run from where the slot is referenced to the next word the
/// pattern states — the first time that word is said after at least one word of
/// the slot's own — or to the end of the utterance when the reference is last.
/// Two slots referenced side by side take one word each, there being no word
/// between them to divide them.
fn spans(pattern: &[PatternToken], said: &[Token]) -> Option<Vec<(String, String)>> {
    let mut captures = Vec::new();
    let mut at = 0usize;
    for (index, token) in pattern.iter().enumerate() {
        match token {
            PatternToken::Word(word) => {
                if said.get(at).map(|token| &token.folded) != Some(word) {
                    return None;
                }
                at += 1;
            }
            PatternToken::Slot(name) => {
                let end = match pattern.get(index + 1) {
                    None => said.len(),
                    Some(PatternToken::Slot(_)) => at + 1,
                    Some(PatternToken::Word(next)) => said
                        .iter()
                        .enumerate()
                        .skip(at + 1)
                        .find(|(_, token)| &token.folded == next)
                        .map(|(position, _)| position)?,
                };
                if end > said.len() || end <= at {
                    return None;
                }
                captures.push((
                    name.clone(),
                    said[at..end]
                        .iter()
                        .map(|token| token.said.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                ));
                at = end;
            }
        }
    }
    (at == said.len()).then_some(captures)
}

/// How much of the sample utterance and of what was said the words they share
/// account for: the words the pattern states that were said, in order, over the
/// longer of the two word counts. It is 0.00 when they share nothing and below
/// 1.00 whenever either carries a word the other does not.
fn overlap(pattern: &[PatternToken], said: &[Token]) -> f64 {
    let words: Vec<&String> = pattern
        .iter()
        .filter_map(|token| match token {
            PatternToken::Word(word) => Some(word),
            PatternToken::Slot(_) => None,
        })
        .collect();
    if words.is_empty() || said.is_empty() {
        return 0.0;
    }
    let mut shared = 0usize;
    let mut at = 0usize;
    for word in &words {
        if at >= said.len() {
            break;
        }
        if let Some(position) = said[at..].iter().position(|token| token.folded == **word) {
            shared += 1;
            at += position + 1;
        }
    }
    let longer = words.len().max(said.len());
    // Two decimals, the precision the modelled score is stated to ("ranges
    // between 0.00 and 1.00").
    (shared as f64 / longer as f64 * 100.0).round() / 100.0
}
