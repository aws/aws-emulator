// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2 service.
//!
//! The bot lifecycle slice (`CreateBot`, `DescribeBot`, `ListBots`, `UpdateBot`,
//! `DeleteBot`) is served entirely out of the `bots` map below: every response those
//! handlers build is read back from a record stored here, and the ids they hand out are
//! minted here and kept.
//!
//! The locale slice (`CreateBotLocale`, `DescribeBotLocale`, `DeleteBotLocale`) is served
//! the same way out of `bot_locales`, keyed by the bot, bot version and locale the request
//! names. A locale is the container every intent, slot type and slot of it is defined in,
//! so deleting one takes those records with it ([`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::remove_bot_locale`]).
//!
//! The slot type slice (`CreateSlotType`, `DescribeSlotType`, `ListSlotTypes`) is served
//! out of `slot_types`: a custom slot type is defined inside one locale of one bot
//! version, so every record carries that triple and every read is scoped by it.
//!
//! The intent slice (`CreateIntent`, `DescribeIntent`, `UpdateIntent`) is served out of
//! `intents` the same way, and is what `DescribeBotLocale` counts: a locale is created
//! with the fallback intent Amazon Lex adds to it, so a locale nothing else was added to
//! holds one intent. What those intents carry is also what decides whether
//! `BuildBotLocale` can build the locale at all.
//!
//! The slot slice (`CreateSlot`, `DescribeSlot`, `ListSlots`) is served out of `slots`:
//! a slot is defined inside one intent of one locale of one bot version, so every record
//! carries those four identifiers and every read is scoped by them.
//!
//! A prompt specification is normalized as it is stored — see [`prompt_defaults`] — so
//! every operation that reports a resource holding one reports the server-side defaults
//! Amazon Lex materialized onto it.
//!
//! The version slice (`CreateBotVersion`, `DescribeBotVersion`) is served out of
//! `bot_versions`, keyed by the bot and the number the version was created under. A
//! version is an immutable SNAPSHOT of the version each of its locales was taken from,
//! so creating one copies those locales and everything defined in them — intents, slot
//! types and slots — into the new version under the ids they already have. That is why a
//! resource of a bot version is keyed by its bot version as well as by its own id: one id
//! names the same resource in every version that snapshotted it.
//!
//! The alias slice (`ListBotAliases`) is served out of `bot_aliases`, keyed by the bot
//! and the alias id: a bot is created with the alias Amazon Lex keeps for testing it, so
//! a bot the store holds always has at least that one alias to report.
//!
//! The export slice (`CreateExport`, `DescribeExport`, `ListExports`) is served out of
//! `exports`, keyed by the `exportId` the create minted: an export job is recorded only
//! once the resource it names resolves against the bots, versions and locales above, so
//! a job the store holds is a job over a resource that was there to export. A job is held
//! for the window AWS documents exports are kept for, which is what bounds that map.

mod exports;
mod prompt_defaults;

pub use exports::*;
pub use prompt_defaults::{
    normalize_intent_confirmation_setting, normalize_slot_value_elicitation_setting,
    normalize_sub_slot_setting,
};

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AudioFillerSettings, BotAliasSummary, BotMember, BotSummary, BotVersionLocaleDetails,
    CompositeSlotTypeSetting, DataPrivacy,
    DialogCodeHookSettings, ErrorLogSettings, ExternalSourceSetting, FulfillmentCodeHookSettings,
    GenerativeAISettings, InitialResponseSetting, InputContext, IntentClosingSetting,
    IntentConfirmationSetting, KendraConfiguration, MultipleValuesSetting, ObfuscationSetting,
    OutputContext, QInConnectIntentConfiguration, QnAIntentConfiguration, SampleUtterance,
    SlotPriority, SlotSummary, SlotTypeSummary, SlotTypeValue, SlotValueElicitationSetting,
    SlotValueSelectionSetting, SpeakerDiarizationSettings, SpeechRecognitionSettings,
    SubSlotSetting, UnifiedSpeechSettings, VoiceSettings,
};

/// `BotStatus` members this crate names. The full modelled set is
/// `smithy_query(query="shape",shape="BotStatus")`:
/// Creating | Available | Inactive | Deleting | Failed | Versioning | Importing | Updating.
pub const BOT_STATUS_CREATING: &str = "Creating";
/// See [`BOT_STATUS_CREATING`].
pub const BOT_STATUS_AVAILABLE: &str = "Available";
/// See [`BOT_STATUS_CREATING`].
pub const BOT_STATUS_DELETING: &str = "Deleting";
/// See [`BOT_STATUS_CREATING`]. The status a bot version carries while Amazon Lex is
/// creating it: `CreateBotVersion` reports it, and the version is `Available` once it is
/// "ready for use".
pub const BOT_STATUS_VERSIONING: &str = "Versioning";

/// `BotLocaleStatus` members this crate names. The full modelled set is
/// `smithy_query(query="shape",shape="BotLocaleStatus")`:
/// Creating | Building | Built | ReadyExpressTesting | Failed | Deleting | NotBuilt |
/// Importing | Processing.
pub const BOT_LOCALE_STATUS_CREATING: &str = "Creating";
/// See [`BOT_LOCALE_STATUS_CREATING`]. AWS documents this as the status of a locale
/// that "was saved but not built", which is where a freshly created locale lands.
pub const BOT_LOCALE_STATUS_NOT_BUILT: &str = "NotBuilt";
/// See [`BOT_LOCALE_STATUS_CREATING`]. AWS documents this as the status while "Amazon
/// Lex is building the bot for testing and use", which is where a submitted build starts.
pub const BOT_LOCALE_STATUS_BUILDING: &str = "Building";
/// See [`BOT_LOCALE_STATUS_CREATING`]. AWS documents this as the status once "the bot is
/// ready for use and can be tested using any utterance", which is where a build that
/// succeeds settles.
pub const BOT_LOCALE_STATUS_BUILT: &str = "Built";
/// See [`BOT_LOCALE_STATUS_CREATING`]. The modelled terminal status of a build that did
/// not succeed: `DescribeBotLocale` documents it as the status whose "reasons for the
/// failure are listed in the failureReasons field", which is where a build the locale's
/// own content cannot satisfy settles.
pub const BOT_LOCALE_STATUS_FAILED: &str = "Failed";
/// See [`BOT_LOCALE_STATUS_CREATING`]. `DeleteBotLocale` documents this as the status it
/// reports: "The locale first enters the Deleting status. Once the locale is deleted it no
/// longer appears in the list of locales for the bot."
pub const BOT_LOCALE_STATUS_DELETING: &str = "Deleting";

/// Every language and locale Amazon Lex V2 accepts, each with the locale name the API
/// reports for it.
///
/// The modelled `LocaleId` is a plain string
/// (`smithy_query(query="shape",shape="LocaleId")`) whose doc says only "The string
/// must match one of the supported locales", so the accepted set and both columns here
/// come from the published Amazon Lex V2 supported languages and locales table — the
/// same source that table's `localeName` column has.
pub const SUPPORTED_LOCALES: [(&str, &str); 27] = [
    ("ar_AE", "Arabic (Gulf)"),
    ("ca_ES", "Catalan (Spain)"),
    ("de_AT", "German (Austria)"),
    ("de_DE", "German (Germany)"),
    ("en_AU", "English (Australia)"),
    ("en_GB", "English (UK)"),
    ("en_IN", "English (India)"),
    ("en_NZ", "English (New Zealand)"),
    ("en_US", "English (US)"),
    ("en_ZA", "English (South Africa)"),
    ("es_419", "Spanish (Latin America)"),
    ("es_ES", "Spanish (Spain)"),
    ("es_US", "Spanish (US)"),
    ("fi_FI", "Finnish (Finland)"),
    ("fr_CA", "French (Canada)"),
    ("fr_FR", "French (France)"),
    ("hi_IN", "Hindi (India)"),
    ("it_IT", "Italian (Italy)"),
    ("ja_JP", "Japanese (Japan)"),
    ("ko_KR", "Korean (Korea)"),
    ("nb_NO", "Norwegian (Norway)"),
    ("nl_NL", "Dutch (Netherlands)"),
    ("pl_PL", "Polish (Poland)"),
    ("pt_BR", "Portuguese (Brazil)"),
    ("pt_PT", "Portuguese (Portugal)"),
    ("sv_SE", "Swedish (Sweden)"),
    ("zh_CN", "Chinese (Simplified)"),
];

/// The locale name Amazon Lex reports for `locale_id`, or `None` when the service
/// supports no such locale.
pub fn locale_name(locale_id: &str) -> Option<&'static str> {
    SUPPORTED_LOCALES
        .iter()
        .find(|(id, _)| *id == locale_id)
        .map(|(_, name)| *name)
}

/// Every supported locale id, in the order [`SUPPORTED_LOCALES`] lists them: what a
/// refusal tells a caller it may send instead.
pub fn supported_locale_ids() -> Vec<&'static str> {
    SUPPORTED_LOCALES.iter().map(|(id, _)| *id).collect()
}

/// Every member of the modelled `BotType` enum
/// (`smithy_query(query="shape",shape="BotType")`).
pub const BOT_TYPES: [&str; 2] = ["Bot", "BotNetwork"];

/// Every member of the modelled `BotSortAttribute` enum
/// (`smithy_query(query="shape",shape="BotSortAttribute")`).
pub const BOT_SORT_ATTRIBUTES: [&str; 1] = ["BotName"];

/// Every member of the modelled `SortOrder` enum
/// (`smithy_query(query="shape",shape="SortOrder")`).
pub const SORT_ORDERS: [&str; 2] = ["Ascending", "Descending"];

