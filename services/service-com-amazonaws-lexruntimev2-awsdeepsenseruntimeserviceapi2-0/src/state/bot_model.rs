// SPDX-License-Identifier: CC0-1.0

//! The built bot locale a conversation turn is interpreted against.
//!
//! `RecognizeText` and `RecognizeUtterance` "interpret the user input using the
//! machine learning model that it build for the bot" — and that bot is NOT this
//! runtime's resource: it is authored through the Lex Models V2 control plane
//! (`CreateBot`, `CreateBotLocale`, `CreateSlotType`, `CreateIntent`,
//! `CreateSlot`) and made servable by `BuildBotLocale`. This module is the one
//! place that reads it: it resolves the bot alias a request addresses to the bot
//! version the alias points at, checks that version's locale is built, and
//! copies out exactly what a turn needs — the locale's confidence threshold, its
//! intents, their sample utterances, and the slots to elicit with the vocabulary
//! each resolves against.
//!
//! Nothing here is authored in this crate: a value this module reports was
//! stored by the control plane, so a turn can only recognise what the client
//! actually built.

use crate::ops::types::{Button, ImageResponseCard, Message, Value};
use crate::state::FALLBACK_INTENT;
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2 as lexmodels;

/// The shapes the control plane authored a bot with, under one name so a reader
/// can tell an authored shape from this service's own runtime shape of the same
/// name (both models declare a `Message`).
use lexmodels::ops::types as authored;

/// What Amazon Lex V2 answers a runtime call addressed to a bot and locale whose
/// model it cannot serve: a locale that was never built has no runtime model, so
/// the combination the request names is not one the runtime can hold a
/// conversation against.
///
/// The sentence has no variable part — it names neither the bot id nor the
/// locale id the request carried — so every such refusal of this service carries
/// exactly this text.
pub const BOT_LOCALE_NOT_VALID_MESSAGE: &str =
    "The provided BotId/BotLocale combination is not valid. Please update the BotId/BotLocale \
     combination and retry your request.";

/// What Amazon Lex V2 answers a runtime call whose `botId`/`botAliasId` pair
/// resolves to no alias of that bot: a different misconfiguration from a locale
/// that was never built ([`BOT_LOCALE_NOT_VALID_MESSAGE`]), and the only thing
/// that tells the two apart is the sentence.
///
/// The two ids the request addressed the alias by are the variable parts, taken
/// from the request's own path labels and reported back verbatim; the text after
/// them is fixed.
pub fn bot_alias_not_valid_message(bot_id: &str, bot_alias_id: &str) -> String {
    format!("{bot_id}/{bot_alias_id} is not a valid BotId/BotAliasId combination.")
}

/// The `slotConstraint` of a slot Amazon Lex V2 must have a value for before the
/// intent can be fulfilled. The modelled `SlotConstraint` admits only this and
/// `Optional` (`lexmodels#SLOT_CONSTRAINT_VALUES`).
pub const SLOT_CONSTRAINT_REQUIRED: &str = "Required";

/// The `resolutionStrategy` that resolves what the user said to a value the slot
/// type declares: "`TOP_RESOLUTION` - If there is a resolution list for the
/// slot, return the first value in the resolution list as the slot type value.
/// If there is no resolution list, null is returned"
/// (`SlotValueSelectionSetting$resolutionStrategy`).
pub const RESOLUTION_STRATEGY_TOP_RESOLUTION: &str = "TopResolution";

/// `PlainText`, and the three other members of the runtime's
/// `MessageContentType` enum (`MESSAGE_CONTENT_TYPE_VALUES`): which one a
/// message carries is read off the authored message's own shape.
pub(crate) const MESSAGE_CONTENT_TYPE_PLAIN_TEXT: &str = "PlainText";
/// See [`MESSAGE_CONTENT_TYPE_PLAIN_TEXT`].
const CONTENT_TYPE_CUSTOM_PAYLOAD: &str = "CustomPayload";
/// See [`MESSAGE_CONTENT_TYPE_PLAIN_TEXT`].
const CONTENT_TYPE_SSML: &str = "SSML";
/// See [`MESSAGE_CONTENT_TYPE_PLAIN_TEXT`].
const CONTENT_TYPE_IMAGE_RESPONSE_CARD: &str = "ImageResponseCard";

