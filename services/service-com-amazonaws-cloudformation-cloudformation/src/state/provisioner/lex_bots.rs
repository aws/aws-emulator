// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the Amazon Lex V2 model-building plane: the bot, the
//! immutable VERSION of one and the ALIAS that points at a version. One file of [`super`]'s
//! per-type arms, dispatched to by its `Provisioner::LexBot` variant, which carries which of
//! the three the declaration is ([`Kind`]).
//!
//! Each of the three is created by that service's own `Create…` call and removed by its own
//! `Delete…`, over the one in-process transport every arm here rides on
//! ([`call_rest_json_named`]), so a bot a stack deploys is the bot `DescribeBot` reports, the
//! version it cut is the one `DescribeBotVersion` reports, and the alias it published is the
//! one `DescribeBotAlias` resolves.
//!
//! ## What each create MINTS
//!
//! Amazon Lex assigns every one of these identities, so none of them is a name a template
//! could state and the values this emulator's naming rules compute for one are only stand-ins
//! until the create has answered
//! ([`crate::state::template::identity_is_service_assigned`] names all three):
//!
//! * a bot's `botId` is `^[0-9a-zA-Z]{10}$`, which is what every bot-addressed call takes and
//!   what "Ref returns" for the type, and the ARN this emulator publishes for it is built
//!   around that id;
//! * a version's number is the count of versions created of the bot ("When you create the
//!   first version of a bot, Amazon Lex sets the version number to 1"), a `NumericalBotVersion`
//!   of `^[0-9]+$` — the value `Fn::GetAtt` reads off the type as `BotVersion` and the value
//!   an alias's own `botVersion` input takes;
//! * an alias's `botAliasId` is drawn from the same 10-character space, and the pair
//!   `(botAliasId, botId)` is what addresses one.
//!
//! ## What a declaration's properties are
//!
//! Every property of these types is the API member of the same name with a lower-cased
//! initial ([`lex_members`]) but for the ones below, which are mapped member by
//! member:
//!
//! * the NAME each type states (`Name`, `BotAliasName`) is the create's own `botName` /
//!   `botAliasName`, and a locale's `NluConfidenceThreshold` is `CreateBotLocale`'s
//!   `nluIntentConfidenceThreshold`;
//! * a bot's `BotLocales`, and each locale's `SlotTypes` and `Intents` (with each intent's
//!   `Slots`), are no members of `CreateBot` at all: they are the resources a client creates
//!   INSIDE the bot, one call each, which is what [`create_bot_locales`] makes;
//! * `BotVersionLocaleSpecification` and `BotAliasLocaleSettings` are LISTS keyed by a
//!   `LocaleId` entry in a template and MAPS keyed by the locale id on the wire, so each is
//!   turned into the map its operation takes ([`keyed_by_locale`]);
//! * an intent's `SlotPriorities` orders its slots by `SlotName` while the API orders them by
//!   `slotId`, and a slot names its type by `SlotTypeName` while `CreateSlot` takes a
//!   `slotTypeId` — so the ids the slot-type and slot creates ANSWERED are what those members
//!   carry ([`created_ids`]).
//!
//! An enum-valued property is a VALUE mapping rather than a member one: CloudFormation
//! documents such a property's allowed values in its own spelling, which for these types is
//! the upper-snake form of the value the API's enum admits, so each reaches the call spelled
//! the way its own enum spells it ([`LEX_ENUM_MEMBERS`]).
//!
//! `TestBotAliasSettings` is a property no member of `CreateBot` carries: it configures the
//! alias Amazon Lex keeps for testing a bot, which that service creates with the bot and whose
//! settings only `UpdateBotAlias` writes, so it is not applied here and the test alias carries
//! the settings that service gives it.

use super::*;

/// Which of the three Amazon Lex V2 resources a declaration is, carried by the one
/// `Provisioner` variant these arms are reached through so the three share one set of arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `AWS::Lex::Bot`.
    Bot,
    /// `AWS::Lex::BotVersion`.
    Version,
    /// `AWS::Lex::BotAlias`.
    Alias,
}

/// The resource-type names of the three, which the registry of recognised types, the
/// return-value catalogue and the primary-identifier table each key an entry by.
pub(crate) const LEX_BOT: &str = "AWS::Lex::Bot";
/// See [`LEX_BOT`].
pub(crate) const LEX_BOT_VERSION: &str = "AWS::Lex::BotVersion";
/// See [`LEX_BOT`].
pub(crate) const LEX_BOT_ALIAS: &str = "AWS::Lex::BotAlias";

/// Which of the three a resource type is, or `None` for every other type.
pub(in crate::state) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        LEX_BOT => Some(Kind::Bot),
        LEX_BOT_VERSION => Some(Kind::Version),
        LEX_BOT_ALIAS => Some(Kind::Alias),
        _ => None,
    }
}

/// The signing name the Amazon Lex V2 model-building plane publishes (that model's
/// `sigv4Name: lex`), which speaks `restJson1` — so its calls ride on
/// [`call_rest_json_named`], each on the URI its own operation's `@http` binding declares.
pub(crate) const LEX_SIGNING_NAME: &str = "lex";

/// The ARN namespace that service mints its resources under, which its model carries as
/// `awsService.arnNamespace` (`lex`) rather than the `Lex` its resource types are spelled
/// with.
pub(crate) const LEX_ARN_NAMESPACE: &str = "lex";

/// The ARN path a BOT is published under, and the one an ALIAS is: Amazon Lex V2 names a bot
/// `bot/<botId>` and an alias `bot-alias/<botId>/<botAliasId>`.
pub(crate) const LEX_BOT_ARN_PATH: &str = "bot";
/// See [`LEX_BOT_ARN_PATH`].
pub(crate) const LEX_BOT_ALIAS_ARN_PATH: &str = "bot-alias";

/// The attributes each of the three types publishes, in the sorted order a resource's own
/// attribute map holds them in.
pub(crate) const LEX_BOT_ATTRIBUTES: &[&str] = &["Arn", "Id"];
/// See [`LEX_BOT_ATTRIBUTES`]: a version publishes the number Amazon Lex assigned it.
pub(crate) const LEX_BOT_VERSION_ATTRIBUTES: &[&str] = &[LEX_BOT_VERSION_ATTRIBUTE];
/// See [`LEX_BOT_ATTRIBUTES`].
pub(crate) const LEX_BOT_ALIAS_ATTRIBUTES: &[&str] = &["Arn", "BotAliasId", "BotAliasStatus"];