/// Every member of the modelled `BotFilterName` enum
/// (`smithy_query(query="shape",shape="BotFilterName")`).
pub const BOT_FILTER_NAMES: [&str; 2] = ["BotName", "BotType"];

/// Every member of the modelled `BotFilterOperator` enum
/// (`smithy_query(query="shape",shape="BotFilterOperator")`).
pub const BOT_FILTER_OPERATORS: [&str; 3] = ["CO", "EQ", "NE"];

/// Every member of the modelled `SlotTypeSortAttribute` enum
/// (`smithy_query(query="shape",shape="SlotTypeSortAttribute")`).
pub const SLOT_TYPE_SORT_ATTRIBUTES: [&str; 2] = ["SlotTypeName", "LastUpdatedDateTime"];

/// Every member of the modelled `SlotTypeFilterName` enum
/// (`smithy_query(query="shape",shape="SlotTypeFilterName")`).
pub const SLOT_TYPE_FILTER_NAMES: [&str; 2] = ["SlotTypeName", "ExternalSourceType"];

/// Every member of the modelled `SlotTypeFilterOperator` enum
/// (`smithy_query(query="shape",shape="SlotTypeFilterOperator")`), which — unlike
/// [`BOT_FILTER_OPERATORS`] — carries no `NE`.
pub const SLOT_TYPE_FILTER_OPERATORS: [&str; 2] = ["CO", "EQ"];

/// Members of the modelled `SlotTypeCategory` enum
/// (`smithy_query(query="shape",shape="SlotTypeCategory")`): Custom | Extended |
/// ExternalGrammar | Composite. Which one a stored slot type falls in is
/// [`SlotType::category`].
pub const SLOT_TYPE_CATEGORY_CUSTOM: &str = "Custom";
/// See [`SLOT_TYPE_CATEGORY_CUSTOM`].
pub const SLOT_TYPE_CATEGORY_EXTENDED: &str = "Extended";
/// See [`SLOT_TYPE_CATEGORY_CUSTOM`].
pub const SLOT_TYPE_CATEGORY_EXTERNAL_GRAMMAR: &str = "ExternalGrammar";
/// See [`SLOT_TYPE_CATEGORY_CUSTOM`].
pub const SLOT_TYPE_CATEGORY_COMPOSITE: &str = "Composite";

/// The external source a slot type takes its values from, as the `ExternalSourceType`
/// filter of `ListSlotTypes` names it: the member of the modelled
/// `ExternalSourceSetting` the slot type carries
/// (`smithy_query(query="shape",shape="ExternalSourceSetting")` declares exactly one,
/// `grammarSlotTypeSetting`).
pub const EXTERNAL_SOURCE_TYPE_GRAMMAR: &str = "GrammarSlotTypeSetting";

/// How long after a create the `Creating` -> settled transition falls due: `Available`
/// for a bot, `NotBuilt` for a bot locale.
///
/// AWS documents the transition but not its duration ("The bot is first in the
/// `Creating` status. Once the bot is ready for use, it changes to the `Available`
/// status"), so the span is CHOSEN: non-zero, so a caller that reads the bot straight
/// back still observes `Creating`, and short enough that a client script moves on
/// quickly. Nothing sleeps for it — the due instant is stored and the status is
/// materialised by the next read, which is what the clock already owed.
pub const CREATION_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// How long after a build is submitted the `Building` -> `Built` transition falls due.
///
/// AWS documents the transition but not its duration (`BuildBotLocale` answers while
/// "Amazon Lex is building the bot" and the locale is `Built` once it "is ready for use"),
/// so the span is CHOSEN, the same way and for the same reasons as
/// [`CREATION_SETTLES_AFTER`]: non-zero, so a caller reading the locale straight after
/// submitting the build still observes `Building`, and materialised by the next read
/// rather than slept for.
///
/// It is this service's OWN span rather than a longer one because the two transitions a
/// locale has are the same kind of wait to a client — a create settles and a build
/// settles — so a locale that is read a few calls after either one answers the same way;
/// stretching only the build would make a recorded client that builds and then reads the
/// locale see a status its own run never saw.
pub const BUILD_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// How many intents one bot locale may hold: the AWS Lex V2 model-building quota on the
/// intents of a bot locale, which is adjustable on request, at the default this emulator
/// enforces.
///
/// `CreateIntent` answers the modelled `ServiceQuotaExceededException` ("You have reached
/// a quota for your bot", HTTP 402) once the locale holds this many, which is what bounds
/// the stored intents: at most [`BOTS_PER_ACCOUNT_QUOTA`] bots times the locale ids of
/// [`SUPPORTED_LOCALES`] times this many records, so a caller cannot grow that map past
/// it.
pub const INTENTS_PER_LOCALE_QUOTA: usize = 1_000;

/// The intent Amazon Lex V2 adds to every locale it creates, and the built-in intent that
/// intent is based on: a locale is created with its fallback intent already in it, which
/// is why a locale nothing else was added to reports one intent.
pub const FALLBACK_INTENT_NAME: &str = "FallbackIntent";
/// The built-in intent [`FALLBACK_INTENT_NAME`] is based on, as `parentIntentSignature`
/// names it — the signature AWS documents for the intent Amazon Lex uses "when it can't
/// determine another intent to invoke".
pub const FALLBACK_INTENT_SIGNATURE: &str = "AMAZON.FallbackIntent";

/// How many bots one account may hold: the published AWS Lex V2 service quota "Bots
/// per account per Region" (default 100, adjustable on request).
///
/// `CreateBot` answers the modelled `ServiceQuotaExceededException` ("You have reached
/// a quota for your bot", HTTP 402) once the store holds this many, which is also what
/// bounds how large this service's state — and therefore any listing built from it —
/// can grow.
pub const BOTS_PER_ACCOUNT_QUOTA: usize = 100;

/// How many slot types one bot locale may hold: the AWS Lex V2 model-building quota on
/// the slot types of a bot locale, which is adjustable on request, at the default this
/// emulator enforces.
///
/// `CreateSlotType` answers the modelled `ServiceQuotaExceededException` ("You have
/// reached a quota for your bot", HTTP 402) once the locale holds this many, which is
/// what bounds the stored slot types: at most [`BOTS_PER_ACCOUNT_QUOTA`] bots times the
/// locale ids of [`SUPPORTED_LOCALES`] times this many records, so a caller cannot grow
/// that map past it.
pub const SLOT_TYPES_PER_LOCALE_QUOTA: usize = 1_000;

/// The locale id in which a slot may return multiple values: the `multipleValuesSetting`
/// member of `CreateSlot` documents the restriction — "Multi-value slots are only
/// available in the en-US locale. If you set this value to true in any other locale,
/// Amazon Lex throws a ValidationException" — and this is how [`SUPPORTED_LOCALES`] names
/// that locale.
pub const MULTIPLE_VALUES_LOCALE_ID: &str = "en_US";

/// How many slots one intent may hold: the AWS Lex V2 model-building quota on the slots
/// of an intent, which is adjustable on request, at the default this emulator enforces.
///
/// `CreateSlot` answers the modelled `ServiceQuotaExceededException` ("You have reached a
/// quota for your bot", HTTP 402) once the intent holds this many, which is what bounds
/// the stored slots: at most the intents the store may hold times this many records, so a
/// caller cannot grow that map past it.
pub const SLOTS_PER_INTENT_QUOTA: usize = 100;

/// Every member of the modelled `SlotSortAttribute` enum
/// (`smithy_query(query="shape",shape="SlotSortAttribute")`).
pub const SLOT_SORT_ATTRIBUTES: [&str; 2] = ["SlotName", "LastUpdatedDateTime"];

/// Every member of the modelled `SlotFilterName` enum
/// (`smithy_query(query="shape",shape="SlotFilterName")`), which names only the slot's
/// own name.
pub const SLOT_FILTER_NAMES: [&str; 1] = ["SlotName"];

/// Every member of the modelled `SlotFilterOperator` enum
/// (`smithy_query(query="shape",shape="SlotFilterOperator")`).
pub const SLOT_FILTER_OPERATORS: [&str; 2] = ["CO", "EQ"];

/// How Amazon Lex V2 names a slot in a message about a name that is already in use.
/// See [`name_already_exists`].
pub const SLOT_RESOURCE_LABEL: &str = "Slot";

/// How Amazon Lex V2 names a bot in a message about a name that is already in use.
/// See [`name_already_exists`].
pub const BOT_RESOURCE_LABEL: &str = "Bot";

/// How Amazon Lex V2 names a slot type in a message about a name that is already in
/// use. See [`name_already_exists`].
pub const SLOT_TYPE_RESOURCE_LABEL: &str = "Slot Type";

/// How Amazon Lex V2 names an intent in a message about a name that is already in use.
/// See [`name_already_exists`].
pub const INTENT_RESOURCE_LABEL: &str = "Intent";

/// The message this service answers when the name a request asks for is already in use
/// where that name has to be unique.
///
/// The template names the kind of resource whose name was taken and the name the
/// request asked for, and carries no other identifier: the scope the name has to be
/// unique in (an account, a bot version, a locale) is not part of the message, so a
/// client matching on this refusal sees the same text whatever the scope was.
///
/// `resource_label` is how the service names that kind of resource —
/// [`BOT_RESOURCE_LABEL`], [`SLOT_TYPE_RESOURCE_LABEL`] — and `name` is the requested
/// name, copied from the request verbatim.
pub fn name_already_exists(resource_label: &str, name: &str) -> String {
    format!("{resource_label} with name {name} already exists. Use another name.")
}

