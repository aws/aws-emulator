// SPDX-License-Identifier: CC0-1.0

//! The archive an export job assembles, and the address that archive is served from.
//!
//! `CreateExport` "creates a zip archive containing the contents of a bot or a bot locale.
//! The archive contains a directory structure that contains JSON files that define the
//! bot" — so the archive is ASSEMBLED HERE, out of the very records this control plane
//! holds for the resource the job names: the bot, the locales of the version exported,
//! the intents of each locale, the slots of each intent and the locale's custom slot
//! types. Nothing is read from outside this store, so what a download hands out is the
//! definition a read of those resources reports.
//!
//! The archive is assembled by the call that records the job, which is what makes it a
//! SNAPSHOT: a later change to the bot does not reach a job already submitted, exactly as
//! a later change does not reach an archive AWS has already written.
//!
//! ## The address it is served from
//!
//! `DescribeExport` reports `downloadUrl`, "a pre-signed S3 URL that points to the bot or
//! bot locale archive". A URL this emulator hands out has to be one a caller can follow,
//! and no production AWS host resolves to it, so the address is PRESIGNED against the base
//! URL this emulator is reached on ([`export_archive_url`]) and the archive's own object
//! path travels in it ([`export_archive_path`]), a path space no modelled operation binds.
//! `crate::service`'s archive route reads that path back through
//! [`export_archive_id`] and answers it with these bytes, so the URL this service mints
//! and the request it serves cannot disagree about which archive is named.
//!
//! ## What it costs
//!
//! One archive is capped twice, by the number of definition files and by their bytes
//! ([`EXPORT_ARCHIVE_MAX_FILES`], [`EXPORT_ARCHIVE_MAX_BYTES`]), and both caps are charged
//! as each file is built, so an export of a resource past either is refused before the rest
//! of its tree is assembled. Reading the store and compressing what was read are both
//! synchronous CPU work, so `crate::ops::create_export` runs them on the blocking pool
//! rather than on the worker serving the request. The bytes are held as a refcounted
//! buffer, so announcing or serving one costs no copy of it.

use std::io::Write;

use ::aws_smithy_schema::codec::{Codec, FinishSerializer};
use ::aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::json::JsonCodec;
use ::serde_json::{json, Map, Value};

use super::*;

/// One file of an archive: where it sits in the tree, and the definition it carries.
#[derive(Debug, Clone)]
pub struct ExportArchiveFile {
    pub path: String,
    /// The definition, rendered. Held as the text the archive carries rather than as the
    /// document it came from, because that text is what both caps one archive has are
    /// charged against as it is built (see
    /// [`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::export_archive_files`]).
    pub document: String,
}

/// The files of one archive as they are built, with the caps charged against them.
///
/// Both caps are spent BEFORE a file joins the archive, so what this holds is never more
/// than one document past them: the bound dominates the growth rather than following it.
struct ExportArchiveFiles {
    files: Vec<ExportArchiveFile>,
    /// How many more files may join ([`EXPORT_ARCHIVE_MAX_FILES`]).
    files_left: usize,
    /// How many more bytes of definitions may join ([`EXPORT_ARCHIVE_MAX_BYTES`]).
    bytes_left: usize,
}

impl ExportArchiveFiles {
    /// An archive with nothing in it yet and both caps unspent.
    fn empty() -> Self {
        Self {
            files: Vec::new(),
            files_left: EXPORT_ARCHIVE_MAX_FILES,
            bytes_left: EXPORT_ARCHIVE_MAX_BYTES,
        }
    }

    /// Render `document` and add it at `path`, or refuse the archive when either cap is
    /// spent.
    fn add(&mut self, path: String, document: &Value) -> Result<(), ExportArchiveRefused> {
        self.files_left = self
            .files_left
            .checked_sub(1)
            .ok_or(ExportArchiveRefused::TooManyFiles)?;
        let rendered = ::serde_json::to_string_pretty(document)
            .map_err(|_| ExportArchiveRefused::Unwritable)?;
        self.bytes_left = self
            .bytes_left
            .checked_sub(rendered.len())
            .ok_or(ExportArchiveRefused::TooLarge)?;
        self.files.push(ExportArchiveFile {
            path,
            document: rendered,
        });
        Ok(())
    }