/// The attribute `AWS::Lex::BotVersion` publishes: the numeric version Amazon Lex assigned
/// when the version was cut, which is exactly what an alias's `botVersion` input takes.
pub(crate) const LEX_BOT_VERSION_ATTRIBUTE: &str = "BotVersion";

/// The number the FIRST version of a bot is created under: "When you create the first version
/// of a bot, Amazon Lex sets the version number to 1." The stand-in the naming rules publish
/// for a version's own attribute, re-pointed at what `CreateBotVersion` really answered.
pub(crate) const LEX_FIRST_BOT_VERSION: &str = "1";

/// How many characters the ids Amazon Lex V2 mints carry: 10, which both the modelled `Id`
/// (len 10..10, `^[0-9a-zA-Z]+$`) and `BotAliasId` are drawn from.
pub(crate) const LEX_ID_DIGITS: usize = 10;

/// The salts the stand-in ids are derived under, one per KIND, so a bot's stand-in id and the
/// stand-in id of an alias of that bot are different digits.
pub(crate) const LEX_BOT_ID_SALT: &str = "lex/bot-id";
/// See [`LEX_BOT_ID_SALT`].
pub(crate) const LEX_BOT_ALIAS_ID_SALT: &str = "lex/bot-alias-id";

/// The status an alias is opened in, which `CreateBotAlias` documents: "The alias is first put
/// into the Creating state. When the alias is ready to be used, it is put into the Available
/// state." The stand-in for the attribute the type publishes, re-pointed at what the create
/// answered.
pub(crate) const LEX_BOT_ALIAS_CREATING: &str = "Creating";

/// The working copy of a bot, which is the version a locale is defined in and the version a
/// snapshot is taken from: the modelled `DraftBotVersion` is `^DRAFT$`.
const DRAFT_VERSION: &str = "DRAFT";

/// The error code Amazon Lex V2 answers for a resource it does not hold on a READ
/// (`ResourceNotFoundException`, the modelled 404 of its describes), and the one its DELETES
/// answer for the same (`PreconditionFailedException`, their modelled 412: "Your request
/// couldn't be completed because one or more request fields aren't valid").
const LEX_NOT_FOUND: &str = "ResourceNotFoundException";
/// See [`LEX_NOT_FOUND`].
const LEX_PRECONDITION_FAILED: &str = "PreconditionFailedException";

/// The operations these arms call, as that service's own model names them.
const CREATE_BOT: &str = "CreateBot";
const DELETE_BOT: &str = "DeleteBot";
const DESCRIBE_BOT: &str = "DescribeBot";
const CREATE_BOT_LOCALE: &str = "CreateBotLocale";
const CREATE_SLOT_TYPE: &str = "CreateSlotType";
const CREATE_INTENT: &str = "CreateIntent";
const UPDATE_INTENT: &str = "UpdateIntent";
const CREATE_SLOT: &str = "CreateSlot";
const BUILD_BOT_LOCALE: &str = "BuildBotLocale";
const CREATE_BOT_VERSION: &str = "CreateBotVersion";
const DELETE_BOT_VERSION: &str = "DeleteBotVersion";
const DESCRIBE_BOT_VERSION: &str = "DescribeBotVersion";
const CREATE_BOT_ALIAS: &str = "CreateBotAlias";
const DELETE_BOT_ALIAS: &str = "DeleteBotAlias";
const DESCRIBE_BOT_ALIAS: &str = "DescribeBotAlias";

/// The members a created resource's identity is answered in.
const BOT_ID_MEMBER: &str = "botId";
const BOT_VERSION_MEMBER: &str = "botVersion";
const BOT_ALIAS_ID_MEMBER: &str = "botAliasId";
const BOT_ALIAS_STATUS_MEMBER: &str = "botAliasStatus";
const SLOT_TYPE_ID_MEMBER: &str = "slotTypeId";
const INTENT_ID_MEMBER: &str = "intentId";
const SLOT_ID_MEMBER: &str = "slotId";

/// The members each create takes the name CloudFormation decides in.
const BOT_NAME_MEMBER: &str = "botName";
const BOT_ALIAS_NAME_MEMBER: &str = "botAliasName";
const INTENT_NAME_MEMBER: &str = "intentName";
const SLOT_TYPE_NAME_MEMBER: &str = "slotTypeName";
const SLOT_NAME_MEMBER: &str = "slotName";

/// The property each type states a name in, where that property is not the member's own
/// upper-initial spelling.
const NAME_PROPERTY: &str = "Name";
const BOT_ALIAS_NAME_PROPERTY: &str = "BotAliasName";
const SLOT_TYPE_NAME_PROPERTY: &str = "SlotTypeName";

/// The confidence threshold a locale declares, and the member `CreateBotLocale` takes it as.
const NLU_THRESHOLD_PROPERTY: &str = "NluConfidenceThreshold";
const NLU_THRESHOLD_MEMBER: &str = "nluIntentConfidenceThreshold";

/// The properties of a bot that are not members of `CreateBot`: the locales a client creates
/// inside the bot one call each, and the switch that asks for each of them to be BUILT.
const BOT_LOCALES_PROPERTY: &str = "BotLocales";
const AUTO_BUILD_PROPERTY: &str = "AutoBuildBotLocales";
/// The settings of the alias Amazon Lex keeps for testing the bot, which no member of
/// `CreateBot` carries (see this module's own documentation).
const TEST_ALIAS_SETTINGS_PROPERTY: &str = "TestBotAliasSettings";
/// The S3 location an imported bot definition is read from, which no member of `CreateBot`
/// carries either: importing a bot definition is `StartImport`'s, not a create's.
const BOT_FILE_PROPERTY: &str = "BotFileS3Location";

/// The properties of a LOCALE that are the resources defined inside it.
const SLOT_TYPES_PROPERTY: &str = "SlotTypes";
const INTENTS_PROPERTY: &str = "Intents";
const CUSTOM_VOCABULARY_PROPERTY: &str = "CustomVocabulary";

/// The properties of an INTENT that are not members of `CreateIntent`: the slots a client
/// creates inside the intent one call each, and the order they are elicited in — which names
/// those slots by NAME while the API orders them by the ids it minted.
const SLOTS_PROPERTY: &str = "Slots";
const SLOT_PRIORITIES_PROPERTY: &str = "SlotPriorities";
const SLOT_PRIORITIES_MEMBER: &str = "slotPriorities";
const SLOT_PRIORITY_NAME: &str = "SlotName";
const SLOT_PRIORITY_VALUE: &str = "Priority";