/// The largest page `ListBots` returns, which is the modelled `MaxResults` maximum
/// (`smithy_query(query="operation",operation="ListBots")`: range 1..1000). It also
/// caps a listing that asks for no page size, so no single response is unbounded; the
/// remainder stays reachable through `nextToken`.
pub const MAX_LIST_PAGE: usize = 1000;

/// A bot, as `CreateBot` stored it and `DescribeBot` / `ListBots` / `UpdateBot` read it
/// back.
#[derive(Debug, Clone)]
pub struct Bot {
    pub bot_id: String,
    pub bot_name: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub data_privacy: DataPrivacy,
    pub idle_session_ttl_in_seconds: i32,
    pub bot_status: String,
    pub bot_type: Option<String>,
    /// The members of the bot network, as the request that last wrote this bot supplied
    /// them — `None` where it supplied none, which is not the same as an empty list.
    pub bot_members: Option<Vec<BotMember>>,
    pub error_log_settings: Option<ErrorLogSettings>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// The tags the create supplied, held exactly as supplied: see [`Bot::bot_members`].
    pub bot_tags: Option<HashMap<String, String>>,
    /// The test-alias tags the create supplied, held exactly as supplied.
    pub test_bot_alias_tags: Option<HashMap<String, String>>,
    /// How many versions of this bot have been created. AWS documents what the count
    /// means for the number a version is created under: "The version number will always
    /// represent the total number of versions created of the bot, not the current number
    /// of versions. If a bot version is deleted, that bot version number will not be
    /// reused." Moved on through [`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::mint_bot_version`].
    pub versions_created: u32,
    /// The status the clock owes this bot, and the instant it falls due. `None` once
    /// the bot has reached a status nothing further is owed from. Set it through
    /// [`Bot::schedule_status`] rather than by hand.
    pub pending_status: Option<(String, Instant)>,
}

impl Bot {
    /// Apply the status transition the clock has already made due, if any.
    ///
    /// A read calls this before answering: the change was owed at that instant whether
    /// or not anybody called, so materialising it is not a mutation a reader invented.
    pub fn settle(&mut self) {
        settle_status(&mut self.bot_status, &mut self.pending_status);
    }

    /// Schedule `status` to become this bot's status once `after` has elapsed.
    pub fn schedule_status(&mut self, status: &str, after: Duration) {
        self.pending_status = Some((status.to_string(), Instant::now() + after));
    }

    /// This bot as a `ListBots` summary entry.
    ///
    /// `latestBotVersion` is "the latest numerical version in use for the bot": the
    /// highest number a version the store still holds was created under, which the caller
    /// resolves and passes in. It stays absent until a numbered version exists, since a
    /// bot that has only its `DRAFT` version has no numerical version.
    pub fn summary(&self, latest_bot_version: Option<String>) -> BotSummary {
        BotSummary {
            bot_id: Some(self.bot_id.clone()),
            bot_name: Some(self.bot_name.clone()),
            description: self.description.clone(),
            bot_status: Some(self.bot_status.clone()),
            latest_bot_version,
            last_updated_date_time: Some(to_date_time(self.last_updated_date_time)),
            bot_type: self.bot_type.clone(),
        }
    }

    /// Does this bot's network include the bot `bot_id` names? A bot the store holds no
    /// members for has no network to include anything.
    pub fn has_network_member(&self, bot_id: &str) -> bool {
        self.bot_members
            .iter()
            .flatten()
            .any(|member| member.bot_member_id == bot_id)
    }
}

/// `BotAliasStatus` members this crate names. The full modelled set is
/// `smithy_query(query="shape",shape="BotAliasStatus")`:
/// Creating | Available | Deleting | Failed. AWS documents this one as the status in
/// which "the alias is ready for use".
pub const BOT_ALIAS_STATUS_AVAILABLE: &str = "Available";

/// The working copy of a bot, which is the version an alias points at until it is
/// pointed at a numbered one: the modelled `BotVersion` admits it beside a number
/// (`smithy_query(query="shape",shape="BotVersion")`, pattern `^(DRAFT|[0-9]+)$`).
pub const DRAFT_BOT_VERSION: &str = "DRAFT";

/// The `botAliasId` of the alias Amazon Lex V2 keeps for testing a bot: the modelled
/// `BotAliasId` singles this id out in its own pattern
/// (`smithy_query(query="shape",shape="BotAliasId")`,
/// `^(\bTSTALIASID\b|[0-9a-zA-Z]+)$`), rather than leaving it to the minted
/// `[0-9a-zA-Z]{10}` space every other alias id is drawn from.
pub const TEST_BOT_ALIAS_ID: &str = "TSTALIASID";

/// The `botAliasName` of that alias, as Amazon Lex V2 publishes it.
pub const TEST_BOT_ALIAS_NAME: &str = "TestBotAlias";

/// How a bot alias is keyed: the bot it belongs to and its own id, which is the pair
/// every alias operation's URI carries.
pub type BotAliasKey = (String, String);

/// The [`BotAliasKey`] for one alias of one bot.
pub fn bot_alias_key(bot_id: &str, bot_alias_id: &str) -> BotAliasKey {
    (bot_id.to_string(), bot_alias_id.to_string())
}

/// An alias of a bot, as `ListBotAliases` reads it back.
///
/// Every bot has at least the alias Amazon Lex V2 keeps for testing it
/// ([`TEST_BOT_ALIAS_ID`]), which exists from the moment the bot does: `CreateBot`
/// documents its `testBotAliasTags` as "a list of tags to add to the test alias for a
/// bot" that "you can only add when you create a bot", so the alias is there to be
/// tagged as the bot is created. That is why a listing of a freshly created bot is not
/// empty, and why nothing else has to be created for an alias of a bot to exist.
#[derive(Debug, Clone)]
pub struct BotAlias {
    pub bot_id: String,
    pub bot_alias_id: String,
    pub bot_alias_name: String,
    pub description: Option<String>,
    /// The version of the bot this alias points at: a number, or
    /// [`DRAFT_BOT_VERSION`] for the working copy.
    pub bot_version: String,
    pub bot_alias_status: String,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
}

impl BotAlias {
    /// The alias Amazon Lex V2 keeps for testing `bot_id`, as created with the bot.
    ///
    /// It points at the bot's working copy ([`DRAFT_BOT_VERSION`]) — the version the
    /// test alias is there to exercise — and is ready for use straight away: nothing
    /// Amazon Lex answers reports this alias in a status it still has to leave, so
    /// there is no transition owed on it, unlike the bot itself, whose own `Creating`
    /// status `CreateBot` reports and [`CREATION_SETTLES_AFTER`] settles.
    pub fn test_alias(bot_id: &str, now: SystemTime) -> Self {
        Self {
            bot_id: bot_id.to_string(),
            bot_alias_id: TEST_BOT_ALIAS_ID.to_string(),
            bot_alias_name: TEST_BOT_ALIAS_NAME.to_string(),
            description: None,
            bot_version: DRAFT_BOT_VERSION.to_string(),
            bot_alias_status: BOT_ALIAS_STATUS_AVAILABLE.to_string(),
            creation_date_time: now,
            last_updated_date_time: now,
        }
    }

    /// This alias as a `ListBotAliases` summary entry.
    pub fn summary(&self) -> BotAliasSummary {
        BotAliasSummary {
            bot_alias_id: Some(self.bot_alias_id.clone()),
            bot_alias_name: Some(self.bot_alias_name.clone()),
            description: self.description.clone(),
            bot_version: Some(self.bot_version.clone()),
            bot_alias_status: Some(self.bot_alias_status.clone()),
            creation_date_time: Some(to_date_time(self.creation_date_time)),
            last_updated_date_time: Some(to_date_time(self.last_updated_date_time)),
        }
    }
}

/// How many versions one bot may hold: the published AWS Lex V2 service quota "Versions
/// per bot" (default 100, adjustable on request).
///
/// `CreateBotVersion` answers the modelled `ServiceQuotaExceededException` ("You have
/// reached a quota for your bot", HTTP 402) once the bot holds this many, which is what
/// bounds the stored versions: at most [`BOTS_PER_ACCOUNT_QUOTA`] bots times this many
/// records, so a caller cannot grow that map past it.
pub const VERSIONS_PER_BOT_QUOTA: usize = 100;

/// How a bot version is keyed: the bot and the number the version was created under,
/// which is the pair every version operation's URI carries.
pub type BotVersionKey = (String, String);

/// The [`BotVersionKey`] for one version of one bot.
pub fn bot_version_key(bot_id: &str, bot_version: &str) -> BotVersionKey {
    (bot_id.to_string(), bot_version.to_string())
}

/// How a resource defined INSIDE a bot version — an intent, a slot type, a slot — is
/// keyed: the bot version holding it and its own id.
///
/// The id alone does not identify the record, because creating a version snapshots the
/// locales it is taken from and everything in them keeps the id it already has: the same
/// intent id names the draft intent and the copy of it each version holds.
pub type VersionedResourceKey = (String, String);

/// The [`VersionedResourceKey`] for one resource of one bot version.
pub fn versioned_key(bot_version: &str, resource_id: &str) -> VersionedResourceKey {
    (bot_version.to_string(), resource_id.to_string())
}