    /// The files, in the order they were added.
    fn into_files(self) -> Vec<ExportArchiveFile> {
        self.files
    }
}

/// Why an archive was not assembled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportArchiveRefused {
    /// The store holds no bot for the resource the archive was asked for, so there is
    /// nothing to assemble: the create reports it as the resource it does not hold.
    Unresolved,
    /// The resource's definition files are more than [`EXPORT_ARCHIVE_MAX_FILES`], so no
    /// job is recorded for them: the create answers its modelled quota error instead.
    TooManyFiles,
    /// They are past [`EXPORT_ARCHIVE_MAX_BYTES`], under the same contract as
    /// [`ExportArchiveRefused::TooManyFiles`].
    TooLarge,
    /// The archive could not be written at all, which nothing about a stored record
    /// reaches.
    Unwritable,
}

/// How many definition files one archive may hold.
///
/// A CHOSEN bound, and the first of the two that cap what a single export assembles: the
/// records an archive is built from are each bounded by one of this service's own quotas
/// ([`BOTS_PER_ACCOUNT_QUOTA`], [`INTENTS_PER_LOCALE_QUOTA`], [`SLOTS_PER_INTENT_QUOTA`],
/// [`SLOT_TYPES_PER_LOCALE_QUOTA`]), but their PRODUCT over the locales of one bot version
/// is not small, and this is the cap on that product. It is spent as the files are built
/// ([`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::export_archive_files`]), so an
/// export past it is refused before the rest of the tree is built, and far above the files
/// any bot a caller authors through this API has.
pub const EXPORT_ARCHIVE_MAX_FILES: usize = 20_000;

/// How many bytes of definition files one archive may hold.
///
/// The second CHOSEN bound, charged the same way and at the same place as
/// [`EXPORT_ARCHIVE_MAX_FILES`], and the one that caps the bytes rather than the count: an
/// export whose definitions pass it records no job at all and answers the modelled quota
/// error. It bounds what ONE request assembles; what the stored jobs hold together is
/// [`EXPORT_ARCHIVES_HELD_BYTES`].
pub const EXPORT_ARCHIVE_MAX_BYTES: usize = 32 * 1024 * 1024;

/// The media type an export archive is served under: the zip archive it is.
pub const EXPORT_ARCHIVE_MEDIA_TYPE: &str = "application/zip";

/// The file every archive opens with, naming what the archive holds.
pub const EXPORT_ARCHIVE_MANIFEST: &str = "manifest.json";

/// The schema version the manifest states for the definition files beside it. One version
/// is published, and every file in an archive this service assembles is written to it.
pub const EXPORT_ARCHIVE_SCHEMA_VERSION: &str = "1.0";

/// The file one bot's definition is written to, under the archive's own directory for that
/// bot.
pub const BOT_DEFINITION_FILE: &str = "Bot.json";

/// The directory the locales of the exported bot version live under.
pub const BOT_LOCALES_DIRECTORY: &str = "BotLocales";

/// The file one locale's definition is written to, under that locale's directory.
pub const BOT_LOCALE_DEFINITION_FILE: &str = "BotLocale.json";

/// The directory the intents of one locale live under.
pub const INTENTS_DIRECTORY: &str = "Intents";

/// The file one intent's definition is written to, under that intent's directory.
pub const INTENT_DEFINITION_FILE: &str = "Intent.json";

/// The directory the slots of one intent live under.
pub const SLOTS_DIRECTORY: &str = "Slots";

/// The file one slot's definition is written to, under that slot's directory.
pub const SLOT_DEFINITION_FILE: &str = "Slot.json";