/// The tag properties each type publishes, which are the tag MAPS its create takes.
const BOT_TAGS_PROPERTY: &str = "BotTags";
const BOT_TAGS_MEMBER: &str = "botTags";
const TEST_ALIAS_TAGS_PROPERTY: &str = "TestBotAliasTags";
const TEST_ALIAS_TAGS_MEMBER: &str = "testBotAliasTags";
const BOT_ALIAS_TAGS_PROPERTY: &str = "BotAliasTags";
const TAGS_MEMBER: &str = "tags";

/// The member a locale entry of either list names its locale by, which is the key the map the
/// API takes is keyed by.
const LOCALE_ID_PROPERTY: &str = "LocaleId";

/// The member each entry of a `BotVersionLocaleSpecification` / `BotAliasLocaleSettings` list
/// carries the entry's VALUE in, beside the locale id that keys it.
const VERSION_LOCALE_DETAILS: &str = "BotVersionLocaleDetails";
const ALIAS_LOCALE_SETTING: &str = "BotAliasLocaleSetting";

/// The properties a version and an alias name the bot they belong to by, which the URI of
/// every call on one carries rather than its body. (The `BotVersion` an alias points at, by
/// contrast, IS a member of `CreateBotAlias`'s body and reaches it under the same name with a
/// lower-cased initial, so it needs no row of its own.)
const BOT_ID_PROPERTY: &str = "BotId";

/// How a name is derived for a resource whose declaration states none, and how long the
/// derived part is. Both named types of this family declare their name as a required
/// property, so this is the fallback for a declaration that states none anyway: the name is
/// derived from the resource's own computed identity (a function of the stack and the
/// resource's logical id), so the same declaration of the same stack asks for the same name.
/// The form satisfies the modelled `Name` pattern (`^([0-9a-zA-Z][_-]?){1,100}$`).
const DERIVED_NAME_PREFIX: &str = "cfn-";
const DERIVED_NAME_DIGITS: usize = 16;

/// Create the Amazon Lex V2 resource a declaration of one of the three types asks for.
///
/// The identity is that service's: the id or the version number it assigned is reported back
/// as this resource's attributes, and the handle every later call takes is this resource's
/// physical id.
pub(super) async fn create_lex_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::Bot => create_bot(ctx, region, resource, propagated).await,
        Kind::Version => create_bot_version(ctx, region, resource).await,
        Kind::Alias => create_bot_alias(ctx, region, resource, propagated).await,
    }
}

/// Delete the Amazon Lex V2 resource a stack created, by the handle it was created under.
///
/// A resource that service no longer holds is already in the state the delete asks for, which
/// is judged on the answer's own CODE and never on a message that carries the request's own
/// values. A bot, a version and an alias are each deleted with the resource-in-use check
/// SKIPPED: CloudFormation is deleting the whole estate the stack declared, so a version an
/// alias of the same stack still points at is on its way out too.
pub(super) async fn delete_lex_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (operation, uri) = match kind {
        Kind::Bot => (
            DELETE_BOT,
            format!("/bots/{}", percent_encoded(&resource.physical_resource_id)),
        ),
        Kind::Version => {
            let Some((bot_id, bot_version)) = version_identity_parts(resource) else {
                // No pair to address a version by: nothing was created under this record.
                return Ok(());
            };
            (
                DELETE_BOT_VERSION,
                format!(
                    "/bots/{}/botversions/{}",
                    percent_encoded(&bot_id),
                    percent_encoded(&bot_version)
                ),
            )
        }
        Kind::Alias => {
            let Some((bot_alias_id, bot_id)) = alias_identity_parts(resource) else {
                return Ok(());
            };
            (
                DELETE_BOT_ALIAS,
                format!(
                    "/bots/{}/botaliases/{}",
                    percent_encoded(&bot_id),
                    percent_encoded(&bot_alias_id)
                ),
            )
        }
    };
    let uri = format!("{uri}?skipResourceInUseCheck=true");
    match call_rest_json_named(ctx, LEX_SIGNING_NAME, region, operation, "DELETE", &uri, None).await
    {
        Ok(_) => Ok(()),
        Err(refusal)
            if refusal.code == LEX_NOT_FOUND || refusal.code == LEX_PRECONDITION_FAILED =>
        {
            Ok(())
        }
        Err(refusal) => Err(refusal),
    }
}

/// What Amazon Lex V2 reports one of the three resources to be now, for drift detection: that
/// service's own read, by the handle this resource was created under. One it no longer holds
/// was deleted.
///
/// The members each read reports under the names the declaring type spells them with are the
/// scalars of the resource itself; a property the read does not report is not evidence of
/// anything ([`super::drift`]).
pub(super) async fn describe_lex_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (operation, uri, reported) = match kind {
        Kind::Bot => (
            DESCRIBE_BOT,
            format!("/bots/{}", percent_encoded(&resource.physical_resource_id)),
            BOT_REPORTED_MEMBERS,
        ),
        Kind::Version => {
            let Some((bot_id, bot_version)) = version_identity_parts(resource) else {
                return Ok(None);
            };
            (
                DESCRIBE_BOT_VERSION,
                format!(
                    "/bots/{}/botversions/{}",
                    percent_encoded(&bot_id),
                    percent_encoded(&bot_version)
                ),
                VERSION_REPORTED_MEMBERS,
            )
        }
        Kind::Alias => {
            let Some((bot_alias_id, bot_id)) = alias_identity_parts(resource) else {
                return Ok(None);
            };
            (
                DESCRIBE_BOT_ALIAS,
                format!(
                    "/bots/{}/botaliases/{}",
                    percent_encoded(&bot_id),
                    percent_encoded(&bot_alias_id)
                ),
                ALIAS_REPORTED_MEMBERS,
            )
        }
    };
    match call_rest_json_named(ctx, LEX_SIGNING_NAME, region, operation, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(&answer, reported))),
        Err(refusal) if refusal.code == LEX_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The members `DescribeBot` reports under the names `AWS::Lex::Bot` spells them with.
const BOT_REPORTED_MEMBERS: &[(&str, &str)] = &[
    ("Name", "botName"),
    ("Description", "description"),
    ("RoleArn", "roleArn"),
    ("IdleSessionTTLInSeconds", "idleSessionTTLInSeconds"),
];