/// One built locale, as a turn reads it.
#[derive(Debug, Clone)]
pub struct BotLocaleModel {
    /// The locale's own `nluIntentConfidenceThreshold`: "the specified
    /// confidence threshold for inserting the `AMAZON.FallbackIntent`", so an
    /// utterance whose best-matching intent scores below it is the fallback.
    pub nlu_intent_confidence_threshold: f64,
    /// The intents the locale declares, in the order it acquired them.
    pub intents: Vec<ModelIntent>,
}

impl BotLocaleModel {
    /// The intent of this locale that name declares, or `None` for a name the
    /// locale does not hold.
    pub fn intent(&self, name: &str) -> Option<&ModelIntent> {
        self.intents.iter().find(|intent| intent.name == name)
    }

    /// The FALLBACK intent of this locale: the one built on the built-in intent
    /// Amazon Lex uses "when it can't determine another intent to invoke",
    /// which the control plane adds to every locale as it creates it.
    pub fn fallback_intent(&self) -> Option<&ModelIntent> {
        self.intents.iter().find(|intent| intent.is_fallback())
    }

    /// The NAME the locale's fallback intent is reported by: the authored one
    /// where the locale declares it, else [`crate::state::FALLBACK_INTENT`], the
    /// name Amazon Lex gives the intent it adds to a locale.
    pub fn fallback_intent_name(&self) -> String {
        self.fallback_intent()
            .map_or_else(|| FALLBACK_INTENT.to_string(), |intent| intent.name.clone())
    }
}

/// One intent of a built locale.
#[derive(Debug, Clone)]
pub struct ModelIntent {
    pub name: String,
    /// The built-in intent this one is based on, as `parentIntentSignature`
    /// names it, or `None` for an intent authored from scratch. It is what tells
    /// the locale's fallback intent from its own intents, whatever either is
    /// named.
    pub parent_intent_signature: Option<String>,
    /// The utterances authored on the intent. An intent built on a built-in
    /// intent Amazon Lex supplies also answers to Amazon's own utterances for
    /// it, which are not part of the bot and so are not held here.
    pub sample_utterances: Vec<String>,
    /// The slots of the intent, in the order `slotPriorities` elicits them.
    pub slots: Vec<ModelSlot>,
    /// The messages the intent's `intentConfirmationSetting` prompt asks the
    /// user to confirm it with; empty when none is authored.
    pub confirmation_messages: Vec<Message>,
    /// The messages the intent's `intentClosingSetting` closing response sends
    /// once the intent is ready to be fulfilled; empty when none is authored or
    /// when the setting is not `active`.
    pub closing_messages: Vec<Message>,
    /// Is the intent's fulfilment somebody else's work — an ACTIVE, enabled
    /// `fulfillmentCodeHook`, the Lambda function Amazon Lex V2 "invokes … when
    /// the intent is ready for fulfillment"? An intent that names no such hook
    /// is fulfilled by the bot itself.
    pub fulfillment_by_code_hook: bool,
}

impl ModelIntent {
    /// Is this the locale's FALLBACK intent — the one built on the built-in
    /// intent Amazon Lex invokes when it can't determine another, which is the
    /// one Amazon Lex V2 inserts among the alternative intents of a turn?
    pub fn is_fallback(&self) -> bool {
        self.parent_intent_signature.as_deref() == Some(lexmodels::state::FALLBACK_INTENT_SIGNATURE)
    }

    /// Does the BOT complete this intent itself, rather than leaving its
    /// fulfilment to its caller?
    ///
    /// It does when it has a closing response of its own to play and no
    /// fulfilment code hook to hand the intent to: nothing else runs, and the
    /// bot has said its last word. An intent whose fulfilment is a code hook's,
    /// or that has nothing authored to close on, is fulfilled by whoever called
    /// the runtime.
    pub fn bot_completes_fulfillment(&self) -> bool {
        !self.closing_messages.is_empty() && !self.fulfillment_by_code_hook
    }

    /// The slot of this intent that name declares. A sample utterance names a
    /// slot by its name, which is unique within the intent and compared without
    /// regard to case, as a bot author writes it in prose.
    pub fn slot(&self, name: &str) -> Option<&ModelSlot> {
        self.slots
            .iter()
            .find(|slot| slot.name.eq_ignore_ascii_case(name))
    }
}