/// The directory the custom slot types of one locale live under.
pub const SLOT_TYPES_DIRECTORY: &str = "SlotTypes";

/// The file one custom slot type's definition is written to, under its own directory.
pub const SLOT_TYPE_DEFINITION_FILE: &str = "SlotType.json";

/// How long the address `DescribeExport` reports for an archive stays valid: the modelled
/// `downloadUrl` member documents it — "The URL is only available for 5 minutes after
/// calling the DescribeExport operation".
pub const EXPORT_ARCHIVE_URL_EXPIRY: Duration = Duration::from_secs(5 * 60);

/// The first path segment every archive address carries, which is what makes a request one
/// the archive route serves rather than one of this model's own operations: no operation of
/// this service binds a URI under it
/// (`smithy_query(query="digest")` lists every bound route).
pub const EXPORT_ARCHIVE_PATH_SEGMENT: &str = "lexv2-export-archives";

/// The suffix an archive object's path ends in — the zip archive it addresses.
const EXPORT_ARCHIVE_PATH_SUFFIX: &str = ".zip";

/// The resource an archive is assembled from: one version of one bot, or one locale of
/// that version.
///
/// Built by `CreateExport` once the `resourceSpecification` has resolved against the store,
/// so an archive is only ever assembled for a resource that was there to export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedBotResource {
    pub bot_id: String,
    pub bot_version: String,
    /// The one locale exported, or `None` for an export of the whole bot version.
    pub locale_id: Option<String>,
}

impl ExportedBotResource {
    /// An export of a whole bot version.
    #[must_use]
    pub fn bot(bot_id: &str, bot_version: &str) -> Self {
        Self {
            bot_id: bot_id.to_string(),
            bot_version: bot_version.to_string(),
            locale_id: None,
        }
    }

    /// An export of one locale of one bot version.
    #[must_use]
    pub fn locale(bot_id: &str, bot_version: &str, locale_id: &str) -> Self {
        Self {
            locale_id: Some(locale_id.to_string()),
            ..Self::bot(bot_id, bot_version)
        }
    }

    /// The kind of resource this is, as the `ExportResourceType` filter of `ListExports`
    /// names it — the same two values a stored job reports through
    /// [`Export::resource_type`].
    #[must_use]
    pub fn resource_type(&self) -> &'static str {
        if self.locale_id.is_some() {
            EXPORT_RESOURCE_TYPE_BOT_LOCALE
        } else {
            EXPORT_RESOURCE_TYPE_BOT
        }
    }
}