/// A version of a bot, as `CreateBotVersion` stored it and `DescribeBotVersion` reads it
/// back.
///
/// A version is IMMUTABLE, so it holds the bot settings as they were when it was created
/// rather than reading them back off the bot, which goes on changing: a later `UpdateBot`
/// changes the draft, never a published version.
#[derive(Debug, Clone)]
pub struct BotVersion {
    pub bot_id: String,
    pub bot_version: String,
    pub bot_name: String,
    /// The description of the VERSION, as the create named it — the bot's own description
    /// is its own.
    pub description: Option<String>,
    pub role_arn: String,
    pub data_privacy: DataPrivacy,
    pub idle_session_ttl_in_seconds: i32,
    pub bot_type: Option<String>,
    /// The bot's network members as they stood when the version was created, held the
    /// way the bot holds them: see [`Bot::bot_members`].
    pub bot_members: Option<Vec<BotMember>>,
    pub bot_status: String,
    pub creation_date_time: SystemTime,
    /// The version each locale of this version was copied from, keyed by locale id —
    /// what the create named, and what it reports back.
    pub locale_specification: HashMap<String, BotVersionLocaleDetails>,
    /// The status the clock owes this version, and the instant it falls due. Set it
    /// through [`BotVersion::schedule_status`] rather than by hand.
    pub pending_status: Option<(String, Instant)>,
}

impl BotVersion {
    /// Apply the status transition the clock has already made due, if any — the same
    /// contract as [`Bot::settle`].
    pub fn settle(&mut self) {
        settle_status(&mut self.bot_status, &mut self.pending_status);
    }

    /// Schedule `status` to become this version's status once `after` has elapsed.
    pub fn schedule_status(&mut self, status: &str, after: Duration) {
        self.pending_status = Some((status.to_string(), Instant::now() + after));
    }
}

/// Move `status` on to the transition the clock has already made due, if any, and answer
/// whether it moved.
///
/// Shared by every record that carries a scheduled status, so one rule decides when a
/// pending transition is owed.
fn settle_status(status: &mut String, pending: &mut Option<(String, Instant)>) -> bool {
    let now = Instant::now();
    if let Some((next, due)) = pending.take() {
        if now >= due {
            *status = next;
            return true;
        }
        *pending = Some((next, due));
    }
    false
}

/// The `resolutionStrategy` a slot type resolves values under when a create names no
/// `valueSelectionSetting`: the member's own documentation
/// (`smithy_query(query="doc",shape="SlotValueSelectionSetting",member="resolutionStrategy")`)
/// says the default is the original value the user entered. The constant is the member of
/// the modelled `SlotValueResolutionStrategy` enum
/// (`smithy_query(query="shape",shape="SlotValueResolutionStrategy")`:
/// OriginalValue | TopResolution | Concatenation) that names that strategy.
pub const DEFAULT_RESOLUTION_STRATEGY: &str = "OriginalValue";

/// What this control plane says when a read names a resource it does not hold.
///
/// The modelled `ResourceNotFoundException`
/// (`smithy_query(query="doc",shape="ResourceNotFoundException")`) documents the
/// condition — "You asked to describe a resource that doesn't exist" — and the service
/// states it with this one sentence, which has no variable part: it names neither the
/// kind of resource nor the identifier the request carried, so every read of this
/// service answers a resource it cannot find with exactly this text.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Failed to retrieve resource since it does not exist";

/// What the operations that CREATE, BUILD or UPDATE a resource inside another say when a
/// request names a PARENT resource this control plane does not hold: the bot, bot
/// version, locale, intent or slot type the resource has to hang off.
///
/// The modelled `PreconditionFailedException` documents the condition — "Your request
/// couldn't be completed because one or more request fields aren't valid" — and
/// `CreateBotVersion`, `BuildBotLocale`, `CreateSlotType`, `CreateSlot`, `CreateIntent`
/// and `UpdateIntent` state it with this one sentence, which, like
/// [`RESOURCE_NOT_FOUND_MESSAGE`], has no variable part: it names neither the kind of
/// parent nor the identifier that failed to resolve.
///
/// It is not the only wording this service gives that exception. Where a recorded
/// response states the resource and the identifiers the request carried for it, that is
/// what the operation answers instead — `CreateExport` does, through
/// [`request_path_resource_missing`] — so the sentence an operation uses is the one its
/// own recorded behaviour states, and this constant is what the operations above use.
pub const PARENT_RESOURCE_NOT_FOUND_MESSAGE: &str =
    "Failed to retrieve parent resource since it does not exist";

/// How Amazon Lex V2 names a bot in a sentence about a resource the request named that
/// does not exist. See [`request_path_resource_missing`].
pub const EXPORTED_BOT_LABEL: &str = "Bot";

/// How it names one locale of a bot version there. See
/// [`request_path_resource_missing`].
pub const EXPORTED_BOT_LOCALE_LABEL: &str = "Bot locale";

/// How it names the custom vocabulary of such a locale there. See
/// [`request_path_resource_missing`].
pub const EXPORTED_CUSTOM_VOCABULARY_LABEL: &str = "Custom vocabulary";

/// How it names a test set there. See [`request_path_resource_missing`].
pub const EXPORTED_TEST_SET_LABEL: &str = "Test set";

/// What this control plane says when the resource a request names does not exist: the
/// kind of resource, the identifiers the request carried for it, and the fixed tail the
/// sentence ends with.
///
/// For a bot and the version of it a request named, the sentence is `Bot with id <botId>
/// and version <botVersion> in the request path does not exist.` — `resource` is how the
/// service names that kind of resource ([`EXPORTED_BOT_LABEL`] and the labels beside it),
/// and `clauses` is each identifier as `(how the service names it, the value the request
/// carried)`, in the order the sentence states them. Every value is copied from the
/// request verbatim, so a caller reading the message sees which identifier it has to fix.
pub fn request_path_resource_missing(resource: &str, clauses: &[(&str, &str)]) -> String {
    let stated: Vec<String> = clauses
        .iter()
        .map(|(name, value)| format!("{name} {value}"))
        .collect();
    format!(
        "{resource} with {} in the request path does not exist.",
        stated.join(" and ")
    )
}

/// The prefix the modelled `BuiltInOrCustomSlotTypeId` shape gives a reference to a slot
/// type Amazon Lex supplies rather than one defined in the bot: its pattern
/// `^((AMAZON\.)[a-zA-Z_]+?|[0-9a-zA-Z]+)$` admits exactly these two forms, so a
/// reference carrying this prefix is a built-in and any other is the id of a custom slot
/// type the locale has to hold.
pub const BUILT_IN_SLOT_TYPE_PREFIX: &str = "AMAZON.";

/// Is this `slotTypeId` a reference to a built-in slot type Amazon Lex supplies?
/// See [`BUILT_IN_SLOT_TYPE_PREFIX`].
pub fn is_built_in_slot_type(slot_type_id: &str) -> bool {
    slot_type_id.starts_with(BUILT_IN_SLOT_TYPE_PREFIX)
}

/// The voice-activity-detection sensitivity a bot locale runs at when a create names
/// none: the member of the modelled `SpeechDetectionSensitivity` enum
/// (`smithy_query(query="shape",shape="SpeechDetectionSensitivity")`) its own
/// documentation calls the standard level — "`Default` - Standard sensitivity level
/// suitable for most environments" — against the two that raise the tolerance for
/// background noise.
pub const DEFAULT_SPEECH_DETECTION_SENSITIVITY: &str = "Default";

/// The only built-in slot type a custom slot type may extend. The modelled
/// `parentSlotTypeSignature` member documents the restriction: "The built-in slot type
/// used as a parent of this slot type. [...] Only AMAZON.AlphaNumeric is supported."
pub const SUPPORTED_PARENT_SLOT_TYPE_SIGNATURE: &str = "AMAZON.AlphaNumeric";

/// The one reason Amazon Lex gives for refusing to build a locale that holds nothing it
/// could build: a locale is buildable only once it has a custom intent carrying at least
/// one valid sample utterance, and the sentence names the locale the build was asked for.
///
/// The locale id is the only variable part; the rest is fixed text for this failure.
pub fn no_utterances_failure_reason(locale_id: &str) -> String {
    format!(
        "The locale '{locale_id}' doesn't have any utterances. A locale must have at least \
         one custom intent with a valid utterance. Add a custom intent and try your request \
         again."
    )
}

/// The reason Amazon Lex gives for refusing to build a locale holding an intent whose
/// slots are not all ordered: the elicitation order of an intent's slots is carried by
/// its `slotPriorities`, and a slot no entry of that list names has no place in it.
///
/// The variable parts are the slots of that intent nothing orders — named one after
/// another, comma separated, inside the brackets — and the name of the intent holding
/// them; everything else is fixed text for this failure. One reason covers one intent,
/// however many of its slots are unordered.
pub fn unprioritised_slots_failure_reason(slots: &[String], intent_name: &str) -> String {
    format!(
        "Slot ids [{}] in intent {intent_name} don't define a slot priority. Update the \
         intent to add a priority to these slots.",
        slots.join(", ")
    )
}

/// A bot locale, as `CreateBotLocale` stored it and `DescribeBotLocale` reads it back.
#[derive(Debug, Clone)]
pub struct BotLocale {
    pub bot_id: String,
    pub bot_version: String,
    pub locale_id: String,
    pub locale_name: String,
    pub description: Option<String>,
    pub nlu_intent_confidence_threshold: f64,
    pub voice_settings: Option<VoiceSettings>,
    pub unified_speech_settings: Option<UnifiedSpeechSettings>,
    pub audio_filler_settings: Option<AudioFillerSettings>,
    pub speech_recognition_settings: Option<SpeechRecognitionSettings>,
    pub generative_ai_settings: Option<GenerativeAISettings>,
    pub speech_detection_sensitivity: Option<String>,
    pub speaker_diarization_settings: Option<SpeakerDiarizationSettings>,
    pub bot_locale_status: String,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// When the locale was last submitted for building; `None` until a build is asked
    /// for.
    pub last_build_submitted_date_time: Option<SystemTime>,
    /// Why the build this locale has already settled on did not succeed; empty for a
    /// locale whose last build succeeded and for one never built. Only a locale in
    /// [`BOT_LOCALE_STATUS_FAILED`] carries any.
    pub failure_reasons: Vec<String>,
    /// The reasons that become [`BotLocale::failure_reasons`] once the status the clock
    /// owes falls due — the outcome of the build in flight, which is not a caller's to
    /// read before the build settles.
    pub pending_failure_reasons: Vec<String>,
    /// The status the clock owes this locale, and the instant it falls due. Set it
    /// through [`BotLocale::schedule_status`] rather than by hand.
    pub pending_status: Option<(String, Instant)>,
}