/// One slot of one intent: what to elicit, how to ask for it, and what the
/// answer resolves against.
#[derive(Debug, Clone)]
pub struct ModelSlot {
    pub name: String,
    /// Is this slot [`SLOT_CONSTRAINT_REQUIRED`]? Amazon Lex V2 cannot fulfil
    /// the intent until a required slot has a value.
    pub required: bool,
    /// The messages the slot's `valueElicitationSetting.promptSpecification`
    /// asks for the value with; empty when none is authored.
    pub prompt_messages: Vec<Message>,
    /// The custom slot type the slot elicits values for, or `None` for a slot
    /// whose type is one Amazon Lex supplies (`AMAZON.*`) or that names none.
    pub slot_type: Option<ModelSlotType>,
}

/// A custom slot type of the locale: the values it declares and the strategy
/// that picks one for what the user said.
#[derive(Debug, Clone)]
pub struct ModelSlotType {
    /// The authored `valueSelectionSetting.resolutionStrategy`, or the default
    /// the control plane applies when a create names no selection setting.
    pub resolution_strategy: String,
    pub values: Vec<ModelSlotTypeValue>,
}

/// One declared value of a slot type, with the synonyms that resolve to it.
#[derive(Debug, Clone)]
pub struct ModelSlotTypeValue {
    /// The `sampleValue`, which is the canonical form of this value.
    pub value: String,
    /// The `synonyms` authored for it.
    pub synonyms: Vec<String>,
}

impl ModelSlotType {
    /// The declared value `said` names: its own `sampleValue` or one of that
    /// value's `synonyms`, compared without regard to case, since what the user
    /// typed is prose. `None` when the slot type declares neither.
    pub fn canonical(&self, said: &str) -> Option<&str> {
        let said = said.trim();
        self.values
            .iter()
            .find(|value| {
                value.value.eq_ignore_ascii_case(said)
                    || value
                        .synonyms
                        .iter()
                        .any(|synonym| synonym.eq_ignore_ascii_case(said))
            })
            .map(|value| value.value.as_str())
    }
}

/// The value a slot holds for what the user said, or `None` when nothing
/// resolves and the slot therefore stays unfilled.
///
/// The slot type's own `resolutionStrategy` decides:
///
/// * [`RESOLUTION_STRATEGY_TOP_RESOLUTION`] returns "the first value in the
///   resolution list as the slot type value. If there is no resolution list,
///   null is returned" — so what the user said must be a declared value or a
///   synonym of one, the canonical value is what the slot is interpreted as, and
///   a word the slot type declares nowhere resolves to nothing;
/// * every other strategy "returns the value entered by the user", so the words
///   stand as the interpreted value and the canonical value, where the slot type
///   declares one, is reported beside it as the resolution.
///
/// A slot whose type is one Amazon Lex supplies, or that names no type, has no
/// declared vocabulary in the bot this emulator holds — the values of a built-in
/// slot type are Amazon's own and are not part of the bot — so the answer stands
/// as the user gave it, which is what the `OriginalValue` strategy does.
pub fn resolve_slot_value(slot_type: Option<&ModelSlotType>, said: &str) -> Option<Value> {
    let said = said.trim();
    if said.is_empty() {
        return None;
    }
    let canonical = slot_type.and_then(|slot_type| slot_type.canonical(said));
    let top_resolution = slot_type.is_some_and(|slot_type| {
        slot_type.resolution_strategy == RESOLUTION_STRATEGY_TOP_RESOLUTION
    });
    if top_resolution {
        let canonical = canonical?;
        return Some(Value {
            original_value: Some(said.to_string()),
            interpreted_value: canonical.to_string(),
            resolved_values: Some(vec![canonical.to_string()]),
        });
    }
    Some(Value {
        original_value: Some(said.to_string()),
        interpreted_value: said.to_string(),
        resolved_values: canonical.map(|canonical| vec![canonical.to_string()]),
    })
}

/// Why a call cannot be served against the bot and locale it addresses.
#[derive(Debug, Clone)]
pub enum LocaleUnavailable {
    /// The `botId`/`botAliasId` pair the request addressed resolves to no alias
    /// of that bot, so there is no bot version — and therefore no locale — to
    /// look for at all.
    UnknownBotAlias,
    /// The alias resolved, but the locale the request names has no model the
    /// runtime can serve: that version holds no such locale, or the locale has
    /// not been built.
    NotBuilt,
    /// The control plane's own store could not be reached, which is this
    /// emulator's fault and not the request's.
    StoreUnavailable,
}