impl ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State {
    /// The definition files an export of `exported` carries, each at its own path in the
    /// archive's tree and each already rendered, or why the archive is refused.
    ///
    /// Every file is rendered and CHARGED as it is built, against the two caps one archive
    /// has — [`EXPORT_ARCHIVE_MAX_FILES`] and [`EXPORT_ARCHIVE_MAX_BYTES`] — so the bound
    /// precedes the growth rather than following it: this holds at most one document more
    /// than those caps allow, whatever the stored records of the exported version add up
    /// to, and an export past either is refused before the rest of the tree is built at
    /// all.
    ///
    /// Synchronous CPU work over the store, so a caller runs it on the blocking pool (see
    /// `crate::ops::create_export`) rather than on the worker serving the request.
    ///
    /// The order is fixed — locales by locale id, then each locale's intents, each
    /// intent's slots and each locale's slot types by name — so two exports of an
    /// unchanged resource assemble the same archive.
    pub fn export_archive_files(
        &self,
        exported: &ExportedBotResource,
        file_format: &str,
    ) -> Result<Vec<ExportArchiveFile>, ExportArchiveRefused> {
        let bot = self
            .bots
            .get(&exported.bot_id)
            .ok_or(ExportArchiveRefused::Unresolved)?;
        let root = path_segment(&bot.bot_name);
        let mut files = ExportArchiveFiles::empty();
        files.add(
            EXPORT_ARCHIVE_MANIFEST.to_string(),
            &manifest(file_format, exported.resource_type()),
        )?;
        // An export of the whole bot version carries the bot's own definition; an export
        // of ONE locale is "only the definition of a single bot locale", so it carries
        // that locale's subtree and not the bot's definition.
        if exported.locale_id.is_none() {
            files.add(
                format!("{root}/{BOT_DEFINITION_FILE}"),
                &bot_definition(bot, &exported.bot_version),
            )?;
        }
        for locale in self.exported_locales(exported) {
            let locale_root =
                format!("{root}/{BOT_LOCALES_DIRECTORY}/{}", path_segment(&locale.locale_id));
            files.add(
                format!("{locale_root}/{BOT_LOCALE_DEFINITION_FILE}"),
                &bot_locale_definition(locale),
            )?;
            let mut intents: Vec<&Intent> = self
                .intents_in_locale(&exported.bot_id, &exported.bot_version, &locale.locale_id)
                .collect();
            intents.sort_by(|left, right| left.intent_name.cmp(&right.intent_name));
            for intent in intents {
                let intent_root = format!(
                    "{locale_root}/{INTENTS_DIRECTORY}/{}",
                    path_segment(&intent.intent_name)
                );
                files.add(
                    format!("{intent_root}/{INTENT_DEFINITION_FILE}"),
                    &intent_definition(intent),
                )?;
                let mut slots: Vec<&Slot> = self
                    .slots_in_intent(
                        &exported.bot_id,
                        &exported.bot_version,
                        &locale.locale_id,
                        &intent.intent_id,
                    )
                    .collect();
                slots.sort_by(|left, right| left.slot_name.cmp(&right.slot_name));
                for slot in slots {
                    files.add(
                        format!(
                            "{intent_root}/{SLOTS_DIRECTORY}/{}/{SLOT_DEFINITION_FILE}",
                            path_segment(&slot.slot_name)
                        ),
                        &slot_definition(slot),
                    )?;
                }
            }
            let mut slot_types: Vec<&SlotType> = self
                .slot_types_in_locale(&exported.bot_id, &exported.bot_version, &locale.locale_id)
                .collect();
            slot_types.sort_by(|left, right| left.slot_type_name.cmp(&right.slot_type_name));
            for slot_type in slot_types {
                files.add(
                    format!(
                        "{locale_root}/{SLOT_TYPES_DIRECTORY}/{}/{SLOT_TYPE_DEFINITION_FILE}",
                        path_segment(&slot_type.slot_type_name)
                    ),
                    &slot_type_definition(slot_type),
                )?;
            }
        }
        Ok(files.into_files())
    }

    /// The locales an export of `exported` carries, in locale-id order: every locale of
    /// the bot version for an export of the whole version, and the one named for an export
    /// of a single locale.
    fn exported_locales(&self, exported: &ExportedBotResource) -> Vec<&BotLocale> {
        let mut locales: Vec<&BotLocale> = self
            .bot_locales
            .values()
            .filter(|locale| {
                locale.bot_id == exported.bot_id && locale.bot_version == exported.bot_version
            })
            .filter(|locale| {
                exported
                    .locale_id
                    .as_ref()
                    .is_none_or(|wanted| *wanted == locale.locale_id)
            })
            .collect();
        locales.sort_by(|left, right| left.locale_id.cmp(&right.locale_id));
        locales
    }
}

/// The path an archive object is addressed at: the published segment, the account whose
/// exports this store holds, and the job's own id.
///
/// ONE place states the form, so the address [`export_archive_url`] signs and the request
/// `crate::service` claims back cannot disagree about it (see [`export_archive_id`]).
#[must_use]
pub fn export_archive_path(export_id: &str) -> String {
    format!(
        "/{EXPORT_ARCHIVE_PATH_SEGMENT}/{account}/{export_id}{EXPORT_ARCHIVE_PATH_SUFFIX}",
        account = ::engine::emulator_account_id(),
    )
}