impl BotLocale {
    /// Apply the status transition the clock has already made due, if any — the same
    /// contract as [`Bot::settle`]. A settled build also publishes its outcome: the
    /// reasons it failed for become readable with the status that carries them.
    pub fn settle(&mut self) {
        if settle_status(&mut self.bot_locale_status, &mut self.pending_status) {
            self.failure_reasons = std::mem::take(&mut self.pending_failure_reasons);
        }
    }

    /// Schedule `status` to become this locale's status once `after` has elapsed.
    pub fn schedule_status(&mut self, status: &str, after: Duration) {
        self.pending_status = Some((status.to_string(), Instant::now() + after));
    }

    /// Record a build submitted at `now` whose outcome is `failure_reasons`: the locale
    /// is building from that instant, the build timestamp is the one a read reports, and
    /// the terminal status the clock then owes is due [`BUILD_SETTLES_AFTER`] later —
    /// [`BOT_LOCALE_STATUS_BUILT`] when the locale was buildable and
    /// [`BOT_LOCALE_STATUS_FAILED`], carrying those reasons, when it was not.
    ///
    /// The reasons a PREVIOUS build settled on go with it: they described that build, so
    /// a locale building again reports none until this one settles.
    pub fn submit_build(&mut self, now: SystemTime, failure_reasons: Vec<String>) {
        self.last_build_submitted_date_time = Some(now);
        self.bot_locale_status = BOT_LOCALE_STATUS_BUILDING.to_string();
        self.failure_reasons = Vec::new();
        let terminal = if failure_reasons.is_empty() {
            BOT_LOCALE_STATUS_BUILT
        } else {
            BOT_LOCALE_STATUS_FAILED
        };
        self.pending_failure_reasons = failure_reasons;
        self.schedule_status(terminal, BUILD_SETTLES_AFTER);
    }

    /// The stored failure reasons as the wire shape renders them: absent rather than an
    /// empty list, since a locale that did not fail a build has none to report.
    pub fn wire_failure_reasons(&self) -> Option<Vec<String>> {
        (!self.failure_reasons.is_empty()).then(|| self.failure_reasons.clone())
    }
}

/// An intent of one locale of one bot version, as `CreateIntent` stored it and
/// `DescribeIntent` reads it back.
///
/// Its optional collections — the sample utterances, the slot priorities, the input and
/// the output contexts — are held as the modelled optional lists they are rather than as
/// lists that are merely empty: a write that supplied none of a kind leaves that member
/// with NO value, and what a read reports is exactly what the record holds.
#[derive(Debug, Clone)]
pub struct Intent {
    pub intent_id: String,
    pub intent_name: String,
    pub bot_id: String,
    pub bot_version: String,
    pub locale_id: String,
    pub intent_display_name: Option<String>,
    pub description: Option<String>,
    pub parent_intent_signature: Option<String>,
    pub sample_utterances: Option<Vec<SampleUtterance>>,
    pub slot_priorities: Option<Vec<SlotPriority>>,
    pub dialog_code_hook: Option<DialogCodeHookSettings>,
    pub fulfillment_code_hook: Option<FulfillmentCodeHookSettings>,
    pub intent_confirmation_setting: Option<IntentConfirmationSetting>,
    pub intent_closing_setting: Option<IntentClosingSetting>,
    pub input_contexts: Option<Vec<InputContext>>,
    pub output_contexts: Option<Vec<OutputContext>>,
    pub kendra_configuration: Option<KendraConfiguration>,
    pub initial_response_setting: Option<InitialResponseSetting>,
    pub qn_a_intent_configuration: Option<QnAIntentConfiguration>,
    pub q_in_connect_intent_configuration: Option<QInConnectIntentConfiguration>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
}

impl Intent {
    /// Is this intent the one that triple names? An intent is defined inside one locale
    /// of one bot version, so every read is scoped by the same triple the operation's URI
    /// carries.
    pub fn is_in_locale(&self, bot_id: &str, bot_version: &str, locale_id: &str) -> bool {
        self.bot_id == bot_id && self.bot_version == bot_version && self.locale_id == locale_id
    }

    /// Is this a CUSTOM intent — one the bot defines itself — rather than one built on a
    /// built-in intent Amazon Lex supplies, which an intent names through
    /// `parentIntentSignature`?
    pub fn is_custom(&self) -> bool {
        self.parent_intent_signature.is_none()
    }

    /// Does this intent carry at least one VALID sample utterance? An utterance with no
    /// text in it is not one Amazon Lex can recognise, so it does not count, and neither
    /// does an intent that carries no utterances at all.
    pub fn has_valid_utterance(&self) -> bool {
        self.sample_utterances
            .iter()
            .flatten()
            .any(|sample| !sample.utterance.trim().is_empty())
    }

    /// Does an entry of this intent's `slotPriorities` order the slot that id names?
    ///
    /// A `SlotPriority` names the slot it orders by its id
    /// (`smithy_query(query="shape",shape="SlotPriority")`), so that is what an entry is
    /// matched on.
    pub fn orders_slot(&self, slot_id: &str) -> bool {
        self.slot_priorities
            .iter()
            .flatten()
            .any(|priority| priority.slot_id == slot_id)
    }
}

/// How a bot locale is keyed: the bot, the bot version and the locale id, which is the
/// same triple every locale operation's URI carries.
pub type BotLocaleKey = (String, String, String);

/// The [`BotLocaleKey`] for one locale of one bot version.
pub fn bot_locale_key(bot_id: &str, bot_version: &str, locale_id: &str) -> BotLocaleKey {
    (
        bot_id.to_string(),
        bot_version.to_string(),
        locale_id.to_string(),
    )
}

/// A custom slot type, as `CreateSlotType` stored it and `DescribeSlotType` reads it
/// back.
#[derive(Debug, Clone)]
pub struct SlotType {
    pub slot_type_id: String,
    pub slot_type_name: String,
    pub bot_id: String,
    pub bot_version: String,
    pub locale_id: String,
    pub description: Option<String>,
    /// The values the slot type enumerates, as the request that wrote it supplied them.
    /// The modelled `SlotTypeValues` list has a minimum length of 1, so a slot type
    /// either carries values or carries the member not at all.
    pub slot_type_values: Option<Vec<SlotTypeValue>>,
    pub value_selection_setting: Option<SlotValueSelectionSetting>,
    pub parent_slot_type_signature: Option<String>,
    pub external_source_setting: Option<ExternalSourceSetting>,
    pub composite_slot_type_setting: Option<CompositeSlotTypeSetting>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
}

impl SlotType {
    /// Is this slot type the one that triple names? A slot type is defined inside one
    /// locale of one bot version, so every read is scoped by the same triple the
    /// operation's URI carries.
    pub fn is_in_locale(&self, bot_id: &str, bot_version: &str, locale_id: &str) -> bool {
        self.bot_id == bot_id && self.bot_version == bot_version && self.locale_id == locale_id
    }

    /// Which member of the modelled `SlotTypeCategory` enum this slot type falls in,
    /// read off how it was defined: one composed of sub slot types is `Composite`, one
    /// whose values come from a source held outside the bot is `ExternalGrammar`, one
    /// built on a parent signature extends it, and a slot type that enumerates its own
    /// values is a plain custom one.
    pub fn category(&self) -> &'static str {
        if self.composite_slot_type_setting.is_some() {
            SLOT_TYPE_CATEGORY_COMPOSITE
        } else if self.external_source_setting.is_some() {
            SLOT_TYPE_CATEGORY_EXTERNAL_GRAMMAR
        } else if self.parent_slot_type_signature.is_some() {
            SLOT_TYPE_CATEGORY_EXTENDED
        } else {
            SLOT_TYPE_CATEGORY_CUSTOM
        }
    }

    /// The external source this slot type takes its values from, as the
    /// `ExternalSourceType` filter of `ListSlotTypes` names it, or `None` when its
    /// values are enumerated in the bot.
    pub fn external_source_type(&self) -> Option<&'static str> {
        self.external_source_setting
            .as_ref()
            .map(|_| EXTERNAL_SOURCE_TYPE_GRAMMAR)
    }

    /// This slot type as a `ListSlotTypes` summary entry.
    pub fn summary(&self) -> SlotTypeSummary {
        SlotTypeSummary {
            slot_type_id: Some(self.slot_type_id.clone()),
            slot_type_name: Some(self.slot_type_name.clone()),
            description: self.description.clone(),
            parent_slot_type_signature: self.parent_slot_type_signature.clone(),
            last_updated_date_time: Some(to_date_time(self.last_updated_date_time)),
            slot_type_category: Some(self.category().to_string()),
        }
    }
}