/// The built locale the four labels of a runtime request address, read off the
/// control plane's store.
///
/// The alias is what the runtime is addressed by, and an alias points at one
/// version of the bot, so the version whose locale is served is the alias's own
/// — which is why the `botId`/`botAliasId` PAIR is what resolves, and an alias
/// that is not that bot's addresses nothing. A locale is then servable only in
/// `Built`, the status AWS documents as the one where "the bot is ready for use
/// and can be tested using any utterance": every other status — including the
/// `NotBuilt` a freshly created locale sits in — has no runtime model behind it.
pub fn built_locale<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bot_id: &str,
    bot_alias_id: &str,
    locale_id: &str,
) -> Result<BotLocaleModel, LocaleUnavailable> {
    let control_plane =
        lexmodels::state::service_state(engine).ok_or(LocaleUnavailable::StoreUnavailable)?;
    let mut control_plane = control_plane.lock();

    let bot_version = control_plane
        .bot_aliases
        .get(&lexmodels::state::bot_alias_key(bot_id, bot_alias_id))
        .map(|alias| alias.bot_version.clone())
        .ok_or(LocaleUnavailable::UnknownBotAlias)?;

    // `settled_bot_locale` applies the status transition the clock has already
    // made due, so a build that fell due is observed here exactly as
    // `DescribeBotLocale` observes it.
    let locale = control_plane
        .settled_bot_locale(bot_id, &bot_version, locale_id)
        .ok_or(LocaleUnavailable::NotBuilt)?;
    if locale.bot_locale_status != lexmodels::state::BOT_LOCALE_STATUS_BUILT {
        return Err(LocaleUnavailable::NotBuilt);
    }
    let nlu_intent_confidence_threshold = locale.nlu_intent_confidence_threshold;

    // The locale's intents, in the order it acquired them, so what a turn
    // reports does not depend on how the store happens to hold its records.
    let mut authored_intents: Vec<&lexmodels::state::Intent> = control_plane
        .intents_in_locale(bot_id, &bot_version, locale_id)
        .collect();
    authored_intents.sort_by(|left, right| {
        left.creation_date_time
            .cmp(&right.creation_date_time)
            .then_with(|| left.intent_name.cmp(&right.intent_name))
    });

    let intents = authored_intents
        .iter()
        .map(|intent| ModelIntent {
            name: intent.intent_name.clone(),
            parent_intent_signature: intent.parent_intent_signature.clone(),
            sample_utterances: intent
                .sample_utterances
                .iter()
                .map(|sample| sample.utterance.clone())
                .collect(),
            slots: elicitation_order(&control_plane, intent, bot_id, &bot_version, locale_id),
            confirmation_messages: intent
                .intent_confirmation_setting
                .as_ref()
                .and_then(|setting| setting.prompt_specification.as_ref())
                .map(|prompt| response_messages(&prompt.message_groups))
                .unwrap_or_default(),
            closing_messages: intent
                .intent_closing_setting
                .as_ref()
                // "active — Specifies whether an intent's closing response is
                // used": a setting that is not active plays nothing, and the
                // member is optional, so an authored setting that omits it is
                // used.
                .filter(|setting| setting.active.unwrap_or(true))
                .and_then(|setting| setting.closing_response.as_ref())
                .map(|response| response_messages(&response.message_groups))
                .unwrap_or_default(),
            // "enabled — Indicates whether a Lambda function should be invoked
            // to fulfill a specific intent", beside the same `active` flag the
            // closing setting carries.
            fulfillment_by_code_hook: intent
                .fulfillment_code_hook
                .as_ref()
                .is_some_and(|hook| hook.enabled && hook.active.unwrap_or(true)),
        })
        .collect();

    Ok(BotLocaleModel {
        nlu_intent_confidence_threshold,
        intents,
    })
}