/// The export whose archive `path` addresses, or `None` when it addresses none.
///
/// Read from the SAME parts [`export_archive_path`] lays out: the published segment, this
/// emulator's account, and an id of the modelled `Id` shape (len 10..10, pattern
/// `^[0-9a-zA-Z]+$`, `smithy_query(query="shape",shape="Id")`). Whether a job is HELD under
/// that id is not decided here but where the jobs are, so a URL for an export the store has
/// dropped is answered as the archive it no longer holds.
#[must_use]
pub fn export_archive_id(path: &str) -> Option<String> {
    let rest = path.strip_prefix(&format!("/{EXPORT_ARCHIVE_PATH_SEGMENT}/"))?;
    let (account, object) = rest.split_once('/')?;
    if account != ::engine::emulator_account_id() {
        return None;
    }
    let export_id = object.strip_suffix(EXPORT_ARCHIVE_PATH_SUFFIX)?;
    let modelled_id =
        export_id.len() == 10 && export_id.bytes().all(|byte| byte.is_ascii_alphanumeric());
    modelled_id.then(|| export_id.to_string())
}

/// Whether `path` addresses an export archive at all — the published segment it opens with.
///
/// The SEGMENT is what makes a request the archive route's rather than a modelled
/// operation's: a URL this service announced is addressed to its own endpoint, which every
/// modelled operation is addressed to as well. The id is no part of that reading, so an
/// address naming an archive this store does not hold is answered by the route as the
/// archive it does not hold, rather than by the router as a request naming no operation.
#[must_use]
pub fn addresses_export_archive(path: &str) -> bool {
    path.trim_start_matches('/')
        .split('/')
        .next()
        .is_some_and(|segment| segment == EXPORT_ARCHIVE_PATH_SEGMENT)
}

/// The address `DescribeExport` reports for one job's archive, pre-signed against the base
/// URL this emulator is reached on, or `None` for a request that cannot be signed.
///
/// Signed through the framework's own URL signer, which is what makes the address both a
/// pre-signed URL and one this emulator ANSWERS: it signs for this service's signing name
/// and for the emulator's current base URL, so the announced host is the host the caller
/// reached this service on rather than a host only AWS serves.
#[must_use]
pub fn export_archive_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    export_id: &str,
) -> Option<String> {
    let path = export_archive_path(export_id);
    let request = smithy_server_runtime::SignedUrlRequest::new(
        smithy_server_runtime::http::Method::GET,
        ctx.identity().signing_name(),
        &path,
    );
    match ctx.url_signer().sign(request, EXPORT_ARCHIVE_URL_EXPIRY) {
        Ok(signed) => Some(signed.as_str().to_string()),
        Err(error) => {
            ::tracing::warn!(%error, "an export archive URL could not be signed");
            None
        }
    }
}

/// What the archive's manifest states: the schema its definition files are written to, the
/// file format the job was asked for, and the kind of resource the archive holds.
fn manifest(file_format: &str, resource_type: &str) -> Value {
    json!({
        "metadata": {
            "schemaVersion": EXPORT_ARCHIVE_SCHEMA_VERSION,
            "fileFormat": file_format,
            "resourceType": resource_type,
        }
    })
}

/// One bot's definition, as the archive writes it: what it is called, which bot and which
/// version of it this is, and the settings the stored record holds for it.
fn bot_definition(bot: &Bot, bot_version: &str) -> Value {
    let mut definition = Map::new();
    definition.insert("name".to_string(), json!(bot.bot_name));
    definition.insert("identifier".to_string(), json!(bot.bot_id));
    definition.insert("version".to_string(), json!(bot_version));
    insert_text(&mut definition, "description", bot.description.as_ref());
    definition.insert(
        "dataPrivacy".to_string(),
        shape_json(&DataPrivacy::SCHEMA, &bot.data_privacy),
    );
    definition.insert(
        "idleSessionTTLInSeconds".to_string(),
        json!(bot.idle_session_ttl_in_seconds),
    );
    insert_text(&mut definition, "botType", bot.bot_type.as_ref());
    insert_list(
        &mut definition,
        "botMembers",
        &BotMember::SCHEMA,
        bot.bot_members.as_deref().unwrap_or_default(),
    );
    insert_optional(
        &mut definition,
        "errorLogSettings",
        &ErrorLogSettings::SCHEMA,
        bot.error_log_settings.as_ref(),
    );
    Value::Object(definition)
}