/// A slot of one intent, as `CreateSlot` stored it and `DescribeSlot` / `ListSlots` read
/// it back.
#[derive(Debug, Clone)]
pub struct Slot {
    pub slot_id: String,
    pub slot_name: String,
    pub bot_id: String,
    pub bot_version: String,
    pub locale_id: String,
    pub intent_id: String,
    pub description: Option<String>,
    /// The slot type the slot elicits values for: the id of a custom slot type of the
    /// same locale, or a reference to a built-in one ([`BUILT_IN_SLOT_TYPE_PREFIX`]).
    /// `None` for a slot the create named none for.
    pub slot_type_id: Option<String>,
    pub value_elicitation_setting: SlotValueElicitationSetting,
    pub obfuscation_setting: Option<ObfuscationSetting>,
    pub multiple_values_setting: Option<MultipleValuesSetting>,
    pub sub_slot_setting: Option<SubSlotSetting>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
}

impl Slot {
    /// Is this slot one that intent of that locale holds? A slot is defined inside one
    /// intent of one locale of one bot version, so every read is scoped by the same four
    /// identifiers the operation's URI carries.
    pub fn is_in_intent(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        intent_id: &str,
    ) -> bool {
        self.bot_id == bot_id
            && self.bot_version == bot_version
            && self.locale_id == locale_id
            && self.intent_id == intent_id
    }

    /// Is this slot one of that locale's, whichever intent of it holds the slot? A slot
    /// is defined inside one intent of one locale, so every slot of an intent of the
    /// locale is one of the locale's.
    pub fn is_in_locale(&self, bot_id: &str, bot_version: &str, locale_id: &str) -> bool {
        self.bot_id == bot_id && self.bot_version == bot_version && self.locale_id == locale_id
    }

    /// This slot as a `ListSlots` summary entry.
    pub fn summary(&self) -> SlotSummary {
        SlotSummary {
            slot_id: Some(self.slot_id.clone()),
            slot_name: Some(self.slot_name.clone()),
            description: self.description.clone(),
            slot_constraint: Some(self.value_elicitation_setting.slot_constraint.clone()),
            slot_type_id: self.slot_type_id.clone(),
            value_elicitation_prompt_specification: self
                .value_elicitation_setting
                .prompt_specification
                .clone(),
            last_updated_date_time: Some(to_date_time(self.last_updated_date_time)),
        }
    }
}

/// Per-service state for com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State {
    /// Every bot this emulator holds, keyed by the `botId` it minted.
    pub bots: HashMap<String, Bot>,
    /// Every bot locale this emulator holds, keyed by [`bot_locale_key`]. A locale
    /// belongs to one version of one bot, so this map is bounded by the bots the store
    /// may hold ([`BOTS_PER_ACCOUNT_QUOTA`]) times the versions a bot may have
    /// ([`VERSIONS_PER_BOT_QUOTA`], plus its `DRAFT`) times the locale ids a bot version
    /// can carry ([`SUPPORTED_LOCALES`]): a caller cannot grow it past that.
    pub bot_locales: HashMap<BotLocaleKey, BotLocale>,
    /// Every custom slot type this emulator holds, keyed by [`versioned_key`]. The record
    /// carries the bot, bot version and locale it was created in, which is what scopes
    /// every read of it. A slot type belongs to one locale of one bot version, so this
    /// map is bounded the way [`Self::bot_locales`] is, times
    /// [`SLOT_TYPES_PER_LOCALE_QUOTA`]: a caller cannot grow it past that.
    pub slot_types: HashMap<VersionedResourceKey, SlotType>,
    /// Every intent this emulator holds, keyed by [`versioned_key`]. The record carries
    /// the bot, bot version and locale it was created in, which is what scopes every read
    /// of it. An intent belongs to one locale of one bot version, so this map is bounded
    /// the way [`Self::bot_locales`] is, times [`INTENTS_PER_LOCALE_QUOTA`]: a caller
    /// cannot grow it past that.
    pub intents: HashMap<VersionedResourceKey, Intent>,
    /// Every slot this emulator holds, keyed by [`versioned_key`]. The record carries the
    /// bot, bot version, locale and intent it was created in, which is what scopes every
    /// read of it. A slot belongs to one intent, so this map is bounded by the intents the
    /// store may hold times [`SLOTS_PER_INTENT_QUOTA`]: a caller cannot grow it past that.
    pub slots: HashMap<VersionedResourceKey, Slot>,
    /// Every bot version this emulator holds, keyed by [`bot_version_key`]. A version
    /// belongs to one bot, so this map is bounded by the bots the store may hold
    /// ([`BOTS_PER_ACCOUNT_QUOTA`]) times [`VERSIONS_PER_BOT_QUOTA`]: a caller cannot
    /// grow it past that.
    pub bot_versions: HashMap<BotVersionKey, BotVersion>,
    /// Every bot alias this emulator holds, keyed by [`bot_alias_key`]. An alias belongs
    /// to one bot, and the alias Amazon Lex keeps for testing a bot
    /// ([`BotAlias::test_alias`]) is created with the bot, so this map holds one record
    /// per stored bot and is bounded by [`BOTS_PER_ACCOUNT_QUOTA`]: a caller cannot grow
    /// it past that.
    pub bot_aliases: HashMap<BotAliasKey, BotAlias>,
    /// Every export job this emulator holds, keyed by the `exportId` [`Self::mint_export_id`]
    /// minted. A job is recorded by `CreateExport`, kept for the window AWS documents
    /// ([`EXPORT_RETENTION`]) and removed once it is past it, so this map holds the jobs a
    /// caller submitted inside one window and no more; what a job holds and how it settles
    /// is in [`Export`].
    pub exports: HashMap<String, Export>,
    /// The monotonic source the minted ids are derived from.
    id_seq: u64,
}