/// The members `DescribeBotVersion` reports under the names `AWS::Lex::BotVersion` spells them
/// with: the bot the version is of, and the description the version carries.
const VERSION_REPORTED_MEMBERS: &[(&str, &str)] = &[
    ("BotId", "botId"),
    ("Description", "description"),
];

/// The members `DescribeBotAlias` reports under the names `AWS::Lex::BotAlias` spells them
/// with.
const ALIAS_REPORTED_MEMBERS: &[(&str, &str)] = &[
    ("BotId", "botId"),
    ("BotAliasName", "botAliasName"),
    ("BotVersion", "botVersion"),
    ("Description", "description"),
];

/// The physical resource id of a bot VERSION: the pair that type's schema declares as its
/// primary identifier — the bot and the number Amazon Lex assigned the version — joined with
/// [`crate::state::template::COMPOSITE_IDENTITY_SEPARATOR`], which is what `Ref` returns.
pub(crate) fn bot_version_identity(bot_id: &str, bot_version: &str) -> String {
    format!(
        "{bot_id}{}{bot_version}",
        crate::state::template::COMPOSITE_IDENTITY_SEPARATOR
    )
}

/// The physical resource id of a bot ALIAS: the pair that type's schema declares as its
/// primary identifier — the alias's own id and the bot it is an alias of, in that order —
/// joined the same way.
pub(crate) fn bot_alias_identity(bot_alias_id: &str, bot_id: &str) -> String {
    format!(
        "{bot_alias_id}{}{bot_id}",
        crate::state::template::COMPOSITE_IDENTITY_SEPARATOR
    )
}

/// The two parts of a version's identity, as `(botId, botVersion)`; `None` for a record whose
/// identity is not such a pair, which is a record nothing was created for.
fn version_identity_parts(resource: &StoredResource) -> Option<(String, String)> {
    identity_pair(&resource.physical_resource_id)
}

/// The two parts of an alias's identity, as `(botAliasId, botId)`; `None` as above.
fn alias_identity_parts(resource: &StoredResource) -> Option<(String, String)> {
    identity_pair(&resource.physical_resource_id)
}

/// One composite identity split at its separator; `None` where the value is not a pair.
fn identity_pair(identity: &str) -> Option<(String, String)> {
    let (first, second) =
        identity.split_once(crate::state::template::COMPOSITE_IDENTITY_SEPARATOR)?;
    (!first.is_empty() && !second.is_empty())
        .then(|| (first.to_string(), second.to_string()))
}

/// The member every message-carrying specification of that service declares its messages in,
/// and the property a template spells it with: `PromptSpecification`,
/// `ResponseSpecification` and `StillWaitingResponseSpecification` each take `messageGroups`
/// (required), which CloudFormation publishes as `MessageGroupsList`.
const MESSAGE_GROUPS_PROPERTY: &str = "MessageGroupsList";
const MESSAGE_GROUPS_MEMBER: &str = "messageGroups";

/// The member an entry of a list a template states for one of that service's MAP-valued
/// members carries its key in: CloudFormation publishes such a map as a list of entries each
/// carrying a `MapBlockKey` beside the entry's own members
/// (`PromptSpecification.PromptAttemptsSpecification`, a slot's
/// `SubSlotSetting.SlotSpecifications`), while the API takes the map itself.
const MAP_BLOCK_KEY: &str = "MapBlockKey";

/// The ENUM-valued members of the Amazon Lex V2 model-building API a declaration of one of
/// these types reaches, each beside the values its enum admits.
///
/// CloudFormation publishes an enum-valued property's allowed values in ITS own spelling,
/// which for a Lex property is the upper-snake form of the API's: the slot-type property
/// `ValueSelectionSetting.ResolutionStrategy` is documented as
/// `ORIGINAL_VALUE | TOP_RESOLUTION | CONCATENATION` while the create's
/// `valueSelectionSetting.resolutionStrategy` targets an enum of
/// `OriginalValue | TopResolution | Concatenation`. The two spellings never meet on the wire,
/// so a declared value reaches the call in the spelling its own enum admits
/// ([`lex_enum_value`]).
///
/// The values are each enum's own, as that service's model declares them:
/// `smithy_query(query="shape",shape="SlotValueResolutionStrategy,AudioRecognitionStrategy,MessageSelectionStrategy,SlotConstraint,ObfuscationSettingType,SlotShape,DialogActionType,VoiceEngine,BotType,SlotResolutionStrategy,BedrockTraceStatus",model_path="services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json")`.
const LEX_ENUM_MEMBERS: &[(&str, &[&str])] = &[
    // `SlotValueSelectionSetting.resolutionStrategy` (a slot type's).
    (
        "resolutionStrategy",
        &["OriginalValue", "TopResolution", "Concatenation"],
    ),
    // `AdvancedRecognitionSetting.audioRecognitionStrategy` (a slot type's).
    (
        "audioRecognitionStrategy",
        &["UseSlotValuesAsCustomVocabulary"],
    ),
    // `PromptSpecification.messageSelectionStrategy` (every prompt's).
    ("messageSelectionStrategy", &["Random", "Ordered"]),
    // `SlotValueElicitationSetting.slotConstraint` (a slot's).
    ("slotConstraint", &["Required", "Optional"]),
    // `ObfuscationSetting.obfuscationSettingType` (a slot's).
    ("obfuscationSettingType", &["None", "DefaultObfuscation"]),
    // `SlotValueOverride.shape` (a dialog step's slot override).
    ("shape", &["Scalar", "List"]),
    // `DialogAction.type` (every dialog step's).
    (
        "type",
        &[
            "ElicitIntent",
            "StartIntent",
            "ElicitSlot",
            "EvaluateConditional",
            "InvokeDialogCodeHook",
            "ConfirmIntent",
            "FulfillIntent",
            "CloseIntent",
            "EndConversation",
        ],
    ),
    // `VoiceSettings.engine` (a locale's).
    (
        "engine",
        &["standard", "neural", "long-form", "generative"],
    ),
    // `CreateBot.botType` (a bot's own).
    ("botType", &["Bot", "BotNetwork"]),
    // `SlotResolutionSetting.slotResolutionStrategy` (a slot's).
    ("slotResolutionStrategy", &["EnhancedFallback", "Default"]),
    // `BedrockModelSpecification.traceStatus`.
    ("traceStatus", &["ENABLED", "DISABLED"]),
];