/// One locale's definition, under the same contract as [`bot_definition`]: the locale it
/// is for, and every setting the stored record holds for it.
///
/// The confidence threshold is written under the name the EXPORT FORMAT gives it,
/// `nluConfidenceThreshold`, which is not the name the API's own `nluIntentConfidenceThreshold`
/// member carries: a definition file is read by the import side, not by an API client.
fn bot_locale_definition(locale: &BotLocale) -> Value {
    let mut definition = Map::new();
    definition.insert("name".to_string(), json!(locale.locale_name));
    definition.insert("identifier".to_string(), json!(locale.locale_id));
    insert_text(&mut definition, "description", locale.description.as_ref());
    definition.insert(
        "nluConfidenceThreshold".to_string(),
        json!(locale.nlu_intent_confidence_threshold),
    );
    insert_optional(
        &mut definition,
        "voiceSettings",
        &VoiceSettings::SCHEMA,
        locale.voice_settings.as_ref(),
    );
    insert_optional(
        &mut definition,
        "unifiedSpeechSettings",
        &UnifiedSpeechSettings::SCHEMA,
        locale.unified_speech_settings.as_ref(),
    );
    insert_optional(
        &mut definition,
        "audioFillerSettings",
        &AudioFillerSettings::SCHEMA,
        locale.audio_filler_settings.as_ref(),
    );
    insert_optional(
        &mut definition,
        "speechRecognitionSettings",
        &SpeechRecognitionSettings::SCHEMA,
        locale.speech_recognition_settings.as_ref(),
    );
    insert_optional(
        &mut definition,
        "generativeAISettings",
        &GenerativeAISettings::SCHEMA,
        locale.generative_ai_settings.as_ref(),
    );
    insert_optional(
        &mut definition,
        "speakerDiarizationSettings",
        &SpeakerDiarizationSettings::SCHEMA,
        locale.speaker_diarization_settings.as_ref(),
    );
    insert_text(
        &mut definition,
        "speechDetectionSensitivity",
        locale.speech_detection_sensitivity.as_ref(),
    );
    Value::Object(definition)
}