impl ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State {
    /// Mint an unused `botId`: exactly 10 characters from `[0-9a-zA-Z]`, as the
    /// modelled `Id` shape requires (len 10..10, pattern `^[0-9a-zA-Z]+$`).
    ///
    /// The id is derived from a monotonic counter, so two calls never collide, and the
    /// result is checked against the stored bots before it is handed out.
    pub fn mint_bot_id(&mut self) -> String {
        loop {
            self.id_seq += 1;
            let candidate = render_id(self.id_seq);
            if !self.bots.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint an unused `slotTypeId`, from the same counter and in the same
    /// `[0-9a-zA-Z]{10}` space [`mint_bot_id`] draws from, and checked against the
    /// stored slot types before it is handed out.
    pub fn mint_slot_type_id(&mut self) -> String {
        loop {
            self.id_seq += 1;
            let candidate = render_id(self.id_seq);
            if !self.holds_slot_type_id(&candidate) {
                return candidate;
            }
        }
    }

    /// Does any bot version hold a slot type under this id? An id is checked across
    /// every version, so one handed out never collides with a snapshot copy of another
    /// slot type.
    fn holds_slot_type_id(&self, slot_type_id: &str) -> bool {
        self.slot_types.keys().any(|(_, id)| id == slot_type_id)
    }

    /// Mint an unused `intentId`, from the same counter and in the same
    /// `[0-9a-zA-Z]{10}` space [`mint_bot_id`] draws from, and checked against the
    /// stored intents before it is handed out.
    pub fn mint_intent_id(&mut self) -> String {
        loop {
            self.id_seq += 1;
            let candidate = render_id(self.id_seq);
            if !self.holds_intent_id(&candidate) {
                return candidate;
            }
        }
    }

    /// Does any bot version hold an intent under this id? See [`Self::holds_slot_type_id`].
    fn holds_intent_id(&self, intent_id: &str) -> bool {
        self.intents.keys().any(|(_, id)| id == intent_id)
    }

    /// Mint an unused `slotId`, from the same counter and in the same `[0-9a-zA-Z]{10}`
    /// space [`Self::mint_bot_id`] draws from, and checked against the stored slots
    /// before it is handed out.
    pub fn mint_slot_id(&mut self) -> String {
        loop {
            self.id_seq += 1;
            let candidate = render_id(self.id_seq);
            if !self.holds_slot_id(&candidate) {
                return candidate;
            }
        }
    }

    /// Does any bot version hold a slot under this id? See [`Self::holds_slot_type_id`].
    fn holds_slot_id(&self, slot_id: &str) -> bool {
        self.slots.keys().any(|(_, id)| id == slot_id)
    }

    /// The number the next version of `bot_id` is created under, counting the versions
    /// the bot has had rather than the ones it still holds: a number a deleted version
    /// was created under is never handed out again. `None` when the store holds no such
    /// bot.
    pub fn mint_bot_version(&mut self, bot_id: &str) -> Option<String> {
        let bot = self.bots.get_mut(bot_id)?;
        bot.versions_created += 1;
        Some(bot.versions_created.to_string())
    }

    /// How many versions of `bot_id` the store holds — what
    /// [`VERSIONS_PER_BOT_QUOTA`] bounds.
    pub fn bot_version_count(&self, bot_id: &str) -> usize {
        self.bot_versions
            .keys()
            .filter(|(bot, _)| bot == bot_id)
            .count()
    }

    /// "The latest numerical version in use for the bot", as a `ListBots` summary reports
    /// it: the highest number a version of `bot_id` the store STILL HOLDS was created
    /// under, which is not [`Bot::versions_created`] once a version has been deleted.
    /// `None` for a bot that has only its `DRAFT` version.
    pub fn latest_bot_version(&self, bot_id: &str) -> Option<String> {
        self.bot_versions
            .keys()
            .filter(|(bot, _)| bot == bot_id)
            .filter_map(|(_, version)| version.parse::<u32>().ok())
            .max()
            .map(|latest| latest.to_string())
    }

    /// Does the store hold the version of the bot this pair names? The `DRAFT` version of
    /// a stored bot always resolves — it is the working copy every bot is created with —
    /// and a numbered one has to have been created and not since deleted.
    ///
    /// This is EXISTENCE alone, so nothing is settled: a version still reaching its
    /// terminal status is a version the store holds.
    pub fn bot_version_exists(&self, bot_id: &str, bot_version: &str) -> bool {
        self.bots.contains_key(bot_id)
            && (bot_version == DRAFT_BOT_VERSION
                || self
                    .bot_versions
                    .contains_key(&bot_version_key(bot_id, bot_version)))
    }

    /// The stored version the pair names, with any due status transition applied first —
    /// the same contract as [`Self::settled_bot`].
    pub fn settled_bot_version(
        &mut self,
        bot_id: &str,
        bot_version: &str,
    ) -> Option<&BotVersion> {
        let version = self
            .bot_versions
            .get_mut(&bot_version_key(bot_id, bot_version))?;
        version.settle();
        Some(version)
    }

    /// Drop every version of `bot_id` and answer how many went: a version cannot outlive
    /// the bot it is a version of.
    pub fn remove_bot_versions_of_bot(&mut self, bot_id: &str) -> usize {
        let before = self.bot_versions.len();
        self.bot_versions.retain(|(bot, _), _| bot != bot_id);
        before - self.bot_versions.len()
    }

    /// Store the alias Amazon Lex keeps for testing a bot, as creating the bot does, and
    /// answer the id it carries ([`TEST_BOT_ALIAS_ID`]).
    pub fn add_test_bot_alias(&mut self, bot_id: &str, now: SystemTime) -> String {
        let alias = BotAlias::test_alias(bot_id, now);
        let bot_alias_id = alias.bot_alias_id.clone();
        self.bot_aliases
            .insert(bot_alias_key(bot_id, &bot_alias_id), alias);
        bot_alias_id
    }

    /// Every alias of one bot, which is what a listing of that bot's aliases is built
    /// from. An alias of any other bot is not in it.
    pub fn bot_aliases_of_bot<'a>(&'a self, bot_id: &'a str) -> impl Iterator<Item = &'a BotAlias> + 'a {
        self.bot_aliases
            .values()
            .filter(move |alias| alias.bot_id == bot_id)
    }

    /// Drop every alias of `bot_id` and answer how many went: an alias cannot outlive
    /// the bot it is an alias of.
    pub fn remove_bot_aliases_of_bot(&mut self, bot_id: &str) -> usize {
        let before = self.bot_aliases.len();
        self.bot_aliases.retain(|(bot, _), _| bot != bot_id);
        before - self.bot_aliases.len()
    }

    /// Copy one locale of one version of a bot into another version of the same bot, the
    /// way creating a version snapshots the version it is taken from: the locale record
    /// itself and every intent, slot type and slot defined in it, each keeping the id it
    /// already has, so one id names the same resource in the source version and in the
    /// snapshot.
    ///
    /// Answers whether there was a source locale to copy.
    pub fn snapshot_locale(
        &mut self,
        bot_id: &str,
        source_version: &str,
        target_version: &str,
        locale_id: &str,
    ) -> bool {
        let Some(mut locale) = self
            .settled_bot_locale(bot_id, source_version, locale_id)
            .cloned()
        else {
            return false;
        };
        locale.bot_version = target_version.to_string();
        // The copy carries the status the source locale had settled on, and nothing is
        // owed on it: the build the source was in the middle of is the source's.
        locale.pending_status = None;
        locale.pending_failure_reasons = Vec::new();
        self.bot_locales
            .insert(bot_locale_key(bot_id, target_version, locale_id), locale);

        let intents: Vec<Intent> = self
            .intents_in_locale(bot_id, source_version, locale_id)
            .cloned()
            .collect();
        for mut intent in intents {
            intent.bot_version = target_version.to_string();
            self.intents
                .insert(versioned_key(target_version, &intent.intent_id), intent);
        }

        let slot_types: Vec<SlotType> = self
            .slot_types_in_locale(bot_id, source_version, locale_id)
            .cloned()
            .collect();
        for mut slot_type in slot_types {
            slot_type.bot_version = target_version.to_string();
            self.slot_types.insert(
                versioned_key(target_version, &slot_type.slot_type_id),
                slot_type,
            );
        }

        let slots: Vec<Slot> = self
            .slots
            .values()
            .filter(|slot| {
                slot.bot_id == bot_id
                    && slot.bot_version == source_version
                    && slot.locale_id == locale_id
            })
            .cloned()
            .collect();
        for mut slot in slots {
            slot.bot_version = target_version.to_string();
            self.slots
                .insert(versioned_key(target_version, &slot.slot_id), slot);
        }

        true
    }

    /// Is `name` already the name of a stored bot? `CreateBot` refuses a duplicate:
    /// "The bot name must be unique in the account that creates the bot."
    pub fn bot_name_taken(&self, name: &str) -> bool {
        self.bots.values().any(|bot| bot.bot_name == name)
    }

    /// The stored bot `bot_id` names, with any due status transition applied first.
    pub fn settled_bot(&mut self, bot_id: &str) -> Option<&Bot> {
        let bot = self.bots.get_mut(bot_id)?;
        bot.settle();
        Some(bot)
    }

    /// The stored bot `bot_id` names, mutably, with any due status transition applied.
    pub fn settled_bot_mut(&mut self, bot_id: &str) -> Option<&mut Bot> {
        let bot = self.bots.get_mut(bot_id)?;
        bot.settle();
        Some(bot)
    }

    /// Apply every due status transition, so a listing reports the same status a
    /// per-bot read would.
    pub fn settle_all(&mut self) {
        for bot in self.bots.values_mut() {
            bot.settle();
        }
    }

    /// Is this locale already defined for that bot version? `CreateBotLocale` refuses a
    /// second locale with the same id on the same bot version.
    pub fn bot_locale_exists(&self, bot_id: &str, bot_version: &str, locale_id: &str) -> bool {
        self.bot_locales
            .contains_key(&bot_locale_key(bot_id, bot_version, locale_id))
    }

    /// The stored locale the triple names, with any due status transition applied
    /// first.
    pub fn settled_bot_locale(
        &mut self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
    ) -> Option<&BotLocale> {
        let locale = self
            .bot_locales
            .get_mut(&bot_locale_key(bot_id, bot_version, locale_id))?;
        locale.settle();
        Some(locale)
    }

    /// The stored locale the triple names, mutably, with any due status transition
    /// applied first — what a build submission moves on.
    pub fn settled_bot_locale_mut(
        &mut self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
    ) -> Option<&mut BotLocale> {
        let locale = self
            .bot_locales
            .get_mut(&bot_locale_key(bot_id, bot_version, locale_id))?;
        locale.settle();
        Some(locale)
    }

    /// Every intent defined in one locale of one bot version.
    pub fn intents_in_locale<'a>(
        &'a self,
        bot_id: &'a str,
        bot_version: &'a str,
        locale_id: &'a str,
    ) -> impl Iterator<Item = &'a Intent> + 'a {
        self.intents
            .values()
            .filter(move |intent| intent.is_in_locale(bot_id, bot_version, locale_id))
    }

    /// Is `name` already the name of an intent in that locale? `CreateIntent` refuses a
    /// duplicate: "Intent names must be unique in the locale that contains the intent".
    pub fn intent_name_taken(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        name: &str,
    ) -> bool {
        self.intents_in_locale(bot_id, bot_version, locale_id)
            .any(|intent| intent.intent_name == name)
    }

    /// The stored intent the id names, but only when it is defined in the locale the read
    /// scopes it to; `None` otherwise, which is what a read reports as not found.
    pub fn intent(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        intent_id: &str,
    ) -> Option<&Intent> {
        self.intents
            .get(&versioned_key(bot_version, intent_id))
            .filter(|intent| intent.is_in_locale(bot_id, bot_version, locale_id))
    }

    /// The stored intent the id names, mutably, but only when it is defined in the locale
    /// the request scopes it to — what an update replaces.
    pub fn intent_mut(
        &mut self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        intent_id: &str,
    ) -> Option<&mut Intent> {
        self.intents
            .get_mut(&versioned_key(bot_version, intent_id))
            .filter(|intent| intent.is_in_locale(bot_id, bot_version, locale_id))
    }

    /// Store the intent Amazon Lex adds to a locale as it creates it
    /// ([`FALLBACK_INTENT_NAME`]), and answer the id it was minted under.
    pub fn add_fallback_intent(
        &mut self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        now: SystemTime,
    ) -> String {
        let intent_id = self.mint_intent_id();
        let intent = Intent {
            intent_id: intent_id.clone(),
            intent_name: FALLBACK_INTENT_NAME.to_string(),
            bot_id: bot_id.to_string(),
            bot_version: bot_version.to_string(),
            locale_id: locale_id.to_string(),
            intent_display_name: None,
            description: None,
            parent_intent_signature: Some(FALLBACK_INTENT_SIGNATURE.to_string()),
            sample_utterances: None,
            slot_priorities: None,
            dialog_code_hook: None,
            fulfillment_code_hook: None,
            intent_confirmation_setting: None,
            intent_closing_setting: None,
            input_contexts: None,
            output_contexts: None,
            kendra_configuration: None,
            initial_response_setting: None,
            qn_a_intent_configuration: None,
            q_in_connect_intent_configuration: None,
            creation_date_time: now,
            last_updated_date_time: now,
        };
        self.intents.insert(versioned_key(bot_version, &intent_id), intent);
        intent_id
    }

    /// Drop every intent of `bot_id`, whatever version or locale holds it, and answer how
    /// many went: an intent cannot outlive the bot it is defined in.
    pub fn remove_intents_of_bot(&mut self, bot_id: &str) -> usize {
        let before = self.intents.len();
        self.intents.retain(|_, intent| intent.bot_id != bot_id);
        before - self.intents.len()
    }