/// One declared value of an enum-valued member as the Amazon Lex V2 API spells it, or `None`
/// where there is nothing to translate: the member is none of that API's enums, or the value
/// is already one its enum admits.
///
/// CloudFormation's spelling of an enum value is the upper-snake form of the API's, so a
/// declared value carrying no lower-case letter whose letters and digits are an admitted
/// value's — the underscores it separates words with being all that differs — IS that
/// admitted value. Anything else passes through unchanged, for that service's own validator to
/// judge.
fn lex_enum_value(member: &str, declared: &str) -> Option<String> {
    let admitted = LEX_ENUM_MEMBERS
        .iter()
        .find(|(name, _)| *name == member)
        .map(|(_, values)| *values)?;
    if admitted.contains(&declared) || declared.chars().any(char::is_lowercase) {
        return None;
    }
    let declared_letters = enum_letters(declared);
    admitted
        .iter()
        .find(|value| enum_letters(value) == declared_letters)
        .map(|value| (*value).to_string())
}

/// The letters and digits of an enum value, upper-cased: what both spellings of one value
/// share, the separators each spells it with being all that differs.
fn enum_letters(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_uppercase())
        .collect()
}

/// One declared property document as the request members the Amazon Lex V2 API takes.
///
/// Every member is the property with its initial letter lower-cased, with the one member that
/// service spells differently renamed ([`MESSAGE_GROUPS_PROPERTY`]), a map a template
/// states as a list of keyed entries turned back into that map ([`MAP_BLOCK_KEY`]) and an
/// enum-valued member's value spelled the way its own enum spells it ([`LEX_ENUM_MEMBERS`]).
/// Bounded by
/// the same nesting limit every other property walk is: the document is the requester's, so a
/// deeper one is refused rather than walked.
fn lex_members(value: &Value, depth: usize) -> Result<Value, Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (name, member) in members {
                let member_name = if name == MESSAGE_GROUPS_PROPERTY {
                    MESSAGE_GROUPS_MEMBER.to_string()
                } else {
                    lower_initial(name)
                };
                let converted_member = match member.as_str() {
                    Some(declared) => lex_enum_value(&member_name, declared).map(Value::String),
                    None => None,
                };
                let converted_member = match converted_member {
                    Some(spelled_as_the_api_does) => spelled_as_the_api_does,
                    None => lex_members(member, depth + 1)?,
                };
                converted.insert(member_name, converted_member);
            }
            Ok(Value::Object(converted))
        }
        Value::Array(items) => {
            // A list whose every entry carries a `MapBlockKey` is the spelling of a MAP: the
            // key is that member's value and the entry's other members are the value.
            if !items.is_empty()
                && items.iter().all(|item| {
                    item.get(MAP_BLOCK_KEY).and_then(Value::as_str).is_some()
                })
            {
                let mut keyed = Map::new();
                for item in items {
                    let Some(entry) = item.as_object() else {
                        continue;
                    };
                    let Some(key) = entry.get(MAP_BLOCK_KEY).and_then(Value::as_str) else {
                        continue;
                    };
                    let rest: Map<String, Value> = entry
                        .iter()
                        .filter(|(name, _)| *name != MAP_BLOCK_KEY)
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect();
                    keyed.insert(
                        key.to_string(),
                        lex_members(&Value::Object(rest), depth + 1)?,
                    );
                }
                return Ok(Value::Object(keyed));
            }
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(lex_members(item, depth + 1)?);
            }
            Ok(Value::Array(converted))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// Create the bot an `AWS::Lex::Bot` declares, and everything the declaration defines INSIDE
/// it: each locale, that locale's slot types and intents, each intent's slots and the order
/// they are elicited in, and the BUILD the declaration asks for.
///
/// The bot's own properties are `CreateBot`'s own members with a lower-cased initial, with the
/// name CloudFormation decides set on the way out and the tag maps the create takes carrying
/// the tags CloudFormation gives the resource beside the ones the declaration asks for.
async fn create_bot(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = object_properties(resource);
    let mut request = match lex_members(&without_members(&properties, BOT_NOT_SENT), 0)? {
        Value::Object(members) => members,
        _ => Map::new(),
    };
    request.insert(
        BOT_NAME_MEMBER.to_string(),
        Value::String(configured_name(resource, &properties, NAME_PROPERTY)),
    );
    for (property, member) in [
        (BOT_TAGS_PROPERTY, BOT_TAGS_MEMBER),
        (TEST_ALIAS_TAGS_PROPERTY, TEST_ALIAS_TAGS_MEMBER),
    ] {
        let tags = tags_as_map(&declared_tags_of(&properties, property, propagated));
        if !tags.is_empty() {
            request.insert(member.to_string(), Value::Object(tags));
        }
    }

    let answered = call_rest_json_named(
        ctx,
        LEX_SIGNING_NAME,
        region,
        CREATE_BOT,
        "PUT",
        "/bots",
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(bot_id) = reported_text(&answered, BOT_ID_MEMBER) else {
        // There would be no handle a client could call Amazon Lex with, and nothing for the
        // stack to publish, so this is a failed create rather than a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "the Amazon Lex V2 model-building plane answered a created bot without an \
                      id"
                .to_string(),
        });
    };

    // Everything defined inside the bot, in the order a client creates it: a locale before the
    // slot types and intents defined in it, and an intent before its slots.
    create_bot_locales(ctx, region, &properties, &bot_id).await?;

    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), bot_id.clone());
    attributes.insert(
        "Arn".to_string(),
        ::emulator_server::emulator_arn(
            LEX_ARN_NAMESPACE,
            region,
            &format!("{LEX_BOT_ARN_PATH}/{bot_id}"),
        ),
    );
    Ok(Created {
        // Amazon Lex assigned the bot's identity, so the id it answered is what `Ref`
        // resolves to and what every call on the bot is addressed by.
        physical_resource_id: Some(bot_id),
        attributes,
    })
}

/// The properties of a bot that are no members of `CreateBot` and so are not sent as ones.
const BOT_NOT_SENT: &[&str] = &[
    NAME_PROPERTY,
    BOT_LOCALES_PROPERTY,
    AUTO_BUILD_PROPERTY,
    BOT_TAGS_PROPERTY,
    TEST_ALIAS_TAGS_PROPERTY,
    TEST_ALIAS_SETTINGS_PROPERTY,
    BOT_FILE_PROPERTY,
];