/// The slots of `intent`, in the order Amazon Lex V2 elicits them: the order its
/// `slotPriorities` fixes, which is the list a build requires every slot of the
/// intent to appear in. A slot nothing orders is elicited after the ordered
/// ones, by name, so the order is total whatever the store holds.
fn elicitation_order(
    control_plane: &lexmodels::state::ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State,
    intent: &lexmodels::state::Intent,
    bot_id: &str,
    bot_version: &str,
    locale_id: &str,
) -> Vec<ModelSlot> {
    let mut slots: Vec<&lexmodels::state::Slot> = control_plane
        .slots_in_intent(bot_id, bot_version, locale_id, &intent.intent_id)
        .collect();
    let priority = |slot: &lexmodels::state::Slot| {
        intent
            .slot_priorities
            .iter()
            .find(|entry| entry.slot_id == slot.slot_id)
            .map(|entry| entry.priority)
    };
    slots.sort_by(|left, right| {
        // An unordered slot sorts after every ordered one, whatever priorities
        // the intent carries.
        priority(left)
            .is_none()
            .cmp(&priority(right).is_none())
            .then_with(|| priority(left).cmp(&priority(right)))
            .then_with(|| left.slot_name.cmp(&right.slot_name))
    });

    slots
        .into_iter()
        .map(|slot| ModelSlot {
            name: slot.slot_name.clone(),
            required: slot.value_elicitation_setting.slot_constraint == SLOT_CONSTRAINT_REQUIRED,
            prompt_messages: slot
                .value_elicitation_setting
                .prompt_specification
                .as_ref()
                .map(|prompt| response_messages(&prompt.message_groups))
                .unwrap_or_default(),
            slot_type: slot
                .slot_type_id
                .as_deref()
                .filter(|slot_type_id| !lexmodels::state::is_built_in_slot_type(slot_type_id))
                .and_then(|slot_type_id| {
                    control_plane.slot_type(bot_id, bot_version, locale_id, slot_type_id)
                })
                .map(slot_type_model),
        })
        .collect()
}

/// One authored slot type, as a turn resolves values against it.
fn slot_type_model(slot_type: &lexmodels::state::SlotType) -> ModelSlotType {
    ModelSlotType {
        resolution_strategy: slot_type
            .value_selection_setting
            .as_ref()
            .map_or_else(
                || lexmodels::state::DEFAULT_RESOLUTION_STRATEGY.to_string(),
                |setting| setting.resolution_strategy.clone(),
            ),
        values: slot_type
            .slot_type_values
            .iter()
            .filter_map(|value| {
                let canonical = value.sample_value.as_ref()?.value.clone();
                Some(ModelSlotTypeValue {
                    value: canonical,
                    synonyms: value
                        .synonyms
                        .iter()
                        .flatten()
                        .map(|synonym| synonym.value.clone())
                        .collect(),
                })
            })
            .collect(),
    }
}

/// The messages one authored response or prompt sends, in the order its message
/// groups are defined — which is the order the runtime reports them in ("the
/// messages are ordered based on the order that the messages are defined in the
/// bot").
///
/// A group's `variations` are the alternatives Amazon Lex V2 may choose between
/// instead of the group's own message; the group's own message is the one
/// reported here, so the same authored prompt always answers the same way.
fn response_messages(groups: &[authored::MessageGroup]) -> Vec<Message> {
    groups
        .iter()
        .filter_map(|group| runtime_message(&group.message))
        .collect()
}

/// One authored message, as the runtime's own `Message` shape carries it: the
/// authored shape holds one of four alternatives, and which one it holds is what
/// the runtime reports as the message's `contentType`.
fn runtime_message(message: &authored::Message) -> Option<Message> {
    if let Some(plain_text) = &message.plain_text_message {
        return Some(Message {
            content: Some(plain_text.value.clone()),
            content_type: MESSAGE_CONTENT_TYPE_PLAIN_TEXT.to_string(),
            image_response_card: None,
        });
    }
    if let Some(custom) = &message.custom_payload {
        return Some(Message {
            content: Some(custom.value.clone()),
            content_type: CONTENT_TYPE_CUSTOM_PAYLOAD.to_string(),
            image_response_card: None,
        });
    }
    if let Some(ssml) = &message.ssml_message {
        return Some(Message {
            content: Some(ssml.value.clone()),
            content_type: CONTENT_TYPE_SSML.to_string(),
            image_response_card: None,
        });
    }
    let card = message.image_response_card.as_ref()?;
    Some(Message {
        content: None,
        content_type: CONTENT_TYPE_IMAGE_RESPONSE_CARD.to_string(),
        image_response_card: Some(ImageResponseCard {
            title: card.title.clone(),
            subtitle: card.subtitle.clone(),
            image_url: card.image_url.clone(),
            buttons: card.buttons.as_ref().map(|buttons| {
                buttons
                    .iter()
                    .map(|button| Button {
                        text: button.text.clone(),
                        value: button.value.clone(),
                    })
                    .collect()
            }),
        }),
    })
}

#[cfg(test)]
mod tests;