/// One intent's definition, under the same contract as [`bot_definition`].
fn intent_definition(intent: &Intent) -> Value {
    let mut definition = Map::new();
    definition.insert("name".to_string(), json!(intent.intent_name));
    definition.insert("identifier".to_string(), json!(intent.intent_id));
    insert_text(
        &mut definition,
        "intentDisplayName",
        intent.intent_display_name.as_ref(),
    );
    insert_text(&mut definition, "description", intent.description.as_ref());
    insert_text(
        &mut definition,
        "parentIntentSignature",
        intent.parent_intent_signature.as_ref(),
    );
    insert_list(
        &mut definition,
        "sampleUtterances",
        &SampleUtterance::SCHEMA,
        intent.sample_utterances.as_deref().unwrap_or_default(),
    );
    insert_list(
        &mut definition,
        "slotPriorities",
        &SlotPriority::SCHEMA,
        intent.slot_priorities.as_deref().unwrap_or_default(),
    );
    insert_list(
        &mut definition,
        "inputContexts",
        &InputContext::SCHEMA,
        intent.input_contexts.as_deref().unwrap_or_default(),
    );
    insert_list(
        &mut definition,
        "outputContexts",
        &OutputContext::SCHEMA,
        intent.output_contexts.as_deref().unwrap_or_default(),
    );
    insert_optional(
        &mut definition,
        "dialogCodeHook",
        &DialogCodeHookSettings::SCHEMA,
        intent.dialog_code_hook.as_ref(),
    );
    insert_optional(
        &mut definition,
        "fulfillmentCodeHook",
        &FulfillmentCodeHookSettings::SCHEMA,
        intent.fulfillment_code_hook.as_ref(),
    );
    insert_optional(
        &mut definition,
        "intentConfirmationSetting",
        &IntentConfirmationSetting::SCHEMA,
        intent.intent_confirmation_setting.as_ref(),
    );
    insert_optional(
        &mut definition,
        "intentClosingSetting",
        &IntentClosingSetting::SCHEMA,
        intent.intent_closing_setting.as_ref(),
    );
    insert_optional(
        &mut definition,
        "kendraConfiguration",
        &KendraConfiguration::SCHEMA,
        intent.kendra_configuration.as_ref(),
    );
    insert_optional(
        &mut definition,
        "initialResponseSetting",
        &InitialResponseSetting::SCHEMA,
        intent.initial_response_setting.as_ref(),
    );
    insert_optional(
        &mut definition,
        "qnAIntentConfiguration",
        &QnAIntentConfiguration::SCHEMA,
        intent.qn_a_intent_configuration.as_ref(),
    );
    insert_optional(
        &mut definition,
        "qInConnectIntentConfiguration",
        &QInConnectIntentConfiguration::SCHEMA,
        intent.q_in_connect_intent_configuration.as_ref(),
    );
    Value::Object(definition)
}

/// One slot's definition, under the same contract as [`bot_definition`].
fn slot_definition(slot: &Slot) -> Value {
    let mut definition = Map::new();
    definition.insert("name".to_string(), json!(slot.slot_name));
    definition.insert("identifier".to_string(), json!(slot.slot_id));
    insert_text(&mut definition, "description", slot.description.as_ref());
    insert_text(&mut definition, "slotTypeId", slot.slot_type_id.as_ref());
    definition.insert(
        "valueElicitationSetting".to_string(),
        shape_json(
            &SlotValueElicitationSetting::SCHEMA,
            &slot.value_elicitation_setting,
        ),
    );
    insert_optional(
        &mut definition,
        "obfuscationSetting",
        &ObfuscationSetting::SCHEMA,
        slot.obfuscation_setting.as_ref(),
    );
    insert_optional(
        &mut definition,
        "multipleValuesSetting",
        &MultipleValuesSetting::SCHEMA,
        slot.multiple_values_setting.as_ref(),
    );
    insert_optional(
        &mut definition,
        "subSlotSetting",
        &SubSlotSetting::SCHEMA,
        slot.sub_slot_setting.as_ref(),
    );
    Value::Object(definition)
}

/// One custom slot type's definition, under the same contract as [`bot_definition`].
fn slot_type_definition(slot_type: &SlotType) -> Value {
    let mut definition = Map::new();
    definition.insert("name".to_string(), json!(slot_type.slot_type_name));
    definition.insert("identifier".to_string(), json!(slot_type.slot_type_id));
    insert_text(
        &mut definition,
        "description",
        slot_type.description.as_ref(),
    );
    insert_text(
        &mut definition,
        "parentSlotTypeSignature",
        slot_type.parent_slot_type_signature.as_ref(),
    );
    insert_list(
        &mut definition,
        "slotTypeValues",
        &SlotTypeValue::SCHEMA,
        slot_type.slot_type_values.as_deref().unwrap_or_default(),
    );
    insert_optional(
        &mut definition,
        "valueSelectionSetting",
        &SlotValueSelectionSetting::SCHEMA,
        slot_type.value_selection_setting.as_ref(),
    );
    insert_optional(
        &mut definition,
        "externalSourceSetting",
        &ExternalSourceSetting::SCHEMA,
        slot_type.external_source_setting.as_ref(),
    );
    insert_optional(
        &mut definition,
        "compositeSlotTypeSetting",
        &CompositeSlotTypeSetting::SCHEMA,
        slot_type.composite_slot_type_setting.as_ref(),
    );
    Value::Object(definition)
}