/// Create every locale an `AWS::Lex::Bot` declares in the bot that was just created, with the
/// slot types, intents and slots defined in each, and submit the build the declaration asks
/// for.
///
/// Every one of these is created in the bot's WORKING COPY — the `DRAFT` version, the only one
/// `CreateBotLocale` and its siblings admit — which is the version a `AWS::Lex::BotVersion`
/// then snapshots.
async fn create_bot_locales(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Map<String, Value>,
    bot_id: &str,
) -> Result<(), Refusal> {
    let auto_build = properties
        .get(AUTO_BUILD_PROPERTY)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    for locale in declared_list(properties, BOT_LOCALES_PROPERTY) {
        let Some(declared) = locale.as_object() else {
            continue;
        };
        let Some(locale_id) = declared.get(LOCALE_ID_PROPERTY).and_then(Value::as_str) else {
            // A locale entry naming no locale addresses none of that service's locale calls,
            // so there is nothing to create for it.
            continue;
        };
        let mut request =
            match lex_members(&without_members(declared, LOCALE_NOT_SENT), 0)? {
                Value::Object(members) => members,
                _ => Map::new(),
            };
        if let Some(threshold) = declared.get(NLU_THRESHOLD_PROPERTY) {
            request.insert(NLU_THRESHOLD_MEMBER.to_string(), threshold.clone());
        }
        // `CreateBotLocale` takes the locale id in its BODY — the URI it is bound to names the
        // bot and the version alone — while every call on a resource INSIDE the locale binds
        // that id as a label of its own URI.
        request.insert(
            "localeId".to_string(),
            Value::String(locale_id.to_string()),
        );
        let locale_path = format!(
            "/bots/{}/botversions/{DRAFT_VERSION}/botlocales",
            percent_encoded(bot_id)
        );
        let locale_scope = format!("{locale_path}/{}", percent_encoded(locale_id));
        call_rest_json_named(
            ctx,
            LEX_SIGNING_NAME,
            region,
            CREATE_BOT_LOCALE,
            "PUT",
            &locale_path,
            Some(&Value::Object(request)),
        )
        .await?;

        // The slot types first: an intent's slot names one of them, and `CreateSlot` takes the
        // id that service minted for it.
        let slot_types = created_ids(
            ctx,
            region,
            CREATE_SLOT_TYPE,
            &format!("{locale_scope}/slottypes"),
            declared_list(declared, SLOT_TYPES_PROPERTY),
            SLOT_TYPE_NAME_PROPERTY,
            SLOT_TYPE_NAME_MEMBER,
            SLOT_TYPE_NOT_SENT,
            SLOT_TYPE_ID_MEMBER,
        )
        .await?;

        for intent in declared_list(declared, INTENTS_PROPERTY) {
            create_intent(
                ctx,
                region,
                &format!("{locale_scope}/intents"),
                &intent,
                &slot_types,
            )
            .await?;
        }

        if auto_build {
            // "Builds a bot so that it can be run" — the declaration asked for every locale to
            // be built, so the build is submitted exactly as a client's own would be, and the
            // locale reaches the status that service's own build earns it.
            call_rest_json_named(
                ctx,
                LEX_SIGNING_NAME,
                region,
                BUILD_BOT_LOCALE,
                "POST",
                &locale_scope,
                Some(&Value::Object(Map::new())),
            )
            .await?;
        }
    }
    Ok(())
}

/// The properties of a LOCALE that are no members of `CreateBotLocale`: the threshold that
/// member renames, the resources defined inside the locale, and the custom vocabulary a client
/// writes with its own batch calls.
const LOCALE_NOT_SENT: &[&str] = &[
    LOCALE_ID_PROPERTY,
    NLU_THRESHOLD_PROPERTY,
    SLOT_TYPES_PROPERTY,
    INTENTS_PROPERTY,
    CUSTOM_VOCABULARY_PROPERTY,
];

/// The properties of a SLOT TYPE that are no members of `CreateSlotType`: its name, which that
/// create takes under its own member.
const SLOT_TYPE_NOT_SENT: &[&str] = &[NAME_PROPERTY];

/// The properties of an INTENT that are no members of `CreateIntent`: its name, the slots
/// defined inside it, and the order those slots are elicited in — which `UpdateIntent` writes
/// once the slots have ids.
const INTENT_NOT_SENT: &[&str] = &[NAME_PROPERTY, SLOTS_PROPERTY, SLOT_PRIORITIES_PROPERTY];

/// The properties of a SLOT that are no members of `CreateSlot`: its name, and the slot type it
/// names by NAME where the create takes the id.
const SLOT_NOT_SENT: &[&str] = &[NAME_PROPERTY, SLOT_TYPE_NAME_PROPERTY];