    /// Drop the one locale the triple names together with everything defined inside it —
    /// its intents, the slots of those intents and its slot types — and answer the record
    /// that went: a resource of a locale cannot outlive the locale any more than a
    /// resource of a bot outlives the bot. `None` when the store holds no such locale,
    /// which is what a delete reports as a resource that does not exist.
    ///
    /// Only the named version of the named bot is touched: the same locale id of another
    /// bot, or of another version of this one, is a different locale and keeps everything
    /// defined in it.
    pub fn remove_bot_locale(
        &mut self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
    ) -> Option<BotLocale> {
        let removed = self
            .bot_locales
            .remove(&bot_locale_key(bot_id, bot_version, locale_id))?;
        self.intents
            .retain(|_, intent| !intent.is_in_locale(bot_id, bot_version, locale_id));
        self.slot_types
            .retain(|_, slot_type| !slot_type.is_in_locale(bot_id, bot_version, locale_id));
        self.slots
            .retain(|_, slot| !slot.is_in_locale(bot_id, bot_version, locale_id));
        Some(removed)
    }

    /// Drop every locale of `bot_id`, whatever version holds it, and answer how many
    /// went: a locale cannot outlive the bot it is defined in.
    pub fn remove_bot_locales(&mut self, bot_id: &str) -> usize {
        let before = self.bot_locales.len();
        self.bot_locales.retain(|(bot, _, _), _| bot != bot_id);
        before - self.bot_locales.len()
    }

    /// Every slot type defined in one locale of one bot version.
    pub fn slot_types_in_locale<'a>(
        &'a self,
        bot_id: &'a str,
        bot_version: &'a str,
        locale_id: &'a str,
    ) -> impl Iterator<Item = &'a SlotType> + 'a {
        self.slot_types
            .values()
            .filter(move |slot_type| slot_type.is_in_locale(bot_id, bot_version, locale_id))
    }

    /// Is `name` already the name of a slot type in that locale? `CreateSlotType`
    /// refuses a duplicate: "A slot type name must be unique" where it is defined.
    pub fn slot_type_name_taken(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        name: &str,
    ) -> bool {
        self.slot_types_in_locale(bot_id, bot_version, locale_id)
            .any(|slot_type| slot_type.slot_type_name == name)
    }

    /// The stored slot type the id names, but only when it is defined in the locale the
    /// read scopes it to; `None` otherwise, which is what a read reports as not found.
    pub fn slot_type(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        slot_type_id: &str,
    ) -> Option<&SlotType> {
        self.slot_types
            .get(&versioned_key(bot_version, slot_type_id))
            .filter(|slot_type| slot_type.is_in_locale(bot_id, bot_version, locale_id))
    }

    /// Drop every slot type of `bot_id`, whatever version or locale holds it, and answer
    /// how many went: a slot type cannot outlive the bot it is defined in.
    pub fn remove_slot_types_of_bot(&mut self, bot_id: &str) -> usize {
        let before = self.slot_types.len();
        self.slot_types
            .retain(|_, slot_type| slot_type.bot_id != bot_id);
        before - self.slot_types.len()
    }

    /// Every slot one intent of one locale of one bot version holds.
    pub fn slots_in_intent<'a>(
        &'a self,
        bot_id: &'a str,
        bot_version: &'a str,
        locale_id: &'a str,
        intent_id: &'a str,
    ) -> impl Iterator<Item = &'a Slot> + 'a {
        self.slots
            .values()
            .filter(move |slot| slot.is_in_intent(bot_id, bot_version, locale_id, intent_id))
    }

    /// Is `name` already the name of a slot of that intent? A slot is defined inside one
    /// intent, and the modelled `slotName` member requires the name to be unique where
    /// the slot lives, so `CreateSlot` refuses a second slot under a name the intent
    /// already carries.
    pub fn slot_name_taken(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        intent_id: &str,
        name: &str,
    ) -> bool {
        self.slots_in_intent(bot_id, bot_version, locale_id, intent_id)
            .any(|slot| slot.slot_name == name)
    }

    /// The stored slot the id names, but only when it is held by the intent the read
    /// scopes it to; `None` otherwise, which is what a read reports as not found.
    pub fn slot(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        intent_id: &str,
        slot_id: &str,
    ) -> Option<&Slot> {
        self.slots
            .get(&versioned_key(bot_version, slot_id))
            .filter(|slot| slot.is_in_intent(bot_id, bot_version, locale_id, intent_id))
    }

    /// Drop every slot of `bot_id`, whatever version, locale or intent holds it, and
    /// answer how many went: a slot cannot outlive the bot it is defined in.
    pub fn remove_slots_of_bot(&mut self, bot_id: &str) -> usize {
        let before = self.slots.len();
        self.slots.retain(|_, slot| slot.bot_id != bot_id);
        before - self.slots.len()
    }

    /// Does `slot_type_id` resolve to a slot type a slot of that locale may elicit values
    /// for? A reference to a built-in slot type Amazon Lex supplies resolves on its own
    /// form ([`is_built_in_slot_type`]); any other id has to name a custom slot type
    /// stored in the very locale the slot is being defined in.
    pub fn slot_type_resolves(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
        slot_type_id: &str,
    ) -> bool {
        is_built_in_slot_type(slot_type_id)
            || self
                .slot_type(bot_id, bot_version, locale_id, slot_type_id)
                .is_some()
    }

    /// Why a build of that locale cannot succeed, as the `failureReasons` the failed
    /// locale reports; empty for a locale the service can build.
    ///
    /// A build validates the locale as a whole, and reports one reason per thing it found
    /// wrong:
    ///
    /// * the locale has to hold a custom intent with at least one valid sample utterance —
    ///   with nothing to recognise, Amazon Lex has no bot to build, and it says so with
    ///   [`no_utterances_failure_reason`];
    /// * every slot of every intent has to be ordered by an entry in that intent's
    ///   `slotPriorities` — the modelled `SlotConstraint` admits only `Required` and
    ///   `Optional`, both of which Amazon Lex elicits in the order that list fixes — and an
    ///   intent holding slots nothing orders is reported with
    ///   [`unprioritised_slots_failure_reason`].
    ///
    /// The reasons are ordered the way the locale acquired what they are about, so the
    /// list a read reports does not depend on how the store happens to hold its records.
    pub fn build_failure_reasons(
        &self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
    ) -> Vec<String> {
        let mut intents: Vec<&Intent> = self
            .intents_in_locale(bot_id, bot_version, locale_id)
            .collect();
        intents.sort_by(|left, right| {
            left.creation_date_time
                .cmp(&right.creation_date_time)
                .then_with(|| left.intent_name.cmp(&right.intent_name))
        });

        let mut reasons = Vec::new();
        let buildable = intents
            .iter()
            .any(|intent| intent.is_custom() && intent.has_valid_utterance());
        if !buildable {
            reasons.push(no_utterances_failure_reason(locale_id));
        }
        for intent in intents {
            let unordered: Vec<String> = self
                .unprioritised_slots(intent)
                .into_iter()
                .map(|slot| slot.slot_name.clone())
                .collect();
            if !unordered.is_empty() {
                reasons.push(unprioritised_slots_failure_reason(
                    &unordered,
                    &intent.intent_name,
                ));
            }
        }
        reasons
    }

    /// The slots of `intent` that no entry of its `slotPriorities` orders, in the order
    /// the intent acquired them.
    ///
    /// Nothing adds a slot to `slotPriorities` as it is created — ordering the slots of an
    /// intent is `UpdateIntent`'s — so this is what a build has to check.
    pub fn unprioritised_slots<'a>(&'a self, intent: &'a Intent) -> Vec<&'a Slot> {
        let mut unordered: Vec<&Slot> = self
            .slots_in_intent(
                &intent.bot_id,
                &intent.bot_version,
                &intent.locale_id,
                &intent.intent_id,
            )
            .filter(|slot| !intent.orders_slot(&slot.slot_id))
            .collect();
        unordered.sort_by(|left, right| {
            left.creation_date_time
                .cmp(&right.creation_date_time)
                .then_with(|| left.slot_name.cmp(&right.slot_name))
        });
        unordered
    }
}

/// A stored instant as the wire timestamp the modelled `Timestamp` members carry.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The opaque `nextToken` that resumes a listing at the entry whose sort key is `at`.
///
/// Hex-encoded, so the token is opaque to the caller and carries no internal key in
/// clear text — the same codec `services/service-com-amazonaws-proton-awsproton20200720`
/// uses for its listings.
pub fn encode_page_token(at: &str) -> String {
    hex::encode(at)
}

/// The sort key an opaque `nextToken` resumes at, or `None` when the token was not
/// minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// Render `seq` into the `[0-9a-zA-Z]{10}` space the modelled `Id` shape allows.
///
/// The counter is mixed first (SplitMix64's finalizer) so consecutive ids do not read
/// as consecutive numbers, the way a real Lex bot id does not; the mix is a bijection
/// on `u64`, and the result is reduced into `62^10` values, so distinct counters can
/// only collide after that reduction — which [`mint_bot_id`]'s re-draw covers.
fn render_id(seq: u64) -> String {
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    const LEN: usize = 10;

    let mut z = seq.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;

    let space = (ALPHABET.len() as u64).pow(LEN as u32);
    let mut value = z % space;
    let mut digits = [b'0'; LEN];
    for slot in digits.iter_mut().rev() {
        *slot = ALPHABET[(value % ALPHABET.len() as u64) as usize];
        value /= ALPHABET.len() as u64;
    }
    String::from_utf8(digits.to_vec()).expect("the id alphabet is ASCII")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State>)
}