/// One modelled shape as the JSON a definition file carries it as.
///
/// The shape's OWN schema renders it, so every member a definition file states is named
/// and typed exactly as this service's model declares it, and a shape nobody wrote a
/// renderer for cannot drift from the model.
fn shape_json(schema: &Schema, value: &dyn SerializableStruct) -> Value {
    let mut serializer = JsonCodec::rest_json().create_serializer();
    if serializer.write_struct(schema, value).is_err() {
        return Value::Null;
    }
    ::serde_json::from_slice(&serializer.finish()).unwrap_or(Value::Null)
}

/// Write `name` carrying `value`'s JSON, for a member the stored record holds; a member it
/// does not hold is not written at all, the way an absent member is not on the wire.
fn insert_optional<S: SerializableStruct>(
    into: &mut Map<String, Value>,
    name: &str,
    schema: &Schema,
    value: Option<&S>,
) {
    if let Some(value) = value {
        into.insert(name.to_string(), shape_json(schema, value));
    }
}

/// [`insert_optional`] for a LIST member: written when the record holds entries, and left
/// out entirely when it holds none — a record that holds no list at all passes the empty
/// one, which is the same thing on the wire.
fn insert_list<S: SerializableStruct>(
    into: &mut Map<String, Value>,
    name: &str,
    schema: &Schema,
    values: &[S],
) {
    if !values.is_empty() {
        into.insert(
            name.to_string(),
            Value::Array(
                values
                    .iter()
                    .map(|value| shape_json(schema, value))
                    .collect(),
            ),
        );
    }
}

/// [`insert_optional`] for a plain string member.
fn insert_text(into: &mut Map<String, Value>, name: &str, value: Option<&String>) {
    if let Some(value) = value {
        into.insert(name.to_string(), json!(value));
    }
}

/// `name` as one path segment of the archive.
///
/// The directories of the tree are named after the resources themselves, and every
/// character outside the set a file name is written with is replaced: no stored name can
/// hold a path separator or a parent reference, so no entry of an archive this service
/// assembles can address anything outside the archive's own tree, whatever a name carried.
fn path_segment(name: &str) -> String {
    let segment: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if segment.chars().all(|character| character == '.') {
        // `.` and `..` name a directory rather than a resource, and a name of dots alone
        // would address one; nothing a caller can store spells it, since every stored name
        // carries a letter or a digit.
        return "_".repeat(segment.chars().count().max(1));
    }
    segment
}

/// The zip archive holding each definition file at its own path, or why it could not be
/// written.
///
/// The files arrive already rendered and already capped
/// ([`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::export_archive_files`]), so
/// this only compresses them: the bytes it holds are bounded by the same two caps, and a
/// caller runs it on the blocking pool beside the read (see `crate::ops::create_export`).
///
/// Every entry's timestamp is fixed rather than read off the clock, so one job's archive is
/// the same bytes however often it is downloaded, and two exports of an unchanged resource
/// are the same archive.
pub fn export_archive_bytes(
    files: &[ExportArchiveFile],
) -> Result<::bytes::Bytes, ExportArchiveRefused> {
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    let options =
        ::zip::write::SimpleFileOptions::default().last_modified_time(::zip::DateTime::default());
    for file in files {
        archive
            .start_file(&file.path, options)
            .map_err(|_| ExportArchiveRefused::Unwritable)?;
        archive
            .write_all(file.document.as_bytes())
            .map_err(|_| ExportArchiveRefused::Unwritable)?;
    }
    archive
        .finish()
        .map(|written| ::bytes::Bytes::from(written.into_inner()))
        .map_err(|_| ExportArchiveRefused::Unwritable)
}