/// Create one declared intent in the locale that was just created, with the slots defined
/// inside it and the order they are elicited in.
async fn create_intent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    intents_uri: &str,
    intent: &Value,
    slot_types: &BTreeMap<String, String>,
) -> Result<(), Refusal> {
    let Some(declared) = intent.as_object() else {
        return Ok(());
    };
    let Some(intent_name) = declared.get(NAME_PROPERTY).and_then(Value::as_str) else {
        return Ok(());
    };
    let mut request = match lex_members(&without_members(declared, INTENT_NOT_SENT), 0)? {
        Value::Object(members) => members,
        _ => Map::new(),
    };
    request.insert(
        INTENT_NAME_MEMBER.to_string(),
        Value::String(intent_name.to_string()),
    );
    let answered = call_rest_json_named(
        ctx,
        LEX_SIGNING_NAME,
        region,
        CREATE_INTENT,
        "PUT",
        intents_uri,
        Some(&Value::Object(request.clone())),
    )
    .await?;
    let Some(intent_id) = reported_text(&answered, INTENT_ID_MEMBER) else {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "the Amazon Lex V2 model-building plane answered a created intent without \
                      an id"
                .to_string(),
        });
    };

    // The slots of the intent, each naming its slot type by the id that service minted for it.
    let slots_uri = format!("{intents_uri}/{}/slots", percent_encoded(&intent_id));
    let mut slots = BTreeMap::new();
    for slot in declared_list(declared, SLOTS_PROPERTY) {
        let Some(declared_slot) = slot.as_object() else {
            continue;
        };
        let Some(slot_name) = declared_slot.get(NAME_PROPERTY).and_then(Value::as_str) else {
            continue;
        };
        let mut body =
            match lex_members(&without_members(declared_slot, SLOT_NOT_SENT), 0)? {
                Value::Object(members) => members,
                _ => Map::new(),
            };
        body.insert(
            SLOT_NAME_MEMBER.to_string(),
            Value::String(slot_name.to_string()),
        );
        if let Some(slot_type) = declared_slot
            .get(SLOT_TYPE_NAME_PROPERTY)
            .and_then(Value::as_str)
        {
            // A slot type the same locale declares reaches the call as the id that service
            // minted for it; a BUILT-IN one is named by its own signature, which the modelled
            // `BuiltInOrCustomSlotTypeId` admits beside a minted id.
            let slot_type_id = slot_types
                .get(slot_type)
                .cloned()
                .unwrap_or_else(|| slot_type.to_string());
            body.insert(SLOT_TYPE_ID_MEMBER.to_string(), Value::String(slot_type_id));
        }
        let answered = call_rest_json_named(
            ctx,
            LEX_SIGNING_NAME,
            region,
            CREATE_SLOT,
            "PUT",
            &slots_uri,
            Some(&Value::Object(body)),
        )
        .await?;
        if let Some(slot_id) = reported_text(&answered, SLOT_ID_MEMBER) {
            slots.insert(slot_name.to_string(), slot_id);
        }
    }

    // The order the slots are elicited in, which only `UpdateIntent` writes: "Nothing adds a
    // slot to slotPriorities as it is created", and the declaration states that order by slot
    // NAME while the API states it by the ids the creates above answered.
    let priorities = slot_priorities(declared, &slots);
    if !priorities.is_empty() {
        request.insert(SLOT_PRIORITIES_MEMBER.to_string(), Value::Array(priorities));
        call_rest_json_named(
            ctx,
            LEX_SIGNING_NAME,
            region,
            UPDATE_INTENT,
            "PUT",
            &format!("{intents_uri}/{}", percent_encoded(&intent_id)),
            Some(&Value::Object(request)),
        )
        .await?;
    }
    Ok(())
}

/// The `slotPriorities` an intent's declaration asks for, each entry naming one of the slots
/// just created by the id that service minted for it.
///
/// An entry naming a slot the intent does not hold orders nothing and is left out: the API
/// takes the ids of this intent's own slots.
fn slot_priorities(
    declared: &Map<String, Value>,
    slots: &BTreeMap<String, String>,
) -> Vec<Value> {
    declared_list(declared, SLOT_PRIORITIES_PROPERTY)
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let slot_name = entry.get(SLOT_PRIORITY_NAME).and_then(Value::as_str)?;
            let slot_id = slots.get(slot_name)?;
            let mut priority = Map::new();
            priority.insert(
                "priority".to_string(),
                entry.get(SLOT_PRIORITY_VALUE).cloned().unwrap_or(Value::Null),
            );
            priority.insert("slotId".to_string(), Value::String(slot_id.clone()));
            Some(Value::Object(priority))
        })
        .collect()
}

/// Create every entry of one declared list in the locale, and answer the ids that service
/// minted for them, keyed by the NAME each was declared under — which is how another
/// declaration inside the same locale names one.
#[allow(clippy::too_many_arguments)]
async fn created_ids(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    uri: &str,
    declared: Vec<Value>,
    name_property: &str,
    name_member: &str,
    not_sent: &[&str],
    id_member: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut created = BTreeMap::new();
    for entry in declared {
        let Some(members) = entry.as_object() else {
            continue;
        };
        // Either spelling of the entry's own name: a slot type declares `Name`, and the
        // property that names one from a slot is `SlotTypeName`.
        let Some(name) = members
            .get(NAME_PROPERTY)
            .or_else(|| members.get(name_property))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let mut request = match lex_members(&without_members(members, not_sent), 0)? {
            Value::Object(request) => request,
            _ => Map::new(),
        };
        request.insert(name_member.to_string(), Value::String(name.to_string()));
        let answered = call_rest_json_named(
            ctx,
            LEX_SIGNING_NAME,
            region,
            operation,
            "PUT",
            uri,
            Some(&Value::Object(request)),
        )
        .await?;
        if let Some(id) = reported_text(&answered, id_member) {
            created.insert(name.to_string(), id);
        }
    }
    Ok(created)
}

/// Create the version an `AWS::Lex::BotVersion` declares, through that service's own
/// `CreateBotVersion`, and publish the number it assigned.
///
/// The declaration states the locales the version is taken from as a LIST keyed by `LocaleId`
/// while the operation takes a MAP keyed by the locale id, so the list is turned into that map
/// ([`keyed_by_locale`]). The bot is named by the URI rather than by the body, and the value
/// the declaration states for it is a reference to the bot — so what reaches Amazon Lex is the
/// `botId` that service minted.
async fn create_bot_version(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = object_properties(resource);
    let bot_id = required_text(&properties, BOT_ID_PROPERTY)?;
    let mut request = Map::new();
    if let Some(description) = properties.get("Description") {
        request.insert("description".to_string(), description.clone());
    }
    request.insert(
        "botVersionLocaleSpecification".to_string(),
        Value::Object(keyed_by_locale(
            &properties,
            "BotVersionLocaleSpecification",
            VERSION_LOCALE_DETAILS,
        )?),
    );
    let answered = call_rest_json_named(
        ctx,
        LEX_SIGNING_NAME,
        region,
        CREATE_BOT_VERSION,
        "PUT",
        &format!("/bots/{}/botversions", percent_encoded(&bot_id)),
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(bot_version) = reported_text(&answered, BOT_VERSION_MEMBER) else {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "the Amazon Lex V2 model-building plane answered a created bot version \
                      without a version number"
                .to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    // The one value this type publishes, and the value an alias's `botVersion` takes: the
    // number Amazon Lex assigned when it cut the version.
    attributes.insert(
        LEX_BOT_VERSION_ATTRIBUTE.to_string(),
        bot_version.clone(),
    );
    Ok(Created {
        physical_resource_id: Some(bot_version_identity(&bot_id, &bot_version)),
        attributes,
    })
}

/// Create the alias an `AWS::Lex::BotAlias` declares, through that service's own
/// `CreateBotAlias`, and publish the identity it assigned.
///
/// The version the alias points at is the `BotVersion` the declaration states, which is
/// normally an `Fn::GetAtt` of the `AWS::Lex::BotVersion` the same template declares — so the
/// value reaching that member is the number `CreateBotVersion` answered for that resource.
async fn create_bot_alias(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = object_properties(resource);
    let bot_id = required_text(&properties, BOT_ID_PROPERTY)?;
    let mut request = match lex_members(&without_members(&properties, ALIAS_NOT_SENT), 0)? {
        Value::Object(members) => members,
        _ => Map::new(),
    };
    request.insert(
        BOT_ALIAS_NAME_MEMBER.to_string(),
        Value::String(configured_name(
            resource,
            &properties,
            BOT_ALIAS_NAME_PROPERTY,
        )),
    );
    let locale_settings = keyed_by_locale(&properties, "BotAliasLocaleSettings", ALIAS_LOCALE_SETTING)?;
    if !locale_settings.is_empty() {
        request.insert(
            "botAliasLocaleSettings".to_string(),
            Value::Object(locale_settings),
        );
    }
    let tags = tags_as_map(&declared_tags_of(
        &properties,
        BOT_ALIAS_TAGS_PROPERTY,
        propagated,
    ));
    if !tags.is_empty() {
        request.insert(TAGS_MEMBER.to_string(), Value::Object(tags));
    }

    let answered = call_rest_json_named(
        ctx,
        LEX_SIGNING_NAME,
        region,
        CREATE_BOT_ALIAS,
        "PUT",
        &format!("/bots/{}/botaliases", percent_encoded(&bot_id)),
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(bot_alias_id) = reported_text(&answered, BOT_ALIAS_ID_MEMBER) else {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "the Amazon Lex V2 model-building plane answered a created bot alias \
                      without an id"
                .to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("BotAliasId".to_string(), bot_alias_id.clone());
    attributes.insert(
        "Arn".to_string(),
        ::emulator_server::emulator_arn(
            LEX_ARN_NAMESPACE,
            region,
            &format!("{LEX_BOT_ALIAS_ARN_PATH}/{bot_id}/{bot_alias_id}"),
        ),
    );
    if let Some(status) = reported_text(&answered, BOT_ALIAS_STATUS_MEMBER) {
        attributes.insert("BotAliasStatus".to_string(), status);
    }
    Ok(Created {
        physical_resource_id: Some(bot_alias_identity(&bot_alias_id, &bot_id)),
        attributes,
    })
}

/// The properties of an ALIAS that are no members of `CreateBotAlias` as written: the bot,
/// which addresses the call, the name and the locale settings the call takes its own way, and
/// the tag list the create takes as a map.
const ALIAS_NOT_SENT: &[&str] = &[
    BOT_ID_PROPERTY,
    BOT_ALIAS_NAME_PROPERTY,
    "BotAliasLocaleSettings",
    BOT_ALIAS_TAGS_PROPERTY,
];

/// One declared list of per-locale entries as the MAP its operation takes: each entry's
/// `LocaleId` is the key, and the member named by `value_property` is the value, with its own
/// members re-cased.
///
/// An entry naming no locale keys nothing and is left out; an entry carrying no such member is
/// keyed with what the entry itself states beside its locale id, so a spelling that inlines
/// the value reaches the same map.
fn keyed_by_locale(
    properties: &Map<String, Value>,
    property: &str,
    value_property: &str,
) -> Result<Map<String, Value>, Refusal> {
    let mut keyed = Map::new();
    for entry in declared_list(properties, property) {
        let Some(members) = entry.as_object() else {
            continue;
        };
        let Some(locale_id) = members.get(LOCALE_ID_PROPERTY).and_then(Value::as_str) else {
            continue;
        };
        let value = match members.get(value_property) {
            Some(value) => value.clone(),
            None => Value::Object(
                members
                    .iter()
                    .filter(|(name, _)| *name != LOCALE_ID_PROPERTY)
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            ),
        };
        keyed.insert(locale_id.to_string(), lex_members(&value, 0)?);
    }
    Ok(keyed)
}

/// One resource's evaluated properties as an object, which is the document every arm here
/// reads; the empty one for a declaration that states none.
fn object_properties(resource: &StoredResource) -> Map<String, Value> {
    resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new)
}

/// The TEXT a property the call cannot be addressed without states; `Err` where the
/// declaration states none, which is a request that would address no resource at all.
fn required_text(properties: &Map<String, Value>, property: &str) -> Result<String, Refusal> {
    match properties.get(property).and_then(scalar_text) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(Refusal {
            code: "ValidationException".to_string(),
            message: format!(
                "1 validation error detected: Value null at '{}' failed to satisfy constraint: \
                 Member must not be null",
                lower_initial(property)
            ),
        }),
    }
}

/// The same document without the members `not_sent` names, which are the properties the call
/// does not take as written.
fn without_members(properties: &Map<String, Value>, not_sent: &[&str]) -> Value {
    Value::Object(
        properties
            .iter()
            .filter(|(name, _)| !not_sent.contains(&name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    )
}

/// The tags one of these resources carries under the tag property `property`: the ones its own
/// declaration states there, plus the ones CloudFormation propagates to it.
///
/// Read through the one reader of a declaration's tags ([`tags_of`]), which takes them under
/// the `Tags` property every other type spells them with, so a list of `{Key, Value}` pairs and
/// a map of key to value both reach the same set.
fn declared_tags_of(
    properties: &Map<String, Value>,
    property: &str,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    let mut declared = Map::new();
    if let Some(tags) = properties.get(property) {
        declared.insert("Tags".to_string(), tags.clone());
    }
    tags_of(&Value::Object(declared), propagated)
}

/// Every entry of a declared LIST property, or the empty list where the declaration states
/// none. A property stated as a single object rather than a list is read as the one entry it
/// is.
fn declared_list(properties: &Map<String, Value>, property: &str) -> Vec<Value> {
    match properties.get(property) {
        Some(Value::Array(entries)) => entries.clone(),
        Some(Value::Object(entry)) => vec![Value::Object(entry.clone())],
        _ => Vec::new(),
    }
}

/// The name the resource is created under: the one its declaration states, and otherwise one
/// derived from the resource's own computed identity ([`DERIVED_NAME_PREFIX`]).
fn configured_name(
    resource: &StoredResource,
    properties: &Map<String, Value>,
    property: &str,
) -> String {
    properties
        .get(property)
        .and_then(scalar_text)
        .filter(|declared| !declared.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{DERIVED_NAME_PREFIX}{}",
                crate::state::stable_identifier_digits(
                    &resource.physical_resource_id,
                    DERIVED_NAME_DIGITS
                )
            )
        })
}
